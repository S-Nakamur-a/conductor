//! ホバー情報ポップアップ — シンボルのシグネチャ・doc コメント・クリック
//! 可能な参照件数（LSP のホバー表示のようなもの）を描画し、ユーザがそこを
//! クリックすると、インタラクティブな参照一覧（レベル1）とコードプレビュー
//! （レベル2）を表示する。ポップアップは Viewer パネル内のホバー中のシンボル
//! を起点に配置される。
//!
//! マウス層がクリックのヒットテストを行えるよう、各レベルは自分の描画済み
//! Rect（参照一覧は各行の Rect も）を app.code_nav.hover_info に書き戻す。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::App;

/// ホバーポップアップと、開いている子レベルがあればそれを area（フレーム）
/// の上に描画する。
pub fn render_hover_info_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.code_nav.hover_info.info.is_none() {
        return;
    }
    let host = {
        let vr = app.layout.cache.columns[2];
        if vr.width > 0 && vr.height > 0 { vr } else { area }
    };
    render_base_popup(frame, host, app);
    // レベル1: 参照一覧（ピン留め）。レベル2: プレビュー。
    if app.code_nav.hover_info.refs.is_some() {
        render_refs_list(frame, host, app);
        if app
            .code_nav.hover_info
            .refs
            .as_ref()
            .is_some_and(|r| r.preview.is_some())
        {
            render_preview(frame, host, app);
        }
    }
}

/// シグネチャ/doc の基本ポップアップ。クリック可能な「N refs」フッター行を持つ。
fn render_base_popup(frame: &mut Frame, host: Rect, app: &mut App) {
    let theme = app.theme.clone();
    // info への不変借用を先に終わらせてから app.code_nav.hover_info に
    // ヒットテスト用の Rect を書き戻せるよう、所有データとして取り出しておく。
    let (symbol_name, signature_lines, doc_lines, loc, ref_count, ref_count_capped) = {
        let info = app.code_nav.hover_info.info.as_ref().unwrap();
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

    // 本文の行: シグネチャ、doc、そして場所のフッター。
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

    // クリック可能な refs 行（専用に確保した最下行に描画する）。+ は件数が
    // 上限で打ち切られたことを示す印で、ありふれた名前の場合に、数え終えて
    // いないのにちょうど50件であるかのように見せず「50+ refs」と表示する。
    let refs_label = if ref_count_capped {
        format!("▸ {ref_count}+ refs — click to list")
    } else {
        format!("▸ {ref_count} refs — click to list")
    };

    // 幅は本文 + refs 行のうち最も広いものに合わせる。
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

    let popup_area = place(host, app.code_nav.hover_info.anchor_row, app.code_nav.hover_info.anchor_col, popup_width, popup_height);

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

    app.code_nav.hover_info.info_rect = popup_area;
    app.code_nav.hover_info.refs_hit = refs_hit;
}

/// 参照一覧（レベル1） — 基本ポップアップの下に配置する（余白がなければ上）。
/// 各行はクリック可能。
fn render_refs_list(frame: &mut Frame, host: Rect, app: &mut App) {
    let theme = app.theme.clone();
    let base = app.code_nav.hover_info.info_rect;
    let Some(refs) = app.code_nav.hover_info.refs.as_mut() else {
        return;
    };

    let count = refs.results.len();
    let title = format!(" {} · {} refs ", refs.symbol, count);

    // 幅: host に収まるように、上限つきで調整する。
    let popup_width = host.width.saturating_sub(2).clamp(20, 90);
    let inner_w = popup_width.saturating_sub(2).max(1) as usize;
    let max_rows = (host.height / 2).clamp(3, 14);
    let visible = (count as u16).min(max_rows).max(1);
    let popup_height = visible + 2;

    // 基本ポップアップの下を優先し、収まらなければ上に; host 内にクランプする。
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

    // 選択中の項目が見える位置を保つ。
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

/// コードプレビュー（レベル2） — クリックした参照の周辺ソース行。一覧の
/// 右側に収まればそこに、収まらなければ下に配置する。
fn render_preview(frame: &mut Frame, host: Rect, app: &mut App) {
    let theme = app.theme.clone();
    let list_rect = app
        .code_nav.hover_info
        .refs
        .as_ref()
        .map(|r| r.rect)
        .unwrap_or_default();
    let Some(preview) = app
        .code_nav.hover_info
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

    // 一覧の右側に収まればそこ、収まらなければ下、host 内にクランプする。
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

/// 指定サイズのポップアップを、シンボルの画面上の位置を起点として host 内に
/// 配置する: 余白があれば anchor 行のすぐ下、なければ上に置く。
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
