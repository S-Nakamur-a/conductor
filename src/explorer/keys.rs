//! キー入力。Explorer 自身の状態だけを書き換え、外に頼むことは [Intent] で返す。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};

use crate::diff_state::DiffListEntry;
use crate::keymap::{Action, KeyContext};
use crate::review_state::CommentListRow;
use crate::widget::list::Viewport;

use super::ctx::Ctx;
use super::intent::{Intent, SectionOp};
use super::state::{BottomView, Explorer, Pane};

/// 上下 2 ペインの分割。描画と入力がこの 1 つの結果を共有する。
///
/// 毎回レイアウトから作り直す値で、Explorer 自身には保存しない。保存すると、
/// まだ描画されていない状態で古い値のまま入力を処理してしまう。
pub struct Panes {
    pub tree_area: Rect,
    pub bottom_area: Rect,
    pub tree: Viewport,
    pub bottom: Viewport,
}

impl Panes {
    pub fn split(area: Rect, tree_pct: u16, bottom: BottomView, has_error: bool) -> Self {
        let chunks = Layout::vertical([
            Constraint::Percentage(tree_pct),
            Constraint::Percentage(100u16.saturating_sub(tree_pct)),
        ])
        .split(area);
        let banner = match bottom {
            BottomView::Changes => super::render::changes_banner_rows(has_error),
            BottomView::Comments => 0,
        };
        Self {
            tree_area: chunks[0],
            bottom_area: chunks[1],
            tree: Viewport::inside(chunks[0], 0),
            bottom: Viewport::inside(chunks[1], banner),
        }
    }
}

pub fn handle_key(
    ex: &mut Explorer,
    key: KeyEvent,
    ctx: &Ctx,
    panes: &Panes,
    in_modal: bool,
) -> Option<Intent> {
    // 前のフレームで中身が入れ替わっていることがある。窓の高さを知っているのは
    // ここだけなので、選択を収め直すのもここでやる。
    ex.tree_cursor
        .clamp(ex.tree.visible_indices().len(), panes.tree);
    ex.changes_cursor
        .clamp(ctx.diff.display_list.len(), panes.bottom);
    ex.comments_cursor
        .clamp(ctx.review.comment_list_rows.len(), panes.bottom);

    if in_modal && key.code == KeyCode::Esc {
        return Some(Intent::CloseModal);
    }

    match ctx.keymap.resolve(&key, KeyContext::Explorer) {
        Some(Action::ShowDiffList) => {
            ex.show(BottomView::Changes);
            return None;
        }
        Some(Action::ShowCommentList) => {
            ex.show(BottomView::Comments);
            return None;
        }
        _ => {}
    }

    match (ex.focus(), ex.bottom()) {
        (Pane::Tree, _) => tree(ex, key, ctx, panes.tree),
        (Pane::Bottom, BottomView::Changes) => changes(ex, key, ctx, panes.bottom),
        (Pane::Bottom, BottomView::Comments) => comments(ex, key, ctx, panes.bottom, in_modal),
    }
}

fn tree(ex: &mut Explorer, key: KeyEvent, ctx: &Ctx, view: Viewport) -> Option<Intent> {
    let visible = ex.tree.visible_indices();
    let len = visible.len();
    let action = ctx.keymap.resolve(&key, KeyContext::Explorer);

    match action? {
        Action::NavigateDown => ex.tree_cursor.step(1, len, view),
        Action::NavigateUp => ex.tree_cursor.step(-1, len, view),
        Action::GoToTop => ex.tree_cursor.select(0, len, view),
        Action::GoToBottom => ex.tree_cursor.select(usize::MAX, len, view),
        Action::SearchFilename => return Some(Intent::OpenFilenameSearch),
        Action::Select | Action::ExpandOrRight | Action::CollapseOrLeft => {
            let idx = *visible.get(ex.tree_cursor.selected())?;
            let entry = ex.tree.file_tree.get(idx)?;
            if !entry.is_dir {
                // 展開も折りたたみもファイルには効かない。Enter だけが意味を持つ。
                return matches!(action?, Action::Select).then(|| Intent::OpenFile {
                    path: entry.path.clone(),
                    how: crate::app::OpenAs::Persistent,
                });
            }
            match action? {
                Action::CollapseOrLeft => ex.tree.collapse_dir(idx),
                _ => {
                    ex.tree.ensure_children_loaded(idx);
                    if matches!(action?, Action::Select) {
                        ex.tree.toggle_dir(idx);
                    } else {
                        ex.tree.expand_dir(idx);
                    }
                }
            }
            // 畳んだり開いたりで可視エントリの数が変わる。
            ex.tree_cursor.clamp(ex.tree.visible_indices().len(), view);
        }
        _ => {}
    }
    None
}

fn changes(ex: &mut Explorer, key: KeyEvent, ctx: &Ctx, view: Viewport) -> Option<Intent> {
    let len = ctx.diff.display_list.len();
    let row = ex.changes_cursor.selected();

    match ctx.keymap.resolve(&key, KeyContext::ExplorerDiffList)? {
        Action::ExitSubPanel => ex.focus_pane(Pane::Tree),
        Action::NavigateDown => ex.changes_cursor.step(1, len, view),
        Action::NavigateUp => ex.changes_cursor.step(-1, len, view),
        Action::GoToTop => ex.changes_cursor.select(0, len, view),
        Action::GoToBottom => ex.changes_cursor.select(usize::MAX, len, view),
        Action::CollapseOrLeft => {
            return Some(Intent::Section {
                op: SectionOp::Collapse,
            });
        }
        Action::ExpandOrRight => {
            return Some(Intent::Section {
                op: SectionOp::Expand,
            });
        }
        Action::ToggleViewed => return Some(Intent::ToggleSelectedViewed),
        Action::Select => {
            return Some(match ctx.diff.display_list.get(row)? {
                DiffListEntry::Summary {} => Intent::OpenSummary,
                DiffListEntry::Directory { .. } => Intent::Section {
                    op: SectionOp::Toggle,
                },
                DiffListEntry::File { .. } => Intent::OpenSelectedChange {
                    how: crate::app::OpenAs::Persistent,
                },
            });
        }
        _ => {}
    }
    None
}

fn comments(
    ex: &mut Explorer,
    key: KeyEvent,
    ctx: &Ctx,
    view: Viewport,
    in_modal: bool,
) -> Option<Intent> {
    let rows = &ctx.review.comment_list_rows;
    let len = rows.len();
    let row = ex.comments_cursor.selected();

    match ctx.keymap.resolve(&key, KeyContext::ExplorerCommentList)? {
        Action::ExitSubPanel => ex.focus_pane(Pane::Tree),
        Action::NavigateDown => ex.comments_cursor.step(1, len, view),
        Action::NavigateUp => ex.comments_cursor.step(-1, len, view),
        Action::GoToTop => ex.comments_cursor.select(0, len, view),
        Action::GoToBottom => ex.comments_cursor.select(usize::MAX, len, view),
        Action::DeleteComment if len > 0 => return Some(Intent::DeleteSelectedComment),
        Action::ToggleResolve if len > 0 => return Some(Intent::ToggleCommentResolved),
        Action::EditComment => return Some(Intent::EditSelectedComment),
        Action::ViewCommentDetail => return Some(Intent::OpenSelectedCommentDetail),
        Action::ReplyToComment if len > 0 => return Some(Intent::BeginReplyToSelected),
        Action::CollapseOrLeft => match rows.get(row)? {
            // 返信の上にいるなら、まず親へ寄ってから畳む。畳んだ瞬間に自分の行が
            // 消えるので、先に行き先を決めておく必要がある。
            CommentListRow::Reply { comment_idx, .. } => {
                let parent = *comment_idx;
                if let Some(at) = rows.iter().position(
                    |r| matches!(r, CommentListRow::Comment { comment_idx } if *comment_idx == parent),
                ) {
                    ex.comments_cursor.select(at, len, view);
                }
                return Some(Intent::ToggleCommentExpansion);
            }
            CommentListRow::Comment { comment_idx } => {
                let expanded = ctx
                    .review
                    .comments
                    .get(*comment_idx)
                    .is_some_and(|c| ctx.review.expanded_comments.contains(&c.id));
                return expanded.then_some(Intent::ToggleCommentExpansion);
            }
        },
        Action::Select | Action::ExpandOrRight => match rows.get(row)? {
            CommentListRow::Comment { comment_idx } => {
                // 返信を持つコメントへの Select はスレッドを開くだけで、位置へは
                // 飛ばない。モーダルもそのまま開けておく。
                let has_replies = ctx
                    .review
                    .comments
                    .get(*comment_idx)
                    .and_then(|c| ctx.review.reply_counts.get(&c.id))
                    .is_some_and(|n| *n > 0);
                if has_replies {
                    return Some(Intent::ToggleCommentExpansion);
                }
                return Some(reveal(*comment_idx, in_modal));
            }
            CommentListRow::Reply { comment_idx, .. } => {
                return Some(reveal(*comment_idx, in_modal));
            }
        },
        _ => {}
    }
    None
}

/// 位置へ飛ぶとき、全画面モーダルの裏にいたならモーダルも閉じる。
fn reveal(comment: usize, in_modal: bool) -> Intent {
    if in_modal {
        Intent::CloseModal
    } else {
        Intent::RevealComment {
            comment,
            focus_viewer: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 描画も入力もこの 1 つの結果を使うので、バナーのぶんは両方に効く。
    #[test]
    fn エラーのバナーは下のペインを1行押し下げる() {
        let area = Rect::new(0, 0, 40, 20);
        let plain = Panes::split(area, 50, BottomView::Changes, false);
        let errored = Panes::split(area, 50, BottomView::Changes, true);
        assert_eq!(errored.bottom.top, plain.bottom.top + 1);
        assert_eq!(errored.bottom.height, plain.bottom.height - 1);
        assert_eq!(errored.tree, plain.tree);
    }

    /// コメント一覧にバナーは無いので、同じ error でもずれない。
    #[test]
    fn コメント一覧はdiffのエラーを無視する() {
        let area = Rect::new(0, 0, 40, 20);
        let a = Panes::split(area, 50, BottomView::Comments, false);
        let b = Panes::split(area, 50, BottomView::Comments, true);
        assert_eq!(a.bottom, b.bottom);
    }
}
