//! worktree パネルのゾーン2、すなわち選択中 worktree の詳細セクション
//! （ブランチ、パス、ステータス、リモート同期、系譜、PR 情報）の描画。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::theme::Theme;

/// 詳細セクション（選択中 worktree の情報）を描画する。
pub(super) fn render_detail(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    theme: &Theme,
    border_color: Color,
) {
    let block = Block::default()
        .title(Span::styled(" Detail ", Style::default().fg(theme.muted)))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let Some(wt) = app.worktrees.selected() else {
        return;
    };

    let mut lines: Vec<Line> = Vec::new();

    // ブランチ名。
    lines.push(Line::from(vec![
        Span::styled(" Branch: ", Style::default().fg(theme.muted)),
        Span::styled(
            wt.branch.as_str(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // パス（簡潔にするため末尾のコンポーネントだけ表示する）。
    let path_display = wt
        .path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| wt.path.display().to_string());
    lines.push(Line::from(vec![
        Span::styled(" Path:   ", Style::default().fg(theme.muted)),
        Span::styled(path_display, Style::default().fg(theme.fg)),
    ]));

    // ステータス。
    let status_spans = if wt.is_clean {
        vec![
            Span::styled(" Status: ", Style::default().fg(theme.muted)),
            Span::styled("\u{2713} clean", Style::default().fg(theme.success)),
        ]
    } else {
        {
            let mut spans = vec![Span::styled(" Status: ", Style::default().fg(theme.muted))];
            let mut first = true;
            if wt.added > 0 {
                spans.push(Span::styled(
                    format!("added: {}", wt.added),
                    Style::default().fg(theme.success),
                ));
                first = false;
            }
            if wt.modified > 0 {
                if !first {
                    spans.push(Span::styled("  ", Style::default()));
                }
                spans.push(Span::styled(
                    format!("modified: {}", wt.modified),
                    Style::default().fg(theme.warning),
                ));
                first = false;
            }
            if wt.deleted > 0 {
                if !first {
                    spans.push(Span::styled("  ", Style::default()));
                }
                spans.push(Span::styled(
                    format!("deleted: {}", wt.deleted),
                    Style::default().fg(theme.error),
                ));
            }
            spans
        }
    };
    lines.push(Line::from(status_spans));

    // リモートとの同期状況。
    let remote_spans = match (wt.ahead, wt.behind) {
        (Some(0), Some(0)) => vec![
            Span::styled(" Remote: ", Style::default().fg(theme.muted)),
            Span::styled("\u{2261} synced", Style::default().fg(theme.success)),
        ],
        (Some(ahead), Some(behind)) => {
            let mut parts = Vec::new();
            if ahead > 0 {
                parts.push(format!("\u{2191}{ahead}"));
            }
            if behind > 0 {
                parts.push(format!("\u{2193}{behind}"));
            }
            vec![
                Span::styled(" Remote: ", Style::default().fg(theme.muted)),
                Span::styled(parts.join(" "), Style::default().fg(theme.info)),
            ]
        }
        _ => vec![
            Span::styled(" Remote: ", Style::default().fg(theme.muted)),
            Span::styled("no upstream", Style::default().fg(theme.muted)),
        ],
    };
    lines.push(Line::from(remote_spans));

    // ブランチの系譜と PR 情報。
    let details = &app.branch_details;
    let is_main = wt.is_main;

    let has_lineage = details.initial_branch.is_some()
        || !details.derived_branches.is_empty()
        || (app.gh_available && !is_main);

    if has_lineage {
        lines.push(Line::from(""));

        // 親ブランチ。
        if let Some(ref base) = details.initial_branch {
            lines.push(Line::from(vec![
                Span::styled(" Parent: ", Style::default().fg(theme.muted)),
                Span::styled(base.as_str(), Style::default().fg(theme.fg)),
            ]));
        }

        // 派生（フォーク）ブランチ — 読みやすさのため1行に1つ。
        if !details.derived_branches.is_empty() {
            // 最初のフォークはラベル行に載せる。
            lines.push(Line::from(vec![
                Span::styled(" Forks:  ", Style::default().fg(theme.muted)),
                Span::styled(
                    details.derived_branches[0].as_str(),
                    Style::default().fg(theme.info),
                ),
            ]));
            // それ以降のフォークは続く行にインデントして並べる。
            for fork in &details.derived_branches[1..] {
                lines.push(Line::from(vec![
                    Span::styled("         ", Style::default().fg(theme.muted)),
                    Span::styled(fork.as_str(), Style::default().fg(theme.info)),
                ]));
            }
        }

        // PR の URL。
        if app.gh_available && !is_main {
            if details.pr_loading {
                lines.push(Line::from(vec![
                    Span::styled(" PR:     ", Style::default().fg(theme.muted)),
                    Span::styled("loading...", Style::default().fg(theme.muted)),
                ]));
            } else if let Some(ref url) = details.pr_url {
                lines.push(Line::from(vec![
                    Span::styled(" PR:     ", Style::default().fg(theme.muted)),
                    Span::styled(
                        url.as_str(),
                        Style::default()
                            .fg(theme.accent)
                            .add_modifier(Modifier::UNDERLINED),
                    ),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(" PR:     ", Style::default().fg(theme.muted)),
                    Span::styled("none", Style::default().fg(theme.muted)),
                ]));
            }
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
