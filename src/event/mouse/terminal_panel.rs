//! Click handling for the right column (Claude Code / Shell terminals).

use crossterm::event::{KeyModifiers, MouseEvent};

use crate::app::{App, Focus};
use crate::terminal_link;

use super::super::terminal::{handle_terminal_tab_click, spawn_terminal_session};
use super::{ClickGeometry, register_double_click};

/// Handle a left click in the right column (Claude terminal / Shell).
pub(super) fn handle_terminal_column_click(
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

    // Right column: top 80% = Claude, bottom 20% = Shell.
    let terminal_x = viewer_end;

    // Cmd+Click (macOS) / Ctrl+Click (Linux) — open file from terminal output.
    let has_open_modifier = mouse.modifiers.contains(KeyModifiers::SUPER)
        || mouse.modifiers.contains(KeyModifiers::CONTROL);

    if has_open_modifier {
        let (session_idx, content_y, scroll_offset) = if row < terminal_split_y {
            (
                app.terminal.active_claude_session,
                main_area.y + 1,
                app.terminal.scroll_claude,
            )
        } else {
            (
                app.terminal.active_shell_session,
                terminal_split_y + 1,
                app.terminal.scroll_shell,
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

            // Drop lock and re-acquire with scrollback.
            drop(parser);

            let text = {
                let mut p = screen_arc.lock().unwrap_or_else(|e| e.into_inner());
                p.set_scrollback(scroll_offset);
                let s = p.screen();
                let t = terminal_link::extract_row_text(s, pty_row, cols);
                p.set_scrollback(0);
                t
            };

            let wt_path = app.selected_worktree_path();
            let links = terminal_link::detect_file_links(&text, &wt_path);
            // Prefer the link under the cursor; fall back to first on row.
            let link =
                terminal_link::file_link_at_offset(&links, pty_col).or_else(|| links.first());
            if let Some(link) = link {
                let path = link.path.clone();
                let line = link.line;
                app.open_file_in_viewer(&path, line);
                return;
            }
        }
        // If no link found, fall through to normal click behavior.
    }

    if row < terminal_split_y {
        app.set_focus(Focus::TerminalClaude);
        // The transcript's "jump to latest" chip, drawn only while the reader
        // has scrolled away from the newest turn. Checked before the tab strip
        // and the blank-area double-click so a click on the chip is never also
        // read as one of those; `jump_hit` is `None` whenever the chip is not
        // on screen, so this costs nothing the rest of the time.
        if app.reflow.active
            && let Some(hit) = app.reflow.jump_hit
            && hit.contains(ratatui::layout::Position::new(col, row))
        {
            app.reflow_jump_to_latest();
            return;
        }
        // Click on tab bar (first row of Claude panel).
        if row == terminal_claude_y {
            handle_terminal_tab_click(app, col, true);
        } else if app.current_worktree_claude_sessions().is_empty() {
            // Double-click required to spawn a new Claude Code session.
            if register_double_click(
                &mut app.terminal.claude_blank_last_click,
                std::time::Instant::now(),
            ) {
                spawn_terminal_session(app);
            }
        }
    } else {
        app.set_focus(Focus::TerminalShell);
        // Click on tab bar (first row of Shell panel).
        if row == terminal_split_y {
            handle_terminal_tab_click(app, col, false);
        } else if app.current_worktree_shell_sessions().is_empty() {
            // Double-click required to spawn a new Shell session.
            if register_double_click(
                &mut app.terminal.shell_blank_last_click,
                std::time::Instant::now(),
            ) {
                spawn_terminal_session(app);
            }
        }
    }
}
