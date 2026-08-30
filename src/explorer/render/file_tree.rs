//! ファイルツリー（Explorer 上半分）の描画。
//!
//! 選択位置は [crate::widget::list::ListCursor] が可視リストの添字でそのまま
//! 持つので、フラット添字から可視添字を線形探索する手順は要らない。

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{List, ListItem, Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::explorer::ctx::{Ctx, Paint};
use crate::explorer::state::{Explorer, Pane};
use crate::git_engine::status_map::TreeGitState;
use crate::revidere::ArtifactState;
use crate::widget::list::Viewport;
use crate::widget::row::{Row, Segment};

/// 深さレベルごとのインデント文字列のキャッシュ。確保の繰り返しを避ける。
const INDENT_CACHE: &[&str] = &[
    "",
    "  ",
    "    ",
    "      ",
    "        ",
    "          ",
    "            ",
    "              ",
    "                ",
    "                  ",
];

/// 指定した深さのインデント文字列を返す。よくある深さはキャッシュを使う。
fn indent_for_depth(depth: usize) -> Cow<'static, str> {
    if depth < INDENT_CACHE.len() {
        Cow::Borrowed(INDENT_CACHE[depth])
    } else {
        Cow::Owned("  ".repeat(depth))
    }
}

/// ファイルツリーを描画する。
pub(super) fn render(frame: &mut Frame, area: Rect, ex: &Explorer, ctx: &Ctx, paint: &Paint) {
    let theme = ctx.theme;
    let icon_set = ctx.config.ui.icon_set();
    let tree_focused = ctx.focused && ex.focus() == Pane::Tree;

    let visible = ex.tree.visible_indices();
    let inner_height = area.height.saturating_sub(2) as usize;
    let view = Viewport::new(area.y + 1, inner_height);

    let selected = ex.tree_cursor.selected();
    let scroll = ex.tree_cursor.scroll();

    let panel_icon = crate::icons::PANEL_EXPLORER.labeled(icon_set);
    let mut title = if visible.len() > inner_height {
        format!(
            " {panel_icon}Explorer ({}/{}) ",
            selected + 1,
            visible.len()
        )
    } else {
        format!(" {panel_icon}Explorer ")
    };
    // レビューの成果物があるサイン。コメントバッジの「まだ開いていないものが
    // ある」というパターンを踏襲する。2 列ビューを開いている間はここが
    // 描かれないので、冗長になる心配は無い。
    if ctx.revidere != ArtifactState::None {
        title.push_str(&crate::icons::PANEL_REVIEW.labeled(icon_set));
    }

    let title_style = if tree_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let block = crate::ui::common::PanelChrome::new(theme, title, ctx.focused, paint.border)
        .with_expand_button(paint.expanded)
        .with_title_style(title_style)
        .into_block();

    let range = ex.tree_cursor.visible(visible.len(), view);
    let items: Vec<ListItem> = range
        .clone()
        .filter_map(|vis_idx| {
            let tree_idx = *visible.get(vis_idx)?;
            let entry = ex.tree.file_tree.get(tree_idx)?;
            let indent = indent_for_depth(entry.depth);

            // 名前部分と切り離すことで、hover 時の下線を名前自体に限定できる
            // (widget::row::decoration_style を参照)。アイコンをさらに分けて
            // いるのは種別ごとの色を乗せるため。
            let prefix = if entry.is_dir {
                let arrow = crate::icons::expand_arrow(entry.is_expanded, icon_set);
                format!("{indent}{arrow} ")
            } else {
                format!("{indent}  ")
            };
            let icon = if entry.is_dir {
                crate::icons::dir_icon(entry.is_expanded)
            } else {
                entry.icon
            };
            let glyph = format!("{} ", icon.glyph(icon_set));

            // 未追跡/無視エントリはファイルかディレクトリかに関わらず暗く表示し、
            // 下のディレクトリ/ファイルの色分けより優先する。theme.muted は
            // 一部テーマで背景と同化するか枠線色と紛れるため、ここでは避ける。
            let base_fg = match entry.git_state {
                TreeGitState::Untracked | TreeGitState::Ignored => theme.hint,
                TreeGitState::Tracked => {
                    if entry.is_dir {
                        theme.info
                    } else {
                        theme.fg
                    }
                }
            };
            let selected_row = vis_idx == selected;
            // アイコンの色はファイル種別を表すが、選択行と減光対象の行では行の色に
            // 譲る。選択の背景色の上で種別色が読める保証は全テーマぶんには無く、
            // untracked/ignored の減光はアイコンにも及ぶべきだからである。
            let icon_fg = if selected_row || entry.git_state != TreeGitState::Tracked {
                None
            } else {
                Some(icon.role.color(theme))
            };

            let hover = paint.hover_tree.phase(vis_idx);
            let line = Row::new(entry.name.clone(), base_fg)
                .lead([
                    Segment::plain(prefix),
                    Segment {
                        text: glyph.into(),
                        fg: icon_fg,
                        bold: false,
                    },
                ])
                .into_line(theme, selected_row, tree_focused, hover);
            Some(ListItem::new(line))
        })
        .collect();

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(List::new(items).block(block), area);

    if visible.len() > inner_height {
        let inner_area = area.inner(Margin {
            horizontal: 0,
            vertical: 1,
        });
        let mut scrollbar_state =
            ScrollbarState::new(visible.len().saturating_sub(inner_height)).position(scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        frame.render_stateful_widget(scrollbar, inner_area, &mut scrollbar_state);
    }
}
