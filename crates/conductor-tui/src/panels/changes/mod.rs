//! 詳細の画面の Files changed タブ。作業ツリー対 base の差分を、ファイルの見出しの下に
//! 全部縦に並べる。差分は Explorer が投げた作業ツリーの結果を写して持ち、自分では投げない
//! — 大きなリポジトリで 9 秒かかる diff を 2 本走らせないため。

pub mod render;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use conductor_core::diff_state::{DiffSource, DiffState, FileDiff};
use conductor_core::keymap::Action;

use crate::layout::{Layout, Region};
use crate::list::{ListCursor, Viewport};
use crate::panels::viewer::diff::{DiffPane, Entry, SideRow, side_by_side};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Code,
    Files,
}

/// 中央の一番上に出ている行。row 0 はファイルの見出し、最後の 1 行は区切りの空行。
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

#[derive(Debug)]
pub struct Section {
    pub entries: Vec<Entry>,
    side_rows: usize,
    pub digits: usize,
}

impl Section {
    fn build(file: &FileDiff) -> Self {
        let mut pane = DiffPane::default();
        pane.build(file, 0);
        let digits = crate::panels::viewer::render::digit_count(pane.max_line_no);
        let side_rows = side_by_side(&pane.entries).len();
        Self {
            entries: pane.entries,
            side_rows,
            digits,
        }
    }
}

#[derive(Debug)]
pub struct ChangesPanel {
    root: PathBuf,
    chosen: HashMap<PathBuf, Tab>,
    diff: Option<DiffState>,
    sections: Vec<Section>,
    top: Top,
    parked: HashMap<PathBuf, Anchor>,
    side_by_side: bool,
    cursor: ListCursor,
    list_view: Viewport,
    body_height: usize,
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
            side_by_side,
            cursor: ListCursor::default(),
            list_view: Viewport::default(),
            body_height: 0,
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

    pub fn sync_layout(&mut self, layout: &Layout) {
        if let Some(rect) = layout.rect(Region::ChangesList) {
            self.list_view = Viewport::inside(rect, 0);
        }
        if let Some(rect) = layout.rect(Region::Changes) {
            self.body_height = crate::list::inner(rect).height as usize;
        }
    }

    /// Explorer の根に付いて動く。離れる根の位置は預け、戻ったときに差分が届いたら戻す。
    pub fn follow(&mut self, root: &Path) {
        if self.root == root {
            return;
        }
        if let Some(anchor) = self.anchor() {
            self.parked.insert(self.root.clone(), anchor);
        }
        self.root = root.to_path_buf();
        self.diff = None;
        self.sections.clear();
        self.top = Top::default();
        self.cursor = ListCursor::default();
        self.pending_reveal = None;
    }

    /// 届いた diff を写す。別の根のものとコミットを出どころにしたものは捨てる。
    /// 見せる約束をしていたファイルがこの差分に無ければ、そのパスを返す。
    pub fn install(&mut self, root: &Path, diff: &DiffState) -> Option<String> {
        if root != self.root || !matches!(diff.source, DiffSource::WorkingTree { .. }) {
            return None;
        }
        let anchor = match self.diff {
            Some(_) => self.anchor(),
            None => self.parked.remove(root),
        };
        self.sections = diff.files.iter().map(Section::build).collect();
        self.diff = Some(diff.clone());
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

    pub fn update_body(&mut self, action: Action) -> Option<()> {
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
            Action::ToggleDiffView => self.toggle_side_by_side(),
            _ => return None,
        }
        self.follow_cursor();
        Some(())
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

    /// 見出し + 本文 + 区切りの空行。
    pub fn height(&self, file: usize) -> usize {
        2 + self.body_rows(file)
    }

    /// 本文の行数。ハンクの無いファイル (バイナリ・空) は断り書きの 1 行。
    pub fn body_rows(&self, file: usize) -> usize {
        let section = &self.sections[file];
        match (section.entries.is_empty(), self.side_by_side) {
            (true, _) => 1,
            (false, true) => section.side_rows,
            (false, false) => section.entries.len(),
        }
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
    fn toggle_side_by_side(&mut self) {
        self.side_by_side = !self.side_by_side;
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
        let lines = self.row_lines(self.top.file);
        let line = self.top.row.checked_sub(1).and_then(|body| {
            let last = lines.len().checked_sub(1)?;
            lines[..=body.min(last)].iter().rev().find_map(|line| *line)
        });
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

    /// 新しい側でその行番号以上の最初の行。無ければ本文の最後の行。
    fn row_of_line(&self, file: usize, line: usize) -> usize {
        let lines = self.row_lines(file);
        let body = lines
            .iter()
            .position(|n| n.is_some_and(|n| n >= line))
            .unwrap_or(lines.len().saturating_sub(1));
        1 + body
    }

    fn row_lines(&self, file: usize) -> Vec<Option<usize>> {
        let entries = &self.sections[file].entries;
        if !self.side_by_side {
            return entries.iter().map(Entry::new_line_no).collect();
        }
        side_by_side(entries)
            .iter()
            .map(|row| match row {
                SideRow::Span(entry) => entry.new_line_no(),
                SideRow::Split { left, right } => right.or(*left).and_then(Entry::new_line_no),
            })
            .collect()
    }
}
