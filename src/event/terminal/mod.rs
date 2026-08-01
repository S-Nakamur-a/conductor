//! Terminal panel helpers — PTY forwarding, session spawning, tab clicks.
//!
//! The `KeyEvent` → ANSI byte sequence conversion used by
//! [`forward_key_to_pty`] lives in the [`ansi`] submodule.

mod ansi;

use crossterm::event::KeyEvent;

use crate::app::{App, Focus, StatusLevel};
use crate::terminal_link;

use ansi::key_event_to_ansi;

/// Forward a key event to the PTY session at the given index.
pub(super) fn forward_key_to_pty(app: &mut App, session_idx: usize, key: KeyEvent) {
    // Programs that enable application cursor keys mode (DECCKM) — pagers like
    // `less`/`bat`, editors like `vim` — expect arrow/Home/End as SS3 (`ESC O`)
    // rather than CSI (`ESC [`); honor the session's current mode so the keys
    // actually register (e.g. arrow-key scrolling in `bat`).
    let app_cursor = app
        .terminal
        .pty_manager
        .session_application_cursor(session_idx);
    let Some(data) = key_event_to_ansi(&key, app_cursor) else {
        return;
    };

    if let Err(e) = app
        .terminal
        .pty_manager
        .write_to_session(session_idx, &data)
    {
        log::warn!("failed to write to PTY session: {e}");
    } else {
        // Snap to live view when the user types into the terminal.
        match app.focus {
            Focus::TerminalClaude => app.terminal.scroll_claude = 0,
            Focus::TerminalShell => app.terminal.scroll_shell = 0,
            _ => {}
        }
        // Clear CC waiting signal when user sends input to a Claude Code session.
        app.clear_cc_waiting_signal(session_idx);
    }
}

/// Spawn a new terminal session based on the current focus (Claude Code or Shell).
pub(super) fn spawn_terminal_session(app: &mut App) {
    match app.focus {
        Focus::TerminalClaude => {
            app.set_status("Starting Claude Code...".to_string(), StatusLevel::Info);
            if let Err(e) = app.spawn_claude_code() {
                app.set_status(
                    format!("Failed to start Claude Code: {e}"),
                    StatusLevel::Error,
                );
                log::warn!("failed to spawn Claude Code session: {e}");
            } else {
                app.status_message = None;
            }
        }
        Focus::TerminalShell => {
            app.set_status("Starting shell...".to_string(), StatusLevel::Info);
            if let Err(e) = app.spawn_shell() {
                app.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
                log::warn!("failed to spawn shell session: {e}");
            } else {
                app.status_message = None;
            }
        }
        _ => {}
    }
}

/// Handle a click on a terminal tab bar.
/// `is_claude` is `true` for Claude panel, `false` for Shell panel.
///
/// Click resolution is driven by the hit regions recorded during render
/// (`tab_bar::render`), so it stays in lockstep with the scrolling tab strip
/// — `click_col` is an absolute screen column (the recorded regions are too).
pub(super) fn handle_terminal_tab_click(app: &mut App, click_col: u16, is_claude: bool) {
    use crate::ui::tab_bar::TabAction;

    let hit = {
        let hits = if is_claude {
            &app.terminal.claude_tab_hits
        } else {
            &app.terminal.shell_tab_hits
        };
        hits.iter()
            .find(|h| click_col >= h.x0 && click_col < h.x1)
            .map(|h| h.action)
    };
    let Some(action) = hit else {
        return;
    };

    match action {
        TabAction::Select(global_idx) => {
            // Switch to the session (resets scroll + render cache so the panel
            // re-renders the newly selected session).
            if is_claude {
                app.switch_claude_session(global_idx);
            } else {
                app.terminal.switch_shell_session(global_idx);
            }
        }
        TabAction::Close(global_idx) => {
            // One click closes, whichever tab it is. This deliberately drops an
            // earlier guard that only closed the *active* session and merely
            // selected an inactive one, so a second click was needed to close
            // it. That guard was paired with the tab colour: the active `[x]`
            // was `theme.error` ("this kills it") and an inactive one
            // `theme.muted` ("this only selects"), so behaviour and appearance
            // agreed. Making every `[x]` close on the first click therefore
            // required repainting them all `theme.error` — done in
            // `ui::tab_bar::render`, and not separable from this change. A
            // grey glyph that silently kills a running session would be a far
            // worse affordance than the two-click guard ever was.
            app.close_terminal_session(global_idx);
            // Closing shifts every later session's index down by one, and the
            // tab labels are fixed-width, so the next tab's `[x]` lands on the
            // same screen column the one just clicked occupied. A second click
            // there — a reflexive double-click, or two events drained in the
            // same frame before a repaint — would resolve against the stale
            // hit map and kill a session the user never aimed at. Dropping the
            // hit regions forces the next click to wait for a fresh render.
            if is_claude {
                app.terminal.claude_tab_hits.clear();
            } else {
                app.terminal.shell_tab_hits.clear();
            }
        }
        TabAction::Add => {
            if is_claude {
                if let Err(e) = app.spawn_claude_code() {
                    app.set_status(
                        format!("Failed to start Claude Code: {e}"),
                        StatusLevel::Error,
                    );
                }
            } else if let Err(e) = app.spawn_shell() {
                app.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
            }
        }
        TabAction::Expand => {
            let target = if is_claude {
                Focus::TerminalClaude
            } else {
                Focus::TerminalShell
            };
            if app.expanded_panel.is_some() {
                app.expanded_panel = None;
            } else {
                app.expanded_panel = Some(target);
            }
        }
        TabAction::ScrollLeft => {
            let scroll = if is_claude {
                &mut app.terminal.claude_tab_scroll
            } else {
                &mut app.terminal.shell_tab_scroll
            };
            *scroll = scroll.saturating_sub(1);
        }
        TabAction::ScrollRight => {
            let scroll = if is_claude {
                &mut app.terminal.claude_tab_scroll
            } else {
                &mut app.terminal.shell_tab_scroll
            };
            *scroll += 1;
        }
    }
}

/// Scan recent terminal output for file paths and open the first found in Viewer.
///
/// Triggered by `Ctrl+G` (or user-configured key). Scans the visible screen
/// rows of the active PTY session, starting from the cursor row upward.
pub(super) fn open_file_from_terminal_output(app: &mut App) {
    let (session_idx, scroll_offset) = match app.focus {
        Focus::TerminalClaude => (
            app.terminal.active_claude_session,
            app.terminal.scroll_claude,
        ),
        Focus::TerminalShell => (app.terminal.active_shell_session, app.terminal.scroll_shell),
        _ => return,
    };

    let Some(idx) = session_idx else {
        app.set_status(
            "No active terminal session".to_string(),
            StatusLevel::Warning,
        );
        return;
    };

    let Some(screen_arc) = app.terminal.pty_manager.get_screen(idx) else {
        return;
    };

    // リンクの実在確認に使う根は Viewer のツリーのもの。ここで確認した相対パスを
    // そのまま open_file_in_viewer に渡すので、別の根で確認すると「リンクとして
    // 認識されたのに開くと空」になる。
    let wt_path = app.viewer_state.root().to_path_buf();

    // Lock the parser, set scrollback, scan rows from cursor upward.
    let found = {
        let mut parser = screen_arc.lock().unwrap_or_else(|e| e.into_inner());
        parser.set_scrollback(scroll_offset);
        let screen = parser.screen();
        let (rows, cols) = screen.size();
        let cursor_row = screen.cursor_position().0;

        let mut result = None;
        // Scan from cursor row upward to find the most recent file reference.
        for offset in 0..rows {
            let r = if cursor_row >= offset {
                cursor_row - offset
            } else {
                break;
            };
            let text = terminal_link::extract_row_text(screen, r, cols);
            let links = terminal_link::detect_file_links(&text, &wt_path);
            if let Some(link) = links.into_iter().next() {
                result = Some((link.path.clone(), link.line));
                break;
            }
        }
        parser.set_scrollback(0);
        result
    };

    match found {
        Some((path, line)) => app.open_file_in_viewer(&path, line),
        None => app.set_status(
            "No file path found in terminal output".to_string(),
            StatusLevel::Warning,
        ),
    }
}
