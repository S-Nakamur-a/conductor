//! ファイルツリーと下区画の描画。行は純関数が組み、frame へ渡すのは最後の 1 手だけ。

use conductor_core::git_engine::status_map::TreeGitState;
use conductor_core::icons::{IconSet, dir_icon, expand_arrow};
use conductor_core::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BottomView, ExplorerPanel, Pane, git_changes};
use crate::list::row_line;
use crate::workspace::Workspace;

pub fn render(frame: &mut Frame, tree_area: Rect, changes_area: Rect, ws: &Workspace) {
    let panel = &ws.panels.explorer;
    let icons = ws.config.ui.icon_set();

    let inner = crate::list::inner(tree_area);
    if inner.height > 0 {
        let lines = tree_lines(panel, &ws.theme, icons, inner.height as usize);
        frame.render_widget(Paragraph::new(lines), inner);
    }

    let inner = crate::list::inner(changes_area);
    if inner.height > 0 {
        let lines = bottom_lines(ws, inner.height as usize);
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

pub fn bottom_lines(ws: &Workspace, height: usize) -> Vec<Line<'static>> {
    let panel = &ws.panels.explorer;
    match panel.bottom() {
        BottomView::GitChanges => git_changes::render::lines(
            &panel.changes,
            panel.tree().status(),
            &ws.review,
            &ws.theme,
            &ws.config,
            height,
            panel.pane() == Pane::Bottom,
        ),
        BottomView::Comments => crate::comment_list::lines(
            &panel.comments,
            &ws.review,
            &ws.theme,
            ws.config.ui.icon_set(),
            height,
            panel.pane() == Pane::Bottom,
        ),
    }
}

/// ツリーの見出し。件数は窓に入り切らないときだけ添える。
pub fn tree_title(panel: &ExplorerPanel, height: usize) -> String {
    let total = panel.tree().visible().len();
    if total > height {
        format!(
            " Explorer ({}/{total}) ",
            panel.tree_cursor().selected() + 1
        )
    } else {
        " Explorer ".to_string()
    }
}

/// 下区画の見出し。中身が入れ替わるので、何を見ているかは枠で示す。
pub fn bottom_title(panel: &ExplorerPanel, ws: &Workspace) -> String {
    match panel.bottom() {
        BottomView::Comments => crate::comment_list::title(&ws.review, ws.config.ui.icon_set()),
        BottomView::GitChanges => git_changes::render::title(&panel.changes),
    }
}

pub fn tree_lines(
    panel: &ExplorerPanel,
    theme: &Theme,
    icons: IconSet,
    height: usize,
) -> Vec<Line<'static>> {
    let tree = panel.tree();
    let visible = tree.visible();
    if visible.is_empty() {
        let text = if panel.is_loading() {
            "  reading the worktree\u{2026}"
        } else {
            "  nothing to show"
        };
        return vec![Line::styled(text, Style::default().fg(theme.muted))];
    }

    let cursor = panel.tree_cursor();
    let focused = panel.pane() == Pane::Tree;
    let range = cursor.visible(visible.len(), panel.tree_viewport());
    range
        .take(height)
        .filter_map(|row| {
            let entry = tree.get(*visible.get(row)?)?;
            let indent = "  ".repeat(entry.depth);
            let prefix = if entry.is_dir {
                format!("{indent}{} ", expand_arrow(entry.expanded, icons))
            } else {
                format!("{indent}  ")
            };
            let icon = if entry.is_dir {
                dir_icon(entry.expanded)
            } else {
                entry.icon
            };
            // 未追跡・無視は種別の色より優先して暗くする。theme.muted は一部のテーマで
            // 背景と同化するので hint を使う。
            let fg = match entry.git {
                TreeGitState::Untracked | TreeGitState::Ignored => theme.hint,
                TreeGitState::Tracked if entry.is_dir => theme.info,
                TreeGitState::Tracked => theme.fg,
            };
            let icon_fg = match entry.git {
                TreeGitState::Tracked => icon.role.color(theme),
                _ => fg,
            };
            Some(row_line(
                vec![
                    Span::styled(prefix, Style::default().fg(theme.muted)),
                    Span::styled(
                        format!("{} ", icon.glyph(icons)),
                        Style::default().fg(icon_fg),
                    ),
                    Span::styled(entry.name.clone(), Style::default().fg(fg)),
                ],
                theme,
                row == cursor.selected(),
                focused,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 読み込み前は空の案内になる() {
        let loading = ExplorerPanel::default();
        assert!(
            texts(&tree_lines(
                &loading,
                &Theme::default(),
                IconSet::Unicode,
                10
            ))[0]
                .contains("nothing to show")
        );
    }

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn 見出しは窓に入り切らないときだけ件数を出す() {
        let panel = ExplorerPanel::default();
        assert_eq!(tree_title(&panel, 10), " Explorer ");
    }
}
