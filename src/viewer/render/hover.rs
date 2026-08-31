//! ホバー情報ポップアップ — シンボルのシグネチャ・doc コメント・クリック
//! 可能な参照件数（LSP のホバー表示のようなもの）を描画し、ユーザがそこを
//! クリックすると、インタラクティブな参照一覧（レベル1）とコードプレビュー
//! （レベル2）を表示する。ポップアップは Viewer パネル内のホバー中のシンボル
//! を起点に配置される。
//!
//! マウス層がクリックのヒットテストを行えるよう、各レベルは自分の描画済み
//! Rect（参照一覧は各行の Rect も）を返し、状態への書き戻しは呼び出し側
//! （[crate::viewer::panel]）が行う。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::App;

use super::outcome::{BaseOutcome, HoverOutcome, RefsOutcome};

/// ホバーポップアップと、開いている子レベルがあればそれを area（フレーム）
/// の上に描画する。何も表示するものが無ければ None。
pub(in crate::viewer) fn render_hover_info_overlay(
    frame: &mut Frame,
    area: Rect,
    app: &App,
) -> Option<HoverOutcome> {
    app.code_nav.hover_info.info.as_ref()?;
    let host = {
        let vr = app.layout.cache.columns[2];
        if vr.width > 0 && vr.height > 0 {
            vr
        } else {
            area
        }
    };
    let base = render_base_popup(frame, host, app);
    // レベル1: 参照一覧（ピン留め）。レベル2: プレビュー。
    let mut refs = None;
    let mut preview_rect = None;
    if app.code_nav.hover_info.refs.is_some()
        && let Some(r) = render_refs_list(frame, host, app)
    {
        if app
            .code_nav
            .hover_info
            .refs
            .as_ref()
            .is_some_and(|r| r.preview.is_some())
        {
            preview_rect = render_preview(frame, host, app, r.rect);
        }
        refs = Some(r);
    }
    Some(HoverOutcome {
        base,
        refs,
        preview_rect,
    })
}

/// シグネチャ/doc の基本ポップアップ。クリック可能な「N refs」フッター行を持つ。
fn render_base_popup(frame: &mut Frame, host: Rect, app: &App) -> BaseOutcome {
    let theme = app.appearance.theme.clone();
    // info への不変借用を先に終わらせてから app.code_nav.hover_info に
    // ヒットテスト用の Rect を書き戻せるよう、所有データとして取り出しておく。
    // 見出しは 1 行に固定する。所属を左、種別を右に置き、種別ごとに行の並びは
    // 変えない。並びが種別で動くと、次にどこを見ればよいかが毎回変わる。
    let (symbol_name, mut body, container, kind, def_label, ref_count, ref_count_capped) = {
        let info = app.code_nav.hover_info.info.as_ref().unwrap();
        let mut def_label = format!("▸ {}:{}", info.file_path, info.line);
        if info.def_count > 1 {
            def_label.push_str(&format!("  (+{} defs)", info.def_count - 1));
        }
        def_label.push_str(" — click to jump");

        // 本文の行: シグネチャ、doc。見出しは幅が決まってから足す。場所と参照は
        // クリックできるフッターに置く。
        let mut body: Vec<Line> = Vec::new();
        // 索引の宣言は型が解決済みで、画面の字面とは違うものを見せている。
        // 字面の写しにしかならない tree-sitter 由来のときだけ、定義行の上で省く。
        if info.signature_from_index || !info.on_definition_line {
            body.extend(highlighted_signature(app, info));
        }
        if !info.doc_lines.is_empty() {
            if !body.is_empty() {
                body.push(Line::from(""));
            }
            for doc in &info.doc_lines {
                body.push(Line::from(Span::styled(
                    doc.clone(),
                    Style::default().fg(theme.fg),
                )));
            }
        }
        (
            info.symbol_name.clone(),
            body,
            info.container.clone().unwrap_or_default(),
            info.kind.clone(),
            def_label,
            info.ref_count,
            info.ref_count_capped,
        )
    };
    let refs_present = ref_count > 0;
    if !body.is_empty() {
        body.push(Line::from(""));
    }
    let header_w = header_width(&container, &kind);

    // クリック可能な refs 行（専用に確保した最下行に描画する）。+ は件数が
    // 上限で打ち切られたことを示す印で、ありふれた名前の場合に、数え終えて
    // いないのにちょうど50件であるかのように見せず「50+ refs」と表示する。
    let refs_label = if ref_count_capped {
        format!("▸ {ref_count}+ refs — click to list")
    } else {
        format!("▸ {ref_count} refs — click to list")
    };

    // 幅は見出し + 本文 + フッター 2 行のうち最も広いものに合わせる。
    let content_w = body
        .iter()
        .map(|l| l.width())
        .chain([
            refs_label.chars().count(),
            def_label.chars().count(),
            header_w,
        ])
        .max()
        .unwrap_or(20)
        .clamp(20, 100) as u16;
    let popup_width = (content_w + 4).min(host.width.saturating_sub(2)).max(4);
    // 本文を描くのは枠の内側ちょうど。ここを狭く見積もると、折り返さない行を
    // 折り返す前提で高さを取って、本文の下に空行が残る。
    let inner_w = popup_width.saturating_sub(2).max(1) as usize;
    if header_w > 0 {
        body.insert(0, header_line(&container, &kind, inner_w, &theme));
    }
    let body_h: usize = body
        .iter()
        .map(|l| {
            let w = l.width();
            if w == 0 {
                1
            } else {
                w.div_ceil(inner_w).max(1)
            }
        })
        .sum();
    let footer_h = 1 + usize::from(refs_present);
    let inner_h = (body_h + footer_h).max(1);
    let popup_height = (inner_h as u16 + 2)
        .min(host.height.saturating_sub(2))
        .max(3);

    let popup_area = place(
        host,
        app.code_nav.hover_info.anchor_row,
        app.code_nav.hover_info.anchor_col,
        popup_width,
        popup_height,
    );

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(format!(" {symbol_name} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // フッターは下から順に: 参照 (あれば)、定義位置。どちらもクリックできる。
    let footer_h = (footer_h as u16).min(inner.height);
    let body_h = inner.height.saturating_sub(footer_h);
    if body_h > 0 {
        let body_area = Rect::new(inner.x, inner.y, inner.width, body_h);
        frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), body_area);
    }
    let row = |offset: u16| Rect::new(inner.x, inner.y + body_h + offset, inner.width, 1);

    let mut def_hit = Rect::default();
    let mut refs_hit = Rect::default();
    if footer_h >= 1 {
        def_hit = row(0);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                def_label,
                Style::default().fg(theme.fg),
            ))),
            def_hit,
        );
    }
    if footer_h >= 2 {
        refs_hit = row(1);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                refs_label,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ))),
            refs_hit,
        );
    }

    BaseOutcome {
        info_rect: popup_area,
        refs_hit,
        def_hit,
    }
}

/// シグネチャは宣言の断片なのでパーサの状態が続かない。全部を単色にするよりは読める、
/// という割り切りは revidere のハンク表示と同じ。
fn highlighted_signature(
    app: &App,
    info: &crate::viewer::hover_info::HoverInfo,
) -> Vec<Line<'static>> {
    use syntect::easy::HighlightLines;

    let syntax_set = &app.appearance.highlight.syntax_set;
    let syntax = crate::viewer::find_syntax(
        syntax_set,
        Some(info.file_path.as_str()),
        info.signature_lines.first().map(String::as_str),
    );
    let mut h = HighlightLines::new(syntax, &app.appearance.highlight.theme);

    info.signature_lines
        .iter()
        .map(|text| {
            let with_nl = format!("{text}\n");
            let Ok(ranges) = h.highlight_line(&with_nl, syntax_set) else {
                return Line::from(Span::styled(
                    text.clone(),
                    Style::default().fg(app.appearance.theme.accent),
                ));
            };
            Line::from(
                ranges
                    .into_iter()
                    .map(|(style, piece)| {
                        Span::styled(
                            piece.trim_end_matches('\n').to_string(),
                            syntect_tui::translate_style(style)
                                .unwrap_or_default()
                                .bg(ratatui::style::Color::Reset),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

/// 参照一覧（レベル1） — 基本ポップアップの下に配置する（余白がなければ上）。
/// 各行はクリック可能。
fn render_refs_list(frame: &mut Frame, host: Rect, app: &App) -> Option<RefsOutcome> {
    let theme = &app.appearance.theme;
    let base = app.code_nav.hover_info.info_rect;
    let refs = app.code_nav.hover_info.refs.as_ref()?;

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
    let scroll = if refs.selected < refs.scroll {
        refs.selected
    } else if refs.selected >= refs.scroll + vis {
        refs.selected + 1 - vis
    } else {
        refs.scroll
    };

    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_focused));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let mut row_hits = Vec::new();
    for (row, idx) in (scroll..(scroll + vis).min(count)).enumerate() {
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
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text, style))),
            row_area,
        );
        row_hits.push((idx, row_area));
    }
    Some(RefsOutcome {
        rect: popup_area,
        row_hits,
        scroll,
    })
}

/// 一覧の右に収まればそこ、収まらなければ下。list_rect は同じフレームで描いた参照一覧の
/// Rect ([render_refs_list] の戻り値)。
fn render_preview(frame: &mut Frame, host: Rect, app: &App, list_rect: Rect) -> Option<Rect> {
    let theme = &app.appearance.theme;
    let preview = app
        .code_nav
        .hover_info
        .refs
        .as_ref()
        .and_then(|r| r.preview.as_ref())?;

    let title = format!(" {}:{} ", preview.file, preview.center_line);
    let content_w = preview
        .lines
        .iter()
        .map(|(n, t)| format!("{n:>5} {t}").chars().count())
        .chain(std::iter::once(title.chars().count()))
        .max()
        .unwrap_or(30) as u16;
    let popup_width = (content_w + 2).min(host.width.saturating_sub(2)).max(10);
    let popup_height = (preview.lines.len() as u16 + 2)
        .min(host.height.saturating_sub(2))
        .max(3);

    // 一覧の右側に収まればそこ、収まらなければ下、host 内にクランプする。
    let right_x = list_rect.x + list_rect.width;
    let (x, y) = if right_x + popup_width <= host.x + host.width {
        (right_x, list_rect.y)
    } else {
        let below = list_rect.y + list_rect.height;
        let y = if below + popup_height <= host.y + host.height {
            below
        } else {
            (host.y + host.height)
                .saturating_sub(popup_height)
                .max(host.y)
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
                Style::default()
                    .fg(theme.fg)
                    .bg(theme.selected_bg)
                    .add_modifier(Modifier::BOLD)
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
    Some(popup_area)
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

/// 見出し行が要る幅。所属も種別も無ければ 0 で、行そのものを出さない。
fn header_width(container: &str, kind: &str) -> usize {
    let (left, right) = (container.chars().count(), kind.chars().count());
    match (left, right) {
        (0, 0) => 0,
        (0, r) => r,
        (l, 0) => l,
        // あいだは最低 2 文字空ける。詰まると 1 つの語に見える。
        (l, r) => l + 2 + r,
    }
}

/// 所属を左、種別を右に置いた見出し行。幅が足りなければ右寄せをやめて詰める。
fn header_line(
    container: &str,
    kind: &str,
    inner_w: usize,
    theme: &crate::theme::Theme,
) -> Line<'static> {
    let mut spans = Vec::new();
    if !container.is_empty() {
        spans.push(Span::styled(
            container.to_string(),
            Style::default()
                .fg(theme.info)
                .add_modifier(Modifier::ITALIC),
        ));
    }
    if !kind.is_empty() {
        // 所属が無くても右端に置く。左に寄せると、すぐ下に並ぶ宣言の先頭と
        // 同じ列から始まって、種別が宣言の一部に見える。
        let used = container.chars().count() + kind.chars().count();
        let gap = inner_w
            .saturating_sub(used)
            .max(usize::from(!container.is_empty()) * 2);
        if gap > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
        spans.push(Span::styled(
            kind.to_string(),
            Style::default()
                .fg(theme.hint)
                .add_modifier(Modifier::ITALIC),
        ));
    }
    Line::from(spans)
}
