//! Explorer パネル — 中央カラムのファイルツリーブラウザ。
//!
//! 上半分に現在選択中の worktree のファイルツリーを、下半分に
//! Changes（変更ファイル一覧）と Comments（レビューコメント一覧）のいずれか
//! を表示する。どちらを出すかは [crate::explorer::state::Explorer::bottom] が
//! 決める。
//!
//! 描画の責務ごとに分割している: [file_tree] が上半分のファイルツリーを、
//! [changes] が Changes ビュー（とそのコメントバッジ）を、[comments] が
//! Comments ビュー（下部ペインと全画面 C オーバーレイの両方）を、
//! [search_field] がパネル内のファイル名検索入力欄を描画する。

use ratatui::Frame;
use ratatui::layout::Rect;

mod changes;
mod comments;
mod file_tree;
pub mod geometry;
mod search_field;

use crate::explorer::ctx::{Ctx, Paint};
use crate::explorer::keys::Panes;
use crate::explorer::state::{BottomView, Explorer};
use crate::widget::list::Viewport;

pub(crate) use changes::{changes_banner_rows, revidere_badge_cols};
pub(crate) use comments::ask_claude_all_cols;
pub use geometry::Geometry;

/// 指定領域に Explorer (ファイルツリー) パネルを描画する。
pub fn render(frame: &mut Frame, area: Rect, ex: &Explorer, ctx: &Ctx, paint: &Paint) -> Geometry {
    if area.width == 0 || area.height == 0 {
        return Geometry::default();
    }

    // 比率は設定値で、実行中に Ctrl+Alt+↑/↓ で変えられる。
    let panes = Panes::split(
        area,
        ctx.config.layout.explorer_split_pct,
        ex.bottom(),
        ctx.diff.error.is_some(),
    );

    file_tree::render(frame, panes.tree_area, panes.tree, ex, ctx, paint);
    let bottom = (panes.bottom_area, panes.bottom);
    match ex.bottom() {
        BottomView::Changes => changes::render(frame, bottom.0, bottom.1, ex, ctx, paint),
        BottomView::Comments => comments::render(frame, bottom.0, bottom.1, ex, ctx, paint),
    }

    let search_cursor = search_field::render(frame, area, ctx, paint);
    Geometry { search_cursor }
}

/// コメント一覧を中央全画面モーダル（C オーバーレイ）として描画する。
/// ブランチ上の全レビューコメントの一覧を表示し、該当箇所へジャンプできる。
/// 下部ペインと同じ描画ロジックを再利用する。
pub fn render_comments_overlay(
    frame: &mut Frame,
    area: Rect,
    ex: &Explorer,
    ctx: &Ctx,
    paint: &Paint,
) {
    // 下限を area にクランプする。そうしないと極小ターミナルで min > max になり
    // u16::clamp が panic する。
    let w = ((area.width as u32 * 70 / 100) as u16).clamp(24.min(area.width), area.width);
    let h = ((area.height as u32 * 80 / 100) as u16).clamp(6.min(area.height), area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let popup = Rect::new(x, y, w, h);
    frame.render_widget(ratatui::widgets::Clear, popup);
    comments::render(frame, popup, Viewport::inside(popup, 0), ex, ctx, paint);
}
