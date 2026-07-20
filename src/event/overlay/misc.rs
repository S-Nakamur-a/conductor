//! Small standalone overlays: the help popup, the command palette, and the
//! theme picker (with live preview).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Focus, StatusLevel};
use crate::overlay::ActiveOverlay;

use crate::event::clipboard_paste;

use super::filterable_overlay_list_nav;
use super::overlay_list_nav;

// ── Overlay: help ───────────────────────────────────────────────────────

pub(in crate::event) fn handle_help_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
            app.overlays.active = ActiveOverlay::None;
        }
        // Allow scrolling through help pages by switching context.
        KeyCode::Char('1') => app.overlays.help.context = Focus::Worktree,
        KeyCode::Char('2') => app.overlays.help.context = Focus::Explorer,
        KeyCode::Char('3') => app.overlays.help.context = Focus::Viewer,
        KeyCode::Char('4') => app.overlays.help.context = Focus::TerminalClaude,
        _ => {}
    }
}

// ── Overlay: command palette ─────────────────────────────────────────────

pub(in crate::event) fn handle_command_palette_key(app: &mut App, key: KeyEvent) {
    use crate::command_palette;

    let filtered = command_palette::filter_commands(
        &app.overlays.command_palette.filter,
        &app.keymap,
        app.focus.key_context(),
    );
    let count = filtered.len();

    if filterable_overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.overlays.command_palette.selected,
        count,
    ) {
        return;
    }

    match key.code {
        KeyCode::Enter => {
            if let Some(scored) = filtered.get(app.overlays.command_palette.selected) {
                let id = command_palette::COMMANDS[scored.index].id;
                app.overlays.active = ActiveOverlay::None;
                app.overlays.command_palette.filter.clear();
                app.execute_palette_command(id);
            }
        }
        KeyCode::Esc => {
            app.overlays.active = ActiveOverlay::None;
            app.overlays.command_palette.filter.clear();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.overlays.command_palette.filter.delete_to_line_start();
            app.overlays.command_palette.selected = 0;
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clipboard_paste(app, |a| &mut a.overlays.command_palette.filter, false);
            app.overlays.command_palette.selected = 0;
        }
        _ => {
            if app.overlays.command_palette.filter.handle_key(key) {
                match key.code {
                    KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_) => {
                        app.overlays.command_palette.selected = 0;
                    }
                    _ => {}
                }
            }
        }
    }
}

// ── Overlay: theme picker ────────────────────────────────────────────────

/// Handle keys for the theme picker overlay.
///
/// Up/Down (or j/k) browse the list with live preview — each movement calls
/// `set_theme(name, false)` so the UI updates immediately without persisting.
/// Enter confirms and persists the selected theme; Esc reverts to the theme
/// that was active when the picker was opened.
pub(in crate::event) fn handle_theme_picker_key(app: &mut App, key: KeyEvent) {
    let count = app.overlays.theme_picker.themes.len();

    if overlay_list_nav(&app.keymap, &key, &mut app.overlays.theme_picker.selected, count) {
        // Live preview: apply the newly highlighted theme without persisting.
        let name = app
            .overlays
            .theme_picker
            .themes
            .get(app.overlays.theme_picker.selected)
            .cloned()
            .unwrap_or_default();
        app.set_theme(&name, false);
        return;
    }

    match key.code {
        KeyCode::Enter => {
            let name = app
                .overlays
                .theme_picker
                .themes
                .get(app.overlays.theme_picker.selected)
                .cloned()
                .unwrap_or_default();
            app.overlays.active = ActiveOverlay::None;
            app.set_theme(&name, true);
            app.set_status(format!("Theme: {name}"), StatusLevel::Success);
        }
        KeyCode::Esc => {
            let orig = app.overlays.theme_picker.original.clone();
            app.overlays.active = ActiveOverlay::None;
            app.set_theme(&orig, false);
        }
        _ => {}
    }
}
