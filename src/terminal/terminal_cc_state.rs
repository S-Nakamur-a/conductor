//! [App] における Claude Code の waiting/active 状態の追跡。
//!
//! CC の状態通知(直接の Unix ソケットイベントと、ファイルシステム上の
//! フック信号フォールバックの両方)を受け取り、cc_waiting_worktrees /
//! cc_active_worktrees を維持する。また、セッションが入力可能になるまで
//! 保留していたプロンプトを送信する。

use std::collections::HashSet;
use std::path::PathBuf;

use crate::app::*;
use crate::git_engine;
use crate::pty_manager;

impl App {
    // Claude Code の入力待ち検出

    /// Unix ソケット経由で受け取った1件の CC 通知を処理する。
    pub fn handle_cc_notify(&mut self, event: crate::cc_notify::CcNotifyEvent) {
        let (kind, cwd) = match event {
            crate::cc_notify::CcNotifyEvent::State { kind, cwd } => (kind, cwd),
            // /clear や /resume でこのパネルのログが別 id に移った。
            // スクロールバックが読むファイルを差し替えるだけで、waiting/active
            // の状態には関係しない。
            crate::cc_notify::CcNotifyEvent::SessionRotated {
                panel_id,
                session_id,
            } => {
                if self
                    .terminal
                    .pty_manager
                    .set_claude_session_id(&panel_id, session_id)
                {
                    // 開きっぱなしのトランスクリプトは古いログを指したままなので
                    // 畳む。次に開いたときに新しいログから読み直される。
                    if self.reflow.active {
                        self.close_reflow();
                    }
                }
                return;
            }
        };

        // cwd を正規化し、既知の worktree と照合する。
        let event_normalized: PathBuf = cwd.components().collect();
        let wt_path = self
            .worktrees
            .iter()
            .find(|wt| {
                let wt_normalized: PathBuf = wt.path.components().collect();
                wt_normalized == event_normalized
            })
            .map(|wt| wt.path.clone());

        let wt_path = match wt_path {
            Some(p) => p,
            None => return, // 未知の worktree — 無視する。
        };

        // この worktree に CC セッションが存在することを確認する。
        let has_session =
            self.terminal.pty_manager.sessions().iter().any(|s| {
                s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == wt_path
            });
        if !has_session {
            return;
        }

        match kind {
            crate::cc_notify::CcNotifyKind::Waiting => {
                self.terminal.cc_active_worktrees.remove(&wt_path);

                // ack による抑制をチェックする。
                if let Some(&ack_time) = self.terminal.cc_waiting_ack_time.get(&wt_path)
                    && let Some(session) = self.terminal.pty_manager.sessions().iter().find(|s| {
                        s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == wt_path
                    })
                {
                    let current = *session
                        .last_output_time
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if current == ack_time {
                        return; // 抑制対象 — ack 以降、新しい出力がない。
                    }
                    self.terminal.cc_waiting_ack_time.remove(&wt_path);
                }

                // フォーカスによる抑制: ユーザがこのターミナルにフォーカスしているなら自動 ack する。
                let is_focused = matches!(self.focus, Focus::TerminalClaude)
                    && self.selected_worktree_path() == wt_path;
                if is_focused {
                    return;
                }

                let is_new = self.terminal.cc_waiting_worktrees.insert(wt_path.clone());
                if is_new {
                    let display_name = self
                        .worktrees
                        .iter()
                        .find(|w| w.path == wt_path)
                        .map(|w| w.branch.clone())
                        .unwrap_or_else(|| "?".to_string());
                    self.set_status(
                        format!("CC waiting for input: {display_name}"),
                        StatusLevel::Info,
                    );
                }
            }
            crate::cc_notify::CcNotifyKind::Active => {
                self.terminal.cc_waiting_worktrees.remove(&wt_path);
                self.terminal.cc_active_worktrees.insert(wt_path);
            }
        }
    }

    /// フック信号ファイルをスキャンして cc_waiting_worktrees と
    /// cc_active_worktrees を更新する。
    ///
    /// プラグインのフックが書き込む .conductor/cc-waiting/ および
    /// .conductor/cc-active/ ディレクトリから信号ファイルを読む。
    ///
    /// worktree が新たに waiting 状態に入り、かつユーザがその worktree の
    /// ターミナルにフォーカスしていない場合、ステータスメッセージを通知として表示する。
    pub fn check_cc_waiting_state(&mut self) -> bool {
        let old_waiting = self.terminal.cc_waiting_worktrees.clone();
        let old_active = self.terminal.cc_active_worktrees.clone();

        // Conductor がリンクされた worktree から起動された場合でも正しい場所を
        // 見るように、メインリポジトリのルートを解決する。
        let conductor_dir = git_engine::GitEngine::open(&self.repo.path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo.path.clone())
            .join(".conductor");

        // ヘルパー: 信号ディレクトリをスキャンし、一致する worktree のパスを集める。
        let scan_signal_dir =
            |dir_name: &str, worktrees: &[crate::git_engine::WorktreeInfo]| -> HashSet<PathBuf> {
                let mut result = HashSet::new();
                let signal_dir = conductor_dir.join(dir_name);
                if let Ok(entries) = std::fs::read_dir(&signal_dir) {
                    for entry in entries.flatten() {
                        let filename = entry.file_name().to_string_lossy().to_string();
                        let signal_path: PathBuf = PathBuf::from(filename.replace("__", "/"));
                        let signal_normalized: PathBuf = signal_path.components().collect();
                        for wt in worktrees {
                            let wt_normalized: PathBuf = wt.path.components().collect();
                            if wt_normalized == signal_normalized {
                                result.insert(wt.path.clone());
                            }
                        }
                    }
                }
                result
            };

        let mut new_waiting = scan_signal_dir("cc-waiting", &self.worktrees);
        let mut new_active = scan_signal_dir("cc-active", &self.worktrees);

        // CC セッションが開かれていない worktree の状態は無視する。
        // 信号ファイルはセッション終了後も残ることがあり、このフィルタがないと
        // 存在しないパネルに対して UI がアニメーションしてしまう。
        let has_cc_session = |wt_path: &PathBuf| -> bool {
            self.terminal.pty_manager.sessions().iter().any(|s| {
                s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == *wt_path
            })
        };
        new_waiting.retain(&has_cc_session);
        new_active.retain(has_cc_session);

        // 新たに waiting 状態に入った worktree を検出する。
        let current_wt_path = self.selected_worktree_path();
        let is_terminal_focused = matches!(self.focus, Focus::TerminalClaude);

        // ユーザが CC ターミナルにフォーカスしている場合、waiting 状態は ack
        // されたものとして扱う — 通知バーと worktree アニメーションが(pulse の
        // 抑制だけでなく)完全にクリアされるよう取り除く。
        if is_terminal_focused && new_waiting.remove(&current_wt_path) {
            // 新しい出力が来るまで PTY のパターンマッチ由来で通知が
            // 再発火しないよう、ack を記録しておく。
            if let Some(session) = self.terminal.pty_manager.sessions().iter().find(|s| {
                s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == current_wt_path
            }) {
                let t = *session
                    .last_output_time
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                self.terminal
                    .cc_waiting_ack_time
                    .insert(current_wt_path.clone(), t);
            }
        }

        // ユーザが既に ack した worktree について、その ack 以降 PTY が
        // 新しい出力を出していなければ再発火を抑制する。
        let mut ack_expired: Vec<PathBuf> = Vec::new();
        new_waiting.retain(|wt_path| {
            if let Some(&ack_time) = self.terminal.cc_waiting_ack_time.get(wt_path) {
                if let Some(session) = self.terminal.pty_manager.sessions().iter().find(|s| {
                    s.kind == pty_manager::SessionKind::ClaudeCode && s.working_dir == *wt_path
                }) {
                    let current = *session
                        .last_output_time
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if current == ack_time {
                        return false; // 新しい出力がない — 抑制する
                    }
                }
                // 新しい出力が来た、またはセッションが消えた — ack は失効している。
                ack_expired.push(wt_path.clone());
            }
            true
        });
        for p in ack_expired {
            self.terminal.cc_waiting_ack_time.remove(&p);
        }

        for wt_path in &new_waiting {
            if !self.terminal.cc_waiting_worktrees.contains(wt_path) {
                // worktree 一覧から表示名を解決する。
                let display_name = self
                    .worktrees
                    .iter()
                    .find(|w| &w.path == wt_path)
                    .map(|w| w.branch.clone())
                    .unwrap_or_else(|| "?".to_string());
                // 新たに waiting になった — そのターミナルにフォーカスしていなければ通知する。
                let skip_notify = is_terminal_focused && *wt_path == current_wt_path;
                if !skip_notify {
                    self.set_status(
                        format!("CC waiting for input: {display_name}"),
                        StatusLevel::Info,
                    );
                }
            }
        }

        self.terminal.cc_waiting_worktrees = new_waiting;
        self.terminal.cc_active_worktrees = new_active;

        self.terminal.cc_waiting_worktrees != old_waiting
            || self.terminal.cc_active_worktrees != old_active
    }

    /// 入力可能になった CC セッションに対して、保留していたプロンプトを送信する。
    ///
    /// 次の2条件のいずれかを満たせばよい:
    /// 1. is_waiting_for_input — セッションがアイドルで "> " プロンプトが出ている
    ///    (通常運用では信頼できる)。
    /// 2. session_has_visible_output — セッションが何かを描画済み
    ///    (まだアイドルに達していない起動直後のセッションではこちらの方が速い)。
    pub fn flush_deferred_prompts(&mut self) {
        let ready: Vec<usize> = self
            .terminal
            .deferred_prompts
            .keys()
            .copied()
            .filter(|&idx| {
                self.terminal.pty_manager.is_waiting_for_input(idx)
                    || self.terminal.pty_manager.session_has_visible_output(idx)
            })
            .collect();
        for idx in ready {
            if let Some(prompt) = self.terminal.deferred_prompts.remove(&idx) {
                let _ = self
                    .terminal
                    .pty_manager
                    .write_chunked_to_session(idx, &prompt);
            }
        }
    }

    /// 指定セッションのフック信号ファイルを削除し、waiting 状態を解除する。
    /// ユーザが CC ターミナルへ入力を送ったときに呼ばれる。
    pub fn clear_cc_waiting_signal(&mut self, session_idx: usize) {
        let session = match self.terminal.pty_manager.sessions().get(session_idx) {
            Some(s) => s,
            None => return,
        };
        if session.kind != pty_manager::SessionKind::ClaudeCode {
            return;
        }
        // PTY 出力のタイムスタンプを記録し、実際に新しい出力が来るまで
        // 定期スキャンが通知を再発火させないようにする。
        let last_output = *session
            .last_output_time
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let working_dir = session.working_dir.clone();
        self.terminal
            .cc_waiting_ack_time
            .insert(working_dir.clone(), last_output);

        let conductor_dir = git_engine::GitEngine::open(&self.repo.path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| self.repo.path.clone())
            .join(".conductor");
        // シェルの $PWD エンコーディングに合わせてパスを正規化する(末尾のスラッシュを除去)。
        let normalized: PathBuf = session.working_dir.components().collect();
        let sanitized = normalized.display().to_string().replace('/', "__");
        let _ = std::fs::remove_file(conductor_dir.join("cc-waiting").join(&sanitized));
        let _ = std::fs::remove_file(conductor_dir.join("cc-active").join(&sanitized));
        self.terminal.cc_waiting_worktrees.remove(&working_dir);
        self.terminal.cc_active_worktrees.remove(&working_dir);
    }
}
