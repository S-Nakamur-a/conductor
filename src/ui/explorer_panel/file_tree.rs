//! Rendering of the explorer's top-half file tree.

use crate::app::{App, Focus};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    List, ListItem, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

/// Cached indent strings by depth level to avoid repeated allocation.
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

/// Get an indent string for a given depth, using cache for common depths.
fn indent_for_depth(depth: usize) -> std::borrow::Cow<'static, str> {
    if depth < INDENT_CACHE.len() {
        std::borrow::Cow::Borrowed(INDENT_CACHE[depth])
    } else {
        std::borrow::Cow::Owned("  ".repeat(depth))
    }
}

/// Render the file tree (top half).
pub(super) fn render_file_tree(frame: &mut Frame, area: Rect, app: &mut App, panel_focused: bool) {
    let on_diff = app.viewer_state.explorer.explorer_focus_on_diff_list;
    let tree_focused = panel_focused && !on_diff;
    // Glide the column-level focus color; the tree is the "active" element when
    // not focused on the diff list, so it eases both when the column gains and
    // when it loses focus. The inactive sub-panel keeps the static secondary tint.
    let border_color = if tree_focused {
        app.animated_border_color(Focus::Explorer)
    } else if panel_focused {
        app.theme.border_secondary
    } else if !on_diff {
        app.animated_border_color(Focus::Explorer)
    } else {
        app.theme.border_unfocused
    };

    let visible = app.viewer_state.visible_indices();
    let inner_height = area.height.saturating_sub(2) as usize;

    let tree_selected = app.viewer_state.tree.tree_selected;
    let selected_vis_idx = visible
        .iter()
        .position(|&i| i == tree_selected)
        .unwrap_or(0);

    let mut title = if visible.len() > inner_height {
        format!(" Explorer ({}/{}) ", selected_vis_idx + 1, visible.len())
    } else {
        " Explorer ".to_string()
    };
    // Walkthrough-ready signal, mirroring the comment badge's "there's
    // something here you haven't opened" pattern — hidden once the
    // walkthrough view is already showing since the badge would be redundant.
    let walkthrough_ready = matches!(
        app.walkthrough.current.as_ref().map(|wt| wt.header.status),
        Some(crate::walkthrough::WalkthroughStatus::Ready)
    );
    if walkthrough_ready
        && app.viewer_state.explorer.explorer_bottom_view
            != crate::viewer::ExplorerBottomView::Walkthrough
    {
        title.push_str("\u{1f9ed} ");
    }

    let theme = &app.theme;
    // ボーダーの太さは Explorer カラム全体のフォーカスで、タイトルの強調は
    // その中でツリー側にフォーカスがあるかで決まる (下半分と共有のカラムなので)。
    let title_style = if tree_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let block = crate::ui::common::PanelChrome::new(theme, title, panel_focused, border_color)
        .with_expand_button(app.expanded_panel == Some(Focus::Explorer))
        .with_title_style(title_style)
        .into_block();

    let scroll = app.viewer_state.tree.tree_scroll;

    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .filter_map(|(vis_idx, &tree_idx)| {
            let entry = app.viewer_state.tree.file_tree.get(tree_idx)?;
            let indent = indent_for_depth(entry.depth);

            // Split from the name so the hover underline can be confined to
            // the name itself (see `list_row::decoration_style`).
            let prefix = if entry.is_dir {
                let arrow = if entry.is_expanded {
                    "\u{25bc}" // ▼
                } else {
                    "\u{25b6}" // ▶
                };
                format!("{indent}{arrow} {} ", entry.icon)
            } else {
                format!("{indent}  {} ", entry.icon)
            };

            // Untracked/ignored entries dim regardless of file-vs-directory,
            // taking priority over the directory/file color split below.
            // `theme.muted` is deliberately avoided here: it's the same RGB
            // as the background on solarized-dark (effectively invisible)
            // and reads as a border color on github-light — see S4 in the
            // plan doc.
            let base_fg = match entry.git_state {
                crate::git_engine::status_map::TreeGitState::Untracked
                | crate::git_engine::status_map::TreeGitState::Ignored => theme.hint,
                crate::git_engine::status_map::TreeGitState::Tracked => {
                    if entry.is_dir {
                        theme.info
                    } else {
                        theme.fg
                    }
                }
            };
            let hover = app.list_hover.explorer_tree.phase(vis_idx);
            let style = crate::ui::common::list_row::row_style(
                theme,
                base_fg,
                vis_idx == selected_vis_idx,
                tree_focused,
                hover,
            );

            Some(ListItem::new(Line::from(vec![
                Span::styled(
                    prefix,
                    crate::ui::common::list_row::decoration_style(style),
                ),
                Span::styled(entry.name.clone(), style),
            ])))
        })
        .collect();

    // Clear first so rows below the last item (or stale rows after scrolling /
    // a height change) don't show the previous frame's glyphs — the same
    // scroll-bleed guard the viewer uses.
    frame.render_widget(ratatui::widgets::Clear, area);
    let list = List::new(items).block(block);
    frame.render_widget(list, area);

    // Render scrollbar when there are more items than fit in the panel.
    if visible.len() > inner_height {
        let inner_area = area.inner(ratatui::layout::Margin {
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
