//! Terminal panel helpers — PTY forwarding, session spawning, tab clicks.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Focus, StatusLevel};

/// Forward a key event to the PTY session at the given index.
pub(super) fn forward_key_to_pty(app: &mut App, session_idx: usize, key: KeyEvent) {
    let data: Vec<u8> = match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+<letter> → control byte (Ctrl+A = 0x01, etc.)
                let ctrl_byte = (c as u8).wrapping_sub(b'a').wrapping_add(1);
                vec![ctrl_byte]
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        _ => return,
    };

    if let Err(e) = app.terminal.pty_manager.write_to_session(session_idx, &data) {
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
                app.set_status(format!("Failed to start Claude Code: {e}"), StatusLevel::Error);
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
pub(super) fn handle_terminal_tab_click(app: &mut App, click_col: u16, tab_area_x: u16, is_claude: bool) {
    // Collect session info (global index + label) to avoid borrow issues.
    let sessions: Vec<(usize, String)> = if is_claude {
        app.current_worktree_claude_sessions()
            .iter()
            .map(|(idx, s)| (*idx, s.label.clone()))
            .collect()
    } else {
        app.current_worktree_shell_sessions()
            .iter()
            .map(|(idx, s)| (*idx, s.label.clone()))
            .collect()
    };

    if sessions.is_empty() {
        return;
    }

    // Build tab title strings to compute widths (must match render logic).
    // Each tab renders as: "[CC:🎹] [x]" — session label + " [x]" suffix.
    let tab_titles: Vec<String> = sessions
        .iter()
        .enumerate()
        .map(|(_, (_, label))| format!("[{}]", label))
        .collect();

    let close_suffix = " [x]"; // 4 chars
    let close_suffix_len = close_suffix.len() as u16;

    let relative_x = click_col.saturating_sub(tab_area_x);

    // Walk through tab titles to find which tab the click falls on.
    let mut x = 0u16;
    for (i, title) in tab_titles.iter().enumerate() {
        let label_width = UnicodeWidthStr::width(title.as_str()) as u16;
        let total_tab_width = label_width + close_suffix_len;
        if relative_x >= x && relative_x < x + total_tab_width {
            let (global_idx, _) = sessions[i];
            // Check if the click falls on the [x] close button area.
            // Only allow closing the currently active session to prevent accidental closes.
            let active_session = if is_claude {
                app.terminal.active_claude_session
            } else {
                app.terminal.active_shell_session
            };
            let close_start = x + label_width + 1; // +1 for the space before [x]
            if relative_x >= close_start && relative_x < x + total_tab_width {
                if Some(global_idx) == active_session {
                    app.close_terminal_session(global_idx);
                }
                return;
            }
            // Otherwise, activate the session.
            app.terminal.pty_manager.activate_session(global_idx);
            if is_claude {
                app.terminal.active_claude_session = Some(global_idx);
                app.terminal.scroll_claude = 0;
            } else {
                app.terminal.active_shell_session = Some(global_idx);
                app.terminal.scroll_shell = 0;
            }
            return;
        }
        x += total_tab_width;
        x += 1; // divider " "
    }

    // Check [+] tab.
    if relative_x >= x && relative_x < x + 3 {
        if is_claude {
            if let Err(e) = app.spawn_claude_code() {
                app.set_status(format!("Failed to start Claude Code: {e}"), StatusLevel::Error);
            }
        } else if let Err(e) = app.spawn_shell() {
            app.set_status(format!("Failed to start shell: {e}"), StatusLevel::Error);
        }
        return;
    }
    x += 3; // [+]
    x += 1; // divider " "

    // Check [<=>] toggle.
    if relative_x >= x && relative_x < x + 5 {
        let target = if is_claude { Focus::TerminalClaude } else { Focus::TerminalShell };
        if app.expanded_panel.is_some() {
            app.expanded_panel = None;
        } else {
            app.expanded_panel = Some(target);
        }
    }
}
