//! AI ウォークスルービュー — Explorer の下部ペインが持つ3つのビュー
//! （差分一覧・コメント一覧と並ぶ; viewer::ExplorerBottomView を参照）の1つ。
//!
//! ステップごとに1行のフラットな一覧として描画し、選択中のステップの本文
//! だけをその行のすぐ下にインライン表示する（ワードラップ、クリップあり）
//! — 「一覧」と「本文」のエリアを固定比率で分割することはしていないので、
//! 狭いペインでも選択中のステップをできる限り表示できる。クリップで
//! 切れた分は space の詳細オーバーレイから全文を見られる。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap};

use crate::app::{App, Focus};
use crate::walkthrough::{WalkthroughStatus, WalkthroughStep, WalkthroughStepKind};

/// ウォークスルービューを Explorer の下部ペインに描画する。
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

    let title = match app.walkthrough.current.as_ref().and_then(|wt| wt.header.title.as_deref()) {
        Some(t) => format!(" Walkthrough: {t} "),
        None => " Walkthrough ".to_string(),
    };
    let block = Block::default()
        .title(Span::styled(title, title_style))
        .borders(Borders::ALL)
        .border_type(border_type)
        .border_style(Style::default().fg(border_color));

    let Some(loaded) = &app.walkthrough.current else {
        let paragraph = Paragraph::new("No walkthrough yet — palette: Generate Walkthrough")
            .style(Style::default().fg(theme.muted))
            .block(block);
        frame.render_widget(Clear, area);
        frame.render_widget(paragraph, area);
        return;
    };

    match loaded.header.status {
        WalkthroughStatus::Generating => {
            let paragraph = Paragraph::new("Generating walkthrough… (this takes a few minutes)")
                .style(Style::default().fg(theme.info))
                .block(block);
            frame.render_widget(Clear, area);
            frame.render_widget(paragraph, area);
        }
        WalkthroughStatus::Failed => {
            let error = loaded.header.error.as_deref().unwrap_or("unknown error");
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
            render_steps(frame, area, app, block, &loaded.steps, list_focused);
        }
    }
}

/// ウォークスルーステップの隣に表示するアイコン。このUIの既存の絵文字バッジ
/// の慣習（コメントバッジ、ファイルツリーのアイコンなど）に合わせている。
/// Viewer のウォークスルーステップバナー（ui::viewer_panel）とも共有している。
pub(crate) fn step_icon(kind: WalkthroughStepKind) -> &'static str {
    match kind {
        WalkthroughStepKind::Intent => "\u{1f3af}", // 🎯
        WalkthroughStepKind::Core => "\u{1f527}",   // 🔧
        WalkthroughStepKind::Ripple => "\u{1f30a}", // 🌊
        WalkthroughStepKind::Test => "\u{1f9ea}",   // 🧪
    }
}

/// テキストを width カラムに貪欲にワードラップする。ステップ本文中の意図的な
/// 段落区切りが残るよう、まず既存の改行で分割してから処理する。
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

/// 準備完了したウォークスルーのフラットなステップ一覧: ステップごとに1行、
/// 選択中のステップだけワードラップした本文をその行のすぐ下にインライン表示
/// する（最大6行、かつペインの残り高さに収まる範囲にクリップ）。スクロールは
/// ステップ単位で行う — walkthrough_scroll/walkthrough_selected は差分一覧が
/// 使うのと同じインデック空間を共有しており、選択中のステップだけが展開
/// するので、それより前の行は常にちょうど1行になる。そのため
/// event::adjust_walkthrough_scroll のクランプで選択中ステップのヘッダを
/// 表示し続けられる。
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

/// 選択中のウォークスルーステップの全文詳細オーバーレイ（ウォークスルー
/// ビューの space キー — コメント一覧が view_comment_detail に使うのと同じ
/// 詳細オーバーレイのパターンを、ステップの省略なしの本文に適用したもの）。
pub fn render_detail_overlay(frame: &mut Frame, area: Rect, app: &mut App) {
    let theme = &app.theme;
    let Some(steps) = app.walkthrough.current.as_ref().map(|wt| &wt.steps) else {
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
