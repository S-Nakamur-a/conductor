//! 右カラム（Claude Code / Shellターミナル）のクリック処理。

use crossterm::event::{KeyModifiers, MouseEvent};

use crate::app::{App, Focus};
use crate::terminal::link as terminal_link;

use crate::event::mouse::ClickGeometry;
use crate::terminal::input::{handle_terminal_tab_click, spawn_terminal_session};

/// 右カラム（Claudeターミナル / Shell）内の左クリックを処理する。
pub(crate) fn handle_terminal_column_click(
    app: &mut App,
    mouse: MouseEvent,
    col: u16,
    row: u16,
    geom: &ClickGeometry,
) {
    let main_area = geom.main_area;
    let viewer_end = geom.viewer_end;
    let terminal_claude_y = geom.terminal_claude_y;
    let terminal_split_y = geom.terminal_split_y;

    // 右カラム: 上80% = Claude、下20% = Shell。
    let terminal_x = viewer_end;

    // Cmd+Click (macOS) / Ctrl+Click (Linux) — ターミナル出力からファイルを開く。
    let has_open_modifier = mouse.modifiers.contains(KeyModifiers::SUPER)
        || mouse.modifiers.contains(KeyModifiers::CONTROL);

    if has_open_modifier {
        let (session_idx, content_y, scroll_offset) = if row < terminal_split_y {
            (
                app.terminal.claude.active_session,
                main_area.y + 1,
                app.terminal.claude.scroll,
            )
        } else {
            (
                app.terminal.shell.active_session,
                terminal_split_y + 1,
                app.terminal.shell.scroll,
            )
        };
        if row > content_y
            && let Some(idx) = session_idx
            && let Some(screen_arc) = app.terminal.pty_manager.get_screen(idx)
        {
            let parser = screen_arc.lock().unwrap_or_else(|e| e.into_inner());
            let (_, cols) = parser.screen().size();
            let pty_row = row - content_y;
            let pty_col = col.saturating_sub(terminal_x) as usize;

            // ロックを解放し、スクロールバック付きで再取得する。
            drop(parser);

            let text = {
                let mut p = screen_arc.lock().unwrap_or_else(|e| e.into_inner());
                p.set_scrollback(scroll_offset);
                let s = p.screen();
                let t = terminal_link::extract_row_text(s, pty_row, cols);
                p.set_scrollback(0);
                t
            };

            // 実在確認と実際に開く先を同じ根に揃える (キーボード側の
            // open_file_from_terminal_output と同じ理由)。
            let wt_path = app.explorer.root().to_path_buf();
            let links = terminal_link::detect_file_links(&text, &wt_path);
            // カーソル下のリンクを優先し、なければその行の最初のリンクにフォールバックする。
            let link =
                terminal_link::file_link_at_offset(&links, pty_col).or_else(|| links.first());
            if let Some(link) = link {
                let path = link.path.clone();
                let line = link.line;
                app.open_file_in_viewer(&path, line);
                return;
            }
        }
        // リンクが見つからなければ、通常のクリック挙動にフォールスルーする。
    }

    if row < terminal_split_y {
        app.set_focus(Focus::TerminalClaude);
        // トランスクリプトの「最新へジャンプ」チップ。読み手が最新のターンから
        // スクロールして離れている間だけ描画される。タブ帯や空白領域のダブル
        // クリックより先に確認することで、チップへのクリックがそれらとしても
        // 解釈されることはない。jump_hit はチップが画面上にない限り常に
        // None なので、それ以外の場合のコストはゼロ。
        if app.reflow.active
            && let Some(hit) = app.reflow.jump_hit
            && hit.contains(ratatui::layout::Position::new(col, row))
        {
            app.reflow_jump_to_latest();
            return;
        }
        // タブ帯（Claudeパネルの1行目）へのクリック。
        if row == terminal_claude_y {
            handle_terminal_tab_click(app, col, true);
        } else if app
            .current_worktree_sessions(crate::pty_manager::SessionKind::ClaudeCode)
            .is_empty()
        {
            // 新しいClaude Codeセッションを起動するにはダブルクリックが必要。
            if app.terminal.claude.blank_clicks.is_double(0) {
                spawn_terminal_session(app);
            }
        }
    } else {
        app.set_focus(Focus::TerminalShell);
        // タブ帯（Shellパネルの1行目）へのクリック。
        if row == terminal_split_y {
            handle_terminal_tab_click(app, col, false);
        } else if app
            .current_worktree_sessions(crate::pty_manager::SessionKind::Shell)
            .is_empty()
        {
            // 新しいShellセッションを起動するにはダブルクリックが必要。
            if app.terminal.shell.blank_clicks.is_double(0) {
                spawn_terminal_session(app);
            }
        }
    }
}
