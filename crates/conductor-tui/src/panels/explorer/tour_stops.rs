//! ツアーの停留所一覧。Explorer の下区画に出る。
//!
//! カーソルが場所の行に来た時点で Viewer を開く。押して確定ではなく上下で切り替わるのが
//! この画面の用途で、preview で開くのはそのため — 溜まったタブを後で畳む手間が要らない。

use std::collections::HashSet;
use std::path::Path;

use conductor_core::diff_state::FileDiff;
use conductor_core::icons::{IconSet, PANEL_REVIEW, expand_arrow};
use conductor_core::keymap::Action;
use conductor_core::theme::Theme;
use conductor_core::tour::{self, Place, Side, Stop, Tour};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::effect::Effect;
use crate::list::{ListCursor, Viewport, row_line};

/// 語りと場所の字下げ。幅の計算と描画が同じ値を読む。
const INDENT: usize = 4;

/// 平坦化した 1 行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Stop(usize),
    Narration { stop: usize, line: usize },
    Place { stop: usize, place: usize },
}

#[derive(Debug, Default)]
pub struct TourStops {
    tour: Option<Box<Tour>>,
    /// 読めるはずのものが読めなかった理由。無い時と読めた時は None。
    error: Option<String>,
    cursor: ListCursor,
    view: Viewport,
    /// 畳んだ停留所。既定は開いているので、覚えるのは裏返した方だけ。
    collapsed: HashSet<usize>,
    /// 末尾に連結した「誰も説明していない変更」の位置。台本の停留所ではないので、
    /// ここだけは差分として開く。
    coverage: Option<usize>,
    misplaced: HashSet<(usize, usize)>,
    /// 語りを折り返す桁数。行数が幅で変わるので、当たり判定と同じ値を使う。
    narration_width: usize,
}

impl TourStops {
    pub fn tour(&self) -> Option<&Tour> {
        self.tour.as_deref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn cursor(&self) -> ListCursor {
        self.cursor
    }

    pub fn viewport(&self) -> Viewport {
        self.view
    }

    pub fn set_viewport(&mut self, view: Viewport) {
        self.view = view;
    }

    pub fn set_width(&mut self, inner: u16) {
        self.narration_width = usize::from(inner).saturating_sub(INDENT);
    }

    /// 台本を読み直す。開く操作のたびに読むので、外で書き換えられた台本が古いまま
    /// 残らない。
    pub fn load(&mut self, worktree: &Path, changed: &[FileDiff]) {
        self.coverage = None;
        self.misplaced.clear();
        match tour::load(worktree) {
            tour::Outcome::Missing => {
                self.tour = None;
                self.error = None;
            }
            tour::Outcome::Loaded(mut tour) => {
                // 検証するのは台本が書いた分だけ。網羅性の車両の場所は diff から計算した
                // ものなので作り話にならず、コミットの diff なら作業ツリーの行数と食い違う。
                self.misplaced = tour::misplaced(&tour, worktree).into_iter().collect();
                self.append_coverage(&mut tour, changed);
                self.tour = Some(tour);
                self.error = None;
            }
            tour::Outcome::Broken(why) => {
                self.tour = None;
                self.error = Some(why);
            }
        }
        self.collapsed.clear();
        self.cursor = ListCursor::default();
        self.clamp();
    }

    fn append_coverage(&mut self, tour: &mut Tour, changed: &[FileDiff]) {
        let missed = tour::unexplained(tour, changed);
        if missed.is_empty() {
            return;
        }
        self.coverage = Some(tour.stops.len());
        tour.stops.push(Stop {
            title: format!("誰も説明していない変更 ({})", missed.len()),
            narration: "台本が触れていない。差分として開く。".to_string(),
            places: missed,
        });
    }

    pub fn rows(&self) -> Vec<Row> {
        let Some(tour) = self.tour() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for (stop, entry) in tour.stops.iter().enumerate() {
            rows.push(Row::Stop(stop));
            if self.collapsed.contains(&stop) {
                continue;
            }
            rows.extend(
                (0..narration_lines(&entry.narration, self.narration_width).len())
                    .map(|line| Row::Narration { stop, line }),
            );
            rows.extend((0..entry.places.len()).map(|place| Row::Place { stop, place }));
        }
        rows
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.rows().get(self.cursor.selected()).copied()
    }

    pub fn clamp(&mut self) {
        self.cursor.clamp(self.rows().len(), self.view);
    }

    pub fn scroll(&mut self, delta: isize) {
        self.cursor.pan(delta, self.rows().len(), self.view);
    }

    pub fn click(&mut self, y: u16) -> Option<Vec<Effect>> {
        let len = self.rows().len();
        let row = self.cursor.index_at(y, len, self.view)?;
        self.cursor.select(row, len, self.view);
        Some(self.reveal_all(false))
    }

    pub fn update(&mut self, action: Action) -> Option<Vec<Effect>> {
        let len = self.rows().len();
        match action {
            Action::NavigateDown => self.cursor.step(1, len, self.view),
            Action::NavigateUp => self.cursor.step(-1, len, self.view),
            Action::GoToTop => self.cursor.select(0, len, self.view),
            Action::GoToBottom => self.cursor.select(usize::MAX, len, self.view),
            Action::CollapseOrLeft => {
                self.collapse();
                return Some(Vec::new());
            }
            Action::ExpandOrRight => {
                self.expand();
                return Some(Vec::new());
            }
            Action::Select => return Some(self.reveal_all(false)),
            _ => return None,
        }
        Some(self.reveal_all(true))
    }

    fn reveal_all(&self, preview: bool) -> Vec<Effect> {
        let mut out: Vec<Effect> = self.reveal(preview).into_iter().collect();
        if let Some(row) = self.selected_row()
            && !self.is_misplaced(row)
            && let Some(place) = self.place_of(row)
            && place.side == Side::New
            && let (Some(start), Some(end)) = (place.start, place.end.or(place.start))
        {
            out.push(Effect::TourRange {
                path: place.path.clone(),
                start,
                end,
            });
        }
        out
    }

    fn place_of(&self, row: Row) -> Option<&Place> {
        match row {
            Row::Narration { .. } => None,
            Row::Stop(stop) => self.place(stop, 0),
            Row::Place { stop, place } => self.place(stop, place),
        }
    }

    fn reveal(&self, preview: bool) -> Option<Effect> {
        let row = self.selected_row()?;
        let place = self.place_of(row)?;
        // 前像の行番号は渡さない。Viewer は行を後像として辿るので、渡すと無関係な行へ着く。
        // 存在しない行も同じ扱いにする。
        let line = match (place.side, self.is_misplaced(row)) {
            (Side::New, false) => place.start.map(|n| n as usize),
            _ => None,
        };
        if self.coverage == Some(stop_of(row)) {
            return Some(Effect::OpenChangedFile {
                path: place.path.clone(),
                line,
            });
        }
        Some(Effect::OpenFile {
            path: place.path.clone().into(),
            line,
            diff: None,
            preview,
        })
    }

    fn is_misplaced(&self, row: Row) -> bool {
        match row {
            Row::Place { stop, place } => self.misplaced.contains(&(stop, place)),
            Row::Stop(stop) => self.misplaced.contains(&(stop, 0)),
            Row::Narration { .. } => false,
        }
    }

    fn place(&self, stop: usize, place: usize) -> Option<&Place> {
        self.tour()?.stops.get(stop)?.places.get(place)
    }

    /// 子の行から畳むと自分の行が消えるので、先に見出しへ寄ってから畳む。
    fn collapse(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let stop = stop_of(row);
        if !matches!(row, Row::Stop(_))
            && let Some(at) = self.rows().iter().position(|r| *r == Row::Stop(stop))
        {
            let len = self.rows().len();
            self.cursor.select(at, len, self.view);
        }
        self.collapsed.insert(stop);
        self.clamp();
    }

    fn expand(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        self.collapsed.remove(&stop_of(row));
        self.clamp();
    }
}

fn stop_of(row: Row) -> usize {
    match row {
        Row::Stop(stop) | Row::Narration { stop, .. } | Row::Place { stop, .. } => stop,
    }
}

/// 語りを区画の実幅で折り返す。空行は落とす — 狭い区画では場所が画面から押し出される。
fn narration_lines(narration: &str, width: usize) -> Vec<String> {
    let width = width.max(4);
    let mut out = Vec::new();
    for paragraph in paragraphs(narration) {
        let mut row = String::new();
        let mut used = 0;
        for c in paragraph.chars() {
            let w = UnicodeWidthChar::width(c).unwrap_or(0);
            if used + w > width && !row.is_empty() {
                out.push(std::mem::take(&mut row));
                used = 0;
            }
            row.push(c);
            used += w;
        }
        if !row.is_empty() {
            out.push(row);
        }
    }
    out
}

/// 空行で区切られたかたまり。中の改行は繋ぐ — 台本が字数で折った跡をそのまま行にすると、
/// 折り返しのたびに半端な行が残る。日本語の境目に空白は入れない。
fn paragraphs(narration: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut open = false;
    for line in narration.lines() {
        let line = line.trim();
        if line.is_empty() {
            open = false;
            continue;
        }
        match open {
            false => {
                out.push(line.to_string());
                open = true;
            }
            true => {
                let held = out.last_mut().expect("open なら 1 つ以上ある");
                let joins_bare =
                    held.chars().next_back().is_some_and(is_wide) && line.starts_with(is_wide);
                if !joins_bare {
                    held.push(' ');
                }
                held.push_str(line);
            }
        }
    }
    out
}

fn is_wide(c: char) -> bool {
    UnicodeWidthChar::width(c) == Some(2)
}

pub fn title(stops: &TourStops, set: IconSet) -> String {
    let Some(tour) = stops.tour() else {
        return format!(" {}Tour ", PANEL_REVIEW.labeled(set));
    };
    let at = stops
        .selected_row()
        .map(|row| stop_of(row) + 1)
        .unwrap_or(0);
    format!(
        " {}Tour ({at}/{}) ",
        PANEL_REVIEW.labeled(set),
        tour.stops.len()
    )
}

pub fn lines(
    stops: &TourStops,
    theme: &Theme,
    set: IconSet,
    height: usize,
    focused: bool,
) -> Vec<Line<'static>> {
    if let Some(why) = stops.error() {
        return vec![Line::styled(
            format!("  {why}"),
            Style::default().fg(theme.warning),
        )];
    }
    let rows = stops.rows();
    if rows.is_empty() {
        return vec![Line::styled(
            "  no tour on this branch",
            Style::default().fg(theme.muted),
        )];
    }
    let cursor = stops.cursor();
    cursor
        .visible(rows.len(), stops.viewport())
        .take(height)
        .filter_map(|row| {
            let spans = spans_for(stops, *rows.get(row)?, theme, set)?;
            Some(row_line(spans, theme, row == cursor.selected(), focused))
        })
        .collect()
}

fn spans_for(
    stops: &TourStops,
    row: Row,
    theme: &Theme,
    set: IconSet,
) -> Option<Vec<Span<'static>>> {
    let tour = stops.tour()?;
    let stop = tour.stops.get(stop_of(row))?;
    let spans = match row {
        Row::Stop(index) => {
            let arrow = expand_arrow(!stops.collapsed.contains(&index), set);
            vec![
                Span::styled(format!("{arrow} "), Style::default().fg(theme.muted)),
                Span::styled(
                    format!("{}. ", index + 1),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    stop.title.clone(),
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                ),
            ]
        }
        Row::Narration { line, .. } => vec![Span::styled(
            format!(
                "{}{}",
                " ".repeat(INDENT),
                narration_lines(&stop.narration, stops.narration_width).get(line)?
            ),
            Style::default().fg(theme.muted),
        )],
        Row::Place { place, .. } => {
            let mut spans = vec![
                Span::styled(
                    format!("{}\u{21b3} ", " ".repeat(INDENT)),
                    Style::default().fg(theme.info),
                ),
                Span::styled(
                    location(stop.places.get(place)?),
                    Style::default().fg(theme.fg),
                ),
            ];
            if stops.is_misplaced(row) {
                spans.push(Span::styled(
                    "  その行は無い",
                    Style::default().fg(theme.warning),
                ));
            }
            spans
        }
    };
    Some(spans)
}

fn location(place: &Place) -> String {
    let name = place.path.rsplit('/').next().unwrap_or(&place.path);
    let deleted = match place.side {
        Side::New => "",
        Side::Old => " 削除",
    };
    match (place.start, place.end) {
        (Some(start), Some(end)) if end != start => format!("{name}:L{start}-{end}{deleted}"),
        (Some(start), _) => format!("{name}:L{start}{deleted}"),
        (None, _) => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"{
      "schema_version": 1,
      "stops": [
        {
          "title": "キーを計画ビューへ回す",
          "narration": "メニューとモーダルより後。\n\nPTY 転送より前。",
          "places": [
            { "path": "crates/conductor-tui/src/route.rs", "start": 40, "end": 49 },
            { "path": "crates/conductor-tui/src/workspace.rs", "start": 210 }
          ]
        },
        { "title": "成果物を読む", "narration": "", "places": [
            { "path": "crates/conductor-core/src/tour.rs" }
        ] }
      ]
    }"#;

    fn changed(path: &str, side: Side, line: usize) -> FileDiff {
        use conductor_core::diff_state::{DiffHunk, DiffLine, DiffLineTag};
        let (tag, old_line_no, new_line_no) = match side {
            Side::New => (DiffLineTag::Insert, None, Some(line)),
            Side::Old => (DiffLineTag::Delete, Some(line), None),
        };
        FileDiff {
            path: path.into(),
            status: conductor_core::diff_state::FileStatus::Modified,
            added_lines: usize::from(side == Side::New),
            deleted_lines: usize::from(side == Side::Old),
            hunks: vec![DiffHunk {
                lines: vec![DiffLine {
                    tag,
                    old_line_no,
                    new_line_no,
                    inline_segments: Vec::new(),
                    content: String::new(),
                }],
                func_header: None,
            }],
        }
    }

    /// 台本が指すファイルを実際に置く。無いと行番号の検証に引っかかり、ここで試したい
    /// 移動の話ではなく「その行は無い」の話になる。
    fn worktree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".conductor")).unwrap();
        std::fs::write(conductor_core::tour::artifact_path(dir.path()), SCRIPT).unwrap();
        let body = "x\n".repeat(300);
        for path in [
            "crates/conductor-tui/src/route.rs",
            "crates/conductor-tui/src/workspace.rs",
            "crates/conductor-core/src/tour.rs",
        ] {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, &body).unwrap();
        }
        dir
    }

    /// 折り返しが行数を変えるので、構造を見るテストは折り返らない幅で開く。
    fn with_changes(changed: &[FileDiff]) -> TourStops {
        let dir = worktree();
        let mut stops = TourStops::default();
        stops.set_viewport(Viewport::new(0, 40));
        stops.set_width(200);
        stops.load(dir.path(), changed);
        stops
    }

    fn stops() -> TourStops {
        let dir = worktree();
        let mut stops = TourStops::default();
        stops.set_viewport(Viewport::new(0, 30));
        stops.set_width(200);
        stops.load(dir.path(), &[]);
        stops
    }

    #[test]
    fn 語りは段落を繋いでから区画の実幅で折り返す() {
        let narration = "メニューとモーダル\nより後。\n\nPTY 転送より前。";

        assert_eq!(
            narration_lines(narration, 40),
            ["メニューとモーダルより後。", "PTY 転送より前。"],
            "日本語の境目に空白を入れない"
        );

        // 日本語は 1 文字 2 桁。13 文字は 26 桁あるので 20 桁には入らない。
        assert_eq!(
            narration_lines(narration, 20),
            ["メニューとモーダルよ", "り後。", "PTY 転送より前。"]
        );
    }

    #[test]
    fn 段落内の英単語は空白で繋ぐ() {
        assert_eq!(
            narration_lines("keeps the\nwords apart", 40),
            ["keeps the words apart"]
        );
    }

    #[test]
    fn 停留所の下に語りと場所が並び空行は数えない() {
        let stops = stops();
        assert_eq!(
            stops.rows(),
            [
                Row::Stop(0),
                Row::Narration { stop: 0, line: 0 },
                Row::Narration { stop: 0, line: 1 },
                Row::Place { stop: 0, place: 0 },
                Row::Place { stop: 0, place: 1 },
                Row::Stop(1),
                Row::Place { stop: 1, place: 0 },
            ]
        );
    }

    #[test]
    fn カーソルを動かすだけでその場所がpreviewで開く() {
        let mut stops = stops();
        let effects = stops.update(Action::NavigateDown).unwrap();
        assert!(effects.is_empty(), "語りの行では開かない: {effects:?}");

        stops.update(Action::NavigateDown);
        let effects = stops.update(Action::NavigateDown).unwrap();
        let [
            Effect::OpenFile {
                path,
                line,
                preview,
                ..
            },
            Effect::TourRange { start, end, .. },
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(*line, Some(40));
        assert!(*preview, "移動で開くタブは preview");
        assert!(path.ends_with("route.rs"), "{path:?}");
        assert_eq!((*start, *end), (40, 49), "帯を出す範囲も渡す");

        let effects = stops.update(Action::NavigateDown).unwrap();
        let [Effect::OpenFile { path, line, .. }, ..] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(*line, Some(210));
        assert!(path.ends_with("workspace.rs"), "{path:?}");
    }

    #[test]
    fn 見出しからは最初の場所へ飛びenterは固定して開く() {
        let mut stops = stops();
        let effects = stops.update(Action::Select).unwrap();
        let [Effect::OpenFile { line, preview, .. }, ..] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(*line, Some(40), "見出しの行は最初の場所を出す");
        assert!(!*preview, "enter で開くタブは固定");
    }

    #[test]
    fn 場所の行から畳むと見出しへ寄る() {
        let mut stops = stops();
        stops.update(Action::NavigateDown);
        stops.update(Action::NavigateDown);
        stops.update(Action::NavigateDown);
        assert!(matches!(
            stops.selected_row(),
            Some(Row::Place { stop: 0, place: 0 })
        ));

        stops.update(Action::CollapseOrLeft);
        assert_eq!(stops.selected_row(), Some(Row::Stop(0)));
        assert_eq!(
            stops.rows(),
            [Row::Stop(0), Row::Stop(1), Row::Place { stop: 1, place: 0 }],
            "畳むのは今いる停留所だけ"
        );

        stops.update(Action::ExpandOrRight);
        assert_eq!(stops.rows().len(), 7);
    }

    #[test]
    fn 説明されていない変更は末尾の停留所になり差分として開く() {
        let mut stops = with_changes(&[changed("wild.rs", Side::New, 7)]);

        let last = stops.tour().unwrap().stops.last().unwrap();
        assert!(last.title.contains("誰も説明していない"), "{}", last.title);
        assert_eq!(
            last.places,
            [Place {
                path: "wild.rs".into(),
                side: Side::New,
                start: Some(7),
                end: Some(7),
            }]
        );

        stops.update(Action::GoToBottom);
        let effects = stops.update(Action::Select).unwrap();
        let [Effect::OpenChangedFile { path, line }, ..] = effects.as_slice() else {
            panic!("差分として開かない: {effects:?}");
        };
        assert_eq!((path.as_str(), *line), ("wild.rs", Some(7)));
    }

    #[test]
    fn 削除行の停留所は行を渡さずラベルに削除と出す() {
        let mut stops = with_changes(&[changed("gone.rs", Side::Old, 42)]);

        stops.update(Action::GoToBottom);
        let effects = stops.update(Action::Select).unwrap();
        let [Effect::OpenChangedFile { path, line }, ..] = effects.as_slice() else {
            panic!("差分として開かない: {effects:?}");
        };
        assert_eq!((path.as_str(), *line), ("gone.rs", None));

        let rendered: Vec<String> = lines(&stops, &Theme::default(), IconSet::Unicode, 40, true)
            .iter()
            .map(Line::to_string)
            .collect();
        assert!(
            rendered.iter().any(|row| row.contains("gone.rs:L42 削除")),
            "{rendered:?}"
        );
    }

    #[test]
    fn 存在しない行を指す場所は印を付けて行を渡さない() {
        let dir = worktree();
        // 台本は 40-49 を指しているが、そこまで行が無い。
        std::fs::write(
            dir.path().join("crates/conductor-tui/src/route.rs"),
            "one\ntwo\n",
        )
        .unwrap();
        let mut stops = TourStops::default();
        stops.set_viewport(Viewport::new(0, 40));
        stops.load(dir.path(), &[]);

        let rendered: Vec<String> = lines(&stops, &Theme::default(), IconSet::Unicode, 40, true)
            .iter()
            .map(Line::to_string)
            .collect();
        assert!(
            rendered.iter().any(|row| row.contains("その行は無い")),
            "{rendered:?}"
        );

        stops.update(Action::GoToTop);
        let effects = stops.update(Action::Select).unwrap();
        let [Effect::OpenFile { line, .. }, ..] = effects.as_slice() else {
            panic!("{effects:?}");
        };
        assert_eq!(*line, None);
    }

    #[test]
    fn 台本が壊れていれば理由を1行出す() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".conductor")).unwrap();
        std::fs::write(conductor_core::tour::artifact_path(dir.path()), "{ nope").unwrap();
        let mut stops = TourStops::default();
        stops.set_viewport(Viewport::new(0, 30));
        stops.load(dir.path(), &[]);

        let rendered = lines(&stops, &Theme::default(), IconSet::Unicode, 10, false);
        assert_eq!(rendered.len(), 1);
        assert!(rendered[0].to_string().contains("tour.json"));
    }

    #[test]
    fn 行は番号と場所を添える() {
        let stops = stops();
        let rendered: Vec<String> = lines(&stops, &Theme::default(), IconSet::Unicode, 30, true)
            .iter()
            .map(Line::to_string)
            .collect();
        assert!(
            rendered[0].contains("1. キーを計画ビューへ回す"),
            "{:?}",
            rendered[0]
        );
        assert!(
            rendered[1].contains("メニューとモーダルより後"),
            "{:?}",
            rendered[1]
        );
        assert!(rendered[3].contains("route.rs:L40-49"), "{:?}", rendered[3]);
        assert!(
            rendered[4].contains("workspace.rs:L210"),
            "{:?}",
            rendered[4]
        );
    }
}
