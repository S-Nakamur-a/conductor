//! Hover-info popup — renders the symbol's signature, doc comment, and a
//! clickable reference count (LSP-`hover`-style), and, when the user clicks
//! into it, the interactive references list (level 1) and code preview (level
//! 2). The popup anchors to the hovered symbol within the Viewer panel.
//!
//! Each level records its rendered `Rect` (and the refs list its per-row rects)
//! back onto `app.hover_info_overlay` so the mouse layer can hit-test clicks.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::App;

/// Render the hover popup and any open child levels over `area` (the frame).
pub fn render_hover_info_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.hover_info_overlay.info.is_none() {
        return;
    }
    let host = {
        let vr = app.layout_cache.columns[2];
        if vr.width > 0 && vr.height > 0 { vr } else { area }
    };
    render_base_popup(frame, host, app);
    // Level 1: references list (pinned). Level 2: preview.
    if app.hover_info_overlay.refs.is_some() {
        render_refs_list(frame, host, app);
        if app
            .hover_info_overlay
            .refs
            .as_ref()
            .is_some_and(|r| r.preview.is_some())
        {
            render_preview(frame, host, app);
        }
    }
}

/// The base signature/doc popup, with a clickable `N refs` footer row.
fn render_base_popup(frame: &mut Frame, host: Rect, app: &mut App) {
    let theme = app.theme.clone();
    // Pull owned data so the immutable borrow of `info` ends before we write
    // the hit-test rects back onto `app.hover_info_overlay`.
    let (symbol_name, signature_lines, doc_lines, loc, ref_count, ref_count_capped) = {
        let info = app.hover_info_overlay.info.as_ref().unwrap();
        let mut loc = format!("{}  {}:{}", info.kind, info.file_path, info.line);
        if info.def_count > 1 {
            loc.push_str(&format!("  (+{} defs)", info.def_count - 1));
        }
        (
            info.symbol_name.clone(),
            info.signature_lines.clone(),
            info.doc_lines.clone(),
            loc,
            info.ref_count,
            info.ref_count_capped,
        )
    };
    let refs_present = ref_count > 0;

    // Body lines: signature, doc, then a location footer.
    let mut body: Vec<Line> = Vec::new();
    for sig in &signature_lines {
        body.push(Line::from(Span::styled(
            sig.clone(),
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
        )));
    }
    if !doc_lines.is_empty() {
        body.push(Line::from(""));
        for doc in &doc_lines {
            body.push(Line::from(Span::styled(doc.clone(), Style::default().fg(theme.fg))));
        }
    }
    body.push(Line::from(""));
    body.push(Line::from(Span::styled(loc, Style::default().fg(theme.muted))));

    // The clickable refs row (drawn on its own reserved bottom line). The `+`
    // marks a count that stopped at the cap, so a common name reads as "50+
    // refs" rather than claiming an exact 50 it never finished counting.
    let refs_label = if ref_count_capped {
        format!("▸ {ref_count}+ refs — click to list")
    } else {
        format!("▸ {ref_count} refs — click to list")
    };

    // Width fits the widest of body + refs row.
    let content_w = body
        .iter()
        .map(|l| l.width())
        .chain(std::iter::once(refs_label.chars().count()))
        .max()
        .unwrap_or(20)
        .clamp(20, 100) as u16;
    let popup_width = (content_w + 4).min(host.width.saturating_sub(2)).max(4);
    let inner_w = popup_width.saturating_sub(4).max(1) as usize;
    let body_h: usize = body
        .iter()
        .map(|l| {
            let w = l.width();
            if w == 0 { 1 } else { w.div_ceil(inner_w).max(1) }
        })
        .sum();
    let inner_h = (body_h + if refs_present { 1 } else { 0 }).max(1);
    let popup_height = (inner_h as u16 + 2).min(host.height.saturating_sub(2)).max(3);

    let popup_area = place(host, app.hover_info_overlay.anchor_row, app.hover_info_overlay.anchor_col, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(format!(" {symbol_name} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let refs_hit = if refs_present && inner.height >= 1 {
        let body_area = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
        let refs_row = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);
        frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), body_area);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                refs_label,
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
            ))),
            refs_row,
        );
        refs_row
    } else {
        frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inner);
        Rect::default()
    };

    app.hover_info_overlay.info_rect = popup_area;
    app.hover_info_overlay.refs_hit = refs_hit;
}

/// The references list (level 1) — anchored below the base popup (above if no
/// room), with clickable rows.
fn render_refs_list(frame: &mut Frame, host: Rect, app: &mut App) {
    let theme = app.theme.clone();
    let base = app.hover_info_overlay.info_rect;
    let Some(refs) = app.hover_info_overlay.refs.as_mut() else {
        return;
    };

    let count = refs.results.len();
    let title = format!(" {} · {} refs ", refs.symbol, count);

    // Width: fit the host, capped.
    let popup_width = host.width.saturating_sub(2).min(90).max(20);
    let inner_w = popup_width.saturating_sub(2).max(1) as usize;
    let max_rows = (host.height / 2).clamp(3, 14);
    let visible = (count as u16).min(max_rows).max(1);
    let popup_height = visible + 2;

    // Prefer below the base popup; else above; clamp into host.
    let below_y = base.y + base.height;
    let y = if below_y + popup_height <= host.y + host.height {
        below_y
    } else {
        base.y.saturating_sub(popup_height).max(host.y)
    };
    let x = base
        .x
        .min((host.x + host.width).saturating_sub(popup_width))
        .max(host.x);
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    // Keep the selection visible.
    let vis = visible as usize;
    if refs.selected < refs.scroll {
        refs.scroll = refs.selected;
    } else if refs.selected >= refs.scroll + vis {
        refs.scroll = refs.selected + 1 - vis;
    }

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut row_hits = Vec::new();
    for (row, idx) in (refs.scroll..(refs.scroll + vis).min(count)).enumerate() {
        let r = &refs.results[idx];
        let text = format!("{}:{}  {}", r.file_path, r.line, r.content.trim());
        let text: String = text.chars().take(inner_w).collect();
        let selected = idx == refs.selected;
        let style = if selected {
            Style::default().fg(theme.selected_fg).bg(theme.selected_bg)
        } else {
            Style::default().fg(theme.fg)
        };
        let row_area = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
        frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), row_area);
        row_hits.push((idx, row_area));
    }
    refs.rect = popup_area;
    refs.row_hits = row_hits;
}

/// The code preview (level 2) — surrounding source lines around the clicked
/// reference, anchored to the right of the list if it fits, else below.
fn render_preview(frame: &mut Frame, host: Rect, app: &mut App) {
    let theme = app.theme.clone();
    let list_rect = app
        .hover_info_overlay
        .refs
        .as_ref()
        .map(|r| r.rect)
        .unwrap_or_default();
    let Some(preview) = app
        .hover_info_overlay
        .refs
        .as_mut()
        .and_then(|r| r.preview.as_mut())
    else {
        return;
    };

    let title = format!(" {}:{} ", preview.file, preview.center_line);
    let content_w = preview
        .lines
        .iter()
        .map(|(n, t)| format!("{n:>5} {t}").chars().count())
        .chain(std::iter::once(title.chars().count()))
        .max()
        .unwrap_or(30) as u16;
    let popup_width = (content_w + 2).min(host.width.saturating_sub(2)).max(10);
    let popup_height = (preview.lines.len() as u16 + 2).min(host.height.saturating_sub(2)).max(3);

    // Right of the list if it fits, else below it, clamped into host.
    let right_x = list_rect.x + list_rect.width;
    let (x, y) = if right_x + popup_width <= host.x + host.width {
        (right_x, list_rect.y)
    } else {
        let below = list_rect.y + list_rect.height;
        let y = if below + popup_height <= host.y + host.height {
            below
        } else {
            (host.y + host.height).saturating_sub(popup_height).max(host.y)
        };
        let x = list_rect
            .x
            .min((host.x + host.width).saturating_sub(popup_width))
            .max(host.x);
        (x, y)
    };
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let lines: Vec<Line> = preview
        .lines
        .iter()
        .map(|(n, t)| {
            let is_center = *n == preview.center_line;
            let num_style = Style::default().fg(theme.muted);
            let text_style = if is_center {
                Style::default().fg(theme.fg).bg(theme.selected_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            Line::from(vec![
                Span::styled(format!("{n:>5} "), num_style),
                Span::styled(t.clone(), text_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    preview.rect = popup_area;
}

/// Place a popup of the given size within `host`, anchored to a symbol screen
/// position: just below the anchor row when there's room, otherwise above.
fn place(host: Rect, anchor_row: u16, anchor_col: u16, w: u16, h: u16) -> Rect {
    let host_top = host.y + 1;
    let host_bottom = host.y + host.height.saturating_sub(1);
    let anchor_row = anchor_row.clamp(host_top, host_bottom.saturating_sub(1));
    let room_below = host_bottom.saturating_sub(anchor_row + 1);
    let y = if room_below >= h {
        anchor_row + 1
    } else {
        anchor_row.saturating_sub(h).max(host_top)
    };
    let max_x = (host.x + host.width).saturating_sub(w);
    let x = anchor_col.clamp(host.x, max_x.max(host.x));
    Rect::new(x, y, w, h)
}
