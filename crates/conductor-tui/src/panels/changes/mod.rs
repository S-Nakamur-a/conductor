//! 詳細の画面の Files changed タブ。作業ツリー対 base の差分を、ファイルの見出しの下に
//! 全部縦に並べる。差分は Explorer が投げた作業ツリーの結果を写して持ち、自分では投げない
//! — 大きなリポジトリで 9 秒かかる diff を 2 本走らせないため。

pub mod render;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use conductor_core::diff_state::{DiffSource, DiffState, FileDiff};
use conductor_core::keymap::Action;
use conductor_core::review_store::ReviewComment;

use crate::comment_list::flip_status;
use crate::effect::Effect;
use crate::layout::{Layout, Region};
use crate::list::{ListCursor, Viewport};
use crate::modal::{CommentEditor, Modal};
use crate::panels::viewer::diff::{DiffPane, Entry, SideRow, side_by_side};
use crate::panels::viewer::render::MARK;
use crate::panels::viewer::thread::{self, ThreadFolds};
use crate::review::{anchor_for, anchors, innermost};
use crate::task::{ReviewWrite, Task};
use crate::workspace::{Ctx, StatusLevel};

pub const THREAD_INDENT: usize = MARK + 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Code,
    Files,
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
pub struct Section {
    pub entries: Vec<Entry>,
    pub digits: usize,
    pub rows: Vec<Row>,
    /// rows と同じ並びの、新しい側の行番号。
    lines: Vec<Option<usize>>,
}

impl Section {
    fn build(file: &FileDiff) -> Self {
        let mut pane = DiffPane::default();
        pane.build(file, 0);
        let digits = crate::panels::viewer::render::digit_count(pane.max_line_no);
        Self {
            entries: pane.entries,
            digits,
            rows: Vec::new(),
            lines: Vec::new(),
        }
    }

    fn lay_out(
        &mut self,
        collapsed: bool,
        side: bool,
        comments: &[&ReviewComment],
        threads: &ThreadFolds,
        thread_height: impl Fn(usize) -> usize,
    ) {
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
    side_by_side: bool,
    cursor: ListCursor,
    list_view: Viewport,
    body_height: usize,
    body_width: usize,
    pending_reveal: Option<Anchor>,
}

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
            side_by_side,
            cursor: ListCursor::default(),
            list_view: Viewport::default(),
            body_height: 0,
            body_width: 0,
            pending_reveal: None,
        }
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

    /// 届いた diff を写す。別の根のものとコミットを出どころにしたものは捨てる。
    /// 見せる約束をしていたファイルがこの差分に無ければ、そのパスを返す。
    pub fn install(&mut self, root: &Path, diff: &DiffState, ctx: &Ctx) -> Option<String> {
        if root != self.root || !matches!(diff.source, DiffSource::WorkingTree { .. }) {
            return None;
        }
        let anchor = match self.diff {
            Some(_) => self.anchor(),
            None => self.parked.remove(root),
        };
        self.sections = diff.files.iter().map(Section::build).collect();
        self.diff = Some(diff.clone());
        self.lay_out(ctx);
        self.top = anchor.map_or_else(Top::default, |anchor| self.locate(&anchor));
        self.follow_cursor();
        let reveal = self.pending_reveal.take()?;
        (!self.reveal_now(&reveal)).then_some(reveal.path)
    }

    /// そのファイルのその行を一番上に出す。今の写しに無くても、差分を読み込み中 (loading)
    /// なら届いたときに出す。
    pub fn reveal(&mut self, path: String, line: Option<usize>, loading: bool) -> Reveal {
        let anchor = Anchor { path, line, row: 0 };
        if self.reveal_now(&anchor) {
            return Reveal::Shown;
        }
        if self.diff.is_none() || loading {
            self.pending_reveal = Some(anchor);
            return Reveal::Waiting;
        }
        Reveal::Missing
    }

    /// レビュー・幅・開閉が変わったあとに組み直す。一番上は (パス, 新しい側の行番号) で保つ。
    pub fn relayout(&mut self, ctx: &Ctx) {
        let anchor = self.anchor();
        self.lay_out(ctx);
        if let Some(anchor) = anchor {
            self.top = self.locate(&anchor);
        }
        self.follow_cursor();
    }

    fn lay_out(&mut self, ctx: &Ctx) {
        let Some(diff) = &self.diff else {
            return;
        };
        let (theme, icons, width) = (ctx.theme, ctx.config.ui.icon_set(), self.body_width);
        for (file, section) in diff.files.iter().zip(&mut self.sections) {
            let comments = ctx.review.for_file(&file.path);
            let collapsed =
                ctx.review.is_viewed(&file.path) != self.folds.files.contains(&file.path);
            let threads = self.folds.threads.entry(file.path.clone()).or_default();
            section.lay_out(collapsed, self.side_by_side, &comments, threads, |anchor| {
                thread::lines(
                    ctx.review,
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

    /// viewed を切り替えたファイルは、手で開閉した分を忘れて viewed のとおりに畳む。
    pub fn follow_viewed(&mut self, path: &str) {
        self.folds.files.remove(path);
    }

    /// パスは Git Changes の一覧と同じ規則で解決する (./ や repo 名の前置き)。
    fn reveal_now(&mut self, anchor: &Anchor) -> bool {
        let Some(file) = (self.diff.as_ref())
            .and_then(|diff| diff.resolve_changed_path(&anchor.path))
            .and_then(|path| self.position(&path))
        else {
            return false;
        };
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
        let comments = ctx.review.for_file(&path);
        let row = self.sections[self.top.file].rows[self.top.row];
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
                if !self.folds.files.remove(&path) {
                    self.folds.files.insert(path);
                }
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

    /// 新しい側でその行番号以上の最初の行。無ければ区切りの空行の手前。畳んでいれば見出し。
    fn row_of_line(&self, file: usize, line: usize) -> usize {
        let lines = &self.sections[file].lines;
        lines
            .iter()
            .position(|n| n.is_some_and(|n| n >= line))
            .unwrap_or(lines.len().saturating_sub(2))
    }
}

fn no_comment_here() -> Vec<Effect> {
    vec![Effect::Status(
        StatusLevel::Info,
        "no comment on this line".into(),
    )]
}
