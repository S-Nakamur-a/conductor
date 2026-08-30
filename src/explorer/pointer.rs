//! マウス。クリックの行解決はカーソルに任せ、外へ頼むことは [Intent] で返す。

use crate::diff_state::DiffListEntry;
use crate::review_state::CommentListRow;

use super::ctx::Ctx;
use super::intent::{Intent, SectionOp};
use super::keys::Panes;
use super::state::{BottomView, Explorer, Pane};

/// 下ペインの枠に置く Ask Claude ボタンの幅。描画側と共有する。
pub const ASK_CLAUDE_W: u16 = 20;

pub fn click(ex: &mut Explorer, x: u16, y: u16, ctx: &Ctx, panes: &Panes) -> Option<Intent> {
    if y < panes.bottom.top.saturating_sub(1) {
        ex.focus_pane(Pane::Tree);
        return tree_click(ex, y, panes);
    }
    ex.focus_pane(Pane::Bottom);
    match ex.bottom() {
        BottomView::Changes => changes_click(ex, y, ctx, panes),
        BottomView::Comments => comments_click(ex, x, y, ctx, panes),
    }
}

fn tree_click(ex: &mut Explorer, y: u16, panes: &Panes) -> Option<Intent> {
    let visible = ex.tree.visible_indices();
    let at = ex.tree_cursor.index_at(y, visible.len(), panes.tree)?;
    ex.tree_cursor.select(at, visible.len(), panes.tree);

    let idx = *visible.get(at)?;
    let entry = ex.tree.file_tree.get(idx)?;
    if entry.is_dir {
        // ディレクトリは 1 クリックで開閉する。ここだけダブルクリックを見ない。
        ex.tree.ensure_children_loaded(idx);
        ex.tree.toggle_dir(idx);
        ex.tree_cursor
            .clamp(ex.tree.visible_indices().len(), panes.tree);
        return None;
    }

    let path = entry.path.clone();
    // 1 クリックは preview タブ。永続タブが開いたまま溜まるのを防ぐ。
    Some(if ex.tree_clicks.is_double(at) {
        Intent::OpenFile { path }
    } else {
        Intent::PreviewFile { path }
    })
}

fn changes_click(ex: &mut Explorer, y: u16, ctx: &Ctx, panes: &Panes) -> Option<Intent> {
    let len = ctx.diff.display_list.len();
    let at = ex.changes_cursor.index_at(y, len, panes.bottom)?;
    ex.changes_cursor.select(at, len, panes.bottom);

    // 変更ファイル一覧はダブルクリックを見ない。常に 1 クリックで開き、常に
    // Viewer へフォーカスが移る (ツリーと非対称)。
    Some(match ctx.diff.display_list.get(at)? {
        DiffListEntry::Summary {} => Intent::OpenSummary,
        DiffListEntry::Directory { .. } => Intent::Section {
            op: SectionOp::Toggle,
        },
        DiffListEntry::File { .. } => Intent::OpenSelectedChange,
    })
}

fn comments_click(ex: &mut Explorer, x: u16, y: u16, ctx: &Ctx, panes: &Panes) -> Option<Intent> {
    let len = ctx.review.comment_list_rows.len();
    let bottom_border = panes.bottom.top + panes.bottom.height as u16;
    if y == bottom_border
        && super::render::ask_claude_all_cols(
            panes.bottom_right.saturating_sub(panes.bottom_width),
            panes.bottom_width,
        )
        .contains(&x)
    {
        return Some(Intent::AskClaudeAboutChanges);
    }

    let at = ex.comments_cursor.index_at(y, len, panes.bottom)?;
    ex.comments_cursor.select(at, len, panes.bottom);

    let comment = match ctx.review.comment_list_rows.get(at)? {
        CommentListRow::Comment { comment_idx } | CommentListRow::Reply { comment_idx, .. } => {
            *comment_idx
        }
    };
    Some(Intent::RevealComment {
        comment,
        focus_viewer: ex.comment_clicks.is_double(at),
    })
}

/// ホイール。選択は動かさず窓だけ動かす。
pub fn scroll(ex: &mut Explorer, lines: isize, y: u16, ctx: &Ctx, panes: &Panes) {
    if y < panes.bottom.top.saturating_sub(1) {
        let len = ex.tree.visible_indices().len();
        ex.tree_cursor.pan(lines, len, panes.tree);
        return;
    }
    match ex.bottom() {
        BottomView::Changes => {
            ex.changes_cursor
                .pan(lines, ctx.diff.display_list.len(), panes.bottom)
        }
        BottomView::Comments => {
            ex.comments_cursor
                .pan(lines, ctx.review.comment_list_rows.len(), panes.bottom)
        }
    }
}
