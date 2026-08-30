//! Viewer パネル向けのインラインコメントスレッド開閉と返信入力
//! （プレーンファイルビューと統合 diff ビューの両方のキーハンドラから使われる）。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, StatusLevel};

/// 現在のカーソル行のインラインスレッド展開を切り替える。
pub(super) fn toggle_inline_thread(app: &mut App) {
    let cursor_line = if let Some((start, _)) = app.viewer.selected_range() {
        start
    } else {
        app.viewer.content.file_scroll + 1
    };

    // その行にコメントがある場合のみ切り替える。共有ヘルパーは範囲の途中の行を
    // そのスレッドの終端行のアンカーへリダイレクトし、マウス操作の挙動と合わせる
    // （diff ビューではスレッドを終端行にしか描画しないため）。
    if !app.review_state.file_comments.contains_key(&cursor_line) {
        return;
    }
    crate::event::mouse::toggle_inline_thread_at(app, cursor_line);
}

/// 現在のカーソル行に対してインライン返信モードを開始する。
///
/// スレッドがまだ展開されていなければ先に展開し、返信を読み込む。その行の
/// 最初のコメントを対象にする。すでにその行のコメントに返信中の場合は、
/// 次のコメントに切り替える（1行に複数コメントがある場合のため）。
pub(super) fn start_inline_reply(app: &mut App) {
    let cursor_line = if let Some((start, _)) = app.viewer.selected_range() {
        start
    } else {
        app.viewer.content.file_scroll + 1
    };

    let comments = match app.review_state.file_comments.get(&cursor_line) {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };

    // まだ展開されていなければスレッドを自動展開する。
    if !app.viewer.inline.expanded.contains(&cursor_line) {
        app.viewer.inline.expanded.insert(cursor_line);
        // キャッシュされていなければ返信を読み込む。
        for comment in comments {
            if !app.review_state.cached_replies.contains_key(&comment.id)
                && let Some(store) = app.review_store.as_ref()
                && let Ok(replies) = store.get_replies(&comment.id)
            {
                app.review_state
                    .cached_replies
                    .insert(comment.id.clone(), replies);
            }
        }
    }

    // すでにこの行に返信中なら、次のコメントに切り替える。
    let target_id = if app.viewer.inline.reply_line == Some(cursor_line) {
        if let Some(current_id) = &app.viewer.inline.reply_comment_id {
            let current_pos = comments.iter().position(|c| &c.id == current_id);
            match current_pos {
                Some(pos) if pos + 1 < comments.len() => comments[pos + 1].id.clone(),
                _ => comments[0].id.clone(),
            }
        } else {
            comments[0].id.clone()
        }
    } else {
        comments[0].id.clone()
    };

    app.viewer.inline.reply_line = Some(cursor_line);
    app.viewer.inline.reply_comment_id = Some(target_id);
    app.viewer.inline.reply_buffer.clear();
}

/// インライン返信の入力モードでのキー操作を処理する。
pub(super) fn handle_inline_reply_input(app: &mut App, key: KeyEvent) {
    // Shift+Enter は改行を挿入し、通常の Enter は送信する — コメント作成
    // モーダルと同じ規約にすることで、インライン返信も本格的な複数行フォーム
    // になる。
    if key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT) {
        app.viewer.inline.reply_buffer.insert_char('\n');
        return;
    }
    match key.code {
        KeyCode::Esc => {
            // 返信をキャンセルする。
            app.viewer.inline.reply_line = None;
            app.viewer.inline.reply_comment_id = None;
            app.viewer.inline.reply_buffer.clear();
        }
        KeyCode::Enter => {
            // 返信を送信する。
            if app.viewer.inline.reply_line.is_none() {
                return;
            }
            let body = app.viewer.inline.reply_buffer.text().to_string();
            if body.trim().is_empty() {
                app.viewer.inline.reply_line = None;
                app.viewer.inline.reply_comment_id = None;
                app.viewer.inline.reply_buffer.clear();
                return;
            }

            // 明示的に追跡しているコメント ID を使う。
            let review_id = app.viewer.inline.reply_comment_id.clone();

            if let Some(review_id) = review_id {
                // store の借用をスコープ内に限定して DB 操作を行う。
                let result = if let Some(store) = app.review_store.as_ref() {
                    match store.add_reply(&review_id, &body, crate::review_store::Author::User) {
                        Ok(()) => {
                            let replies = store.get_replies(&review_id).ok();
                            let wt = app.selected_worktree_branch();
                            let counts = store.reply_counts_for_worktree(&wt).ok();
                            Ok((replies, counts))
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    Err(anyhow::anyhow!("No review store"))
                };

                match result {
                    Ok((replies, counts)) => {
                        app.set_status("Reply added.".to_string(), StatusLevel::Success);
                        if let Some(replies) = replies {
                            app.review_state.cached_replies.insert(review_id, replies);
                        }
                        if let Some(counts) = counts {
                            app.review_state.reply_counts = counts;
                        }
                    }
                    Err(e) => {
                        app.set_status(format!("Error: {e}"), StatusLevel::Error);
                    }
                }
            }

            app.viewer.inline.reply_line = None;
            app.viewer.inline.reply_comment_id = None;
            app.viewer.inline.reply_buffer.clear();
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            app.viewer.inline.reply_buffer.delete_to_line_start();
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            crate::event::clipboard_paste(app, |a| &mut a.viewer.inline.reply_buffer, true);
        }
        _ => {
            // 通常の編集操作（文字入力、矢印、Home/End、単語単位移動、Backspace/Delete）。
            app.viewer.inline.reply_buffer.handle_key(key);
        }
    }
}
