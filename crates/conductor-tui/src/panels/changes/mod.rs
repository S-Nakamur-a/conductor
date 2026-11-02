//! 詳細の画面の Files changed タブ。差分を、ファイルの見出しの下に全部縦に並べる。
//! Files changed は Explorer が投げた作業ツリーの結果を写して持ち、自分では投げない
//! — 大きなリポジトリで 9 秒かかる diff を 2 本走らせないため。
//! Commits タブの中央も読むだけにした同じパネルで、差分は [commits::CommitsTab] が投げる。

pub mod commits;
pub mod render;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use conductor_core::diff_state::{DiffSource, DiffState, FileDiff, FileStatus};
use conductor_core::keymap::Action;
use conductor_core::review_store::ReviewComment;

use crate::comment_list::flip_status;
use crate::effect::Effect;
use crate::layout::{Layout, Region};
use crate::list::{ListCursor, Viewport};
use crate::modal::{CommentEditor, Modal};
use crate::panels::viewer::diff::{DiffPane, Entry, SideRow, side_by_side};
use crate::panels::viewer::render::{MARK, digit_count};
use crate::panels::viewer::syntax::Highlighter;
use crate::panels::viewer::thread::{self, ThreadFolds};
use crate::review::{ReviewState, anchor_for, anchors, innermost};
use crate::task::{ReviewWrite, Task};
use crate::workspace::{Ctx, StatusLevel};

pub const THREAD_INDENT: usize = MARK + 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Code,
    Files,
    Commits,
}

/// 中央の一番上に出ている行。row は [Section::rows] の添字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Top {
    pub file: usize,
    pub row: usize,
}

/// 差分を読み直しても、worktree を離れて戻っても、同じところを一番上に出すための目印。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Anchor {
    path: String,
    /// 新しい側の行番号。見出しと、手前に新しい側の行が無い行 (削除されたファイルの本文など) は None。
    line: Option<usize>,
    /// line が無いときに使う、ファイル内の行の位置。
    row: usize,
}

/// 頼まれたファイルを出せたか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    Shown,
    /// 読み込み中なので、届いたときに出す。
    Waiting,
    Missing,
}

/// 中央に描く 1 行。スレッドの行数は幅で変わるので、位置と送りはこの並びで数える。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Header,
    /// 本文の行。左右表示なら side_by_side の行の添字。
    Body(usize),
    /// 新しい側の行 anchor に開いたスレッドの line 行目。
    Thread {
        anchor: usize,
        line: usize,
    },
    /// ハンクの無いファイル (バイナリ・空) の断り書き。
    Note,
    Blank,
}

#[derive(Debug)]
pub enum Source {
    Unread,
    /// 削除・ハンクの無いファイルと、読めなかったファイル。
    Absent,
    Lines(Vec<String>),
}

#[derive(Debug)]
pub struct Section {
    pub entries: Vec<Entry>,
    pub digits: usize,
    pub rows: Vec<Row>,
    pub source: Source,
    /// 本文を読んだ generation。差分が届くたびに見えているファイルを読み直すのに使う。
    /// 同じ差分でも基点が動けば (rebase) 新しい側は変わる。
    read_at: u64,
    pub highlighted: Vec<Vec<(ratatui::style::Style, String)>>,
    collapsed: bool,
    /// rows と同じ並びの、新しい側の行番号。
    lines: Vec<Option<usize>>,
}

impl Section {
    fn build(file: &FileDiff) -> Self {
        let mut section = Self {
            digits: 0,
            entries: Vec::new(),
            rows: Vec::new(),
            source: Source::Unread,
            read_at: 0,
            highlighted: Vec::new(),
            collapsed: false,
            lines: Vec::new(),
        };
        section.reload(file, Source::Unread);
        if file.status == FileStatus::Deleted || file.hunks.is_empty() {
            section.source = Source::Absent;
        }
        section
    }

    /// entries と本文だけを差し替える。rows は組み直すまで前のまま残し、一番上を探す元にする。
    fn reload(&mut self, file: &FileDiff, source: Source) {
        let total = match &source {
            Source::Lines(lines) => lines.len(),
            _ => 0,
        };
        let mut pane = DiffPane::default();
        pane.build(file, total);
        self.digits = digit_count(pane.max_line_no);
        self.entries = pane.entries;
        self.source = source;
        self.highlighted.clear();
    }

    fn lay_out(
        &mut self,
        collapsed: bool,
        side: bool,
        comments: &[&ReviewComment],
        threads: &ThreadFolds,
        thread_height: impl Fn(usize) -> usize,
    ) {
        self.collapsed = collapsed;
        self.rows = vec![Row::Header];
        self.lines = vec![None];
        if !collapsed && self.entries.is_empty() {
            self.push(Row::Note, None);
        } else if !collapsed {
            let body: Vec<Option<usize>> = match side {
                true => side_by_side(&self.entries)
                    .iter()
                    .map(|row| match row {
                        SideRow::Span(entry) => entry.new_line_no(),
                        SideRow::Split { left, right } => {
                            right.or(*left).and_then(Entry::new_line_no)
                        }
                    })
                    .collect(),
                false => self.entries.iter().map(Entry::new_line_no).collect(),
            };
            let anchors = anchors(comments);
            for (i, line) in body.into_iter().enumerate() {
                self.push(Row::Body(i), line);
                let Some(anchor) = line.filter(|n| anchors.contains(n)) else {
                    continue;
                };
                if threads.is_open(comments, anchor) {
                    for line in 0..thread_height(anchor) {
                        self.push(Row::Thread { anchor, line }, None);
                    }
                }
            }
        }
        self.push(Row::Blank, None);
    }

    pub fn line_of(&self, row: usize) -> Option<usize> {
        self.lines.get(row).copied().flatten()
    }

    fn push(&mut self, row: Row, line: Option<usize>) {
        self.rows.push(row);
        self.lines.push(line);
    }
}

/// 人が手で開閉したもの。worktree ごとに預ける。
#[derive(Debug, Default)]
struct Folds {
    /// viewed による畳みを手で裏返したファイル。
    files: HashSet<String>,
    threads: HashMap<String, ThreadFolds>,
}

#[derive(Debug)]
pub struct ChangesPanel {
    root: PathBuf,
    chosen: HashMap<PathBuf, Tab>,
    diff: Option<DiffState>,
    sections: Vec<Section>,
    top: Top,
    parked: HashMap<PathBuf, Anchor>,
    folds: Folds,
    parked_folds: HashMap<PathBuf, Folds>,
    /// 開いた塊と読んだ本文も worktree ごとに預ける。戻って届いた差分と比べて持ち越す。
    parked_sections: HashMap<PathBuf, (DiffState, Vec<Section>)>,
    side_by_side: bool,
    cursor: ListCursor,
    list_view: Viewport,
    body_height: usize,
    body_width: usize,
    pending_reveal: Option<Anchor>,
    /// 差分が届くたびに上げ、それより前に投げた本文の読みを捨てる。
    generation: u64,
    /// 読んでいる本文の generation。結果が捨てられても、generation が上がれば次を投げられる。
    reading: Option<u64>,
    highlighted_by: String,
    /// Commits タブのコミットの差分。作業ツリーのレビューとは別物なので review を見ない。
    read_only: bool,
}

static NO_REVIEW: std::sync::LazyLock<ReviewState> = std::sync::LazyLock::new(ReviewState::default);

impl ChangesPanel {
    pub fn new(side_by_side: bool) -> Self {
        Self {
            root: PathBuf::new(),
            chosen: HashMap::new(),
            diff: None,
            sections: Vec::new(),
            top: Top::default(),
            parked: HashMap::new(),
            folds: Folds::default(),
            parked_folds: HashMap::new(),
            parked_sections: HashMap::new(),
            side_by_side,
            cursor: ListCursor::default(),
            list_view: Viewport::default(),
            body_height: 0,
            body_width: 0,
            pending_reveal: None,
            generation: 0,
            reading: None,
            highlighted_by: String::new(),
            read_only: false,
        }
    }

    pub fn read_only(side_by_side: bool) -> Self {
        Self {
            read_only: true,
            ..Self::new(side_by_side)
        }
    }

    /// このパネルが描き、畳みを決めるのに使うレビュー。
    pub fn review<'a>(&self, review: &'a ReviewState) -> &'a ReviewState {
        if self.read_only { &NO_REVIEW } else { review }
    }

    /// 別のコミットを出す前に、前のコミットの位置と畳みを捨てる。
    pub fn forget(&mut self) {
        self.diff = None;
        self.sections.clear();
        self.top = Top::default();
        self.folds = Folds::default();
        self.parked.remove(&self.root);
        self.parked_folds.remove(&self.root);
        self.parked_sections.remove(&self.root);
    }

    /// 人が選んだタブが無ければ、main worktree は Code、それ以外は Files changed。
    /// 一覧が届く前 (is_main が分からない) は Code。
    pub fn tab(&self, root: &Path, is_main: Option<bool>) -> Tab {
        match (self.chosen.get(root), is_main) {
            (Some(tab), _) => *tab,
            (None, Some(false)) => Tab::Files,
            (None, _) => Tab::Code,
        }
    }

    pub fn choose(&mut self, root: &Path, tab: Tab) {
        self.chosen.insert(root.to_path_buf(), tab);
    }

    pub fn diff(&self) -> Option<&DiffState> {
        self.diff.as_ref()
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn top(&self) -> Top {
        self.top
    }

    pub fn side_by_side(&self) -> bool {
        self.side_by_side
    }

    pub fn cursor(&self) -> ListCursor {
        self.cursor
    }

    pub fn list_viewport(&self) -> Viewport {
        self.list_view
    }

    /// 中央の幅が変わったら true。スレッドの折り返しが変わるので組み直しが要る。
    pub fn sync_layout(&mut self, layout: &Layout) -> bool {
        if let Some(rect) = layout.rect(Region::ChangesList) {
            self.list_view = Viewport::inside(rect, 0);
        }
        let Some(rect) = layout.rect(Region::Changes) else {
            return false;
        };
        let inner = crate::list::inner(rect);
        self.body_height = inner.height as usize;
        let width = inner.width as usize;
        std::mem::replace(&mut self.body_width, width) != width
    }

    /// Explorer の根に付いて動く。離れる根の位置は預け、戻ったときに差分が届いたら戻す。
    pub fn follow(&mut self, root: &Path) {
        if self.root == root {
            return;
        }
        if let Some(anchor) = self.anchor() {
            self.parked.insert(self.root.clone(), anchor);
        }
        if let Some(diff) = self.diff.take() {
            let sections = std::mem::take(&mut self.sections);
            self.parked_sections
                .insert(self.root.clone(), (diff, sections));
        }
        let folds = std::mem::take(&mut self.folds);
        self.parked_folds.insert(self.root.clone(), folds);
        self.folds = self.parked_folds.remove(root).unwrap_or_default();
        self.root = root.to_path_buf();
        self.diff = None;
        self.sections.clear();
        self.top = Top::default();
        self.cursor = ListCursor::default();
        self.pending_reveal = None;
    }

    /// 届いた diff を写す。別の根のものと、このパネルの出さない出どころ (読むだけのパネルは作業ツリー、
    /// それ以外はコミット) は捨てる。
    /// 見せる約束をしていたファイルがこの差分に無ければ、そのパスを返す。
    pub fn install(&mut self, root: &Path, diff: &DiffState, ctx: &Ctx) -> Option<String> {
        let commit = matches!(diff.source, DiffSource::Commit { .. });
        if root != self.root || commit != self.read_only {
            return None;
        }
        let anchor = match self.diff {
            Some(_) => self.anchor(),
            None => self.parked.remove(root),
        };
        let mut kept = self.unchanged_sections(diff);
        self.sections = (diff.files.iter())
            .map(|file| (kept.remove(&file.path)).unwrap_or_else(|| Section::build(file)))
            .collect();
        self.generation += 1;
        self.diff = Some(diff.clone());
        self.lay_out(ctx);
        self.top = anchor.map_or_else(Top::default, |anchor| self.locate(&anchor));
        self.follow_cursor();
        let reveal = self.pending_reveal.take()?;
        (!self.reveal_now(&reveal, ctx)).then_some(reveal.path)
    }

    /// そのファイルのその行を一番上に出す。今の写しに無くても、差分を読み込み中 (loading)
    /// なら届いたときに出す。
    pub fn reveal(
        &mut self,
        path: String,
        line: Option<usize>,
        loading: bool,
        ctx: &Ctx,
    ) -> Reveal {
        let anchor = Anchor { path, line, row: 0 };
        if self.reveal_now(&anchor, ctx) {
            return Reveal::Shown;
        }
        if self.diff.is_none() || loading {
            self.pending_reveal = Some(anchor);
            return Reveal::Waiting;
        }
        Reveal::Missing
    }

    /// レビュー・幅・開閉が変わったあとに組み直す。差分は同じなので一番上は同じ行のまま
    /// (スレッドの途中や削除行も)。その行が無くなれば (パス, 新しい側の行番号) で探し、
    /// この組み直しで畳む・開くが変わったファイルなら見出しへ。
    pub fn relayout(&mut self, ctx: &Ctx) {
        self.relayout_from(ctx, false);
    }

    /// by_line: 一番上のファイルの本文が別の中身に変わり、開いた塊が閉じた。Body の添字の
    /// 指す行が変わるので、同じ Row の値ではなく (パス, 行番号) で探す。
    fn relayout_from(&mut self, ctx: &Ctx, by_line: bool) {
        let anchor = self.anchor();
        let held = (self.sections.get(self.top.file))
            .map(|section| (section.collapsed, section.rows.get(self.top.row).copied()));
        self.lay_out(ctx);
        if let Some((was_collapsed, row)) = held {
            let section = &self.sections[self.top.file];
            let same = row.and_then(|row| section.rows.iter().position(|r| *r == row));
            match same {
                _ if section.collapsed != was_collapsed => self.top.row = 0,
                Some(row) if !by_line => self.top.row = row,
                _ => {
                    if let Some(anchor) = anchor {
                        self.top = self.locate(&anchor);
                    }
                }
            }
        }
        self.follow_cursor();
    }

    fn lay_out(&mut self, ctx: &Ctx) {
        let Some(diff) = &self.diff else {
            return;
        };
        let (theme, icons, width) = (ctx.theme, ctx.config.ui.icon_set(), self.body_width);
        let review = self.review(ctx.review);
        for (file, section) in diff.files.iter().zip(&mut self.sections) {
            let comments = review.for_file(&file.path);
            let collapsed = review.is_viewed(&file.path) != self.folds.files.contains(&file.path);
            let threads = self.folds.threads.entry(file.path.clone()).or_default();
            section.lay_out(collapsed, self.side_by_side, &comments, threads, |anchor| {
                thread::lines(
                    review,
                    &comments,
                    anchor,
                    theme,
                    icons,
                    width,
                    THREAD_INDENT,
                )
                .len()
            });
        }
    }

    fn flip_file_fold(&mut self, path: String) {
        if !self.folds.files.remove(&path) {
            self.folds.files.insert(path);
        }
    }

    /// 差分が前と同じファイルは、読んだ本文・色・開いた塊を持ち越す。差分はファイルを
    /// 保存するたびに届き直すので、作り直すと色と開いた塊が毎回消える。
    fn unchanged_sections(&mut self, diff: &DiffState) -> HashMap<String, Section> {
        let (old, sections) = match self.diff.take() {
            Some(old) => (old, std::mem::take(&mut self.sections)),
            None => match self.parked_sections.remove(&self.root) {
                Some((old, mut sections)) => {
                    sections.iter_mut().for_each(|s| s.highlighted.clear());
                    (old, sections)
                }
                None => return HashMap::new(),
            },
        };
        let new: HashMap<&str, &FileDiff> =
            (diff.files.iter()).map(|f| (f.path.as_str(), f)).collect();
        (old.files.into_iter().zip(sections))
            .filter(|(file, _)| new.get(file.path.as_str()) == Some(&file))
            .map(|(file, section)| (file.path, section))
            .collect()
    }

    pub fn wanted(&mut self) -> Option<Task> {
        if self.read_only || self.reading == Some(self.generation) {
            return None;
        }
        let diff = self.diff.as_ref()?;
        let file = (self.visible_files()).find(|file| {
            let section = &self.sections[*file];
            match section.source {
                Source::Unread => true,
                Source::Lines(_) => section.read_at != self.generation,
                Source::Absent => false,
            }
        })?;
        self.reading = Some(self.generation);
        Some(Task::LoadChangeSource {
            root: self.root.clone(),
            path: diff.files[file].path.clone(),
            generation: self.generation,
        })
    }

    pub fn accept_source(
        &mut self,
        root: &Path,
        path: &str,
        generation: u64,
        lines: Result<Vec<String>, String>,
        ctx: &Ctx,
    ) {
        if self.reading == Some(generation) {
            self.reading = None;
        }
        if root != self.root || generation != self.generation {
            return;
        }
        let Some(file) = self.position(path) else {
            return;
        };
        let section = &mut self.sections[file];
        section.read_at = generation;
        let lines = lines.ok();
        let changed = match (&section.source, &lines) {
            (Source::Lines(old), Some(new)) if old == new => return,
            (old, _) => matches!(old, Source::Lines(_)),
        };
        let source = lines.map_or(Source::Absent, Source::Lines);
        let diff = (self.diff.as_ref()).expect("position found the file in the diff");
        self.sections[file].reload(&diff.files[file], source);
        self.relayout_from(ctx, changed && file == self.top.file);
    }

    /// Highlighter は作るのが重いので、要るときだけ作る。
    pub fn shows_source(&self) -> bool {
        (self.visible_files()).any(|file| matches!(self.sections[file].source, Source::Lines(_)))
    }

    pub fn highlight(&mut self, highlighter: &Highlighter) {
        if self.highlighted_by != highlighter.id() {
            self.sections.iter_mut().for_each(|s| s.highlighted.clear());
            self.highlighted_by = highlighter.id().to_string();
        }
        let Some(diff) = &self.diff else {
            return;
        };
        for file in self.visible_files() {
            let section = &mut self.sections[file];
            if let Source::Lines(lines) = &section.source
                && section.highlighted.is_empty()
            {
                section.highlighted = highlighter.highlight(Some(&diff.files[file].path), lines);
            }
        }
    }

    fn visible_files(&self) -> std::ops::Range<usize> {
        let mut end = self.top.file;
        let mut shown = 0;
        while end < self.sections.len() && shown < self.body_height.max(1) {
            let skipped = if end == self.top.file {
                self.top.row
            } else {
                0
            };
            shown += self.height(end) - skipped;
            end += 1;
        }
        self.top.file..end
    }

    fn expand(&mut self, all: bool, ctx: &Ctx) -> Vec<Effect> {
        let Some((file, start)) = self.first_visible_block() else {
            return Vec::new();
        };
        let Some(path) = self.diff.as_ref().map(|d| d.files[file].path.clone()) else {
            return Vec::new();
        };
        let Section {
            entries,
            source,
            digits,
            ..
        } = &mut self.sections[file];
        let lines = match source {
            Source::Lines(lines) => lines,
            Source::Unread => return status(format!("{path} is still loading")),
            Source::Absent => return status(format!("{path} could not be read")),
        };
        let mut pane = DiffPane {
            entries: std::mem::take(entries),
            ..DiffPane::default()
        };
        if let Some(idx) = pane.expandable_at(start) {
            pane.expand(idx, all, lines);
        }
        *entries = pane.entries;
        *digits = digit_count(pane.max_line_no);
        self.relayout(ctx);
        Vec::new()
    }

    /// (ファイル, 塊の先頭の行番号)。
    fn first_visible_block(&self) -> Option<(usize, usize)> {
        let mut left = self.body_height.max(1);
        for file in self.visible_files() {
            let section = &self.sections[file];
            let start = if file == self.top.file {
                self.top.row
            } else {
                0
            };
            for row in start..section.rows.len() {
                if left == 0 {
                    return None;
                }
                left -= 1;
                let Some(line) = section.line_of(row) else {
                    continue;
                };
                if (section.entries.iter()).any(|e| {
                    matches!(e, Entry::ExpandableContext { new_line_start, .. } if *new_line_start == line)
                }) {
                    return Some((file, line));
                }
            }
        }
        None
    }

    /// viewed を切り替えたファイルは、手で開閉した分を忘れて viewed のとおりに畳む。
    pub fn follow_viewed(&mut self, path: &str) {
        self.folds.files.remove(path);
    }

    /// パスは `DiffState::resolve_changed_path` で解決する (./ や repo 名の前置き)。
    /// 畳んだファイルは手で開いたのと同じに開く。頼まれた行を見せるため。
    fn reveal_now(&mut self, anchor: &Anchor, ctx: &Ctx) -> bool {
        let Some((file, path)) = (self.diff.as_ref())
            .and_then(|diff| diff.resolve_changed_path(&anchor.path))
            .and_then(|path| Some((self.position(&path)?, path)))
        else {
            return false;
        };
        if self.sections[file].collapsed {
            self.flip_file_fold(path);
            self.lay_out(ctx);
        }
        self.top = Top {
            file,
            row: anchor.line.map_or(0, |line| self.row_of_line(file, line)),
        };
        self.follow_cursor();
        true
    }

    pub fn update_list(&mut self, action: Action) -> Option<()> {
        let len = self.sections.len();
        match action {
            Action::NavigateDown => self.cursor.step(1, len, self.list_view),
            Action::NavigateUp => self.cursor.step(-1, len, self.list_view),
            Action::GoToTop => self.cursor.select(0, len, self.list_view),
            Action::GoToBottom => self.cursor.select(usize::MAX, len, self.list_view),
            Action::Select => {}
            _ => return None,
        }
        self.show_selected();
        Some(())
    }

    pub fn update_body(&mut self, action: Action, ctx: &Ctx) -> Option<Vec<Effect>> {
        if let Some(effects) = self.comment_key(action, ctx) {
            return Some(effects);
        }
        if matches!(action, Action::ExpandContext | Action::ExpandAllContext) {
            if self.read_only {
                return Some(status("hidden lines are not shown for commits".into()));
            }
            return Some(self.expand(action == Action::ExpandAllContext, ctx));
        }
        let half = (self.body_height / 2).max(1) as isize;
        match action {
            Action::NavigateDown => self.scroll(1),
            Action::NavigateUp => self.scroll(-1),
            Action::ScrollHalfPageDown => self.scroll(half),
            Action::ScrollHalfPageUp => self.scroll(-half),
            Action::GoToTop => self.top = Top::default(),
            Action::GoToBottom => self.go_to_bottom(),
            Action::NextChangedFile => self.step_file(1),
            Action::PrevChangedFile => self.step_file(-1),
            Action::ToggleDiffView => self.toggle_side_by_side(ctx),
            _ => return None,
        }
        self.follow_cursor();
        Some(Vec::new())
    }

    /// コメントと viewed のキー。どれも一番上の行に効く (Viewer と同じ)。
    fn comment_key(&mut self, action: Action, ctx: &Ctx) -> Option<Vec<Effect>> {
        if !matches!(
            action,
            Action::AddComment
                | Action::ToggleInlineThread
                | Action::ReplyToComment
                | Action::ToggleResolve
                | Action::NextComment
                | Action::PrevComment
                | Action::ToggleViewed
        ) {
            return None;
        }
        let path = self.diff.as_ref()?.files.get(self.top.file)?.path.clone();
        let row = self.sections[self.top.file].rows[self.top.row];
        if self.read_only && !(action == Action::ToggleInlineThread && row == Row::Header) {
            return Some(status("Commits are read only".into()));
        }
        let comments = ctx.review.for_file(&path);
        let line = self.comment_line();
        let effects = match action {
            Action::ToggleViewed => vec![Effect::ToggleViewed(path)],
            Action::AddComment => self.start_comment(row, path, &ctx.branch),
            Action::NextComment | Action::PrevComment => {
                match self.step_comment(ctx, action == Action::NextComment) {
                    true => Vec::new(),
                    false => no_comment_here(),
                }
            }
            Action::ToggleInlineThread if row == Row::Header => {
                self.flip_file_fold(path);
                self.relayout(ctx);
                Vec::new()
            }
            Action::ToggleInlineThread => {
                if let Some(anchor) = line.and_then(|line| anchor_for(&comments, line)) {
                    self.folds.threads.entry(path).or_default().flip(anchor);
                    self.relayout(ctx);
                }
                Vec::new()
            }
            _ => match line.and_then(|line| innermost(&comments, line)) {
                Some(comment) if action == Action::ReplyToComment => vec![Effect::PushModal(
                    Modal::CommentEditor(CommentEditor::reply_to(comment)),
                )],
                Some(comment) => vec![Effect::Spawn(Task::WriteReview(ReviewWrite::SetStatus {
                    id: comment.id.clone(),
                    status: flip_status(comment.status),
                }))],
                None => no_comment_here(),
            },
        };
        Some(effects)
    }

    /// 一番上の行の新しい側の行番号。スレッドの行は付いている行。
    fn comment_line(&self) -> Option<usize> {
        let section = &self.sections[self.top.file];
        match section.rows[self.top.row] {
            Row::Thread { anchor, .. } => Some(anchor),
            _ => section.lines[self.top.row],
        }
    }

    fn start_comment(&self, row: Row, path: String, branch: &str) -> Vec<Effect> {
        let refuse = |text: &str| vec![Effect::Status(StatusLevel::Warning, text.to_string())];
        let Row::Body(i) = row else {
            return refuse("scroll a line of code to the top to comment on it");
        };
        if self.side_by_side {
            return refuse("comments are added in the unified view (s)");
        }
        let entry = &self.sections[self.top.file].entries[i];
        match (entry.new_line_no(), entry) {
            (Some(line), _) => vec![Effect::PushModal(Modal::CommentEditor(
                CommentEditor::new_comment(branch.to_string(), path, line as u32, None),
            ))],
            (None, Entry::Line { .. }) => refuse("a deleted line has no place to hang a comment"),
            (None, _) => refuse("scroll a line of code to the top to comment on it"),
        }
    }

    /// 次 / 前のスレッドのある行を一番上へ。ファイルを跨ぐ。無ければ false。
    fn step_comment(&mut self, ctx: &Ctx, forward: bool) -> bool {
        let Some(diff) = &self.diff else {
            return false;
        };
        let here = (self.top.file, self.comment_line());
        let order: Vec<usize> = match forward {
            true => (self.top.file..self.sections.len()).collect(),
            false => (0..=self.top.file).rev().collect(),
        };
        for file in order {
            let anchors = anchors(&ctx.review.for_file(&diff.files[file].path));
            let section = &self.sections[file];
            let threaded = |row: &usize| {
                matches!(section.rows[*row], Row::Body(_))
                    && section.lines[*row].is_some_and(|n| anchors.contains(&n))
                    && (file, section.lines[*row]) != here
            };
            let found = match (forward, file == self.top.file) {
                (true, true) => (self.top.row + 1..section.rows.len()).find(threaded),
                (true, false) => (0..section.rows.len()).find(threaded),
                (false, true) => (0..self.top.row).rev().find(threaded),
                (false, false) => (0..section.rows.len()).rev().find(threaded),
            };
            if let Some(row) = found {
                self.top = Top { file, row };
                self.follow_cursor();
                return true;
            }
        }
        false
    }

    /// 一覧の行のクリック。行の外なら false。
    pub fn click_list(&mut self, y: u16) -> bool {
        let len = self.sections.len();
        let Some(row) = self.cursor.index_at(y, len, self.list_view) else {
            return false;
        };
        self.cursor.select(row, len, self.list_view);
        self.show_selected();
        true
    }

    pub fn scroll_list(&mut self, delta: isize) {
        self.cursor.pan(delta, self.sections.len(), self.list_view);
    }

    /// 中央を delta 行送る。両端で止まる。
    pub fn scroll(&mut self, delta: isize) {
        if self.sections.is_empty() {
            return;
        }
        let Top { mut file, mut row } = self.top;
        let mut left = delta.unsigned_abs();
        if delta >= 0 {
            while left > 0 {
                let room = self.height(file) - 1 - row;
                if left <= room {
                    row += left;
                    left = 0;
                } else if file + 1 < self.sections.len() {
                    left -= room + 1;
                    file += 1;
                    row = 0;
                } else {
                    row = self.height(file) - 1;
                    left = 0;
                }
            }
        } else {
            while left > 0 {
                if left <= row {
                    row -= left;
                    left = 0;
                } else if file > 0 {
                    left -= row + 1;
                    file -= 1;
                    row = self.height(file) - 1;
                } else {
                    row = 0;
                    left = 0;
                }
            }
        }
        self.top = Top { file, row };
    }

    pub fn wheel(&mut self, delta: isize) {
        self.scroll(delta);
        self.follow_cursor();
    }

    pub fn height(&self, file: usize) -> usize {
        self.sections[file].rows.len()
    }

    fn go_to_bottom(&mut self) {
        let Some(last) = self.sections.len().checked_sub(1) else {
            return;
        };
        self.top = Top {
            file: last,
            row: self.height(last) - 1,
        };
        self.scroll(-(self.body_height.saturating_sub(1) as isize));
    }

    fn step_file(&mut self, delta: isize) {
        let Top { file, row } = self.top;
        let file = match delta {
            1.. if file + 1 < self.sections.len() => file + 1,
            1.. => file,
            _ if row > 0 => file,
            _ => file.saturating_sub(1),
        };
        self.top = Top { file, row: 0 };
    }

    /// 行の添字は表示ごとに違うので、一番上は今のファイルの見出しに戻す。
    fn toggle_side_by_side(&mut self, ctx: &Ctx) {
        self.side_by_side = !self.side_by_side;
        self.lay_out(ctx);
        self.top.row = 0;
    }

    fn show_selected(&mut self) {
        if self.sections.is_empty() {
            return;
        }
        self.top = Top {
            file: self.cursor.selected(),
            row: 0,
        };
    }

    fn follow_cursor(&mut self) {
        self.cursor
            .select(self.top.file, self.sections.len(), self.list_view);
    }

    fn position(&self, path: &str) -> Option<usize> {
        self.diff
            .as_ref()?
            .files
            .iter()
            .position(|f| f.path == path)
    }

    fn anchor(&self) -> Option<Anchor> {
        let file = self.diff.as_ref()?.files.get(self.top.file)?;
        let lines = &self.sections.get(self.top.file)?.lines;
        let line = match self.top.row {
            0 => None,
            row => lines
                .get(..=row)
                .unwrap_or(lines)
                .iter()
                .rev()
                .find_map(|line| *line),
        };
        Some(Anchor {
            path: file.path.clone(),
            line,
            row: self.top.row,
        })
    }

    /// そのファイルが消えていたら、パス順で次のファイルの見出し。
    fn locate(&self, anchor: &Anchor) -> Top {
        let Some(diff) = self.diff.as_ref() else {
            return Top::default();
        };
        match self.position(&anchor.path) {
            Some(file) => Top {
                file,
                row: match anchor.line {
                    Some(line) => self.row_of_line(file, line),
                    None => anchor.row.min(self.height(file) - 1),
                },
            },
            None => Top {
                file: diff
                    .files
                    .iter()
                    .position(|f| f.path > anchor.path)
                    .unwrap_or(diff.files.len().saturating_sub(1)),
                row: 0,
            },
        }
    }

    /// 新しい側でその行番号以上の最初の行。隠れた塊の中の行なら塊の行。無ければ区切りの
    /// 空行の手前。畳んでいれば見出し。
    fn row_of_line(&self, file: usize, line: usize) -> usize {
        let Section { entries, lines, .. } = &self.sections[file];
        let line = (entries.iter())
            .find_map(|e| match e {
                Entry::ExpandableContext {
                    new_line_start,
                    new_line_end,
                    ..
                } if (*new_line_start..=*new_line_end).contains(&line) => Some(*new_line_start),
                _ => None,
            })
            .unwrap_or(line);
        lines
            .iter()
            .position(|n| n.is_some_and(|n| n >= line))
            .unwrap_or(lines.len().saturating_sub(2))
    }
}

fn status(text: String) -> Vec<Effect> {
    vec![Effect::Status(StatusLevel::Info, text)]
}

fn no_comment_here() -> Vec<Effect> {
    status("no comment on this line".into())
}
