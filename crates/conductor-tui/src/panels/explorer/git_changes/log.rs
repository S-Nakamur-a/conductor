//! コミット一覧。HEAD から 1 ページずつ遡る。

use std::path::Path;

use conductor_core::diff_state::DiffSource;
use conductor_core::git_engine::CommitInfo;
use conductor_core::keymap::Action;

use crate::effect::Effect;
use crate::list::{ListCursor, Viewport};
use crate::task::Task;

/// 巨大リポジトリで全履歴を読まないための 1 ページの件数。
pub const PAGE: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    WorkingTree,
    Commit(usize),
    LoadMore,
}

#[derive(Debug, Default)]
pub struct CommitLog {
    commits: Vec<CommitInfo>,
    cursor: ListCursor,
    view: Viewport,
    loading: bool,
    /// 最後のページが PAGE 件に満たなければ、その先は無い。
    exhausted: bool,
}

impl CommitLog {
    pub fn commits(&self) -> &[CommitInfo] {
        &self.commits
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

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn len(&self) -> usize {
        1 + self.commits.len() + usize::from(!self.exhausted)
    }

    pub fn row(&self, index: usize) -> Option<Row> {
        match index {
            0 => Some(Row::WorkingTree),
            i if i <= self.commits.len() => Some(Row::Commit(i - 1)),
            i if i + 1 == self.len() => Some(Row::LoadMore),
            _ => None,
        }
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.row(self.cursor.selected())
    }

    pub fn selected_commit(&self) -> Option<&CommitInfo> {
        match self.selected_row()? {
            Row::Commit(i) => self.commits.get(i),
            _ => None,
        }
    }

    pub fn restart(&mut self, worktree: &Path) -> Effect {
        self.commits.clear();
        self.exhausted = false;
        self.cursor = ListCursor::default();
        self.request(worktree)
    }

    pub fn load_more(&mut self, worktree: &Path) -> Effect {
        self.request(worktree)
    }

    fn request(&mut self, worktree: &Path) -> Effect {
        self.loading = true;
        Effect::Spawn(Task::HeadLog {
            worktree: worktree.to_path_buf(),
            skip: self.commits.len(),
            limit: PAGE,
        })
    }

    /// 読み直した後に届いた古いページは捨てる。
    pub fn install(&mut self, skip: usize, commits: Vec<CommitInfo>) {
        if skip != self.commits.len() {
            return;
        }
        self.loading = false;
        self.exhausted = commits.len() < PAGE;
        self.commits.extend(commits);
        self.clamp();
    }

    pub fn clamp(&mut self) {
        self.cursor.clamp(self.len(), self.view);
    }

    pub fn select_source(&mut self, source: &DiffSource) {
        let at = match source {
            DiffSource::WorkingTree { .. } => Some(0),
            DiffSource::Commit { oid } => self
                .commits
                .iter()
                .position(|c| c.oid == *oid)
                .map(|i| i + 1),
        };
        if let Some(at) = at {
            self.cursor.select(at, self.len(), self.view);
        }
    }

    /// 移動のキーなら消費して true。
    pub fn navigate(&mut self, action: Action) -> bool {
        let len = self.len();
        match action {
            Action::NavigateDown => self.cursor.step(1, len, self.view),
            Action::NavigateUp => self.cursor.step(-1, len, self.view),
            Action::GoToTop => self.cursor.select(0, len, self.view),
            Action::GoToBottom => self.cursor.select(usize::MAX, len, self.view),
            _ => return false,
        }
        true
    }

    pub fn scroll(&mut self, delta: isize) {
        self.cursor.pan(delta, self.len(), self.view);
    }

    /// 区画の外なら None。
    pub fn select_at(&mut self, y: u16) -> Option<Row> {
        let row = self.cursor.index_at(y, self.len(), self.view)?;
        self.cursor.select(row, self.len(), self.view);
        self.row(row)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn commit(oid: &str) -> CommitInfo {
        CommitInfo {
            short_oid: oid[..8.min(oid.len())].to_string(),
            oid: oid.to_string(),
            message: format!("message of {oid}"),
            author: "someone".into(),
            time_ago: "2h ago".into(),
        }
    }

    fn page(n: usize, from: usize) -> Vec<CommitInfo> {
        (from..from + n)
            .map(|i| commit(&format!("{i:040}")))
            .collect()
    }

    #[test]
    fn 行は作業ツリーとコミットと読み足しの順() {
        let mut log = CommitLog::default();
        log.restart(Path::new("/tmp"));
        assert_eq!(log.len(), 2, "読み込み中でも作業ツリーと読み足しの行はある");

        log.install(0, page(PAGE, 0));
        assert_eq!(log.len(), PAGE + 2);
        assert_eq!(log.row(0), Some(Row::WorkingTree));
        assert_eq!(log.row(1), Some(Row::Commit(0)));
        assert_eq!(log.row(PAGE + 1), Some(Row::LoadMore));
        assert_eq!(log.row(PAGE + 2), None);

        log.install(PAGE, page(3, PAGE));
        assert_eq!(
            log.len(),
            PAGE + 3 + 1,
            "満たないページで読み足しの行が消える"
        );
        assert_eq!(log.row(PAGE + 3), Some(Row::Commit(PAGE + 2)));
    }

    #[test]
    fn 読み直した後に届いた古いページは捨てる() {
        let mut log = CommitLog::default();
        log.restart(Path::new("/tmp"));
        log.install(0, page(PAGE, 0));
        log.load_more(Path::new("/tmp"));
        log.restart(Path::new("/tmp"));
        log.install(PAGE, page(3, PAGE));
        assert!(log.commits().is_empty());
        assert!(log.is_loading());
    }

    #[test]
    fn 次のページは今の件数の続きを頼む() {
        let mut log = CommitLog::default();
        log.restart(Path::new("/tmp"));
        log.install(0, page(PAGE, 0));
        let effect = log.load_more(Path::new("/tmp"));
        let Effect::Spawn(Task::HeadLog { skip, limit, .. }) = effect else {
            panic!("{effect:?}");
        };
        assert_eq!((skip, limit), (PAGE, PAGE));
    }

    #[test]
    fn 出どころの行へ寄せる() {
        let mut log = CommitLog::default();
        log.set_viewport(Viewport::new(0, 20));
        log.restart(Path::new("/tmp"));
        log.install(0, page(3, 0));
        log.select_source(&DiffSource::commit(&format!("{:040}", 2)));
        assert_eq!(log.selected_row(), Some(Row::Commit(2)));
        log.select_source(&DiffSource::commit("unknown"));
        assert_eq!(log.selected_row(), Some(Row::Commit(2)), "無ければ動かない");
        log.select_source(&DiffSource::working_tree("main"));
        assert_eq!(log.selected_row(), Some(Row::WorkingTree));
    }
}
