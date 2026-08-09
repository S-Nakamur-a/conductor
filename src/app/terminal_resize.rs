//! [App] の PTY サイズ同期と diff/viewer の陳腐化ポーリング。
//!
//! Claude/Shell/エディタの PTY グリッドを、実際に描画されるパネル領域に
//! 合わせたサイズに保つ。また、diff パネルと viewer パネルの更新要否を
//! 判定する、tick ごとの軽量なチェックも担う。

use super::*;

impl App {
    /// PTY セッションのサイズを、キャッシュ済みレイアウトの寸法と同期させる。
    /// 寸法が実際に変わったときだけリサイズする。
    pub fn sync_pty_sizes(
        &mut self,
        last_claude_size: &mut (u16, u16),
        last_shell_size: &mut (u16, u16),
    ) {
        let cols = &self.layout.cache.columns;
        let is_terminal_expanded = matches!(
            self.expanded_panel,
            Some(crate::app::Focus::TerminalClaude | crate::app::Focus::TerminalShell)
        );
        let border_cols: u16 = if is_terminal_expanded { 0 } else { 2 };
        let border_rows: u16 = if is_terminal_expanded { 1 } else { 2 };
        let right_w = cols[3].width;
        if right_w > border_cols {
            let right_cols = right_w.saturating_sub(border_cols);
            let claude_pty_rows = self.layout.cache.terminal_split[0]
                .height
                .saturating_sub(border_rows);
            let shell_pty_rows = self.layout.cache.terminal_split[1]
                .height
                .saturating_sub(border_rows);

            if (claude_pty_rows, right_cols) != *last_claude_size
                && claude_pty_rows > 0
                && right_cols > 0
            {
                *last_claude_size = (claude_pty_rows, right_cols);
                self.update_claude_terminal_size(claude_pty_rows, right_cols);
            }
            if (shell_pty_rows, right_cols) != *last_shell_size
                && shell_pty_rows > 0
                && right_cols > 0
            {
                *last_shell_size = (shell_pty_rows, right_cols);
                self.update_shell_terminal_size(shell_pty_rows, right_cols);
            }
        }

        // 埋め込みエディタの PTY を、その(Explorer+Viewer が統合された)領域に
        // 合わせたサイズに保つ。キャッシュ済みレイアウトから計算するため、
        // パネルのリサイズと最大化トグルの両方に追従する。
        if let Some(idx) = self.editor.as_ref().map(|e| e.session_idx) {
            let size = self.editor_pty_size();
            if size != self.terminal.size_editor && size.0 > 0 && size.1 > 0 {
                self.terminal.size_editor = size;
                self.terminal
                    .pty_manager
                    .resize_session(idx, size.0, size.1);
            }
        }
    }

    /// Claude の PTY セッションの端末コンテンツ領域サイズを更新し、リサイズする。
    pub fn update_claude_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal.size_claude = (rows, cols);
        if self.resize_sessions_of_kind(pty_manager::SessionKind::ClaudeCode, rows, cols) {
            // グリッドが新しい幅で再構築されたので、キャッシュ済みの描画は古くなっている。
            //
            // *スクロールオフセット* は意図的にそのままにしている。かつてはこれを
            // 0 にリセットしていたが、それだと幅が変わるたびに、過去にスクロール
            // していた読者がライブの末尾へ引き戻されてしまっていた。しかもここでは
            // 幅の変化は珍しい出来事ではない: ウィンドウのリサイズ、パネルの最大化、
            // 分割線のドラッグ、パネル間のフォーカス移動(列幅はフォーカス駆動)、
            // これらが全てこの経路に到達する。別のウィンドウをちらっと見ただけで
            // 自分の位置を見失う、というのがまさにこの不満の正体だった。
            //
            // 数値をそのまま保持するのは近似的でしかない — 再ラップはビューポート
            // より上の行の番号を振り直すため、表示位置が数行ずれることがある —
            // が、それでも履歴の末端ではなく読者がいた位置の近くに着地する。
            // トランスクリプトビューが LineMeta でやっているように正確にアンカー
            // するには、vt100::Parser::set_scrollback を候補オフセットに対して
            // 総当たりで試す必要があり、その API は1画面分を超えるオフセットでは
            // アンダーフローする(Grid::visible_rows, vt100 0.15.2) — 今のところ
            // リリースビルドがオーバーフロー時にラップしてくれるおかげで生きて
            // いるだけである。
            self.terminal.cache_claude = Default::default();
            self.terminal.dirty_claude = true;
        }
    }

    /// Shell の PTY セッションの端末コンテンツ領域サイズを更新し、リサイズする。
    pub fn update_shell_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal.size_shell = (rows, cols);
        if self.resize_sessions_of_kind(pty_manager::SessionKind::Shell, rows, cols) {
            // 上の Claude パネルと同様: 描画キャッシュを無効化し、読者のスクロール
            // オフセットは保持する。実際にこれが発火するのは今のところ shell だけ
            // であり、resize_session が reflow を報告する契機となる
            // raw_history を記録しているのは shell セッションだけである。
            self.terminal.cache_shell = Default::default();
            self.terminal.dirty_shell = true;
        }
    }

    /// 選択中の worktree について kind の全セッションを (rows, cols) にリサイズする。
    /// いずれかのセッションが reflow した(幅の変化でグリッドが再構築された)場合 true を返す。
    fn resize_sessions_of_kind(
        &mut self,
        kind: pty_manager::SessionKind,
        rows: u16,
        cols: u16,
    ) -> bool {
        let wt_path = self.selected_worktree_path();
        let count = self.terminal.pty_manager.session_count();
        let mut reflowed = false;
        for idx in 0..count {
            let s = &self.terminal.pty_manager.sessions()[idx];
            if s.working_dir == wt_path && s.kind == kind {
                reflowed |= self.terminal.pty_manager.resize_session(idx, rows, cols);
            }
        }
        reflowed
    }

    // 変更検出の軽量ポーリング

    /// 選択中の worktree の HEAD oid とステータス件数を前回の既知値と比較し、
    /// diff パネルと viewer パネルの更新が必要かを判定する。実際に変化が
    /// 検出された場合のみ、コストの高い refresh_diff() と refresh_viewer()
    /// を呼び出す。
    ///
    /// ポーリングループ内で refresh_worktrees() の後に呼ばれる。
    /// refresh_worktrees() はその副作用として既に HEAD oid とステータス
    /// 件数を取得済みである。
    pub fn check_diff_viewer_staleness(&mut self) {
        let wt = match self.worktrees.selected() {
            Some(wt) => wt,
            None => return,
        };

        let current_head = self.worktree_heads.get(&wt.branch).cloned();
        // staged をここに含めているのは、git add / git reset を可視化するため。
        // 他の3つはインデックスを先にチェックして1ファイルにつき1バケットで
        // 数えるため、変更済みファイルをステージしても値は変わらない — かつ
        // ファイルウォッチャーも .git/ を無視するので役に立たず、ステージング
        // は他に何も触らない。この要素がなければ、Explorer のステージ状態の
        // 色は、たまたま無関係な編集が更新をトリガーしたときにしか更新されない
        // ことになる。
        let current_status = (wt.added, wt.modified, wt.deleted, wt.staged);

        let head_changed = self.last_poll_head_oid.as_ref() != current_head.as_ref();
        let status_changed = self.last_poll_status != Some(current_status);

        if head_changed || status_changed {
            log::debug!(
                "Change detected for worktree '{}': head_changed={}, status_changed={}",
                wt.branch,
                head_changed,
                status_changed,
            );
            self.refresh_diff();
            self.refresh_viewer();
        }

        self.last_poll_head_oid = current_head;
        self.last_poll_status = Some(current_status);
    }
}
