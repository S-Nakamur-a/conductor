//! 画面上部のタイトルバー（リポジトリバッジ、ブランチ、パス、右揃えの
//! ワークスペース/使用状況統計）。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::color::{format_tokens, name_to_color};

/// worktree 名と作業ディレクトリを表示する、画面上部のタイトルバーを描画する。
pub fn render_title_bar(frame: &mut Frame, area: Rect, app: &mut crate::app::App) {
    let theme = &app.theme;
    let wt_name = app
        .worktrees
        .get(app.worktrees.selected_index())
        .map(|w| w.branch.as_str())
        .unwrap_or("—");
    let wt_path = app
        .worktrees
        .get(app.worktrees.selected_index())
        .map(|w| w.path.display().to_string())
        .unwrap_or_else(|| app.repo.path.display().to_string());

    let (badge_bg, badge_fg, branch_fg) = name_to_color(&app.repo.main_name);

    // Color::Reset を使い、端末自体の背景（背景画像を含む）がタイトルバー越しに
    // 透けて見えるようにする。
    let bar_bg = Color::Reset;
    let conductor_bg = badge_bg;
    let conductor_fg = badge_fg;

    let badge_text = format!(" {} ", app.repo.main_name);
    let line = Line::from(vec![
        Span::styled(
            &badge_text,
            Style::default()
                .fg(conductor_fg)
                .bg(conductor_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            wt_name,
            Style::default().fg(branch_fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" │ ", Style::default().fg(theme.muted)),
        Span::styled(wt_path, Style::default().fg(theme.dir_fg)),
    ]);
    let paragraph = Paragraph::new(line).style(Style::default().bg(bar_bg));
    frame.render_widget(paragraph, area);

    // 右揃えの統計表示（本日の活動 + ccusage）。
    {
        let sep = Span::styled(" | ", Style::default().fg(theme.muted).bg(bar_bg));
        let mut spans: Vec<Span> = Vec::new();

        // ワークスペースの概況: worktree 数、実行中の Claude Code セッション数、
        // 現在の Conductor のバージョン。
        let worktree_count = app.worktrees.len();
        let claude_session_count = app
            .terminal
            .pty_manager
            .sessions()
            .iter()
            .filter(|s| s.kind == crate::pty_manager::SessionKind::ClaudeCode)
            .count();
        spans.push(Span::styled(
            format!("{} worktrees", worktree_count),
            Style::default().fg(theme.info).bg(bar_bg),
        ));
        spans.push(sep.clone());
        spans.push(Span::styled(
            format!("{} sessions", claude_session_count),
            Style::default().fg(theme.success).bg(bar_bg),
        ));
        spans.push(sep.clone());
        spans.push(Span::styled(
            format!("v{}", crate::update_checker::current_version()),
            Style::default().fg(theme.warning).bg(bar_bg),
        ));
        if let Some(ref info) = app.stats.ccusage {
            if !spans.is_empty() {
                spans.push(sep.clone());
            }
            spans.push(Span::styled(
                format!("{} tokens", format_tokens(info.total_tokens)),
                Style::default().fg(theme.accent).bg(bar_bg),
            ));
            spans.push(sep.clone());
            spans.push(Span::styled(
                format!("${:.2}", info.total_cost),
                Style::default().fg(theme.success).bg(bar_bg),
            ));
        }
        // 位置計算のため、更新バッジのテキストを保持しておく。
        let mut update_badge_text: Option<String> = None;
        if let Some(ref update) = app.update.info {
            if !spans.is_empty() {
                spans.push(Span::styled(" ", Style::default().bg(bar_bg)));
            }
            let badge = format!(" ↑ v{} available ", update.latest_version);
            update_badge_text = Some(badge.clone());
            spans.push(Span::styled(
                badge,
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        if !spans.is_empty() {
            // 余白のスペースを追加する
            spans.insert(0, Span::styled(" ", Style::default().bg(bar_bg)));
            spans.push(Span::styled(" ", Style::default().bg(bar_bg)));

            let stats_line = Line::from(spans);
            let stats_w = stats_line.width() as u16;
            if stats_w + 2 < area.width {
                let stats_x = area.x + area.width - stats_w;
                let stats_area = Rect::new(stats_x, area.y, stats_w, 1);
                frame.render_widget(Paragraph::new(stats_line), stats_area);

                // 更新バッジの絶対カラム範囲を計算する。
                if let Some(ref badge) = update_badge_text {
                    let badge_w = UnicodeWidthStr::width(badge.as_str()) as u16;
                    // バッジは統計表示の末尾（末尾の余白スペースの手前）にある。
                    let badge_end = stats_x + stats_w - 1; // 末尾の " " の分を引く
                    let badge_start = badge_end - badge_w;
                    app.update.badge_cols = Some((badge_start, badge_end));
                } else {
                    app.update.badge_cols = None;
                }
            } else {
                app.update.badge_cols = None;
            }
        } else {
            app.update.badge_cols = None;
        }
    }
}
