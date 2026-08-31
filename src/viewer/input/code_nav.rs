//! Viewer パネルの g プレフィックス（gd / gi / gr）から呼ばれる、定義へ移動・
//! 実装へ移動・参照検索のハンドラ。

use crate::app::{App, StatusLevel};
use crate::overlay::HintAction;
use crate::viewer::code_nav::LinePick;

pub(super) fn handle_go_to_definition(app: &mut App) {
    dispatch(app, HintAction::Definition);
}

pub(super) fn handle_go_to_implementation(app: &mut App) {
    dispatch(app, HintAction::Implementation);
}

pub(super) fn handle_find_references(app: &mut App) {
    dispatch(app, HintAction::References);
}

pub(super) fn handle_show_hover_info(app: &mut App) {
    dispatch(app, HintAction::Hover);
}

/// カーソル行の対象を決めてから操作を走らせる。複数候補ならヒントを出して待つ。
fn dispatch(app: &mut App, action: HintAction) {
    match app.pick_line_identifier(action) {
        LinePick::Asked => {}
        LinePick::None => {
            app.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
        }
        LinePick::One(line_idx, occurrence, symbol) => {
            run(app, action, line_idx, occurrence, &symbol, 0);
        }
    }
}

/// 対象が決まったあとの実行。行内ヒントで選んだ場合もここに合流する。
pub(crate) fn run(
    app: &mut App,
    action: HintAction,
    line_idx: usize,
    occurrence: usize,
    symbol: &str,
    source_screen_row: usize,
) {
    match action {
        HintAction::Definition => {
            go_to_definition_at(app, line_idx, occurrence, symbol, source_screen_row)
        }
        HintAction::Implementation => go_to_implementation_at(app, line_idx, occurrence, symbol),
        HintAction::References => find_references_at(app, line_idx, occurrence, symbol),
        HintAction::Hover => app.show_hover_info_at(line_idx, symbol),
    }
}

fn go_to_definition_at(
    app: &mut App,
    line_idx: usize,
    occurrence: usize,
    symbol: &str,
    source_screen_row: usize,
) {
    // 意味索引が引ければそちらが答える。構文層への切り替えまで sheaf 側で
    // 済むので、下の名前ベースの経路は索引が無いときだけ走る。
    if let Some(answer) = app.semantic_definition(line_idx, occurrence) {
        // 定義の上で押したなら、行きたいのは定義ではなく使われている場所。
        if app.definition_is_here(&answer, line_idx) {
            find_references_at(app, line_idx, occurrence, symbol);
            return;
        }
        app.apply_semantic_definition(symbol, answer, source_screen_row);
        return;
    }

    if !app.code_nav.index.is_available() {
        app.set_status(
            "Symbol index not ready yet".to_string(),
            StatusLevel::Warning,
        );
        return;
    }

    // 文脈依存: カーソルが定義位置にある場合は代わりに参照一覧を表示する。
    if app.is_cursor_at_definition(symbol) {
        let root = app.code_nav.index.root();
        let refs = app.code_nav.index.find_references(symbol, &root);
        if refs.is_empty() {
            app.set_status(
                format!("No references found for '{symbol}' [no index]"),
                StatusLevel::Warning,
            );
        } else {
            let count = refs.len();
            app.code_nav.references.show(symbol.to_string(), refs);
            app.set_status(
                format!("At definition — {count} references for '{symbol}' [no index]"),
                StatusLevel::Info,
            );
        }
        return;
    }

    let defs = app
        .code_nav
        .index
        .find_definitions(symbol, std::path::Path::new(app.reading_file()));
    match defs.len() {
        0 => {
            app.set_status(
                format!("No definition found for '{symbol}' [no index]"),
                StatusLevel::Warning,
            );
        }
        1 => {
            let def = &defs[0];
            let file = def.file_path.clone();
            let line = def.line;
            app.jump_to_location(&file, line, 0);
            app.set_status(
                format!("Jumped to definition of '{symbol}' [no index] {file}:{line}"),
                StatusLevel::Success,
            );
        }
        n => {
            // 定義が複数ある場合は参照オーバーレイに表示する。
            app.code_nav.references.show(
                format!("{symbol} (definitions, no index)"),
                defs.iter()
                    .map(|d| crate::symbol_index::Reference {
                        file_path: d.file_path.clone(),
                        line: d.line,
                        content: format!("{:?} {}", d.kind, d.name),
                    })
                    .collect(),
            );
            app.set_status(
                format!("{n} definitions found for '{symbol}' [no index]"),
                StatusLevel::Info,
            );
        }
    }
}

fn go_to_implementation_at(app: &mut App, line_idx: usize, occurrence: usize, symbol: &str) {
    // 索引が答えるのは「この trait を実装しているもの」で、名前の一致ではない。
    // 索引が黙ったときだけ下の名前ベースの経路へ落ちる。
    if let Some(
        sheaf_core::Implementations::Exact(impls) | sheaf_core::Implementations::Derived(impls),
    ) = app.semantic_implementations(line_idx, occurrence)
        && !impls.is_empty()
    {
        apply_semantic_implementations(app, symbol, impls);
        return;
    }

    if !app.code_nav.index.is_available() {
        app.set_status(
            "Symbol index not ready yet".to_string(),
            StatusLevel::Warning,
        );
        return;
    }

    let impls = app.code_nav.index.find_implementations(symbol);
    match impls.len() {
        0 => {
            app.set_status(
                format!("No implementations found for '{symbol}' [tree-sitter]"),
                StatusLevel::Warning,
            );
        }
        1 => {
            let imp = &impls[0];
            let file = imp.file_path.clone();
            let line = imp.line;
            app.jump_to_location(&file, line, 0);
            app.set_status(
                format!("Jumped to implementation of '{symbol}' [tree-sitter] {file}:{line}"),
                StatusLevel::Success,
            );
        }
        n => {
            app.code_nav.references.show(
                format!("{symbol} (implementations, tree-sitter)"),
                impls
                    .iter()
                    .map(|d| crate::symbol_index::Reference {
                        file_path: d.file_path.clone(),
                        line: d.line,
                        content: format!("{:?} {}", d.kind, d.name),
                    })
                    .collect(),
            );
            app.set_status(
                format!("{n} implementations found for '{symbol}' [tree-sitter]"),
                StatusLevel::Info,
            );
        }
    }
}

/// impl ブロックの符号が無い形 (ジェネリックな impl) では着地点が最初のメソッドになる。
/// 行そのものより「どの型の実装か」が要る情報なので、一覧にはその型を並べる。
fn apply_semantic_implementations(
    app: &mut App,
    symbol: &str,
    impls: Vec<sheaf_core::Implementation>,
) {
    if let [only] = impls.as_slice() {
        let file = only.site.path.to_string_lossy().into_owned();
        let line = only.site.line as usize + 1;
        let ty = only.ty.clone();
        app.jump_to_location(&file, line, 0);
        app.set_status(
            format!("Jumped to {ty}'s impl of '{symbol}' [index] {file}:{line}"),
            StatusLevel::Success,
        );
        return;
    }

    let n = impls.len();
    app.code_nav.references.show(
        format!("{symbol} (implementations, index)"),
        impls
            .iter()
            .map(|imp| crate::symbol_index::Reference {
                file_path: imp.site.path.to_string_lossy().into_owned(),
                line: imp.site.line as usize + 1,
                content: format!("impl {symbol} for {}", imp.ty),
            })
            .collect(),
    );
    app.set_status(
        format!("{n} implementations found for '{symbol}' [index]"),
        StatusLevel::Info,
    );
}

fn find_references_at(app: &mut App, line_idx: usize, occurrence: usize, symbol: &str) {
    if let Some(answer) = app.semantic_references(line_idx, occurrence) {
        app.apply_semantic_references(symbol, answer);
        return;
    }

    let root = app.code_nav.index.root();
    let refs = app.code_nav.index.find_references(symbol, &root);

    if refs.is_empty() {
        app.set_status(
            format!("No references found for '{symbol}' [no index]"),
            StatusLevel::Warning,
        );
        return;
    }

    app.code_nav.references.show(symbol.to_string(), refs);
}
