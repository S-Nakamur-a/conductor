//! Viewer パネルから起動するコメント操作: 現在位置をプレフィルしたコメント
//! 追加入力欄を開く、現在行のコメント詳細モーダルを開く、送信されたコメント
//! テキストをパースして新しいレビューコメントにする。

use crate::app::App;
use crate::review_state::ReviewInputMode;
use crate::review_store::{Author, CommentKind};

/// Viewer からレビューコメント入力欄を開き、位置をプレフィルする。
pub(in crate::event) fn open_viewer_comment(app: &mut App) {
    let file_path = match app.viewer.content.current_file.clone() {
        Some(p) => p,
        None => return,
    };

    let location = if let Some((start, end)) = app.viewer.selected_range() {
        if start == end {
            format!("{file_path}:{start} ")
        } else {
            format!("{file_path}:{start}-{end} ")
        }
    } else {
        let line = app.viewer.content.file_scroll + 1;
        format!("{file_path}:{line} ")
    };

    app.viewer.clear_selection();
    app.review_state.input_buffer.set_text(&location);
    app.review_state.input_kind = CommentKind::Suggest;
    app.review_state.input_mode = ReviewInputMode::AddingComment;
    app.review_state.status_message = Some("Add comment: [s:|q:]file:line body".to_string());
}

/// Viewer パネルから、現在行のコメント詳細モーダルを開く。
pub(in crate::event) fn open_viewer_comment_detail(app: &mut App) {
    // カーソルがどの行にあるかを求める (プレビューと同じロジック)。
    let cursor_line = if let Some((start, _)) = app.viewer.selected_range() {
        start
    } else {
        app.viewer.content.file_scroll + 1
    };

    // その行にあるコメントを探す。
    let comments = match app.review_state.file_comments.get(&cursor_line) {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };

    // マスターのコメント一覧における先頭コメントのインデックスを探す。
    let target_id = &comments[0].id;
    let comment_idx = match app
        .review_state
        .comments
        .iter()
        .position(|c| &c.id == target_id)
    {
        Some(idx) => idx,
        None => return,
    };

    // キャッシュされていなければ返信を読み込む。
    let cid = target_id.clone();
    if !app.review_state.cached_replies.contains_key(&cid)
        && let Some(store) = app.review_store.as_ref()
        && let Ok(replies) = store.get_replies(&cid)
    {
        app.review_state.cached_replies.insert(cid, replies);
    }

    app.review_state.comment_detail_idx = comment_idx;
    app.review_state.comment_detail_scroll = 0;
    app.review_state.comment_detail_active = true;
}

/// 入力バッファをパースして新しいレビューコメントを追加する。
///
/// フォーマット: [s:|q:]file_path:line[-end] body_text
pub(in crate::event) fn submit_new_comment(app: &mut App, input: &str) {
    let input = input.trim();
    if input.is_empty() {
        app.review_state.status_message = Some("Empty input, cancelled.".to_string());
        return;
    }

    let (kind, rest) = if let Some(stripped) = input.strip_prefix("s:") {
        (CommentKind::Suggest, stripped)
    } else if let Some(stripped) = input.strip_prefix("q:") {
        (CommentKind::Question, stripped)
    } else {
        (app.review_state.input_kind, input)
    };

    let Some(space_pos) = rest.find(' ') else {
        app.review_state.status_message =
            Some("Format: file:line body  (e.g. src/main.rs:42 fix this)".to_string());
        return;
    };

    let location = &rest[..space_pos];
    let body = rest[space_pos + 1..].trim();

    if body.is_empty() {
        app.review_state.status_message = Some("Comment body is empty.".to_string());
        return;
    }

    let Some(colon_pos) = location.rfind(':') else {
        app.review_state.status_message =
            Some("Format: file:line body  (e.g. src/main.rs:42 fix this)".to_string());
        return;
    };

    let file_path = &location[..colon_pos];
    let line_part = &location[colon_pos + 1..];

    // 行範囲をパースする: "42" または "42-50"。
    let (line_start, line_end) = if let Some(dash_pos) = line_part.find('-') {
        let start_str = &line_part[..dash_pos];
        let end_str = &line_part[dash_pos + 1..];
        let Ok(start) = start_str.parse::<u32>() else {
            app.review_state.status_message = Some(format!("Invalid line number: '{start_str}'"));
            return;
        };
        let Ok(end) = end_str.parse::<u32>() else {
            app.review_state.status_message = Some(format!("Invalid line number: '{end_str}'"));
            return;
        };
        (start, Some(end))
    } else {
        let Ok(line) = line_part.parse::<u32>() else {
            app.review_state.status_message = Some(format!("Invalid line number: '{line_part}'"));
            return;
        };
        (line, None)
    };

    app.add_review_comment(file_path, line_start, line_end, kind, body, Author::User);
}
