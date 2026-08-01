//! Go-to-definition, go-to-implementation, and find-references handlers,
//! triggered from the viewer panel by the `g` prefix (gd / gi / gr).

use crate::app::{App, StatusLevel};

pub(super) fn handle_go_to_definition(app: &mut App) {
    let symbol = match app.get_symbol_at_cursor() {
        Some(s) => s,
        None => {
            app.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
            return;
        }
    };

    if !app.code_nav.index.is_available() {
        app.set_status(
            "Symbol index not ready yet".to_string(),
            StatusLevel::Warning,
        );
        return;
    }

    // Context-aware: if cursor is at a definition site, show references instead.
    if app.is_cursor_at_definition(&symbol) {
        let root = app.code_nav.index.root();
        let refs = app.code_nav.index.find_references(&symbol, &root);
        if refs.is_empty() {
            app.set_status(
                format!("No references found for '{symbol}'"),
                StatusLevel::Warning,
            );
        } else {
            let count = refs.len();
            app.code_nav.references.active = true;
            app.code_nav.references.symbol_name = symbol.clone();
            app.code_nav.references.results = refs;
            app.code_nav.references.selected = 0;
            app.code_nav.references.scroll = 0;
            app.set_status(
                format!("At definition — showing {count} references for '{symbol}'"),
                StatusLevel::Info,
            );
        }
        return;
    }

    let defs = app.code_nav.index.find_definitions(&symbol);
    match defs.len() {
        0 => {
            app.set_status(
                format!("No definition found for '{symbol}'"),
                StatusLevel::Warning,
            );
        }
        1 => {
            let def = &defs[0];
            let file = def.file_path.clone();
            let line = def.line;
            app.jump_to_location(&file, line, 0);
            app.set_status(
                format!("Jumped to definition of '{symbol}'"),
                StatusLevel::Success,
            );
        }
        n => {
            // Multiple definitions — show in references overlay.
            app.code_nav.references.active = true;
            app.code_nav.references.symbol_name = format!("{symbol} (definitions)");
            app.code_nav.references.results = defs
                .iter()
                .map(|d| crate::symbol_index::Reference {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    content: format!("{:?} {}", d.kind, d.name),
                })
                .collect();
            app.code_nav.references.selected = 0;
            app.code_nav.references.scroll = 0;
            app.set_status(
                format!("{n} definitions found for '{symbol}'"),
                StatusLevel::Info,
            );
        }
    }
}

pub(super) fn handle_go_to_implementation(app: &mut App) {
    let symbol = match app.get_symbol_at_cursor() {
        Some(s) => s,
        None => {
            app.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
            return;
        }
    };

    if !app.code_nav.index.is_available() {
        app.set_status(
            "Symbol index not ready yet".to_string(),
            StatusLevel::Warning,
        );
        return;
    }

    let impls = app.code_nav.index.find_implementations(&symbol);
    match impls.len() {
        0 => {
            app.set_status(
                format!("No implementations found for '{symbol}'"),
                StatusLevel::Warning,
            );
        }
        1 => {
            let imp = &impls[0];
            let file = imp.file_path.clone();
            let line = imp.line;
            app.jump_to_location(&file, line, 0);
            app.set_status(
                format!("Jumped to implementation of '{symbol}'"),
                StatusLevel::Success,
            );
        }
        n => {
            app.code_nav.references.active = true;
            app.code_nav.references.symbol_name = format!("{symbol} (implementations)");
            app.code_nav.references.results = impls
                .iter()
                .map(|d| crate::symbol_index::Reference {
                    file_path: d.file_path.clone(),
                    line: d.line,
                    content: format!("{:?} {}", d.kind, d.name),
                })
                .collect();
            app.code_nav.references.selected = 0;
            app.code_nav.references.scroll = 0;
            app.set_status(
                format!("{n} implementations found for '{symbol}'"),
                StatusLevel::Info,
            );
        }
    }
}

pub(super) fn handle_find_references(app: &mut App) {
    let symbol = match app.get_symbol_at_cursor() {
        Some(s) => s,
        None => {
            app.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
            return;
        }
    };

    let root = app.code_nav.index.root();
    let refs = app.code_nav.index.find_references(&symbol, &root);

    if refs.is_empty() {
        app.set_status(
            format!("No references found for '{symbol}'"),
            StatusLevel::Warning,
        );
        return;
    }

    app.code_nav.references.active = true;
    app.code_nav.references.symbol_name = symbol;
    app.code_nav.references.results = refs;
    app.code_nav.references.selected = 0;
    app.code_nav.references.scroll = 0;
}
