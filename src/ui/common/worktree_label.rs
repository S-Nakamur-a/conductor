//! Right-aligned worktree branch / repository name label overlaid on a row.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// Render the current worktree branch and repository name at the far right
/// of the given row area (overlays on the same line).
pub fn render_worktree_label(
    frame: &mut Frame,
    row_area: Rect,
    worktree_branch: &str,
    repo_path: &std::path::Path,
    theme: &Theme,
) {
    let repo_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| repo_path.display().to_string());

    let branch_part = worktree_branch;
    let repo_part = format!("[{repo_name}]");
    let total_width =
        UnicodeWidthStr::width(branch_part) + 1 + UnicodeWidthStr::width(repo_part.as_str());

    if total_width as u16 + 1 > row_area.width {
        return;
    }

    let label_area = Rect::new(
        row_area.x + row_area.width - total_width as u16,
        row_area.y,
        total_width as u16,
        1,
    );

    let line = Line::from(vec![
        Span::styled(branch_part, Style::default().fg(theme.info)),
        Span::raw(" "),
        Span::styled(repo_part, Style::default().fg(theme.muted)),
    ]);
    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, label_area);
}
