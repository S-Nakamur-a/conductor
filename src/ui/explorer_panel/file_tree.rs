//! Rendering of the explorer's top-half file tree.

use crate::app::{App, Focus};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListItem, Scrollbar, ScrollbarOrientation, ScrollbarState,
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
        app.current_walkthrough.as_ref().map(|(w, _)| w.status),
        Some(crate::walkthrough::WalkthroughStatus::Ready)
    );
    if walkthrough_ready
        && app.viewer_state.explorer.explorer_bottom_view != crate::viewer::ExplorerBottomView::Walkthrough
    {
        title.push_str("\u{1f9ed} ");
    }

    let is_expanded = app.expanded_panel == Some(Focus::Explorer);
    let theme = &app.theme;
    let (expand_label, expand_color) = if is_expanded {
        ("[>=<]", theme.border_focused)
    } else {
        ("[<=>]", theme.border_unfocused)
    };

    let border_type = if panel_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };

    let title_style = if tree_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };
    let block = Block::default()
        .title(Span::styled(title, title_style))
        .title_top(
            Line::from(Span::styled(
                expand_label,
                Style::default().fg(expand_color),
            ))
            .alignment(Alignment::Right),
        )
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    let scroll = app.viewer_state.tree.tree_scroll;

    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .filter_map(|(vis_idx, &tree_idx)| {
            let entry = app.viewer_state.tree.file_tree.get(tree_idx)?;
            let indent = indent_for_depth(entry.depth);

            let label = if entry.is_dir {
                let arrow = if entry.is_expanded {
                    "\u{25bc}" // ▼
                } else {
                    "\u{25b6}" // ▶
                };
                format!("{indent}{arrow} {} {}", entry.icon, entry.name)
            } else {
                format!("{indent}  {} {}", entry.icon, entry.name)
            };

            let style = if vis_idx == selected_vis_idx && tree_focused {
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else if vis_idx == selected_vis_idx {
                Style::default()
                    .fg(theme.selected_fg_inactive)
                    .bg(theme.selected_bg_inactive)
                    .add_modifier(Modifier::BOLD)
            } else if entry.is_dir {
                Style::default().fg(theme.info)
            } else {
                Style::default().fg(theme.fg)
            };

            Some(ListItem::new(Span::styled(label, style)))
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
