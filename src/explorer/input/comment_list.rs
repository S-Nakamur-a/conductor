//! Explorer コメント一覧サブパネル: レビューコメント・返信のナビゲーションと操作、
//! および選択したコメントの位置への Viewer ジャンプ。

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, Focus};
use crate::keymap::{Action, KeyContext};
use crate::overlay::ActiveOverlay;
use crate::review_state::CommentListRow;

pub fn handle_explorer_comment_list_key(app: &mut App, key: KeyEvent) -> Option<KeyEvent> {
    let row_count = app.review_state.comment_list_rows.len();
    let action = app.keymap.resolve(&key, KeyContext::ExplorerCommentList);

    // これが全画面コメント一覧モーダルの裏で動いているときは、Esc でモーダルを
    // 閉じ、コメントを選択したらそこへジャンプしてからモーダルを閉じる。
    let in_modal = app.overlays.active == ActiveOverlay::CommentList;
    if in_modal && key.code == KeyCode::Esc {
        app.overlays.active = ActiveOverlay::None;
        return None;
    }
    // Select が実際に位置へジャンプしたときだけモーダルを閉じる — 返信を持つ
    // コメントへの Select はその場でスレッドを開くだけなので、その場合は
    // モーダルを開いたままにしておく必要がある。
    let close_after = in_modal && matches!(action, Some(Action::Select)) && {
        let visual = app.explorer.comment_list_selected;
        match app.review_state.comment_list_rows.get(visual) {
            Some(CommentListRow::Comment { comment_idx }) => {
                let has_replies = app
                    .review_state
                    .comments
                    .get(*comment_idx)
                    .and_then(|c| app.review_state.reply_counts.get(&c.id))
                    .copied()
                    .unwrap_or(0)
                    > 0;
                !has_replies
            }
            Some(CommentListRow::Reply { .. }) => true,
            None => false,
        }
    };

    match action {
        Some(Action::ExitSubPanel) => {
            app.explorer.focus_on_diff_list = false;
        }
        Some(Action::DeleteComment) if row_count > 0 => {
            app.request_delete_selected_review_item();
        }
        Some(Action::ToggleResolve) if row_count > 0 => {
            app.toggle_selected_review_status();
        }
        Some(Action::EditComment) => {
            app.start_edit_selected_review_item();
        }
        Some(Action::ReplyToComment) if row_count > 0 => {
            let comment_idx = app
                .review_state
                .selected_comment_idx(app.explorer.comment_list_selected);
            if let Some(idx) = comment_idx {
                app.review_state.input_buffer.clear();
                app.review_state.input_mode =
                    crate::review_state::ReviewInputMode::ReplyingToComment;
                app.review_state.selected = idx;
                app.review_state.status_message =
                    Some("Reply to comment (Enter to send, Esc to cancel)".to_string());
            }
        }
        Some(Action::NavigateDown)
            if row_count > 0 && app.explorer.comment_list_selected + 1 < row_count =>
        {
            app.explorer.comment_list_selected += 1;
        }
        Some(Action::NavigateUp) if app.explorer.comment_list_selected > 0 => {
            app.explorer.comment_list_selected -= 1;
        }
        Some(Action::GoToTop) => {
            app.explorer.comment_list_selected = 0;
        }
        Some(Action::GoToBottom) if row_count > 0 => {
            app.explorer.comment_list_selected = row_count - 1;
        }
        Some(Action::CollapseOrLeft) => {
            let visual = app.explorer.comment_list_selected;
            match app.review_state.comment_list_rows.get(visual).cloned() {
                Some(CommentListRow::Reply { comment_idx, .. }) => {
                    if let Some(parent_visual) = app
                        .review_state
                        .comment_list_rows
                        .iter()
                        .position(|r| matches!(r, CommentListRow::Comment { comment_idx: ci } if *ci == comment_idx))
                    {
                        app.explorer.comment_list_selected = parent_visual;
                    }
                    app.toggle_comment_expansion();
                }
                Some(CommentListRow::Comment { comment_idx }) => {
                    if let Some(comment) = app.review_state.comments.get(comment_idx)
                        && app.review_state.expanded_comments.contains(&comment.id)
                    {
                        app.toggle_comment_expansion();
                    }
                }
                None => {}
            }
        }
        Some(Action::Select) | Some(Action::ExpandOrRight) => {
            let visual = app.explorer.comment_list_selected;
            match app.review_state.comment_list_rows.get(visual).cloned() {
                Some(CommentListRow::Comment { comment_idx }) => {
                    let has_replies = app
                        .review_state
                        .comments
                        .get(comment_idx)
                        .and_then(|c| app.review_state.reply_counts.get(&c.id))
                        .copied()
                        .unwrap_or(0)
                        > 0;

                    if has_replies {
                        app.toggle_comment_expansion();
                    } else {
                        navigate_to_comment_with_focus(app, comment_idx, false);
                    }
                }
                Some(CommentListRow::Reply { comment_idx, .. }) => {
                    navigate_to_comment_with_focus(app, comment_idx, false);
                }
                None => {}
            }
        }
        Some(Action::ViewCommentDetail) => {
            let visual = app.explorer.comment_list_selected;
            if let Some(comment_idx) = app.review_state.selected_comment_idx(visual) {
                app.review_state.comment_detail_idx = comment_idx;
                app.review_state.comment_detail_scroll = 0;
                app.review_state.comment_detail_active = true;
                if let Some(comment) = app.review_state.comments.get(comment_idx) {
                    let cid = comment.id.clone();
                    if !app.review_state.cached_replies.contains_key(&cid)
                        && let Some(store) = app.review_store.as_ref()
                        && let Ok(replies) = store.get_replies(&cid)
                    {
                        app.review_state.cached_replies.insert(cid, replies);
                    }
                }
            }
        }
        _ => {}
    }

    if close_after {
        app.overlays.active = ActiveOverlay::None;
    }

    // コメント一覧のスクロールを調整する。
    let selected = app.explorer.comment_list_selected;
    let page_size = app.explorer.diff_list_height.max(1);
    if selected < app.explorer.comment_list_scroll {
        app.explorer.comment_list_scroll = selected;
    } else if selected >= app.explorer.comment_list_scroll + page_size {
        app.explorer.comment_list_scroll = selected.saturating_sub(page_size - 1);
    }
    None
}

/// 指定インデックスのコメントのファイルと行へ移動する。
/// focus_viewer が true ならフォーカスを Viewer パネルへ移す。
/// そうでなければ現在のパネルフォーカスを維持する (コメント一覧など)。
pub(in crate::explorer) fn navigate_to_comment_with_focus(
    app: &mut App,
    comment_idx: usize,
    focus_viewer: bool,
) {
    let Some(comment) = app.review_state.comments.get(comment_idx) else {
        return;
    };
    let file_path = comment.file_path.clone();
    let line = comment.line_start as usize;
    let tab_width = app.config.viewer.tab_width;
    app.viewer
        .open_file(app.explorer.root(), &file_path, tab_width);
    app.rehighlight_viewer();
    app.viewer.content.file_scroll = line.saturating_sub(1);
    // コメントはソース行に紐づくので source を表示する: markdown レンダリング
    // だと本文の先頭に飛ばされ、選択箇所が見えなくなってしまう。
    app.viewer.show_raw_for_line_target();
    app.viewer.selection = crate::viewer::LineSelection::Selected {
        start: line,
        end: line,
    };
    app.review_state.build_file_comment_cache(&file_path);
    if focus_viewer {
        app.set_focus(Focus::Viewer);
    }
}
