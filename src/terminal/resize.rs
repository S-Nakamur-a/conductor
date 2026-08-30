//! [App] の PTY サイズ同期。
//!
//! Claude/Shell/エディタの PTY グリッドを、実際に描画されるパネル領域に
//! 合わせたサイズに保つ。

use crate::app::*;
use crate::pty_manager;

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
            Some(crate::types::Focus::TerminalClaude | crate::types::Focus::TerminalShell)
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
        self.terminal.claude.size = (rows, cols);
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
            self.terminal.claude.cache = Default::default();
            self.terminal.claude.dirty = true;
        }
    }

    /// Shell の PTY セッションの端末コンテンツ領域サイズを更新し、リサイズする。
    pub fn update_shell_terminal_size(&mut self, rows: u16, cols: u16) {
        self.terminal.shell.size = (rows, cols);
        if self.resize_sessions_of_kind(pty_manager::SessionKind::Shell, rows, cols) {
            // 上の Claude パネルと同様: 描画キャッシュを無効化し、読者のスクロール
            // オフセットは保持する。実際にこれが発火するのは今のところ shell だけ
            // であり、resize_session が reflow を報告する契機となる
            // raw_history を記録しているのは shell セッションだけである。
            self.terminal.shell.cache = Default::default();
            self.terminal.shell.dirty = true;
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
}
