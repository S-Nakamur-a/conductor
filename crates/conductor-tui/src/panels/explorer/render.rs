//! ファイルツリーと変更ファイル一覧の描画。行は純関数が組み、frame へ渡すのは
//! 最後の 1 手だけ。

use conductor_core::config::Config;
use conductor_core::diff_state::DiffListEntry;
use conductor_core::git_engine::status_map::TreeGitState;
use conductor_core::icons::{COMMENT, IconSet, dir_icon, expand_arrow, file_icon};
use conductor_core::review_store::CommentStatus;
use conductor_core::theme::Theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{BottomView, ExplorerPanel, Pane};
use crate::list::row_line;
use crate::review::ReviewState;
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
        BottomView::Changes => changes_lines(panel, &ws.review, &ws.theme, &ws.config, height),
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
        BottomView::Changes => changes_title(panel),
    }
}

pub fn changes_title(panel: &ExplorerPanel) -> String {
    let total = panel.diff().files.len();
    match panel.diff().error.is_some() {
        true => format!(" Changes ({total}, error) "),
        false => format!(" Changes ({total}) "),
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

pub fn changes_lines(
    panel: &ExplorerPanel,
    review: &ReviewState,
    theme: &Theme,
    config: &Config,
    height: usize,
) -> Vec<Line<'static>> {
    let icons = config.ui.icon_set();
    let diff = panel.diff();
    let mut lines = Vec::with_capacity(height);

    // base 解決の失敗を「変更なし」と混同させないため、先頭行に固定する。バナーは
    // display_list の一部ではないので選択の添字をずらさない。改行を潰すのは、複数行の
    // 行が確保した 1 行より多くを静かに消費するため。
    if let Some(error) = &diff.error {
        lines.push(Line::styled(
            format!("  \u{26a0} {}", error.replace('\n', " ")),
            Style::default().fg(theme.error),
        ));
    }
    if diff.display_list.is_empty() {
        lines.push(Line::styled(
            "  no changes",
            Style::default().fg(theme.muted),
        ));
        return lines;
    }

    let cursor = panel.changes_cursor();
    let focused = panel.pane() == Pane::Bottom;
    let rows = cursor.visible(diff.display_list.len(), panel.changes_viewport());
    for row in rows.take(height.saturating_sub(panel.banner_rows())) {
        let Some(entry) = diff.display_list.get(row) else {
            continue;
        };
        let spans = match entry {
            DiffListEntry::Summary => {
                vec![Span::styled(
                    "  branch summary".to_string(),
                    Style::default().fg(theme.accent),
                )]
            }
            DiffListEntry::Directory {
                name,
                depth,
                collapsed,
                ..
            } => {
                let indent = "  ".repeat(*depth);
                let icon = dir_icon(!*collapsed);
                vec![
                    Span::styled(
                        format!(
                            "  {indent}{} {} ",
                            expand_arrow(!*collapsed, icons),
                            icon.glyph(icons)
                        ),
                        Style::default().fg(theme.info),
                    ),
                    Span::styled(name.clone(), Style::default().fg(theme.info)),
                ]
            }
            DiffListEntry::File { file_index, depth } => {
                // 添字アクセスにしない。display_list とファイルの vec は別のティックで
                // 組み直されるので、片方が古いままのフレームがありうる。
                let Some(file) = diff.files.get(*file_index) else {
                    continue;
                };
                let name = file.path.rsplit('/').next().unwrap_or(&file.path);
                let indent = "  ".repeat(*depth);
                // ファイル名の色は git のステージ状態。行数はベースからの合計なので、
                // その内訳がコミット済みか手元の編集かはこの色でしか分からない。
                let fg = stage_color(theme, panel.tree().status().status(&file.path));
                let icon = file_icon(name);
                let mark = if review.is_viewed(&file.path) {
                    " \u{2713} "
                } else {
                    "   "
                };
                let mut spans = vec![
                    Span::raw(format!("  {indent}")),
                    Span::styled(
                        format!("{} ", icon.glyph(icons)),
                        Style::default().fg(icon.role.color(theme)),
                    ),
                    Span::styled(name.to_string(), Style::default().fg(fg)),
                    Span::styled(
                        format!("  +{}", file.added_lines),
                        Style::default().fg(theme.diff_add),
                    ),
                    Span::styled(
                        format!(" -{}", file.deleted_lines),
                        Style::default().fg(theme.diff_del),
                    ),
                ];
                if let Some(badge) = comment_badge(review, &file.path, theme, icons) {
                    spans.push(badge);
                }
                spans.push(Span::styled(mark, Style::default().fg(theme.success)));
                spans
            }
        };
        lines.push(row_line(spans, theme, row == cursor.selected(), focused));
    }
    lines
}

/// 解決済みが muted でなく hint なのは、muted が一部のテーマで背景と同化するため。
fn comment_badge(
    review: &ReviewState,
    path: &str,
    theme: &Theme,
    icons: IconSet,
) -> Option<Span<'static>> {
    let comments = review.for_file(path);
    if comments.is_empty() {
        return None;
    }
    let unresolved = comments.iter().any(|c| c.status == CommentStatus::Pending);
    let color = if unresolved { theme.accent } else { theme.hint };
    Some(Span::styled(
        format!("  {}{}", COMMENT.get(icons), comments.len()),
        Style::default().fg(color),
    ))
}

/// None は status のエントリが無い、つまり HEAD に対してクリーン。編集 → add → さらに
/// 編集で WT_* と INDEX_* が両方立つので、unstaged を先に見る。
fn stage_color(theme: &Theme, status: Option<git2::Status>) -> ratatui::style::Color {
    let Some(status) = status else {
        return theme.success;
    };
    if status.is_wt_new() {
        theme.hint
    } else if status.is_wt_modified()
        || status.is_wt_deleted()
        || status.is_wt_renamed()
        || status.is_wt_typechange()
    {
        theme.error
    } else if status.is_index_new()
        || status.is_index_modified()
        || status.is_index_deleted()
        || status.is_index_renamed()
        || status.is_index_typechange()
    {
        theme.warning
    } else {
        theme.success
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::Viewport;
    use crate::task::TaskResult;
    use conductor_core::diff_state::{DiffState, FileDiff};

    fn file(path: &str, added: usize) -> FileDiff {
        FileDiff {
            path: path.into(),
            added_lines: added,
            deleted_lines: 0,
            hunks: Vec::new(),
        }
    }

    fn with_diff(files: &[&str], error: Option<&str>) -> ExplorerPanel {
        let mut panel = ExplorerPanel::default();
        let mut diff = DiffState::new("main");
        diff.files = files.iter().map(|p| file(p, 3)).collect();
        diff.error = error.map(str::to_string);
        diff.rebuild_display_list();
        panel.apply_result(TaskResult::Diff(Box::new(diff)));
        panel.changes_view = Viewport::new(0, 20);
        panel
    }

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn エラーの見出しは本当の0件と区別できる() {
        let empty = with_diff(&[], None);
        let errored = with_diff(&[], Some("no such ref"));
        assert_ne!(changes_title(&empty), changes_title(&errored));
        assert!(changes_title(&with_diff(&["a"], Some("x"))).contains('1'));
    }

    #[test]
    fn バナーはエラー時にだけ1行使う() {
        assert_eq!(with_diff(&["a"], None).banner_rows(), 0);
        assert_eq!(with_diff(&["a"], Some("boom")).banner_rows(), 1);

        let lines = texts(&changes_lines(
            &with_diff(&["a"], Some("boom\nsecond")),
            &ReviewState::default(),
            &Theme::default(),
            &Config::default(),
            10,
        ));
        assert!(
            lines[0].contains("boom second"),
            "改行は潰す: {:?}",
            lines[0]
        );
        assert!(lines[1].contains('a'));
    }

    #[test]
    fn 変更なしと読み込み前は別の行になる() {
        let lines = texts(&changes_lines(
            &with_diff(&[], None),
            &ReviewState::default(),
            &Theme::default(),
            &Config::default(),
            10,
        ));
        assert!(lines[0].contains("no changes"));

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

    #[test]
    fn 変更ファイルの行は増減とviewedを添える() {
        let panel = with_diff(&["a.rs"], None);
        let render = |review: &ReviewState| {
            texts(&changes_lines(
                &panel,
                review,
                &Theme::default(),
                &Config::default(),
                10,
            ))
        };
        let unmarked = render(&ReviewState::default());
        assert!(unmarked[0].contains("+3 -0"), "{:?}", unmarked[0]);
        assert!(!unmarked[0].contains('\u{2713}'));

        let mut review = ReviewState::default();
        review.install(Ok(crate::review::Snapshot {
            viewed: ["a.rs".to_string()].into_iter().collect(),
            ..crate::review::Snapshot::default()
        }));
        assert!(render(&review)[0].contains('\u{2713}'));
    }

    #[test]
    fn 見出しは窓に入り切らないときだけ件数を出す() {
        let panel = ExplorerPanel::default();
        assert_eq!(tree_title(&panel, 10), " Explorer ");
    }
}
