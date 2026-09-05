//! Git Changes の描画。行は純関数が組む。

use conductor_core::config::Config;
use conductor_core::diff_state::{DiffListEntry, DiffSource};
use conductor_core::git_engine::GitStatusMap;
use conductor_core::icons::{COMMENT, IconSet, dir_icon, expand_arrow, file_icon};
use conductor_core::review_store::CommentStatus;
use conductor_core::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::log::Row;
use super::{GitChanges, Listing};
use crate::list::row_line;
use crate::review::ReviewState;

pub fn title(changes: &GitChanges) -> String {
    if changes.listing() == Listing::Log {
        return format!(" Git Changes: commits ({}) ", changes.log().commits().len());
    }
    let source = match changes.source() {
        DiffSource::WorkingTree { .. } => String::new(),
        other => format!("{} ", other.label()),
    };
    let total = changes.diff().files.len();
    match changes.diff().error.is_some() {
        true => format!(" Git Changes {source}({total}, error) "),
        false => format!(" Git Changes {source}({total}) "),
    }
}

pub fn lines(
    changes: &GitChanges,
    status: &GitStatusMap,
    review: &ReviewState,
    theme: &Theme,
    config: &Config,
    height: usize,
    focused: bool,
) -> Vec<Line<'static>> {
    if changes.listing() == Listing::Log {
        return log_lines(changes, theme, height, focused);
    }
    let icons = config.ui.icon_set();
    let diff = changes.diff();
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
        let text = if changes.is_loading() {
            "  loading\u{2026}"
        } else {
            "  no changes"
        };
        lines.push(Line::styled(text, Style::default().fg(theme.muted)));
        return lines;
    }

    let cursor = changes.cursor();
    let rows = cursor.visible(diff.display_list.len(), changes.viewport());
    for row in rows.take(height.saturating_sub(changes.banner_rows())) {
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
                let fg = match diff.source {
                    DiffSource::WorkingTree { .. } => stage_color(theme, status.status(&file.path)),
                    DiffSource::Commit { .. } => theme.fg,
                };
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

fn log_lines(
    changes: &GitChanges,
    theme: &Theme,
    height: usize,
    focused: bool,
) -> Vec<Line<'static>> {
    let log = changes.log();
    let cursor = log.cursor();
    let current = changes.source();
    let mut lines = Vec::with_capacity(height);
    for row in cursor.visible(log.len(), log.viewport()).take(height) {
        let spans = match log.row(row) {
            Some(Row::WorkingTree) => {
                let fg = match current {
                    DiffSource::WorkingTree { .. } => theme.accent,
                    DiffSource::Commit { .. } => theme.fg,
                };
                vec![Span::styled("  working tree", Style::default().fg(fg))]
            }
            Some(Row::Commit(i)) => {
                let Some(commit) = log.commits().get(i) else {
                    continue;
                };
                // 背景色が使えないテーマがあるので前景で示す。
                let showing = matches!(current, DiffSource::Commit { oid } if *oid == commit.oid);
                let hash_fg = if showing { theme.accent } else { theme.info };
                vec![
                    Span::styled(
                        format!("  {} ", commit.short_oid),
                        Style::default().fg(hash_fg),
                    ),
                    Span::styled(
                        format!("{:<8}", commit.time_ago),
                        Style::default().fg(theme.hint),
                    ),
                    Span::styled(commit.message.clone(), Style::default().fg(theme.fg)),
                ]
            }
            Some(Row::LoadMore) => {
                let text = if log.is_loading() {
                    "  loading\u{2026}"
                } else {
                    "  load more\u{2026}"
                };
                // Enter で効く行なので、背景と同化しうる muted ではなく hint。
                vec![Span::styled(text, Style::default().fg(theme.hint))]
            }
            None => continue,
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
    use conductor_core::diff_state::{DiffState, FileDiff};

    fn file(path: &str, added: usize) -> FileDiff {
        FileDiff {
            path: path.into(),
            added_lines: added,
            deleted_lines: 0,
            hunks: Vec::new(),
        }
    }

    fn with_diff(files: &[&str], error: Option<&str>) -> GitChanges {
        let mut changes = GitChanges::default();
        let mut diff = DiffState::new(changes.source().clone());
        diff.files = files.iter().map(|p| file(p, 3)).collect();
        diff.error = error.map(str::to_string);
        diff.rebuild_display_list();
        changes.install(diff);
        changes.set_viewport(Viewport::new(0, 20));
        changes
    }

    fn texts(changes: &GitChanges, review: &ReviewState) -> Vec<String> {
        lines(
            changes,
            &GitStatusMap::default(),
            review,
            &Theme::default(),
            &Config::default(),
            10,
            true,
        )
        .iter()
        .map(|l| l.to_string())
        .collect()
    }

    #[test]
    fn エラーの見出しは本当の0件と区別できる() {
        let empty = with_diff(&[], None);
        let errored = with_diff(&[], Some("no such ref"));
        assert_ne!(title(&empty), title(&errored));
        assert!(title(&with_diff(&["a"], Some("x"))).contains('1'));
        assert!(title(&empty).starts_with(" Git Changes "));
    }

    #[test]
    fn コミットの見出しには短縮ハッシュが付く() {
        let mut changes = GitChanges::default();
        let oid = "0123456789abcdef0123456789abcdef01234567";
        changes.set_source(DiffSource::commit(oid));
        changes.install(DiffState::new(DiffSource::commit(oid)));
        assert_eq!(title(&changes), " Git Changes 01234567 (0) ");
    }

    #[test]
    fn コミット一覧は短縮ハッシュで出す() {
        let mut changes = GitChanges::default();
        changes.set_viewport(Viewport::new(0, 20));
        changes.show_log();
        let commits = vec![super::super::log::tests::commit(
            "0123456789abcdef0123456789abcdef01234567",
        )];
        changes.install_log(0, Ok(commits));
        let lines = texts(&changes, &ReviewState::default());
        assert!(lines[0].contains("working tree"), "{lines:?}");
        assert!(
            lines[1].starts_with("  01234567 "),
            "ハッシュの列は短縮形: {:?}",
            lines[1]
        );
        assert_eq!(lines.len(), 2, "1 件で尽きたので読み足しの行は無い");
        assert!(title(&changes).contains("commits (1)"));
    }

    #[test]
    fn バナーはエラー時にだけ1行使う() {
        assert_eq!(with_diff(&["a"], None).banner_rows(), 0);
        assert_eq!(with_diff(&["a"], Some("boom")).banner_rows(), 1);

        let lines = texts(
            &with_diff(&["a"], Some("boom\nsecond")),
            &ReviewState::default(),
        );
        assert!(
            lines[0].contains("boom second"),
            "改行は潰す: {:?}",
            lines[0]
        );
        assert!(lines[1].contains('a'));
    }

    #[test]
    fn 変更なしと読み込み中は別の行になる() {
        let lines = texts(&with_diff(&[], None), &ReviewState::default());
        assert!(lines[0].contains("no changes"));

        let mut loading = GitChanges::default();
        loading.reload();
        let lines = texts(&loading, &ReviewState::default());
        assert!(lines[0].contains("loading"), "{lines:?}");
    }

    #[test]
    fn 変更ファイルの行は増減とviewedを添える() {
        let changes = with_diff(&["a.rs"], None);
        let unmarked = texts(&changes, &ReviewState::default());
        assert!(unmarked[0].contains("+3 -0"), "{:?}", unmarked[0]);
        assert!(!unmarked[0].contains('\u{2713}'));

        let mut review = ReviewState::default();
        review.install(Ok(crate::review::Snapshot {
            viewed: ["a.rs".to_string()].into_iter().collect(),
            ..crate::review::Snapshot::default()
        }));
        assert!(texts(&changes, &review)[0].contains('\u{2713}'));
    }
}
