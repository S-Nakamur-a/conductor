//! 一覧の下区画: 選んだ行のブランチの変更ファイル。Explorer の Git Changes と同じ diff を読んで同じ形で描く。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use conductor_core::diff_state::{DiffSource, DiffState};
use conductor_core::git_engine::GitStatusMap;

use crate::list::Viewport;
use crate::panels::explorer::git_changes::GitChanges;

/// 同じ dir を読み直すまでの間。前の結果が届いてから数える。
pub const REFRESH: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
pub struct BranchChanges {
    /// 据えた結果と、それを頼んだ dir。
    shown: Option<(PathBuf, GitChanges, GitStatusMap)>,
    view: Viewport,
    fetching: bool,
    landed: Option<Instant>,
}

impl BranchChanges {
    /// 次に読む dir。投げ中は何も返さないので、遅い repo でも 1 本ずつになる。
    pub fn wanted(&self, target: Option<PathBuf>, now: Instant) -> Option<PathBuf> {
        let target = target?;
        if self.fetching {
            return None;
        }
        let fresh = self.shown_dir() == Some(target.as_path())
            && self.landed.is_some_and(|at| now < at + REFRESH);
        (!fresh).then_some(target)
    }

    pub fn note_fetching(&mut self) {
        self.fetching = true;
    }

    pub fn forget_in_flight(&mut self) {
        self.fetching = false;
    }

    /// 届いた dir が今の対象でなければ捨てる。base が同じ別 worktree の diff は source では見分けられない。
    pub fn install(
        &mut self,
        dir: PathBuf,
        diff: DiffState,
        status: GitStatusMap,
        target: Option<&Path>,
        now: Instant,
    ) {
        self.fetching = false;
        if target != Some(dir.as_path()) {
            return;
        }
        let base = match &diff.source {
            DiffSource::WorkingTree { base } => base.clone(),
            DiffSource::Commit { .. } => return,
        };
        let mut changes = GitChanges::new(dir.clone(), &base);
        changes.set_viewport(self.view);
        // 帯への警告は捨てる。読み直すたびに同じ警告が出続ける。理由は区画の中に出る。
        let _ = changes.install(diff);
        self.shown = Some((dir, changes, status));
        self.landed = Some(now);
    }

    pub fn set_viewport(&mut self, view: Viewport) {
        self.view = view;
        if let Some((_, changes, _)) = &mut self.shown {
            changes.set_viewport(view);
        }
    }

    pub fn for_target(&self, target: Option<&Path>) -> Option<(&GitChanges, &GitStatusMap)> {
        let (dir, changes, status) = self.shown.as_ref()?;
        (target == Some(dir.as_path())).then_some((changes, status))
    }

    fn shown_dir(&self) -> Option<&Path> {
        self.shown.as_ref().map(|(dir, _, _)| dir.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    fn diff(base: &str) -> DiffState {
        DiffState::new(DiffSource::working_tree(base))
    }

    fn landed_on_a(now: Instant) -> BranchChanges {
        let mut changes = BranchChanges::default();
        changes.note_fetching();
        changes.install(
            dir("/a"),
            diff("main"),
            GitStatusMap::default(),
            Some(Path::new("/a")),
            now,
        );
        changes
    }

    #[test]
    fn 選んでいる行が無ければ投げない() {
        assert_eq!(BranchChanges::default().wanted(None, Instant::now()), None);
    }

    #[test]
    fn 投げ中は別の行を選んでも次を投げない() {
        let mut changes = BranchChanges::default();
        changes.note_fetching();
        assert_eq!(changes.wanted(Some(dir("/b")), Instant::now()), None);
    }

    #[test]
    fn 同じ_dir_は届いてから5秒経つまで投げ直さない() {
        let now = Instant::now();
        let changes = landed_on_a(now);
        let early = now + REFRESH - Duration::from_millis(100);
        assert_eq!(changes.wanted(Some(dir("/a")), early), None);
        assert_eq!(
            changes.wanted(Some(dir("/a")), now + REFRESH),
            Some(dir("/a"))
        );
    }

    #[test]
    fn 別の_dir_を選んだら待たずに投げる() {
        let now = Instant::now();
        let changes = landed_on_a(now);
        assert_eq!(changes.wanted(Some(dir("/b")), now), Some(dir("/b")));
    }

    #[test]
    fn 選択が移った後に届いた前の行の結果は据えず_投げ中だけ下ろす() {
        let now = Instant::now();
        let mut changes = landed_on_a(now);
        changes.note_fetching();
        let later = now + Duration::from_secs(1);
        changes.install(
            dir("/a"),
            diff("main"),
            GitStatusMap::default(),
            Some(Path::new("/b")),
            later,
        );
        assert!(changes.for_target(Some(Path::new("/b"))).is_none());
        assert_eq!(changes.wanted(Some(dir("/b")), later), Some(dir("/b")));
        // 捨てた結果で届いた時刻を進めると、a に戻ったときの読み直しが遅れる。
        assert_eq!(
            changes.wanted(Some(dir("/a")), now + REFRESH),
            Some(dir("/a"))
        );
    }

    #[test]
    fn 既定ブランチが_main_でない_repo_の結果も据える() {
        let mut changes = BranchChanges::default();
        changes.install(
            dir("/a"),
            diff("staging"),
            GitStatusMap::default(),
            Some(Path::new("/a")),
            Instant::now(),
        );
        let (shown, _) = changes.for_target(Some(Path::new("/a"))).unwrap();
        assert_eq!(shown.diff().source, DiffSource::working_tree("staging"));
    }

    #[test]
    fn repo_を切り替えて結果が捨てられても次を投げられる() {
        let mut changes = BranchChanges::default();
        changes.note_fetching();
        changes.forget_in_flight();
        assert_eq!(
            changes.wanted(Some(dir("/a")), Instant::now()),
            Some(dir("/a"))
        );
    }
}
