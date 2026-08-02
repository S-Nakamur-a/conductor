//! "SUMMARY" 疑似ファイルビュー。行に紐づくレビューコメントの対となる、
//! ブランチの変更概要を全面表示するレンダラー。

use crate::app::App;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

/// 変更概要全体を、専用のスクロール可能な全面ビュー（"SUMMARY" 疑似ファイル）として
/// 描画する。これは行に紐づくレビューコメントに対する PR 説明文の対で、パネル全体を
/// 使い（省略しない）、diff/file ビューと同じ j/k スクロールを再利用する。
pub(super) fn render_summary_view(frame: &mut Frame, area: Rect, app: &mut App, focused: bool) {
    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;

    let (block, lines): (Block, Vec<Line>) = {
        let theme = &app.theme;
        let border_color = if focused {
            theme.border_focused
        } else {
            theme.border_unfocused
        };
        let title_style = if focused {
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        let block = crate::ui::common::PanelChrome::new(
            theme,
            " \u{25A3} SUMMARY ",
            focused,
            border_color,
        )
        .with_title_style(title_style)
        .into_block();

        let summary = app
            .review_state
            .change_summary
            .as_deref()
            .unwrap_or("")
            .trim();

        let mut lines: Vec<Line> = Vec::new();
        if summary.is_empty() {
            for (text, _) in [
                ("(no change summary on this branch)", ()),
                ("", ()),
                ("Generating a walkthrough writes one — palette:", ()),
                ("\"Review: Generate Walkthrough\". Claude can also set it", ()),
                ("directly with the conductor `set_change_summary` MCP tool.", ()),
            ] {
                lines.push(Line::from(Span::styled(
                    text,
                    Style::default().fg(theme.muted),
                )));
            }
        } else {
            // 概要を Markdown として描画する: 見出し/リスト/引用は装飾され、
            // フェンスコードブロックはシンタックスハイライトされる。Markdown 記法を
            // 含まないプレーンテキストは通常の段落として描画されるので、既存の
            // 概要は影響を受けない。
            lines = crate::ui::markdown::render_markdown(
                summary,
                inner_width.saturating_sub(1),
                theme,
                &app.highlight.syntax_set,
                &app.highlight.theme,
            );
        }
        (block, lines)
    };

    // キーハンドラがスクロールをクランプできるよう総行数を記録し、概要が短くなっても
    // ナビゲーションが正しく効くようクランプ後のスクロール値を書き戻す。
    app.viewer_state.summary_total_lines = lines.len();
    let scroll = app
        .viewer_state
        .summary_scroll
        .min(lines.len().saturating_sub(1));
    app.viewer_state.summary_scroll = scroll;
    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(inner_height).collect();

    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(Paragraph::new(visible).block(block), area);
}
