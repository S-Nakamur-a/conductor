//! ある出どころ ([DiffSource]) の変更ファイル一覧。Explorer はこれで作業ツリーの diff を
//! 持ち、Sessions の Git Changes はこれを描く。

pub mod render;

use std::path::PathBuf;

use conductor_core::diff_state::{DiffSource, DiffState};

use crate::effect::Effect;
use crate::list::{ListCursor, Viewport};
use crate::task::Task;
use crate::workspace::StatusLevel;

#[derive(Debug)]
pub struct GitChanges {
    worktree: PathBuf,
    /// いま見ている出どころ。
    source: DiffSource,
    diff: DiffState,
    cursor: ListCursor,
    view: Viewport,
    loading: bool,
}

impl Default for GitChanges {
    fn default() -> Self {
        Self::new(PathBuf::new(), "main")
    }
}

impl GitChanges {
    pub fn new(worktree: PathBuf, base: &str) -> Self {
        let source = DiffSource::working_tree(base);
        Self {
            worktree,
            source: source.clone(),
            diff: DiffState::new(source),
            cursor: ListCursor::default(),
            view: Viewport::default(),
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

    /// view は区画全体。バナーの分はここで食う。
    pub fn set_viewport(&mut self, view: Viewport) {
        let banner = self.banner_rows();
        self.view = Viewport::new(view.top + banner as u16, view.height.saturating_sub(banner));
    }

    /// 一覧の先頭でバナーが使う行数。
    pub fn banner_rows(&self) -> usize {
        usize::from(self.diff.error.is_some())
    }

    pub fn reset(&mut self, worktree: PathBuf, base: &str) {
        *self = Self::new(worktree, base);
    }

    pub fn reload(&mut self) -> Effect {
        self.loading = true;
        Effect::Spawn(Task::ComputeDiff {
            worktree: self.worktree.clone(),
            source: self.source.clone(),
        })
    }

    /// 届いた diff を据える。頼んだ出どころと違えば捨てる。同じ worktree に Commits タブが
    /// 頼んだコミットの diff も届くので、これが無いと作業ツリーの diff が上書きされる。
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
        self.diff
            .error
            .clone()
            .map(|e| Effect::Status(StatusLevel::Warning, e))
            .into_iter()
            .collect()
    }

    fn clamp(&mut self) {
        self.cursor.clamp(self.diff.display_list.len(), self.view);
    }

    /// 選んでいる行のファイル。ディレクトリ行なら None。
    pub fn selected_path(&self) -> Option<&str> {
        self.diff
            .resolve_file(self.cursor.selected())
            .map(|f| f.path.as_str())
    }

    /// そのパスのファイルを選ぶ。一覧に無ければ何もしない。
    pub fn select_path(&mut self, path: &str) -> bool {
        let Some(at) = self.diff.display_index_for_path(path) else {
            return false;
        };
        self.cursor
            .select(at, self.diff.display_list.len(), self.view);
        true
    }

    pub fn select_first_file(&mut self) {
        let first = self.file_rows().next();
        match first {
            Some(at) => self
                .cursor
                .select(at, self.diff.display_list.len(), self.view),
            None => self.cursor = ListCursor::default(),
        }
    }

    /// ディレクトリ行を飛ばして前後のファイルへ。端ではそのまま。
    pub fn step_file(&mut self, delta: isize) {
        let at = self.cursor.selected();
        let next = match delta.signum() {
            1 => self.file_rows().find(|&i| i > at),
            -1 => self.file_rows().rfind(|&i| i < at),
            _ => None,
        };
        match next {
            Some(i) => self
                .cursor
                .select(i, self.diff.display_list.len(), self.view),
            None if self.selected_path().is_none() => self.select_first_file(),
            None => self.cursor.reveal(self.diff.display_list.len(), self.view),
        }
    }

    /// 画面 y の行がファイルならそれを選ぶ。
    pub fn select_at(&mut self, y: u16) -> bool {
        let len = self.diff.display_list.len();
        let Some(at) = self.cursor.index_at(y, len, self.view) else {
            return false;
        };
        if self.diff.resolve_file(at).is_none() {
            return false;
        }
        self.cursor.select(at, len, self.view);
        true
    }

    /// 選択は動かさず窓だけ動かす。
    pub fn pan(&mut self, delta: isize) {
        self.cursor
            .pan(delta, self.diff.display_list.len(), self.view);
    }

    fn file_rows(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
        (0..self.diff.display_list.len()).filter(|&i| self.diff.resolve_file(i).is_some())
    }
}
