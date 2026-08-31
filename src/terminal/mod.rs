//! [App] のターミナル/PTY セッションのライフサイクル。
//!
//! Claude Code と Shell の PTY セッションの起動・切り替え・終了・回収を担う。
//! セッション再開の処理は [terminal_resume]、PTY サイズの同期は [resize]、
//! Claude Code の waiting/active 状態検出は [terminal_cc_state] にある。

pub mod editor;
pub mod input;
pub mod link;
pub mod mouse;
pub mod render;
mod resize;
pub mod state;
mod terminal_cc_state;
mod terminal_resume;

use crate::app::*;
use crate::pty_manager;

const SESSION_ICONS: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8", "9"];

impl App {
    /// Claude パネルの表示を idx のセッションへ切り替える。
    ///
    /// 先にアクティブな reflow トランスクリプトを閉じる。reflow ビューは開かれた
    /// 時点で表示していたセッションに紐付いているため、セッションを切り替えると
    /// そのトランスクリプトは古くなり、ここで畳む必要がある。これは
    /// [TerminalState::switch_claude_session] 内でのスクロール/キャッシュリセット
    /// (「パネルが今表示しているのは別セッション」という同じ不変条件)と対応するが、
    /// focus が指す端末パネルの表示を、添字 idx のセッションへ切り替える。
    ///
    /// Claude パネルでは reflow ビューも閉じる。reflow の状態は App 側にあるので
    /// TerminalState だけでは閉じられず、開いたままだとパネルが前のセッションの
    /// トランスクリプトを描き続ける。全ての切り替えをここに通すのはそのため。
    pub fn switch_session(&mut self, focus: Focus, idx: usize) {
        if focus == Focus::TerminalClaude && self.reflow.active {
            self.close_reflow();
        }
        let Some(pane) = self.terminal.pane_mut(focus) else {
            return;
        };
        pane.switch_to(idx);
        self.terminal.pty_manager.activate_session(idx);
    }

    /// Claude パネルの切り替え。[Self::switch_session] の別名。
    pub fn switch_claude_session(&mut self, idx: usize) {
        self.switch_session(Focus::TerminalClaude, idx);
    }

    /// Shell パネルの切り替え。[Self::switch_session] の別名。
    pub fn switch_shell_session(&mut self, idx: usize) {
        self.switch_session(Focus::TerminalShell, idx);
    }

    /// フォーカス中のターミナルパネルで次(forward)または前のセッションタブへ
    /// 巡回する — タブをクリックする操作のキーボード版。ターミナルパネルが
    /// フォーカスされていて、かつセッションが2つ以上ない限り何もしない。周回する。
    pub fn cycle_terminal_session(&mut self, forward: bool) {
        let Some((kind, active)) = self
            .terminal
            .pane(self.focus.current())
            .map(|p| (p.kind, p.active_session))
        else {
            return;
        };
        let sessions: Vec<usize> = self
            .current_worktree_sessions(kind)
            .iter()
            .map(|(i, _)| *i)
            .collect();
        if sessions.len() <= 1 {
            return;
        }
        let pos = active
            .and_then(|a| sessions.iter().position(|&i| i == a))
            .unwrap_or(0);
        let next = if forward {
            (pos + 1) % sessions.len()
        } else {
            (pos + sessions.len() - 1) % sessions.len()
        };
        let target = sessions[next];
        self.switch_session(self.focus.current(), target);
    }

    /// 現在選択中の worktree に新しい Claude Code の PTY セッションを起動する。
    pub fn spawn_claude_code(&mut self) -> anyhow::Result<usize> {
        self.spawn_claude_code_with_name(None)
    }

    /// 任意で --name フラグを指定して新しい Claude Code の PTY セッションを起動する。
    pub fn spawn_claude_code_with_name(
        &mut self,
        session_name: Option<&str>,
    ) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let used_ids: Vec<&str> = self
            .terminal
            .pty_manager
            .sessions()
            .iter()
            .filter(|s| {
                s.working_dir == working_dir && s.kind == pty_manager::SessionKind::ClaudeCode
            })
            .filter_map(|s| s.label.strip_prefix("CC:"))
            .collect();
        let id = SESSION_ICONS
            .iter()
            .find(|e| !used_ids.contains(e))
            .unwrap_or(&SESSION_ICONS[used_ids.len() % SESSION_ICONS.len()]);
        let label = format!("CC:{id}");
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.claude.size;
        let idx = self.terminal.pty_manager.spawn_session(
            pty_manager::SessionKind::ClaudeCode,
            &worktree_name,
            &label,
            &shell,
            &working_dir,
            rows,
            cols,
            None,
            &self.repo.path,
            session_name,
        )?;
        self.switch_claude_session(idx);
        self.rebuild_worktree_list_rows();
        Ok(idx)
    }

    /// 現在選択中の worktree に新しい対話シェルの PTY セッションを起動する。
    pub fn spawn_shell(&mut self) -> anyhow::Result<usize> {
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let sh_count = self
            .terminal
            .pty_manager
            .sessions()
            .iter()
            .filter(|s| s.working_dir == working_dir && s.kind == pty_manager::SessionKind::Shell)
            .count();
        let label = format!("SH:{}", sh_count + 1);
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.shell.size;
        let idx = self.terminal.pty_manager.spawn_session(
            pty_manager::SessionKind::Shell,
            &worktree_name,
            &label,
            &shell,
            &working_dir,
            rows,
            cols,
            None,
            &self.repo.path,
            None,
        )?;
        self.switch_shell_session(idx);
        Ok(idx)
    }

    /// グローバルインデックスでターミナルセッションを閉じる(kill + remove)。
    ///
    /// active_claude_session と active_shell_session のインデックスを調整し、
    /// 現在の worktree で次に使えるセッションへフォールバックする。
    pub fn close_terminal_session(&mut self, global_idx: usize) {
        // セッションを kill して取り除く。
        let _ = self.terminal.pty_manager.kill_session(global_idx);
        self.terminal.pty_manager.remove_session(global_idx);

        // 保留プロンプトを調整する: 閉じたセッション分を除去し、それより大きい
        // インデックスをずらす。
        self.terminal.deferred_prompts.remove(&global_idx);
        let shifted: Vec<(usize, String)> = self
            .terminal
            .deferred_prompts
            .drain()
            .map(|(k, v)| (if k > global_idx { k - 1 } else { k }, v))
            .collect();
        self.terminal.deferred_prompts.extend(shifted);

        // アクティブセッションのインデックスを調整する。
        for a in [
            &mut self.terminal.claude.active_session,
            &mut self.terminal.shell.active_session,
        ]
        .into_iter()
        .flatten()
        {
            if *a == global_idx {
                *a = usize::MAX; // クリア対象として印を付ける
            } else if *a > global_idx {
                *a -= 1;
            }
        }

        // 自分より小さいインデックスのセッションが下で取り除かれたとき、埋め込み
        // エディタのセッションインデックスを有効なまま保つ。エディタ自体はこの経路
        // で閉じられることはなく(exit_editor で畳まれる)、ずれることはあっても
        // 無効化されることはない。
        if let Some(editor) = self.editor.as_mut()
            && editor.session_idx > global_idx
        {
            editor.session_idx -= 1;
        }

        // 無効化されたインデックスをクリアし、次に使えるセッションへフォールバック
        // する。閉じたセッションは表示中のものだったため、フォールバック先の内容は
        // 異なる — ヘルパー経由で切り替えてスクロールと描画キャッシュをリセットする
        // (そうしないとパネルが閉じたセッションの古い出力を表示し続けてしまう)。
        // 残っているセッションがない場合はキャッシュを直接クリアする。
        for focus in [Focus::TerminalClaude, Focus::TerminalShell] {
            let Some((kind, active)) = self
                .terminal
                .pane(focus)
                .map(|p| (p.kind, p.active_session))
            else {
                continue;
            };
            if active != Some(usize::MAX) {
                continue;
            }
            let next = self
                .current_worktree_sessions(kind)
                .first()
                .map(|(idx, _)| *idx);
            match next {
                Some(idx) => self.switch_session(focus, idx),
                None => {
                    if let Some(pane) = self.terminal.pane_mut(focus) {
                        pane.active_session = None;
                        pane.scroll = 0;
                        pane.cache = Default::default();
                    }
                }
            }
        }
        self.rebuild_worktree_list_rows();
    }

    /// 子プロセスが終了した PTY セッションを取り除く。
    ///
    /// 後ろから前へ走査することで、取り除いていく間、まだ確認していない前方の
    /// セッションのインデックスがずれないようにする。取り除いた後、
    /// active_claude_session と active_shell_session のインデックスを調整する。
    pub fn cleanup_dead_sessions(&mut self) -> bool {
        let count = self.terminal.pty_manager.session_count();
        let mut removed_any = false;

        // 取り除いた結果、まだ確認していないインデックスがずれないよう後ろから走査する。
        for idx in (0..count).rev() {
            // エディタ自身のセッションは poll_editor_exit(レイアウトを復元し
            // ファイルを再読み込みする)が所有しているので、ここでは決して回収しない。
            if self.editor.as_ref().is_some_and(|e| e.session_idx == idx) {
                continue;
            }
            if !self.terminal.pty_manager.is_session_alive(idx) {
                log::info!("removing dead PTY session at index {idx}");
                self.terminal.pty_manager.remove_session(idx);
                removed_any = true;

                // 自分より小さいインデックスのセッションが下で回収されたとき、
                // エディタのセッションインデックスをずらす。
                if let Some(editor) = self.editor.as_mut()
                    && editor.session_idx > idx
                {
                    editor.session_idx -= 1;
                }

                // 保留プロンプトを調整する。
                self.terminal.deferred_prompts.remove(&idx);
                let shifted: Vec<(usize, String)> = self
                    .terminal
                    .deferred_prompts
                    .drain()
                    .map(|(k, v)| (if k > idx { k - 1 } else { k }, v))
                    .collect();
                self.terminal.deferred_prompts.extend(shifted);

                // アクティブセッションのインデックスを調整する。
                for a in [
                    &mut self.terminal.claude.active_session,
                    &mut self.terminal.shell.active_session,
                ]
                .into_iter()
                .flatten()
                {
                    if *a == idx {
                        *a = usize::MAX; // クリア対象として印を付ける
                    } else if *a > idx {
                        *a -= 1;
                    }
                }
            }
        }

        if removed_any {
            // 取り除かれたセッションを指していたインデックスをクリアする。
            for pane in self.terminal.panes_mut() {
                if pane.active_session == Some(usize::MAX) {
                    pane.active_session = None;
                }
            }
        }

        removed_any
    }

    /// 現在選択中の worktree に属する Claude Code セッションについて、
    /// 現在選択中の worktree にある、指定種別の PTY セッション。
    /// pty_manager 全体の添字を添えて返す。
    pub fn current_worktree_sessions(
        &self,
        kind: pty_manager::SessionKind,
    ) -> Vec<(usize, &pty_manager::PtySession)> {
        let wt_path = self.selected_worktree_path();
        self.terminal
            .pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path && s.kind == kind)
            .collect()
    }
}
