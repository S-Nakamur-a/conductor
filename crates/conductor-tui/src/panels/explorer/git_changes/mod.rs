//! Git Changes: ある出どころ ([DiffSource]) の変更ファイル一覧。Explorer の下区画の 1 つ。

pub mod render;

use std::path::{Path, PathBuf};

use conductor_core::diff_state::{DiffListEntry, DiffSource, DiffState};
use conductor_core::keymap::Action;

use crate::click::ClickTracker;
use crate::effect::Effect;
use crate::list::{ListCursor, Viewport};
use crate::task::Task;
use crate::workspace::StatusLevel;

#[derive(Debug)]
pub struct GitChanges {
    /// 作業ツリー差分のベース。出どころを作業ツリーへ戻すときに要る。
    base: String,
    /// いま見ている出どころ。
    source: DiffSource,
    diff: DiffState,
    cursor: ListCursor,
    view: Viewport,
    clicks: ClickTracker,
    loading: bool,
}

impl Default for GitChanges {
    fn default() -> Self {
        Self::new("main")
    }
}

impl GitChanges {
    pub fn new(base: &str) -> Self {
        let source = DiffSource::working_tree(base);
        Self {
            base: base.to_string(),
            source: source.clone(),
            diff: DiffState::new(source),
            cursor: ListCursor::default(),
            view: Viewport::default(),
            clicks: ClickTracker::default(),
            loading: false,
        }
    }

    pub fn diff(&self) -> &DiffState {
        &self.diff
    }

    pub fn source(&self) -> &DiffSource {
        &self.source
    }

    pub fn is_loading(&self) -> bool {
        self.loading
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

    /// 一覧の先頭でバナーが使う行数。
    pub fn banner_rows(&self) -> usize {
        usize::from(self.diff.error.is_some())
    }

    /// worktree が変わった。前の worktree のコミットは新しい方に無いので出どころも戻す。
    pub fn reset(&mut self, base: &str) {
        *self = Self::new(base);
    }

    pub fn set_source(&mut self, source: DiffSource, worktree: &Path) -> Effect {
        self.source = source;
        self.reload(worktree)
    }

    pub fn reload(&mut self, worktree: &Path) -> Effect {
        self.loading = true;
        Effect::Spawn(Task::ComputeDiff {
            worktree: worktree.to_path_buf(),
            source: self.source.clone(),
        })
    }

    /// 届いた diff を据える。頼んだ出どころと違えば (その後に替えた) 捨てる。
    pub fn install(&mut self, diff: DiffState) -> Vec<Effect> {
        if diff.source != self.source {
            return Vec::new();
        }
        self.loading = false;
        let selected = self
            .diff
            .resolve_file(self.cursor.selected())
            .map(|f| f.path.clone());
        self.diff = diff;
        // 添字ではなくパスで選び直す。ファイルが増減しても指す先が動かない。
        if let Some(at) = selected.and_then(|p| self.diff.display_index_for_path(&p)) {
            self.cursor.place(at, self.diff.display_list.len());
        }
        self.clamp();
        match self.diff.error.clone() {
            Some(e) => vec![Effect::Status(StatusLevel::Warning, e)],
            None => Vec::new(),
        }
    }

    /// 中身が入れ替わっていることがあるので、窓の高さを知っているここで収め直す。
    pub fn clamp(&mut self) {
        self.cursor.clamp(self.diff.display_list.len(), self.view);
    }

    pub fn update(&mut self, action: Action) -> Option<Vec<Effect>> {
        let len = self.diff.display_list.len();
        let row = self.cursor.selected();
        match action {
            Action::ToggleViewed => {
                let path = self.diff.resolve_file(row)?.path.clone();
                return Some(vec![Effect::ToggleViewed(path)]);
            }
            Action::NavigateDown => self.cursor.step(1, len, self.view),
            Action::NavigateUp => self.cursor.step(-1, len, self.view),
            Action::GoToTop => self.cursor.select(0, len, self.view),
            Action::GoToBottom => self.cursor.select(usize::MAX, len, self.view),
            Action::CollapseOrLeft => self.diff.collapse_section(row),
            Action::ExpandOrRight => self.diff.expand_section(row),
            Action::Select => return Some(self.activate(row, false)),
            _ => return None,
        }
        Some(Vec::new())
    }

    /// 行のクリック。ファイルは preview で開いて 2 回目で固定する。区画の外なら None。
    pub fn click(&mut self, y: u16) -> Option<Vec<Effect>> {
        let len = self.diff.display_list.len();
        let row = self.cursor.index_at(y, len, self.view)?;
        self.cursor.select(row, len, self.view);
        let preview = !self.clicks.is_double(row);
        Some(self.activate(row, preview))
    }

    /// ホイール。選択は動かさず窓だけ送る。
    pub fn scroll(&mut self, delta: isize) {
        self.cursor
            .pan(delta, self.diff.display_list.len(), self.view);
    }

    /// 1 行を発火させる。ディレクトリは開閉し、ファイルは diff を開く。
    fn activate(&mut self, row: usize, preview: bool) -> Vec<Effect> {
        match self.diff.display_list.get(row) {
            Some(DiffListEntry::Directory { .. }) => {
                self.diff.toggle_section(row);
                Vec::new()
            }
            Some(DiffListEntry::File { .. }) => {
                self.open_row(row, None, preview).into_iter().collect()
            }
            _ => Vec::new(),
        }
    }

    fn open_row(&self, row: usize, line: Option<usize>, preview: bool) -> Option<Effect> {
        let diff = self.diff.open_diff(row)?;
        Some(Effect::OpenFile {
            path: PathBuf::from(&diff.file.path),
            line,
            diff: Some(Box::new(diff)),
            preview,
        })
    }

    /// 一覧を 1 つ送り、その diff を開く。ディレクトリ行は飛ばす。
    pub fn step_file(&mut self, delta: isize) -> Option<Effect> {
        let files: Vec<usize> = self
            .diff
            .display_list
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, DiffListEntry::File { .. }))
            .map(|(i, _)| i)
            .collect();
        let row = self.cursor.selected();
        let at = files.partition_point(|&i| i < row);
        let next = if delta > 0 {
            *files.get(at + usize::from(files.get(at) == Some(&row)))?
        } else {
            *files.get(at.checked_sub(1)?)?
        };
        self.cursor
            .select(next, self.diff.display_list.len(), self.view);
        self.open_row(next, None, false)
    }

    /// 変更ファイルとして開く。折りたたまれたディレクトリの中にあれば先に展開する
    /// — 展開するまで表示行が無く、一覧の選択が動かせない。
    pub fn open_path(&mut self, path: &str, line: Option<usize>) -> Option<Effect> {
        let path = self.diff.resolve_changed_path(path)?;
        let row = self.diff.reveal_path(&path)?;
        self.cursor
            .select(row, self.diff.display_list.len(), self.view);
        self.open_row(row, line, false)
    }
}
