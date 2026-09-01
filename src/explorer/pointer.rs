//! マウス。クリックの行解決はカーソルに任せ、外へ頼むことは [Intent] で返す。

use crate::app::OpenAs;
use crate::diff_state::DiffListEntry;
use crate::review_state::CommentListRow;
use crate::widget::click::ClickTracker;

use super::ctx::Ctx;
use super::intent::{Intent, SectionOp};
use super::keys::Panes;
use super::state::{BottomView, Explorer, Pane};

/// 下ペインの枠に置く Ask Claude ボタンの幅。描画側と共有する。
pub const ASK_CLAUDE_W: u16 = 20;

pub fn click(ex: &mut Explorer, x: u16, y: u16, ctx: &Ctx, panes: &Panes) -> Option<Intent> {
    // 枠の 1 行上も上ペイン扱いにしない。ここを削ると、パネル最大化中 (枠の
    // ドラッグ判定が無効な間) に境界行のクリックが下へ抜けて別ファイルを開く。
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
    Some(Intent::OpenFile {
        path,
        how: open_as(&mut ex.tree_clicks, at),
    })
}

/// 1 クリックは preview タブ、続けてもう一度で固定する。ツリーと変更ファイル一覧で
/// 揃える。永続タブが開いたまま溜まるのを防ぐため。
fn open_as(clicks: &mut ClickTracker, at: usize) -> OpenAs {
    if clicks.is_double(at) {
        OpenAs::Persistent
    } else {
        OpenAs::Preview
    }
}

fn changes_click(ex: &mut Explorer, y: u16, ctx: &Ctx, panes: &Panes) -> Option<Intent> {
    let len = ctx.diff.display_list.len();
    let at = ex.changes_cursor.index_at(y, len, panes.bottom)?;
    ex.changes_cursor.select(at, len, panes.bottom);

    Some(match ctx.diff.display_list.get(at)? {
        DiffListEntry::Summary {} => Intent::OpenSummary,
        DiffListEntry::Directory { .. } => Intent::Section {
            op: SectionOp::Toggle,
        },
        DiffListEntry::File { .. } => Intent::OpenSelectedChange {
            how: open_as(&mut ex.changes_clicks, at),
        },
    })
}

fn comments_click(ex: &mut Explorer, x: u16, y: u16, ctx: &Ctx, panes: &Panes) -> Option<Intent> {
    let len = ctx.review.comment_list_rows.len();
    let bottom_border = panes.bottom.top + panes.bottom.height as u16;
    if y == bottom_border
        && super::render::ask_claude_all_cols(panes.bottom_area.x, panes.bottom_area.width)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_state::{DiffState, DiffViewMode};
    use crate::review_state::ReviewState;

    /// 1 クリックは preview、続けてもう一度で固定。ツリーと変更ファイル一覧で
    /// 揃っていることを、両方の経路について押さえる。
    #[test]
    fn 変更ファイル一覧のクリックはツリーと同じ意味を持つ() {
        let theme = crate::theme::Theme::default();
        let config = crate::config::Config::default();
        let keymap = crate::keymap::KeyMap::new(&toml::Table::new());
        let review = ReviewState::new();
        let mut diff = DiffState::new("main", DiffViewMode::Unified);
        diff.display_list = vec![crate::diff_state::DiffListEntry::File {
            file_index: 0,
            depth: 0,
        }];
        let ctx = Ctx {
            theme: &theme,
            config: &config,
            keymap: &keymap,
            focused: true,
            diff: &diff,
            review: &review,
            revidere: crate::revidere::ArtifactState::None,
        };

        let panes = Panes::split(
            ratatui::layout::Rect::new(0, 0, 40, 20),
            50,
            BottomView::Changes,
            false,
        );
        let mut ex = Explorer::default();
        ex.show(BottomView::Changes);
        let y = panes.bottom.top;

        assert!(matches!(
            click(&mut ex, 0, y, &ctx, &panes),
            Some(Intent::OpenSelectedChange {
                how: OpenAs::Preview
            })
        ));
        assert!(matches!(
            click(&mut ex, 0, y, &ctx, &panes),
            Some(Intent::OpenSelectedChange {
                how: OpenAs::Persistent
            })
        ));
    }
}
