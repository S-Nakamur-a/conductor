//! 一覧の下区画: 選んだ行のブランチの変更ファイルと、選んだファイルの diff。Explorer が持つ
//! 作業ツリーの diff と同じ読み方で、ツリーは同じ形で描く。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use conductor_core::diff_state::{DiffSource, DiffState};
use conductor_core::git_engine::GitStatusMap;
use ratatui::layout::Rect;

use crate::list::Viewport;
use crate::panels::explorer::git_changes::GitChanges;
use crate::panels::viewer::diff::{DiffPane, Entry};
use crate::panels::viewer::render::digit_count;

/// 同じ dir を読み直すまでの間。前の結果が届いてから数える。
pub const REFRESH: Duration = Duration::from_secs(5);

const TREE_WIDTH: u16 = 30;
/// 右の diff にこれだけ取れないなら割らず、ツリーと diff を入れ替えて出す。
const BODY_MIN_WIDTH: u16 = 60;

/// 区画の中身の置き場。描画とマウスの当たり判定が同じものを読む。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Split {
    pub tree: Option<Rect>,
    pub divider: Option<Rect>,
    pub body: Option<Rect>,
}

/// 枠の内側をツリーと diff に分ける。狭ければ、diff を開いているかでどちらか一方。
pub fn split(inner: Rect, body_open: bool) -> Split {
    if inner.width >= TREE_WIDTH + 1 + BODY_MIN_WIDTH {
        let tree = Rect {
            width: TREE_WIDTH,
            ..inner
        };
        let divider = Rect {
            x: inner.x + TREE_WIDTH,
            width: 1,
            ..inner
        };
        let body = Rect {
            x: inner.x + TREE_WIDTH + 1,
            width: inner.width - TREE_WIDTH - 1,
            ..inner
        };
        return Split {
            tree: Some(tree),
            divider: Some(divider),
            body: Some(body),
        };
    }
    match body_open {
        true => Split {
            tree: None,
            divider: None,
            body: Some(inner),
        },
        false => Split {
            tree: Some(inner),
            divider: None,
            body: None,
        },
    }
}

/// worktree を移っても戻れるように預けておくもの。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Pick {
    path: Option<String>,
    scroll: usize,
}

/// 選んだファイルの diff を、描く形にしたもの。
#[derive(Debug, Default)]
pub struct Body {
    pub entries: Vec<Entry>,
    pub digits: usize,
}

#[derive(Debug, Default)]
pub struct BranchChanges {
    /// 据えた結果と、それを頼んだ dir。
    shown: Option<(PathBuf, GitChanges, GitStatusMap)>,
    view: Viewport,
    fetching: bool,
    landed: Option<Instant>,
    pick: Pick,
    picks: HashMap<PathBuf, Pick>,
    body: Body,
    /// 右の diff に使える行数。見出しの 1 行は除く。
    body_height: usize,
    /// 狭くて割れないときに、diff の側を出しているか。
    body_open: bool,
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
        // 同じ dir なら作り直さない。作り直すと選んだファイルが先頭へ戻る。
        let same = self
            .shown
            .as_ref()
            .is_some_and(|(shown, changes, _)| shown == &dir && changes.source() == &diff.source);
        if same {
            if let Some((_, _, held)) = &mut self.shown {
                *held = status;
            }
        } else {
            self.park();
            self.pick = self.picks.get(&dir).cloned().unwrap_or_default();
            let mut changes = GitChanges::new(dir.clone(), &base);
            changes.set_viewport(self.view);
            self.shown = Some((dir, changes, status));
        }
        let Some((_, changes, _)) = &mut self.shown else {
            return;
        };
        // 帯への警告は捨てる。読み直すたびに同じ警告が出続ける。理由は区画の中に出る。
        let _ = changes.install(diff);
        let kept = match self.pick.path.as_deref() {
            Some(path) => changes.select_path(path),
            None => false,
        };
        if !kept {
            changes.select_first_file();
            self.pick.scroll = 0;
        }
        self.landed = Some(now);
        self.rebuild_body();
    }

    pub fn set_viewport(&mut self, view: Viewport) {
        self.view = view;
        self.body_height = view.height.saturating_sub(1);
        if let Some((_, changes, _)) = &mut self.shown {
            changes.set_viewport(view);
        }
        self.clamp_scroll();
    }

    pub fn for_target(&self, target: Option<&Path>) -> Option<(&GitChanges, &GitStatusMap)> {
        let (dir, changes, status) = self.shown.as_ref()?;
        (target == Some(dir.as_path())).then_some((changes, status))
    }

    pub fn body(&self) -> &Body {
        &self.body
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.pick.path.as_deref()
    }

    pub fn body_scroll(&self) -> usize {
        self.pick.scroll
    }

    pub fn body_open(&self) -> bool {
        self.body_open
    }

    pub fn open_body(&mut self) {
        self.body_open = self.pick.path.is_some();
    }

    pub fn close_body(&mut self) {
        self.body_open = false;
    }

    pub fn step_file(&mut self, delta: isize) {
        if let Some((_, changes, _)) = &mut self.shown {
            changes.step_file(delta);
            self.chose();
        }
    }

    /// 画面 y のツリーの行がファイルなら選ぶ。
    pub fn select_at(&mut self, y: u16) -> bool {
        let chosen = self
            .shown
            .as_mut()
            .is_some_and(|(_, changes, _)| changes.select_at(y));
        if chosen {
            self.chose();
        }
        chosen
    }

    pub fn pan_tree(&mut self, delta: isize) {
        if let Some((_, changes, _)) = &mut self.shown {
            changes.pan(delta);
        }
    }

    pub fn scroll_body(&mut self, delta: isize) {
        self.pick.scroll = self.pick.scroll.saturating_add_signed(delta);
        self.clamp_scroll();
    }

    pub fn body_height(&self) -> usize {
        self.body_height
    }

    fn chose(&mut self) {
        let path = self
            .shown
            .as_ref()
            .and_then(|(_, changes, _)| changes.selected_path())
            .map(str::to_string);
        if path != self.pick.path {
            self.pick = Pick { path, scroll: 0 };
            self.rebuild_body();
        }
    }

    fn rebuild_body(&mut self) {
        let file = self.shown.as_ref().and_then(|(_, changes, _)| {
            let path = changes.selected_path()?;
            changes.diff().files.iter().find(|f| f.path == path)
        });
        self.pick.path = file.map(|f| f.path.clone());
        let mut pane = DiffPane::default();
        if let Some(file) = file {
            pane.build(file, 0);
        }
        self.body = Body {
            digits: digit_count(pane.max_line_no),
            entries: pane.entries,
        };
        if self.pick.path.is_none() {
            self.body_open = false;
        }
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        let max = self
            .body
            .entries
            .len()
            .saturating_sub(self.body_height.max(1));
        self.pick.scroll = self.pick.scroll.min(max);
    }

    /// 今の dir の選択を預ける。
    fn park(&mut self) {
        if let Some((dir, _, _)) = &self.shown {
            self.picks
                .insert(dir.clone(), std::mem::take(&mut self.pick));
        }
        self.body_open = false;
    }

    fn shown_dir(&self) -> Option<&Path> {
        self.shown.as_ref().map(|(dir, _, _)| dir.as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::diff_state::{DiffHunk, DiffLine, DiffLineTag, FileDiff, FileStatus};

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

    fn changed(path: &str, lines: usize) -> FileDiff {
        let lines = (1..=lines)
            .map(|n| DiffLine {
                tag: DiffLineTag::Insert,
                old_line_no: None,
                new_line_no: Some(n),
                inline_segments: Vec::new(),
                content: format!("{path}:{n}"),
            })
            .collect::<Vec<_>>();
        FileDiff {
            path: path.into(),
            status: FileStatus::Modified,
            added_lines: lines.len(),
            deleted_lines: 0,
            hunks: vec![DiffHunk {
                lines,
                func_header: None,
            }],
        }
    }

    fn files(paths: &[(&str, usize)]) -> DiffState {
        let mut diff = diff("main");
        diff.files = paths.iter().map(|&(p, n)| changed(p, n)).collect();
        diff.rebuild_display_list();
        diff
    }

    fn land(changes: &mut BranchChanges, at: &str, diff: DiffState) {
        changes.install(
            dir(at),
            diff,
            GitStatusMap::default(),
            Some(Path::new(at)),
            Instant::now(),
        );
    }

    fn sized() -> BranchChanges {
        let mut changes = BranchChanges::default();
        changes.set_viewport(Viewport::new(0, 6));
        changes
    }

    #[test]
    fn 初めての_worktree_ではディレクトリ行を飛ばして先頭のファイルを選ぶ() {
        let mut changes = sized();
        land(&mut changes, "/a", files(&[("crates/x.rs", 3)]));
        assert_eq!(changes.selected_path(), Some("crates/x.rs"));
        assert!(!changes.body().entries.is_empty());
    }

    #[test]
    fn ファイルの送りはディレクトリ行を飛ばす() {
        let mut changes = sized();
        land(
            &mut changes,
            "/a",
            files(&[("a.rs", 1), ("crates/x.rs", 1), ("crates/y.rs", 1)]),
        );
        // ツリーはディレクトリを先に出す: crates/ → x.rs → y.rs → a.rs
        assert_eq!(changes.selected_path(), Some("crates/x.rs"));
        changes.step_file(1);
        changes.step_file(1);
        assert_eq!(changes.selected_path(), Some("a.rs"));
        changes.step_file(1);
        assert_eq!(changes.selected_path(), Some("a.rs"));
        changes.step_file(-1);
        changes.step_file(-1);
        changes.step_file(-1);
        assert_eq!(changes.selected_path(), Some("crates/x.rs"));

        // 前へ送るときもディレクトリ行 (docs/) を飛ばす
        land(
            &mut changes,
            "/b",
            files(&[("crates/x.rs", 1), ("docs/y.rs", 1)]),
        );
        changes.step_file(1);
        assert_eq!(changes.selected_path(), Some("docs/y.rs"));
    }

    #[test]
    fn 同じ_worktree_の読み直しは選んだファイルとスクロールを保つ() {
        let mut changes = sized();
        land(&mut changes, "/a", files(&[("a.rs", 1), ("b.rs", 20)]));
        changes.step_file(1);
        changes.scroll_body(4);
        land(&mut changes, "/a", files(&[("a.rs", 1), ("b.rs", 20)]));
        assert_eq!(changes.selected_path(), Some("b.rs"));
        assert_eq!(changes.body_scroll(), 4);
    }

    #[test]
    fn 選んでいたファイルが消えたら先頭のファイルに戻りスクロールも戻す() {
        let mut changes = sized();
        land(&mut changes, "/a", files(&[("a.rs", 20), ("b.rs", 20)]));
        changes.step_file(1);
        changes.scroll_body(4);
        land(&mut changes, "/a", files(&[("a.rs", 20)]));
        assert_eq!(changes.selected_path(), Some("a.rs"));
        assert_eq!(changes.body_scroll(), 0);
    }

    #[test]
    fn 変更が無くなればツリーも右も空になる() {
        let mut changes = sized();
        land(&mut changes, "/a", files(&[("a.rs", 3)]));
        land(&mut changes, "/a", files(&[]));
        assert_eq!(changes.selected_path(), None);
        assert!(changes.body().entries.is_empty());
    }

    #[test]
    fn 別の_worktree_から戻ると選んだファイルとスクロールが戻る() {
        let mut changes = sized();
        land(&mut changes, "/a", files(&[("a.rs", 1), ("b.rs", 20)]));
        changes.step_file(1);
        changes.scroll_body(3);
        land(&mut changes, "/b", files(&[("c.rs", 1)]));
        assert_eq!(changes.selected_path(), Some("c.rs"));
        land(&mut changes, "/a", files(&[("a.rs", 1), ("b.rs", 20)]));
        assert_eq!(changes.selected_path(), Some("b.rs"));
        assert_eq!(changes.body_scroll(), 3);
    }

    #[test]
    fn diff_が短くなったらスクロールを末尾に丸める() {
        let mut changes = sized();
        land(&mut changes, "/a", files(&[("a.rs", 30)]));
        changes.scroll_body(20);
        land(&mut changes, "/a", files(&[("a.rs", 8)]));
        let height = changes.body_height();
        assert_eq!(changes.body_scroll(), changes.body().entries.len() - height);
    }

    #[test]
    fn 右に_60_桁取れる幅なら割り_取れなければどちらか一方を出す() {
        let at = |width| Rect::new(0, 0, width, 10);
        for width in [95, 123] {
            let split = split(at(width), false);
            assert_eq!(split.tree.map(|r| r.width), Some(30), "{width}");
            assert!(split.body.is_some_and(|r| r.width >= 60), "{width}");
        }
        let narrow = split(at(90), false);
        assert_eq!(
            (narrow.tree.map(|r| r.width), narrow.body),
            (Some(90), None)
        );
        let reading = split(at(59), true);
        assert_eq!(
            (reading.tree, reading.body.map(|r| r.width)),
            (None, Some(59))
        );
    }
}
