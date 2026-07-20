//! Code-navigation overlays: the references list popup, the single-key
//! symbol-hint jump overlay (labeled hints over symbol occurrences), and the
//! symbol action menu (go to definition/implementation/references) it opens
//! into.

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;

use super::overlay_list_nav;

// ── References overlay ──────────────────────────────────────────────────

pub(in crate::event) fn handle_references_key(app: &mut App, key: KeyEvent) {
    let count = app.references_overlay.results.len();
    if count == 0 {
        if key.code == KeyCode::Esc {
            app.references_overlay.active = false;
        }
        return;
    }

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.references_overlay.selected,
        count,
    ) {
        adjust_references_scroll(app);
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.references_overlay.active = false;
        }
        KeyCode::Enter => {
            let selected = app.references_overlay.selected;
            if let Some(reference) = app.references_overlay.results.get(selected).cloned() {
                app.references_overlay.active = false;
                app.jump_to_location(&reference.file_path, reference.line, 0);
            }
        }
        _ => {}
    }
}

fn adjust_references_scroll(app: &mut App) {
    let selected = app.references_overlay.selected;
    let scroll = &mut app.references_overlay.scroll;
    // Assume ~20 visible lines in the popup.
    let visible = 20usize;
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + visible {
        *scroll = selected.saturating_sub(visible - 1);
    }
}

// ── Symbol hint overlay ─────────────────────────────────────────────────

/// Handle key input while the symbol hint overlay is waiting for the second label character.
pub(in crate::event) fn handle_symbol_hint_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.symbol_hint_overlay = Default::default();
        }
        KeyCode::Char(c) if c.is_ascii_lowercase() => {
            app.symbol_hint_overlay.input.push(c);
            let input = app.symbol_hint_overlay.input.clone();
            // Find matching hint.
            let matched = app
                .symbol_hint_overlay
                .hints
                .iter()
                .find(|h| h.label == input)
                .cloned();
            // Dismiss hints.
            let scroll = app.viewer_state.content.file_scroll;
            app.symbol_hint_overlay = Default::default();
            if let Some(hint) = matched {
                // Build action overlay for this symbol.
                let screen_row = hint.line.saturating_sub(1).saturating_sub(scroll);
                open_symbol_action_overlay(app, &hint.symbol_name, screen_row);
            }
        }
        _ => {
            app.symbol_hint_overlay = Default::default();
        }
    }
}

/// Build and show the symbol action overlay for the given symbol.
/// `source_screen_row` is the screen row (0-indexed) where the symbol appeared.
fn open_symbol_action_overlay(app: &mut App, symbol_name: &str, source_screen_row: usize) {
    use crate::overlay::{SymbolAction, SymbolActionOverlay};

    let mut actions = Vec::new();

    // Definitions.
    let defs = app.symbol_index.find_definitions(symbol_name);
    if defs.len() == 1 {
        actions.push(SymbolAction {
            key: 'd',
            label: "Go to definition".to_string(),
            file_path: defs[0].file_path.clone(),
            line: defs[0].line,
        });
    } else if defs.len() > 1 {
        actions.push(SymbolAction {
            key: 'd',
            label: format!("Go to definition ({} results)", defs.len()),
            file_path: defs[0].file_path.clone(),
            line: defs[0].line,
        });
    }

    // Implementations.
    let impls = app.symbol_index.find_implementations(symbol_name);
    if impls.len() == 1 {
        actions.push(SymbolAction {
            key: 'i',
            label: "Go to implementation".to_string(),
            file_path: impls[0].file_path.clone(),
            line: impls[0].line,
        });
    } else if impls.len() > 1 {
        actions.push(SymbolAction {
            key: 'i',
            label: format!("Go to implementation ({} results)", impls.len()),
            file_path: impls[0].file_path.clone(),
            line: impls[0].line,
        });
    }

    // References (always show — count requires file scan).
    let root = app.symbol_index.root();
    let refs = app.symbol_index.find_references(symbol_name, &root);
    if !refs.is_empty() {
        actions.push(SymbolAction {
            key: 'r',
            label: format!("Find references ({} refs)", refs.len()),
            file_path: refs[0].file_path.clone(),
            line: refs[0].line,
        });
    }

    if actions.is_empty() {
        app.set_status(
            format!("No navigation targets for '{symbol_name}'"),
            crate::app::StatusLevel::Warning,
        );
        return;
    }

    // Context-aware default selection: if cursor is at the definition site,
    // pre-select "Find references" so pressing Enter goes to references.
    let at_def = app.is_cursor_at_definition(symbol_name);
    let default_idx = if at_def {
        actions.iter().position(|a| a.key == 'r').unwrap_or(0)
    } else {
        0
    };

    app.symbol_action_overlay = SymbolActionOverlay {
        active: true,
        symbol_name: symbol_name.to_string(),
        actions,
        selected: default_idx,
        source_screen_row,
    };
}

// ── Symbol action overlay ───────────────────────────────────────────────

/// Handle key input in the symbol action overlay.
pub(in crate::event) fn handle_symbol_action_key(app: &mut App, key: KeyEvent) {
    let count = app.symbol_action_overlay.actions.len();

    if overlay_list_nav(
        &app.keymap,
        &key,
        &mut app.symbol_action_overlay.selected,
        count,
    ) {
        return;
    }

    let symbol = app.symbol_action_overlay.symbol_name.clone();
    let screen_row = app.symbol_action_overlay.source_screen_row;
    match key.code {
        KeyCode::Esc => {
            app.symbol_action_overlay = Default::default();
        }
        KeyCode::Char('d') => {
            app.symbol_action_overlay = Default::default();
            jump_to_symbol_definition(app, &symbol, screen_row);
        }
        KeyCode::Char('i') => {
            app.symbol_action_overlay = Default::default();
            jump_to_symbol_implementation(app, &symbol, screen_row);
        }
        KeyCode::Char('r') => {
            app.symbol_action_overlay = Default::default();
            jump_to_symbol_references(app, &symbol);
        }
        KeyCode::Enter => {
            let idx = app.symbol_action_overlay.selected;
            if let Some(action) = app.symbol_action_overlay.actions.get(idx).cloned() {
                app.symbol_action_overlay = Default::default();
                match action.key {
                    'd' => jump_to_symbol_definition(app, &symbol, screen_row),
                    'i' => jump_to_symbol_implementation(app, &symbol, screen_row),
                    'r' => jump_to_symbol_references(app, &symbol),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn jump_to_symbol_definition(app: &mut App, symbol: &str, screen_row: usize) {
    let defs = app.symbol_index.find_definitions(symbol);
    match defs.len() {
        0 => {
            app.set_status(
                format!("No definition found for '{symbol}'"),
                crate::app::StatusLevel::Warning,
            );
        }
        1 => {
            app.jump_to_location(&defs[0].file_path, defs[0].line, screen_row);
            app.set_status(
                format!("Jumped to definition of '{symbol}' (Ctrl+O to go back)"),
                crate::app::StatusLevel::Success,
            );
        }
        _ => {
            app.references_overlay.active = true;
            app.references_overlay.symbol_name = format!("{symbol} (definitions)");
            app.references_overlay.results = defs
                .iter()
                .map(|d| crate::symbol_index::Reference {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    content: format!("{:?} {}", d.kind, d.name),
                })
                .collect();
            app.references_overlay.selected = 0;
            app.references_overlay.scroll = 0;
        }
    }
}

fn jump_to_symbol_implementation(app: &mut App, symbol: &str, screen_row: usize) {
    let impls = app.symbol_index.find_implementations(symbol);
    match impls.len() {
        0 => {
            app.set_status(
                format!("No implementations found for '{symbol}'"),
                crate::app::StatusLevel::Warning,
            );
        }
        1 => {
            app.jump_to_location(&impls[0].file_path, impls[0].line, screen_row);
            app.set_status(
                format!("Jumped to implementation of '{symbol}' (Ctrl+O to go back)"),
                crate::app::StatusLevel::Success,
            );
        }
        _ => {
            app.references_overlay.active = true;
            app.references_overlay.symbol_name = format!("{symbol} (implementations)");
            app.references_overlay.results = impls
                .iter()
                .map(|d| crate::symbol_index::Reference {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    content: format!("{:?} {}", d.kind, d.name),
                })
                .collect();
            app.references_overlay.selected = 0;
            app.references_overlay.scroll = 0;
        }
    }
}

fn jump_to_symbol_references(app: &mut App, symbol: &str) {
    let root = app.symbol_index.root();
    let refs = app.symbol_index.find_references(symbol, &root);
    if refs.is_empty() {
        app.set_status(
            format!("No references found for '{symbol}'"),
            crate::app::StatusLevel::Warning,
        );
        return;
    }
    app.references_overlay.active = true;
    app.references_overlay.symbol_name = symbol.to_string();
    app.references_overlay.results = refs;
    app.references_overlay.selected = 0;
    app.references_overlay.scroll = 0;
}
