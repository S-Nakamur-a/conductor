//! The AI walkthrough view — one of the Explorer's three bottom-pane views
//! (alongside the diff list and comment list; see
//! `viewer::ExplorerBottomView`).
//!
//! Renders as a single flat list, one row per step, with only the selected
//! step's body inlined (word-wrapped, clipped) directly below its row —
//! there is no fixed-percentage split between "list" and "body" areas, so a
//! narrow pane still shows as much of the selected step as fits. The full
//! body is available via the `space` detail overlay for anything the clip
//! cuts off.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Focus};
use crate::walkthrough::{WalkthroughStatus, WalkthroughStep, WalkthroughStepKind};

/// Render the walkthrough view into the Explorer's bottom pane.
pub fn render(frame: &mut Frame, area: Rect, app: &App, panel_focused: bool) {
    let theme = &app.theme;
    let vs_explorer = &app.viewer_state.explorer;
    let list_focused = panel_focused && vs_explorer.explorer_focus_on_diff_list;
    let border_color = if list_focused {
        app.animated_border_color(Focus::Explorer)
    } else if panel_focused {
        theme.border_secondary
    } else {
        theme.border_unfocused
    };
    let border_type = if panel_focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    };
    let title_style = if list_focused {
        Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.muted)
    };

    let title = match app.current_walkthrough.as_ref().and_then(|(w, _)| w.title.as_deref()) {
        Some(t) => format!(" Walkthrough: {t} "),
        None => " Walkthrough ".to_string(),
    };
    let block = Block::default()
        .title(Span::styled(title, title_style))
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    let Some((walkthrough, steps)) = &app.current_walkthrough else {
        let paragraph = Paragraph::new("No walkthrough yet — palette: Generate Walkthrough")
            .style(Style::default().fg(theme.muted))
            .block(block);
        frame.render_widget(Clear, area);
        frame.render_widget(paragraph, area);
        return;
    };

    match walkthrough.status {
        WalkthroughStatus::Generating => {
            let paragraph = Paragraph::new("Generating walkthrough… (this takes a few minutes)")
                .style(Style::default().fg(theme.info))
                .block(block);
            frame.render_widget(Clear, area);
            frame.render_widget(paragraph, area);
        }
        WalkthroughStatus::Failed => {
            let error = walkthrough.error.as_deref().unwrap_or("unknown error");
            let paragraph = Paragraph::new(vec![
                Line::from(Span::styled(
                    format!("Generation failed: {error}"),
                    Style::default().fg(theme.error),
                )),
                Line::from(Span::styled(
                    "palette: Generate Walkthrough to retry",
                    Style::default().fg(theme.muted),
                )),
            ])
            .block(block);
            frame.render_widget(Clear, area);
            frame.render_widget(paragraph, area);
        }
        WalkthroughStatus::Ready => {
            render_steps(frame, area, app, block, steps, list_focused);
        }
    }
}

/// The icon shown next to a walkthrough step, matching this UI's existing
/// emoji-badge convention (comment badges, file-tree icons, …). Shared with
/// the Viewer's walkthrough step banner (`ui::viewer_panel`).
pub(crate) fn step_icon(kind: WalkthroughStepKind) -> &'static str {
    match kind {
        WalkthroughStepKind::Intent => "\u{1f3af}", // 🎯
        WalkthroughStepKind::Core => "\u{1f527}",   // 🔧
        WalkthroughStepKind::Ripple => "\u{1f30a}", // 🌊
        WalkthroughStepKind::Test => "\u{1f9ea}",   // 🧪
    }
}

/// Greedily word-wrap `text` to `width` columns, splitting on existing
/// newlines first so intentional paragraph breaks in the step body survive.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for para in text.lines() {
        if para.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in para.split_whitespace() {
            let candidate_len = if current.is_empty() {
                word.len()
            } else {
                current.len() + 1 + word.len()
            };
            if candidate_len > width && !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        out.push(current);
    }
    out
}

/// The ready walkthrough's flat step list: one row per step, with the
/// selected step's word-wrapped body inlined directly below its row (clipped
/// to at most 6 lines and whatever fits the remaining pane height). Scrolling
/// is in step units — `walkthrough_scroll`/`walkthrough_selected` share the
/// same index space the diff list uses, and since only the selected step
/// ever expands, every row before it is exactly one line, so the clamp in
/// `event::adjust_walkthrough_scroll` keeps the selected step's header
/// visible.
fn render_steps(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    block: Block,
    steps: &[WalkthroughStep],
    focused: bool,
) {
    let theme = &app.theme;
    let selected = app.viewer_state.explorer.walkthrough_selected;
    let scroll = app.viewer_state.explorer.walkthrough_scroll;
    let viewed_steps = &app.viewer_state.explorer.viewed_steps;

    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let inner_height = inner.height as usize;
    let body_indent = "    ";
    let wrap_width = (inner.width as usize).saturating_sub(body_indent.len()).max(1);

    let mut items: Vec<ListItem> = Vec::new();
    let mut consumed = 0usize;
    const MAX_BODY_LINES: usize = 6;

    for (idx, step) in steps.iter().enumerate().skip(scroll) {
        if consumed >= inner_height {
            break;
        }
        let is_current = idx == selected;
        let is_viewed = viewed_steps.contains(&step.id);
        let style = if is_current && focused {
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selected_bg)
                .add_modifier(Modifier::BOLD)
        } else if is_current {
            Style::default()
                .fg(theme.selected_fg_inactive)
                .bg(theme.selected_bg_inactive)
                .add_modifier(Modifier::BOLD)
        } else if is_viewed {
            Style::default().fg(theme.muted)
        } else {
            Style::default().fg(theme.fg)
        };
        let filename = step.file_path.rsplit('/').next().unwrap_or(&step.file_path);
        items.push(ListItem::new(Span::styled(
            format!(
                "  {} {} — {} ({filename})",
                step_icon(step.kind),
                step.kind,
                step.title
            ),
            style,
        )));
        consumed += 1;

        if is_current {
            let budget = (inner_height - consumed).min(MAX_BODY_LINES);
            for wrapped_line in wrap_text(&step.body, wrap_width).into_iter().take(budget) {
                items.push(ListItem::new(Span::styled(
                    format!("{body_indent}{wrapped_line}"),
                    Style::default().fg(theme.fg),
                )));
                consumed += 1;
            }
        }
    }

    frame.render_widget(List::new(items), inner);
}

/// Full-text detail overlay for the selected walkthrough step (`space` in the
/// walkthrough view — the same detail-overlay pattern the comment list uses
/// for `view_comment_detail`, applied to a step's untruncated body).
pub fn render_detail_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = &app.theme;
    let Some((_, steps)) = &app.current_walkthrough else {
        app.viewer_state.explorer.walkthrough_detail_active = false;
        return;
    };
    let selected = app.viewer_state.explorer.walkthrough_selected;
    let Some(step) = steps.get(selected) else {
        app.viewer_state.explorer.walkthrough_detail_active = false;
        return;
    };

    let popup_width = 72_u16.min(area.width.saturating_sub(4));
    let popup_height = area.height.saturating_sub(4).max(10);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let filename = step.file_path.rsplit('/').next().unwrap_or(&step.file_path);
    let title = format!(
        " {} {} \u{2502} {filename} (Esc/q/space: close) ",
        step_icon(step.kind),
        step.kind
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut lines = vec![
        Line::from(Span::styled(
            step.title.clone(),
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    lines.extend(step.body.lines().map(|l| Line::from(l.to_string())));

    let paragraph = Paragraph::new(lines)
        .style(Style::default().fg(theme.fg))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}
