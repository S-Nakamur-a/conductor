//! [App] のターミナル/PTY セッションのライフサイクル。
//!
//! Claude Code と Shell の PTY セッションの起動・切り替え・終了・回収を担う。
//! セッション再開の処理は [super::terminal_resume]、PTY サイズの同期は
//! [super::terminal_resize]、Claude Code の waiting/active 状態検出は
//! [super::terminal_cc_state] にある。

use super::*;

const SESSION_ICONS: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8", "9"];

impl App {
    /// Claude パネルの表示を idx のセッションへ切り替える。
    ///
    /// 先にアクティブな reflow トランスクリプトを閉じる。reflow ビューは開かれた
    /// 時点で表示していたセッションに紐付いているため、セッションを切り替えると
    /// そのトランスクリプトは古くなり、ここで畳む必要がある。これは
    /// [TerminalState::switch_claude_session] 内でのスクロール/キャッシュリセット
    /// (「パネルが今表示しているのは別セッション」という同じ不変条件)と対応するが、
    /// reflow の状態は App 側にあるため、この close は一段上のレイヤで行う必要が
    /// ある。全ての Claude セッション切り替えをこのラッパー経由にすることで、
    /// タブ/ストリップの切り替え後にパネルが前のセッションのトランスクリプトを
    /// 描画し続けることを防いでいる。
    pub fn switch_claude_session(&mut self, idx: usize) {
        if self.reflow.active {
            self.close_reflow();
        }
        self.terminal.switch_claude_session(idx);
    }

    /// フォーカス中のターミナルパネルで次(forward)または前のセッションタブへ
    /// 巡回する — タブをクリックする操作のキーボード版。ターミナルパネルが
    /// フォーカスされていて、かつセッションが2つ以上ない限り何もしない。周回する。
    pub fn cycle_terminal_session(&mut self, forward: bool) {
        let (sessions, active): (Vec<usize>, Option<usize>) = match self.focus {
            Focus::TerminalClaude => (
                self.current_worktree_claude_sessions()
                    .iter()
                    .map(|(i, _)| *i)
                    .collect(),
                self.terminal.active_claude_session,
            ),
            Focus::TerminalShell => (
                self.current_worktree_shell_sessions()
                    .iter()
                    .map(|(i, _)| *i)
                    .collect(),
                self.terminal.active_shell_session,
            ),
            _ => return,
        };
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
        match self.focus {
            Focus::TerminalClaude => self.switch_claude_session(target),
            Focus::TerminalShell => self.terminal.switch_shell_session(target),
            _ => {}
        }
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
        let (rows, cols) = self.terminal.size_claude;
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
        let (rows, cols) = self.terminal.size_shell;
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
        self.terminal.switch_shell_session(idx);
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
            &mut self.terminal.active_claude_session,
            &mut self.terminal.active_shell_session,
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
        if self.terminal.active_claude_session == Some(usize::MAX) {
            match self
                .current_worktree_claude_sessions()
                .first()
                .map(|(idx, _)| *idx)
            {
                Some(idx) => self.switch_claude_session(idx),
                None => {
                    self.terminal.active_claude_session = None;
                    self.terminal.scroll_claude = 0;
                    self.terminal.cache_claude = Default::default();
                }
            }
        }
        if self.terminal.active_shell_session == Some(usize::MAX) {
            match self
                .current_worktree_shell_sessions()
                .first()
                .map(|(idx, _)| *idx)
            {
                Some(idx) => self.terminal.switch_shell_session(idx),
                None => {
                    self.terminal.active_shell_session = None;
                    self.terminal.scroll_shell = 0;
                    self.terminal.cache_shell = Default::default();
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
                    &mut self.terminal.active_claude_session,
                    &mut self.terminal.active_shell_session,
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
            if self.terminal.active_claude_session == Some(usize::MAX) {
                self.terminal.active_claude_session = None;
            }
            if self.terminal.active_shell_session == Some(usize::MAX) {
                self.terminal.active_shell_session = None;
            }
        }

        removed_any
    }

    /// 現在選択中の worktree に属する Claude Code セッションについて、
    /// (index_in_pty_manager, &PtySession) のペアを返す。
    pub fn current_worktree_claude_sessions(&self) -> Vec<(usize, &pty_manager::PtySession)> {
        let wt_path = self.selected_worktree_path();
        self.terminal
            .pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.working_dir == wt_path && s.kind == pty_manager::SessionKind::ClaudeCode
            })
            .collect()
    }

    /// 現在選択中の worktree に属する Shell セッションについて、
    /// (index_in_pty_manager, &PtySession) のペアを返す。
    pub fn current_worktree_shell_sessions(&self) -> Vec<(usize, &pty_manager::PtySession)> {
        let wt_path = self.selected_worktree_path();
        self.terminal
            .pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path && s.kind == pty_manager::SessionKind::Shell)
            .collect()
    }
}
