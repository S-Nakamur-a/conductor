//! Overlays for Claude Code session history: the saved-history browser and
//! the resume-session picker.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus, StatusLevel};
use crate::overlay::ActiveOverlay;

use crate::event::clipboard_paste;

use super::filterable_overlay_list_nav;

// ── Overlay: session history ────────────────────────────────────────────

pub(in crate::event) fn handle_history_key(app: &mut App, key: KeyEvent) {
    if app.overlays.history.search_active {
        match key.code {
            KeyCode::Enter => {
                app.overlays.history.search_active = false;
                app.search_session_history();
            }
            KeyCode::Esc => {
                app.overlays.history.search_active = false;
                app.overlays.history.search_query.clear();
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                app.overlays.history.search_query.delete_to_line_start();
            }
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                clipboard_paste(app, |a| &mut a.overlays.history.search_query, false);
            }
            _ => {
                app.overlays.history.search_query.handle_key(key);
            }
        }
        return;
    }

    let count = app.overlays.history.records.len();

    if filterable_overlay_list_nav(&app.keymap, &key, &mut app.overlays.history.selected, count) {
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.history.search_query.clear();
            app.overlays.history.search_active = false;
        }
        KeyCode::Char('/') => {
            app.overlays.history.search_active = true;
            app.overlays.history.search_query.clear();
        }
        KeyCode::Char('s') => {
            app.save_current_session_history();
        }
        _ => {}
    }
}

// ── Overlay: resume Claude session ──────────────────────────────────────

pub(in crate::event) fn handle_resume_session_key(app: &mut App, key: KeyEvent) {
    let filtered_count = app.filtered_resume_sessions().len();

    if filterable_overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.resume_session.selected,
        filtered_count,
    ) {
        return;
    }

    match key.code {
        KeyCode::Enter => {
            let filtered = app.filtered_resume_sessions();
            if let Some(&(original_idx, _)) = filtered.get(app.overlays.resume_session.selected) {
                let Some(session) = app
                    .overlays
                    .resume_session
                    .sessions
                    .get(original_idx)
                    .cloned()
                else {
                    return;
                };
                app.overlays.active = ActiveOverlay::None;
                app.overlays.resume_session.filter.clear();
                app.set_status(
                    format!(
                        "Resuming: {}...",
                        session.display.chars().take(40).collect::<String>()
                    ),
                    StatusLevel::Info,
                );
                match app.resume_claude_session(&session.session_id, &session.display) {
                    Ok(_) => {
                        app.status_message = None;
                        app.set_focus(Focus::TerminalClaude);
                    }
                    Err(e) => {
                        app.set_status(format!("Failed to resume: {e}"), StatusLevel::Error);
                        log::warn!("failed to resume Claude session: {e}");
                    }
                }
            }
        }
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.resume_session.filter.clear();
        }
        KeyCode::Tab => {
            // Toggle between current-repo-only and all-projects mode.
            app.overlays.resume_session.all_projects = !app.overlays.resume_session.all_projects;
            app.load_resume_sessions();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.resume_session.filter.delete_to_line_start();
            app.overlays.resume_session.selected = 0;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.resume_session.filter, false);
            app.overlays.resume_session.selected = 0;
        }
        _ => {
            if app.overlays.resume_session.filter.handle_key(key) {
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.overlays.resume_session.selected = 0;
                    }
                    _ => {}
                }
            }
        }
    }
}
