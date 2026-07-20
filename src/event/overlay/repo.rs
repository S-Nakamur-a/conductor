//! Overlays for repo selection and opening: the multi-repo switcher, the
//! open-repo-by-path prompt, and PR intake (Review Pull Request).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;
use crate::overlay::ActiveOverlay;

use crate::event::clipboard_paste;

use super::overlay_list_nav;

// ── Overlay: repo selector ──────────────────────────────────────────────

pub(in crate::event) fn handle_repo_selector_key(app: &mut App, key: KeyEvent) {
    let count = app.repo_list.len();

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.repo_selector.selected,
        count,
    ) {
        return;
    }

    match key.code {
        KeyCode::Enter => {
            let selected = app.overlays.repo_selector.selected;
            app.overlays.active = ActiveOverlay::None;
            app.switch_repo(selected);
        }
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
        }
        _ => {}
    }
}

// ── Overlay: open repo path input ───────────────────────────────────────

pub(in crate::event) fn handle_open_repo_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.open_repo.buffer.clear();
        }
        KeyCode::Enter => {
            let buffer = app.overlays.open_repo.buffer.text().to_string();
            app.overlays.active = ActiveOverlay::None;
            app.overlays.open_repo.buffer.clear();
            app.open_repo_from_path(&buffer);
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.open_repo.buffer.delete_to_line_start();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.open_repo.buffer, false);
        }
        _ => {
            app.overlays.open_repo.buffer.handle_key(key);
        }
    }
}

// ── Overlay: PR intake (Review Pull Request) ────────────────────────────

pub(in crate::event) fn handle_pr_input_key(app: &mut App, key: KeyEvent) {
    // While a gh/git intake is running, only Esc is honored — the input
    // itself is frozen so a stray keystroke can't race the background thread.
    if app.overlays.pr_input.loading {
        if key.code == KeyCode::Esc {
            app.overlays.active = ActiveOverlay::None;
        }
        return;
    }
    match key.code {
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.pr_input.buffer.clear();
            app.overlays.pr_input.error = None;
        }
        KeyCode::Enter => {
            let input = app.overlays.pr_input.buffer.text().to_string();
            if !input.trim().is_empty() {
                app.overlays.pr_input.error = None;
                app.start_pr_intake(&input);
            }
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.pr_input.buffer.delete_to_line_start();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.pr_input.buffer, false);
        }
        _ => {
            // Any edit after a failed attempt clears the stale error so it
            // doesn't linger next to input the user has already changed.
            app.overlays.pr_input.error = None;
            app.overlays.pr_input.buffer.handle_key(key);
        }
    }
}
