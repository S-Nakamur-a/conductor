//! Git Changes の描画。行は純関数が組む。

use conductor_core::config::Config;
use conductor_core::diff_state::{DiffListEntry, DiffSource, FileDiff, FileStatus};
use conductor_core::git_engine::GitStatusMap;
use conductor_core::icons::{COMMENT, IconSet, expand_arrow};
use conductor_core::review_store::CommentStatus;
use conductor_core::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::log::Row;
use super::{GitChanges, Listing};
use crate::list::row_line;
use crate::review::ReviewState;
use crate::strip::{truncate_to_width, width_of};

pub fn title(changes: &GitChanges) -> String {
    if changes.listing() == Listing::Log {
        return format!(" Git Changes: commits ({}) ", changes.log().commits().len());
    }
    let source = match changes.source() {
        DiffSource::WorkingTree { .. } => String::new(),
        other => format!("{} ", other.label()),
    };
    let diff = changes.diff();
    let total = diff.files.len();
    if diff.error.is_some() {
        return format!(" Git Changes {source}({total}, error) ");
    }
    if diff.files.is_empty() {
        return format!(" Git Changes {source}({total}) ");
    }
    format!(" Git Changes {source}({total}) {} ", totals(&diff.files))
}

/// 一覧全体の増減。見出しに出す。
fn totals(files: &[FileDiff]) -> String {
    let added: usize = files.iter().map(|f| f.added_lines).sum();
    let deleted: usize = files.iter().map(|f| f.deleted_lines).sum();
    format!("+{added} -{deleted}")
}

/// 行を組むのに要る区画の大きさ。増減を右端へ寄せるので幅が要る。
#[derive(Clone, Copy)]
pub struct Area {
    pub width: usize,
    pub height: usize,
}

pub fn lines(
    changes: &GitChanges,
    status: &GitStatusMap,
    review: &ReviewState,
    theme: &Theme,
    config: &Config,
    area: Area,
    focused: bool,
) -> Vec<Line<'static>> {
    let Area { width, height } = area;
    if changes.listing() == Listing::Log {
        return log_lines(changes, theme, width, height, focused);
    }
    let icons = config.ui.icon_set();
    let diff = changes.diff();
    let stats_w = StatsWidth::of(&diff.files);
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
                vec![
                    Span::styled(
                        format!("  {indent}{} ", expand_arrow(!*collapsed, icons)),
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
                let indent = "  ".repeat(*depth);
                // ファイル名の色は git のステージ状態。変更の種類は行頭の 1 文字が持つので、
                // コミットを見ているときも追加と削除が区別できる。
                let fg = match diff.source {
                    DiffSource::WorkingTree { .. } => stage_color(theme, status.status(&file.path)),
                    DiffSource::Commit { .. } => theme.fg,
                };
                let head = format!("  {indent}");
                let badge = comment_badge(review, &file.path, theme, icons);
                let viewed = review.is_viewed(&file.path);
                let stats = Stats::new(file, stats_w);
                // 名前を削ってでも増減は残す。
                let reserved = width_of(&head) as usize
                    + 2
                    + badge.as_ref().map_or(0, |b| width_of(&b.content) as usize)
                    + usize::from(viewed) * 2
                    + stats.width()
                    + 1;
                let name = file.path.rsplit('/').next().unwrap_or(&file.path);
                let mut spans = vec![
                    Span::raw(head),
                    Span::styled(
                        format!("{} ", file.status.letter()),
                        Style::default().fg(status_color(theme, file.status)),
                    ),
                    Span::styled(
                        truncate_to_width(name, width.saturating_sub(reserved)),
                        Style::default().fg(fg),
                    ),
                ];
                if let Some(badge) = badge {
                    spans.push(badge);
                }
                if viewed {
                    spans.push(Span::styled(
                        " \u{2713}",
                        Style::default().fg(theme.success),
                    ));
                }
                stats.push(&mut spans, width, theme);
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
    width: usize,
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
                let showing = matches!(current, DiffSource::WorkingTree { .. });
                let fg = if showing { theme.accent } else { theme.fg };
                let mut spans = vec![Span::styled(
                    format!("{} working tree", pointer(showing)),
                    Style::default().fg(fg),
                )];
                // 合計を出せるのは作業ツリーを読み込んでいるときだけ。コミットを見ている
                // 間の数字は手元の変更を表さないので、出さない。
                if showing {
                    let total = totals(&changes.diff().files);
                    push_right(&mut spans, total, width, theme.hint);
                }
                spans
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
                        format!("{} ", pointer(showing)),
                        Style::default().fg(theme.accent),
                    ),
                    Span::styled(
                        format!("{} ", commit.short_oid),
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

/// 増減の桁。一覧で一番大きいものに合わせて縦に読めるようにする。
#[derive(Clone, Copy, Default)]
struct StatsWidth {
    added: usize,
    deleted: usize,
}

impl StatsWidth {
    fn of(files: &[FileDiff]) -> Self {
        let digits = |n: usize| n.to_string().len();
        Self {
            added: files
                .iter()
                .map(|f| digits(f.added_lines))
                .max()
                .unwrap_or(1),
            deleted: files
                .iter()
                .map(|f| digits(f.deleted_lines))
                .max()
                .unwrap_or(1),
        }
    }
}

/// 出どころの目印。色差の弱いテーマ向けに文字で示す (`▸` は CJK 端末で 2 桁になる)。
fn pointer(showing: bool) -> &'static str {
    if showing { " >" } else { "  " }
}

fn status_color(theme: &Theme, status: FileStatus) -> ratatui::style::Color {
    match status {
        FileStatus::Added | FileStatus::Untracked => theme.diff_add,
        FileStatus::Deleted => theme.diff_del,
        FileStatus::Modified => theme.warning,
        FileStatus::Renamed | FileStatus::TypeChange => theme.info,
    }
}

/// 右端に寄せる増減。名前より先に幅を取る。
struct Stats {
    added: String,
    deleted: String,
}

impl Stats {
    fn new(file: &FileDiff, w: StatsWidth) -> Self {
        Self {
            added: format!("{:>1$}", format!("+{}", file.added_lines), w.added + 1),
            deleted: format!(
                " {:>1$} ",
                format!("-{}", file.deleted_lines),
                w.deleted + 1
            ),
        }
    }

    fn width(&self) -> usize {
        (width_of(&self.added) + width_of(&self.deleted)) as usize
    }

    fn push(self, spans: &mut Vec<Span<'static>>, width: usize, theme: &Theme) {
        spans.push(Span::raw(gap(spans, width, self.width())));
        spans.push(Span::styled(
            self.added,
            Style::default().fg(theme.diff_add),
        ));
        spans.push(Span::styled(
            self.deleted,
            Style::default().fg(theme.diff_del),
        ));
    }
}

fn push_right(
    spans: &mut Vec<Span<'static>>,
    text: String,
    width: usize,
    color: ratatui::style::Color,
) {
    let tail = width_of(&text) as usize + 1;
    spans.push(Span::raw(gap(spans, width, tail)));
    spans.push(Span::styled(format!("{text} "), Style::default().fg(color)));
}

/// 右端に寄せるための空白。入り切らないときも 1 つは空けて、名前と数字がくっつかないようにする。
fn gap(spans: &[Span<'static>], width: usize, tail: usize) -> String {
    let used: usize = spans.iter().map(|s| width_of(&s.content) as usize).sum();
    " ".repeat(width.saturating_sub(used + tail).max(1))
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
    use conductor_core::diff_state::{DiffState, FileDiff, FileStatus};

    fn file(path: &str, added: usize) -> FileDiff {
        FileDiff {
            path: path.into(),
            status: FileStatus::Modified,
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

    fn with_files(source: DiffSource, files: Vec<FileDiff>) -> GitChanges {
        let mut changes = GitChanges::default();
        changes.set_source(source.clone());
        let mut diff = DiffState::new(source);
        diff.files = files;
        diff.rebuild_display_list();
        changes.install(diff);
        changes.set_viewport(Viewport::new(0, 20));
        changes
    }

    fn sized(path: &str, status: FileStatus, added: usize, deleted: usize) -> FileDiff {
        FileDiff {
            path: path.into(),
            status,
            added_lines: added,
            deleted_lines: deleted,
            hunks: Vec::new(),
        }
    }

    #[test]
    fn 変更の種類はコミットを見ている間も文字で分かる() {
        let source = DiffSource::commit("0123456789abcdef0123456789abcdef01234567");
        let changes = with_files(
            source,
            vec![
                sized("added.rs", FileStatus::Added, 9, 0),
                sized("gone.rs", FileStatus::Deleted, 0, 4),
            ],
        );
        let lines = texts(&changes, &ReviewState::default());
        assert!(lines[0].starts_with("  A added.rs"), "{:?}", lines[0]);
        assert!(lines[1].starts_with("  D gone.rs"), "{:?}", lines[1]);
    }

    #[test]
    fn 増減は右端で桁が揃う() {
        let changes = with_files(
            DiffSource::working_tree("main"),
            vec![
                sized("a.rs", FileStatus::Modified, 1, 128),
                sized("b.rs", FileStatus::Modified, 12, 3),
            ],
        );
        let lines = texts(&changes, &ReviewState::default());
        assert!(lines[0].ends_with(" +1 -128 "), "{:?}", lines[0]);
        assert!(lines[1].ends_with("+12   -3 "), "{:?}", lines[1]);
        assert_eq!(
            lines[0].chars().count(),
            lines[1].chars().count(),
            "右端が揃う: {lines:?}"
        );
    }

    fn texts(changes: &GitChanges, review: &ReviewState) -> Vec<String> {
        texts_at(changes, review, 44)
    }

    fn texts_at(changes: &GitChanges, review: &ReviewState, width: usize) -> Vec<String> {
        lines(
            changes,
            &GitStatusMap::default(),
            review,
            &Theme::default(),
            &Config::default(),
            Area { width, height: 10 },
            true,
        )
        .iter()
        .map(|l| l.to_string())
        .collect()
    }

    #[test]
    fn 狭い区画では増減より先に名前が削られる() {
        let changes = with_files(
            DiffSource::working_tree("main"),
            vec![sized(
                "crates/tui/very_long_file_name.rs",
                FileStatus::Modified,
                3,
                128,
            )],
        );
        let line = texts_at(&changes, &ReviewState::default(), 24)
            .pop()
            .unwrap();
        assert!(line.ends_with(" -128 "), "削除の数が残る: {line:?}");
        assert!(line.contains('\u{2026}'), "名前が削られる: {line:?}");
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
            lines[1].starts_with("   01234567 "),
            "ハッシュの列は短縮形: {:?}",
            lines[1]
        );
        assert_eq!(lines.len(), 2, "1 件で尽きたので読み足しの行は無い");
        assert!(title(&changes).contains("commits (1)"));
    }

    #[test]
    fn 出どころの目印は色ではなく文字で付く() {
        let mut changes = GitChanges::default();
        changes.set_viewport(Viewport::new(0, 20));
        changes.show_log();
        let oid = "0123456789abcdef0123456789abcdef01234567";
        changes.install_log(0, Ok(vec![super::super::log::tests::commit(oid)]));

        let lines = texts(&changes, &ReviewState::default());
        assert!(lines[0].starts_with(" >"), "作業ツリー: {:?}", lines[0]);
        assert!(!lines[1].starts_with(" >"), "コミット: {:?}", lines[1]);

        changes.set_source(DiffSource::commit(oid));
        let lines = texts(&changes, &ReviewState::default());
        assert!(!lines[0].starts_with(" >"), "作業ツリー: {:?}", lines[0]);
        assert!(lines[1].starts_with(" >"), "コミット: {:?}", lines[1]);
    }

    #[test]
    fn バナーの分だけ窓がずれるのはファイル一覧だけ() {
        let mut changes = with_diff(&["a"], Some("boom"));
        changes.set_viewport(Viewport::new(5, 10));
        assert_eq!(changes.viewport(), Viewport::new(6, 9));
        assert_eq!(changes.log().viewport(), Viewport::new(5, 10));
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
