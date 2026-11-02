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
}
