//! 一覧の下区画: 選んだ行のブランチの変更ファイルと、選んだファイルの diff (AI が頼んだファイルは全体)。Explorer が持つ
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
    /// 右にファイル全体を出す。変更の無いファイルはツリーに行が無いので、仮の行で示す。
    whole: bool,
    /// AI が出した。人が選び直すまで、diff から消えても全体に移って残る。
    asked: bool,
    /// 次に中身が据わったとき、1 回だけそこへ送る行 (1 始まり、新しい側)。
    line: Option<usize>,
    /// AI が出した後に人がスクロールした。送り直さない。
    scrolled: bool,
}

/// diff で出してと頼まれたが、手元の diff にまだ無いファイル。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pending {
    /// 頼んだ会話。一覧でこれが選ばれている間だけ生きる。
    panel: String,
    dir: PathBuf,
    path: String,
    line: Option<usize>,
    diff: bool,
    /// 頼んだ時点で始めていた読みの数。これより後に始めた読みで判定する。
    after: u64,
    /// もう出してある。新しい読みでは頼まれた行へ送り直すだけ。
    shown: bool,
}

/// ファイル全体の読み。seq で照合し、古い結果は据えない。
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileRead {
    dir: PathBuf,
    path: String,
    seq: u64,
    asked: bool,
}

/// 右に描くもの。
#[derive(Debug, Default)]
pub enum Body {
    #[default]
    Nothing,
    Diff {
        entries: Vec<Entry>,
        digits: usize,
    },
    Reading,
    Whole {
        path: String,
        lines: Result<Vec<String>, String>,
    },
}

impl Body {
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Diff { entries, .. } => entries.len(),
            Self::Whole {
                lines: Ok(lines), ..
            } => lines.len(),
            _ => 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct BranchChanges {
    /// 据えた結果と、それを頼んだ dir。
    shown: Option<(PathBuf, GitChanges, GitStatusMap)>,
    view: Viewport,
    fetching: bool,
    landed: Option<Instant>,
    /// 始めた読みの数と、いま投げている読みの番号。
    reads: u64,
    reading: u64,
    pick: Pick,
    picks: HashMap<PathBuf, Pick>,
    pending: Option<Pending>,
    file: Option<FileRead>,
    file_seq: u64,
    body: Body,
    /// 右の diff に使える行数。見出しの 1 行は除く。
    body_height: usize,
    /// 狭くて割れないときに、diff の側を出しているか。
    body_open: bool,
    /// 左右に割れる幅か。sync_layout が渡す。
    split: bool,
    /// ホイールでツリーを動かしたか。動かしていれば読み直しで窓を選択へ寄せない。
    tree_panned: bool,
}

impl BranchChanges {
    /// 次に読む dir。投げ中は何も返さないので、遅い repo でも 1 本ずつになる。
    pub fn wanted(&self, target: Option<PathBuf>, now: Instant) -> Option<PathBuf> {
        let target = target?;
        if self.fetching {
            return None;
        }
        let waiting = self.pending.as_ref().is_some_and(|p| p.dir == target);
        let fresh = self.shown_dir() == Some(target.as_path())
            && self.landed.is_some_and(|at| now < at + REFRESH);
        (waiting || !fresh).then_some(target)
    }

    pub fn note_fetching(&mut self) {
        self.fetching = true;
        self.reads += 1;
        self.reading = self.reads;
    }

    /// 投げた結果は捨てられる。全体の読みは頼み直す。
    pub fn forget_in_flight(&mut self) {
        self.fetching = false;
        if let Some(read) = &mut self.file {
            read.asked = false;
        }
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
            self.shown = Some((dir.clone(), changes, status));
        }
        if self.pending.as_ref().is_some_and(|p| p.dir != dir) {
            self.pending = None;
        }
        let Some((_, changes, _)) = &mut self.shown else {
            return;
        };
        let panned = (same && self.tree_panned).then(|| changes.cursor().scroll());
        let tree_file = changes.selected_path().map(str::to_string);
        // 帯への警告は捨てる。読み直すたびに同じ警告が出続ける。理由は区画の中に出る。
        let _ = changes.install(diff);
        let picked = match self.pick.path.as_deref() {
            Some(path) => changes.select_path(path),
            None => false,
        };
        // 右が木に無いファイル (仮の行) でも、木のカーソルは木自身のファイルが消えるまで保つ。
        let off_tree = !picked && (self.pick.whole || self.pick.asked) && self.pick.path.is_some();
        let kept = picked || off_tree && tree_file.is_some_and(|f| changes.select_path(&f));
        if !kept {
            changes.select_first_file();
            self.tree_panned = false;
        }
        if off_tree {
            self.pick.whole = true;
        } else if !picked {
            self.pick.scroll = 0;
        }
        if let Some(scroll) = panned.filter(|_| kept) {
            changes.pan(scroll as isize - changes.cursor().scroll() as isize);
        }
        self.landed = Some(now);
        let reading = self.reading;
        let resolved = self.pending.take_if(|p| p.dir == dir && reading > p.after);
        match resolved {
            Some(p) if !p.shown => {
                let has = self.has_change(&p.path);
                self.show_asked(p.path, p.line, !(p.diff && has));
            }
            resolved => {
                let resend = resolved.filter(|p| {
                    self.pick.asked
                        && !self.pick.scrolled
                        && self.pick.path.as_ref() == Some(&p.path)
                });
                if let Some(p) = resend {
                    self.pick.line = p.line;
                }
                if self.pick.whole {
                    self.read_file();
                }
                self.rebuild_body();
                self.apply_view();
            }
        }
    }

    /// 頼んだ会話が一覧で選ばれていなければ、待っている頼みを捨てる。
    pub fn keep_pending_for(&mut self, selected: Option<&str>) {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| Some(p.panel.as_str()) != selected)
        {
            self.pending = None;
        }
    }

    /// AI の頼み。dir の結果がまだ据わっていないか、diff の頼みで手元の diff に無ければ、
    /// 読み直しを 1 本挟んでから出す。
    /// 手元の diff は数秒古いことがあるので、diff の頼みは頼みより後に始めた読みでもう一度見る。
    pub fn open_asked(
        &mut self,
        panel: &str,
        dir: &Path,
        path: String,
        line: Option<usize>,
        diff: bool,
    ) {
        let here = self.shown_dir() == Some(dir);
        let shown = here && (!diff || self.has_change(&path));
        if shown {
            self.show_asked(path.clone(), line, !diff);
            if !diff {
                return;
            }
        }
        self.pending = Some(Pending {
            panel: panel.to_string(),
            dir: dir.to_path_buf(),
            path,
            line,
            diff,
            after: if diff { self.reads } else { 0 },
            shown,
        });
        self.landed = None;
    }

    /// 右にそのファイルが出ていれば、ツリーの選択の diff に戻す。
    pub fn close_asked(&mut self, dir: &Path, path: &str) -> bool {
        let showing = self.shown_dir() == Some(dir) && self.pick.path.as_deref() == Some(path);
        if showing {
            self.pick = Pick::default();
            self.pending = None;
            self.file = None;
            self.rebuild_body();
            self.apply_view();
        }
        showing
    }

    /// 全体の読みが要るなら、その (dir, path, seq) を 1 回だけ渡す。
    pub fn take_file_read(&mut self) -> Option<(PathBuf, String, u64)> {
        let read = self.file.as_mut().filter(|r| !r.asked)?;
        read.asked = true;
        Some((read.dir.clone(), read.path.clone(), read.seq))
    }

    pub fn install_file(
        &mut self,
        dir: &Path,
        path: &str,
        seq: u64,
        lines: Result<Vec<String>, String>,
    ) {
        let current = self
            .file
            .as_ref()
            .is_some_and(|r| r.seq == seq && r.dir == dir && r.path == path);
        if !current || !self.pick.whole || self.pick.path.as_deref() != Some(path) {
            return;
        }
        self.file = None;
        self.body = Body::Whole {
            path: path.to_string(),
            lines,
        };
        if let Some(line) = self.pick.line.take() {
            self.pick.scroll = line.saturating_sub(1);
        }
        self.clamp_scroll();
    }

    /// ツリーに行の無いファイルを出しているとき、そのパス。ツリーの先頭に仮の行で出す。
    pub fn provisional(&self) -> Option<&str> {
        let path = self.pick.path.as_deref().filter(|_| self.pick.whole)?;
        (!self.has_change(path)).then_some(path)
    }

    pub fn set_viewport(&mut self, view: Viewport) {
        self.view = view;
        self.body_height = view.height.saturating_sub(1);
        self.apply_view();
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

    pub fn split(&self) -> bool {
        self.split
    }

    /// 割れる幅では diff を区画いっぱいに出す状態を持たない。広げたら畳む。
    pub fn set_split(&mut self, split: bool) {
        self.split = split;
        if split {
            self.body_open = false;
        }
    }

    /// 区画いっぱいの diff は、左下がキーボードを持っている間か、AI が出している間だけ。
    pub fn note_focus(&mut self, inside: bool) {
        if !inside && !self.pick.asked {
            self.body_open = false;
        }
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
            self.tree_panned = true;
        }
    }

    pub fn scroll_body(&mut self, delta: isize) {
        self.pick.scrolled = true;
        self.pick.scroll = self.pick.scroll.saturating_add_signed(delta);
        self.clamp_scroll();
    }

    pub fn body_height(&self) -> usize {
        self.body_height
    }

    /// 人が選んだ。AI の頼みも、全体の表示も、待っている頼みもここで終わる。
    fn chose(&mut self) {
        self.tree_panned = false;
        self.pending = None;
        let path = self
            .shown
            .as_ref()
            .and_then(|(_, changes, _)| changes.selected_path())
            .map(str::to_string);
        let plain = !self.pick.whole && !self.pick.asked;
        if path != self.pick.path || !plain {
            self.pick = Pick {
                path,
                ..Pick::default()
            };
            self.file = None;
            self.rebuild_body();
            self.apply_view();
        }
    }

    fn show_asked(&mut self, path: String, line: Option<usize>, whole: bool) {
        self.pending = None;
        self.tree_panned = false;
        if let Some((_, changes, _)) = &mut self.shown {
            changes.select_path(&path);
        }
        self.pick = Pick {
            path: Some(path),
            whole,
            asked: true,
            line,
            ..Pick::default()
        };
        self.body_open = !self.split;
        if whole {
            self.body = Body::Reading;
            self.read_file();
        }
        self.rebuild_body();
        self.apply_view();
    }

    fn read_file(&mut self) {
        let (Some(dir), Some(path)) = (
            self.shown_dir().map(Path::to_path_buf),
            self.pick.path.clone(),
        ) else {
            return;
        };
        self.file_seq += 1;
        self.file = Some(FileRead {
            dir,
            path,
            seq: self.file_seq,
            asked: false,
        });
    }

    fn has_change(&self, path: &str) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|(_, changes, _)| changes.diff().files.iter().any(|f| f.path == path))
    }

    /// 仮の行を出している間は、ツリーをその 1 行ぶん下げる。
    fn apply_view(&mut self) {
        let mut view = self.view;
        if self.provisional().is_some() {
            view.top = view.top.saturating_add(1);
            view.height = view.height.saturating_sub(1);
        }
        if let Some((_, changes, _)) = &mut self.shown {
            changes.set_viewport(view);
        }
    }

    fn rebuild_body(&mut self) {
        if self.pick.whole {
            let kept = matches!(&self.body, Body::Whole { path, .. } if Some(path) == self.pick.path.as_ref());
            if !kept {
                self.body = Body::Reading;
            }
            self.clamp_scroll();
            return;
        }
        let file = self.shown.as_ref().and_then(|(_, changes, _)| {
            let path = changes.selected_path()?;
            changes.diff().files.iter().find(|f| f.path == path)
        });
        self.pick.path = file.map(|f| f.path.clone());
        let mut pane = DiffPane::default();
        if let Some(file) = file {
            pane.build(file, 0);
        }
        if let Some(line) = self.pick.line.take() {
            self.pick.scroll = row_of_line(&pane.entries, line);
        }
        self.body = match self.pick.path {
            Some(_) => Body::Diff {
                digits: digit_count(pane.max_line_no),
                entries: pane.entries,
            },
            None => Body::Nothing,
        };
        if self.pick.path.is_none() {
            self.body_open = false;
        }
        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        let max = self.body.len().saturating_sub(self.body_height.max(1));
        self.pick.scroll = self.pick.scroll.min(max);
    }

    /// 今の dir の選択を預ける。
    fn park(&mut self) {
        if let Some((dir, _, _)) = &self.shown {
            let mut pick = std::mem::take(&mut self.pick);
            pick.line = None;
            self.picks.insert(dir.clone(), pick);
        }
        self.file = None;
        self.body = Body::Nothing;
        self.body_open = false;
        self.tree_panned = false;
    }

    fn shown_dir(&self) -> Option<&Path> {
        self.shown.as_ref().map(|(dir, _, _)| dir.as_path())
    }
}

/// 新しい側の行番号が line 以上の最初の行。無ければ最後のハンクの頭。
fn row_of_line(entries: &[Entry], line: usize) -> usize {
    let at_or_after = entries
        .iter()
        .position(|e| matches!(e, Entry::Line { new_line_no: Some(n), .. } if *n >= line));
    at_or_after.unwrap_or_else(|| {
        entries
            .iter()
            .rposition(|e| {
                matches!(
                    e,
                    Entry::HunkSeparator { .. } | Entry::ExpandableContext { .. }
                )
            })
            .unwrap_or(0)
    })
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
        assert!(!changes.body().is_empty());
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
        assert!(changes.body().is_empty());
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
        assert_eq!(changes.body_scroll(), changes.body().len() - height);
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

    #[test]
    fn ホイールに触れていなければ上にファイルが増えても選んだ行は見えたまま() {
        let mut changes = sized();
        let names: Vec<String> = (0..10).map(|i| format!("m{i:02}.rs")).collect();
        let refs: Vec<(&str, usize)> = names.iter().map(|p| (p.as_str(), 1)).collect();
        land(&mut changes, "/a", files(&refs));
        for _ in 0..5 {
            changes.step_file(1);
        }
        let mut more: Vec<String> = (0..3).map(|i| format!("a{i}.rs")).collect();
        more.extend(names.iter().cloned());
        let refs: Vec<(&str, usize)> = more.iter().map(|p| (p.as_str(), 1)).collect();
        land(&mut changes, "/a", files(&refs));
        assert_eq!(changes.selected_path(), Some("m05.rs"));
        let tree = changes
            .for_target(Some(Path::new("/a")))
            .unwrap()
            .0
            .cursor();
        let window = tree.scroll()..tree.scroll() + 6;
        assert!(
            window.contains(&tree.selected()),
            "{} が {window:?} の外",
            tree.selected()
        );
    }

    fn top_new_line(changes: &BranchChanges) -> Option<usize> {
        let Body::Diff { entries, .. } = changes.body() else {
            return None;
        };
        match entries.get(changes.body_scroll())? {
            Entry::Line { new_line_no, .. } => *new_line_no,
            _ => None,
        }
    }

    fn whole_lines(n: usize) -> Result<Vec<String>, String> {
        Ok((1..=n).map(|i| format!("line {i}")).collect())
    }

    /// 全体の読みを頼まれていれば、その通りに読んだことにして据える。
    fn read_whole(changes: &mut BranchChanges, n: usize) {
        let (dir, path, seq) = changes.take_file_read().expect("全体の読みを頼んでいない");
        changes.install_file(&dir, &path, seq, whole_lines(n));
    }

    fn asked_long() -> BranchChanges {
        let mut changes = sized();
        land(&mut changes, "/a", files(&[("a.rs", 3), ("b.rs", 40)]));
        changes
    }

    #[test]
    fn diff_で頼まれたファイルを選び頼まれた行へ一度だけ送る() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), Some(20), true);
        assert_eq!(changes.selected_path(), Some("b.rs"));
        assert_eq!(top_new_line(&changes), Some(20));
        changes.scroll_body(-5);
        changes.note_fetching();
        land(&mut changes, "/a", files(&[("a.rs", 3), ("b.rs", 40)]));
        assert_eq!(
            top_new_line(&changes),
            Some(15),
            "読み直しで頼まれた行へ引き戻した"
        );
    }

    #[test]
    fn 頼まれた行がハンクより後なら最後のハンクの頭へ送る() {
        let entries = vec![
            Entry::HunkSeparator { func_header: None },
            Entry::Line {
                tag: DiffLineTag::Insert,
                old_line_no: None,
                new_line_no: Some(3),
                inline_segments: Vec::new(),
                content: String::new(),
            },
            Entry::HunkSeparator { func_header: None },
            Entry::Line {
                tag: DiffLineTag::Insert,
                old_line_no: None,
                new_line_no: Some(9),
                inline_segments: Vec::new(),
                content: String::new(),
            },
        ];
        assert_eq!(row_of_line(&entries, 5), 3);
        assert_eq!(row_of_line(&entries, 50), 2);
    }

    #[test]
    fn 全体で頼まれた変更のあるファイルはツリーで選び_全体を読んで頼まれた行から出す() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), Some(30), false);
        assert_eq!(changes.selected_path(), Some("b.rs"));
        assert!(matches!(changes.body(), Body::Reading));
        assert_eq!(changes.provisional(), None);
        read_whole(&mut changes, 100);
        assert!(matches!(changes.body(), Body::Whole { lines: Ok(_), .. }));
        assert_eq!(changes.body_scroll(), 29);
    }

    #[test]
    fn 変更の無いファイルは仮の行で選び_ファイルを送ると仮の行が消えて_diff_に戻る() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "plain.rs".into(), None, false);
        assert_eq!(changes.provisional(), Some("plain.rs"));
        read_whole(&mut changes, 5);
        changes.step_file(1);
        assert_eq!(changes.provisional(), None);
        assert!(matches!(changes.body(), Body::Diff { .. }));
    }

    #[test]
    fn 仮の行がある間はツリーを一行下げて押した行とファイルを合わせる() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "plain.rs".into(), None, false);
        assert!(!changes.select_at(0), "仮の行を押してツリーの先頭を選んだ");
        assert!(changes.select_at(1));
        assert_eq!(changes.selected_path(), Some("a.rs"));
    }

    #[test]
    fn 全体を出している間に同じファイルを選び直すと_diff_に戻る() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "a.rs".into(), None, false);
        read_whole(&mut changes, 5);
        assert!(changes.select_at(0));
        assert!(matches!(changes.body(), Body::Diff { .. }));
    }

    #[test]
    fn 手元の_diff_に無いファイルは頼みより後に始めた読みで判定する() {
        let mut changes = asked_long();
        changes.note_fetching();
        changes.open_asked("p", Path::new("/a"), "new.rs".into(), None, true);
        let now = Instant::now();
        assert_eq!(changes.wanted(Some(dir("/a")), now), None, "投げ中に重ねた");
        land(&mut changes, "/a", files(&[("a.rs", 3), ("new.rs", 4)]));
        assert_ne!(
            changes.selected_path(),
            Some("new.rs"),
            "頼みより前の読みで判定した"
        );
        assert_eq!(
            changes.wanted(Some(dir("/a")), now),
            Some(dir("/a")),
            "読み直しを待たせた"
        );
        changes.note_fetching();
        land(&mut changes, "/a", files(&[("a.rs", 3), ("new.rs", 4)]));
        assert_eq!(changes.selected_path(), Some("new.rs"));
        assert!(matches!(changes.body(), Body::Diff { .. }));
    }

    #[test]
    fn 読み直しても_diff_に無ければ全体を出す() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "plain.rs".into(), None, true);
        changes.note_fetching();
        land(&mut changes, "/a", files(&[("a.rs", 3)]));
        assert_eq!(changes.provisional(), Some("plain.rs"));
        assert!(matches!(changes.body(), Body::Reading));
    }

    #[test]
    fn 待っている頼みは選択が別の_worktree_に移ると捨て_同じパスを選ばない() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "new.rs".into(), None, true);
        changes.note_fetching();
        land(&mut changes, "/b", files(&[("new.rs", 1), ("z.rs", 1)]));
        changes.note_fetching();
        land(&mut changes, "/b", files(&[("new.rs", 1), ("z.rs", 1)]));
        assert_eq!(
            changes.selected_path(),
            Some("new.rs"),
            "先頭のファイルのまま"
        );
        assert_eq!(changes.provisional(), None);
        assert!(matches!(changes.body(), Body::Diff { .. }));
        changes.note_fetching();
        land(&mut changes, "/a", files(&[("a.rs", 3), ("new.rs", 4)]));
        assert_ne!(
            changes.selected_path(),
            Some("new.rs"),
            "捨てた頼みで選んだ"
        );
    }

    #[test]
    fn 待っている間に二つ目の頼みが来たら後の頼みが勝つ() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "new.rs".into(), None, true);
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), None, true);
        assert_eq!(changes.selected_path(), Some("b.rs"));
        changes.note_fetching();
        land(
            &mut changes,
            "/a",
            files(&[("a.rs", 3), ("b.rs", 40), ("new.rs", 4)]),
        );
        assert_eq!(changes.selected_path(), Some("b.rs"));
    }

    #[test]
    fn ai_が_diff_で出したファイルが_diff_から消えたらその全体へ移る() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), None, true);
        land(&mut changes, "/a", files(&[("a.rs", 3)]));
        assert_eq!(changes.selected_path(), Some("b.rs"));
        assert_eq!(changes.provisional(), Some("b.rs"));
        read_whole(&mut changes, 40);
        assert!(matches!(changes.body(), Body::Whole { .. }));
    }

    #[test]
    fn 人が選んだファイルが消えたら先頭のファイルへ移り全体にはしない() {
        let mut changes = asked_long();
        changes.step_file(1);
        land(&mut changes, "/a", files(&[("a.rs", 3)]));
        assert_eq!(changes.selected_path(), Some("a.rs"));
        assert!(matches!(changes.body(), Body::Diff { .. }));
    }

    #[test]
    fn 古い全体の読みは据えない() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), None, false);
        let (d, p, old) = changes.take_file_read().unwrap();
        changes.open_asked("p", Path::new("/a"), "a.rs".into(), None, false);
        changes.install_file(&d, &p, old, whole_lines(3));
        assert!(matches!(changes.body(), Body::Reading));
        let (d, p, seq) = changes.take_file_read().unwrap();
        changes.install_file(&d, &p, seq + 1, whole_lines(3));
        assert!(matches!(changes.body(), Body::Reading));
        changes.step_file(1);
        changes.install_file(&d, &p, seq, whole_lines(3));
        assert!(
            matches!(changes.body(), Body::Diff { .. }),
            "選び直した後に据えた"
        );
    }

    #[test]
    fn 全体を読めない理由はそのまま右に出す() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "img.bin".into(), None, false);
        let (d, p, seq) = changes.take_file_read().unwrap();
        changes.install_file(&d, &p, seq, Err("binary file".into()));
        assert!(matches!(changes.body(), Body::Whole { lines: Err(r), .. } if r == "binary file"));
    }

    #[test]
    fn 全体の読みが捨てられたら頼み直す() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), None, false);
        assert!(changes.take_file_read().is_some());
        assert!(changes.take_file_read().is_none(), "同じ読みを 2 回頼んだ");
        changes.forget_in_flight();
        assert!(changes.take_file_read().is_some());
    }

    #[test]
    fn 全体を出したまま別の_worktree_から戻ると読み直して全体を出す() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "plain.rs".into(), None, false);
        read_whole(&mut changes, 5);
        land(&mut changes, "/b", files(&[("x.rs", 1)]));
        assert!(
            matches!(changes.body(), Body::Diff { .. }),
            "前の worktree の全体を出した"
        );
        changes.open_asked("p", Path::new("/b"), "plain.rs".into(), None, false);
        read_whole(&mut changes, 7);
        land(&mut changes, "/a", files(&[("a.rs", 3)]));
        assert_eq!(changes.provisional(), Some("plain.rs"));
        assert!(matches!(changes.body(), Body::Reading));
        read_whole(&mut changes, 5);
        assert!(matches!(changes.body(), Body::Whole { .. }));
    }

    #[test]
    fn 閉じる頼みは右にそのファイルが出ていればツリーの選択の_diff_に戻す() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "plain.rs".into(), None, false);
        assert!(!changes.close_asked(Path::new("/a"), "a.rs"));
        assert!(changes.close_asked(Path::new("/a"), "plain.rs"));
        assert_eq!(changes.provisional(), None);
        assert_eq!(changes.selected_path(), Some("a.rs"));
        assert!(matches!(changes.body(), Body::Diff { .. }));
    }

    #[test]
    fn 狭い区画では_ai_の頼みを区画いっぱいに出し_左下に入っていなくても閉じない() {
        let mut changes = asked_long();
        changes.set_split(false);
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), None, true);
        assert!(changes.body_open());
        changes.note_focus(false);
        assert!(changes.body_open());
        changes.step_file(-1);
        changes.note_focus(false);
        assert!(!changes.body_open(), "人が選んだ後も出し続けた");
    }

    #[test]
    fn 待っている頼みは人が選び直すと捨てる() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "new.rs".into(), None, true);
        changes.step_file(1);
        changes.note_fetching();
        land(
            &mut changes,
            "/a",
            files(&[("a.rs", 3), ("b.rs", 40), ("new.rs", 4)]),
        );
        assert_eq!(changes.selected_path(), Some("b.rs"));
    }

    #[test]
    fn 全体を出している間の読み直しは前の中身を出したままスクロールを保つ() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "plain.rs".into(), Some(50), false);
        read_whole(&mut changes, 100);
        changes.scroll_body(-5);
        let at = changes.body_scroll();
        land(&mut changes, "/a", files(&[("a.rs", 3), ("b.rs", 40)]));
        assert!(matches!(changes.body(), Body::Whole { lines: Ok(_), .. }));
        assert_eq!(changes.body_scroll(), at);
        assert!(changes.take_file_read().is_some(), "読み直しを頼んでいない");
    }

    #[test]
    fn 仮の行を出している間の読み直しは木のカーソルとホイールの位置を保つ() {
        let mut changes = sized();
        let names: Vec<String> = (0..20).map(|i| format!("m{i:02}.rs")).collect();
        let refs: Vec<(&str, usize)> = names.iter().map(|p| (p.as_str(), 1)).collect();
        land(&mut changes, "/a", files(&refs));
        for _ in 0..8 {
            changes.step_file(1);
        }
        changes.open_asked("p", Path::new("/a"), "plain.rs".into(), None, false);
        read_whole(&mut changes, 5);
        changes.pan_tree(6);
        let tree = |c: &BranchChanges| {
            let cursor = c.for_target(Some(Path::new("/a"))).unwrap().0.cursor();
            (cursor.selected(), cursor.scroll())
        };
        let before = tree(&changes);
        land(&mut changes, "/a", files(&refs));
        assert_eq!(tree(&changes), before);
        assert_eq!(changes.provisional(), Some("plain.rs"));
    }

    #[test]
    fn diff_の頼みは頼みより後の読みが着いたら頼まれた行へ送り直す() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), Some(60), true);
        changes.note_fetching();
        land(&mut changes, "/a", files(&[("a.rs", 3), ("b.rs", 80)]));
        assert_eq!(top_new_line(&changes), Some(60));
    }

    #[test]
    fn 送り直しの前に人がスクロールしていたら送らない() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), Some(60), true);
        changes.scroll_body(3);
        let at = top_new_line(&changes);
        changes.note_fetching();
        land(&mut changes, "/a", files(&[("a.rs", 3), ("b.rs", 80)]));
        assert_eq!(top_new_line(&changes), at);
    }

    #[test]
    fn 区画の高さが変わっても人がスクロールしていなければ新しい読みで送り直す() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), Some(38), true);
        changes.set_viewport(Viewport::new(0, 20));
        changes.note_fetching();
        land(&mut changes, "/a", files(&[("a.rs", 3), ("b.rs", 80)]));
        assert_eq!(top_new_line(&changes), Some(38));
    }

    #[test]
    fn diff_で出したファイルが新しい読みで消えたら全体を頼まれた行から出す() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "b.rs".into(), Some(30), true);
        changes.note_fetching();
        land(&mut changes, "/a", files(&[("a.rs", 3)]));
        read_whole(&mut changes, 100);
        assert_eq!(changes.body_scroll(), 29);
    }

    #[test]
    fn 待っている頼みは頼んだ会話が選ばれていなければ捨てる() {
        let mut changes = asked_long();
        changes.open_asked("p", Path::new("/a"), "new.rs".into(), None, true);
        changes.keep_pending_for(Some("p"));
        changes.keep_pending_for(Some("q"));
        changes.note_fetching();
        land(&mut changes, "/a", files(&[("a.rs", 3), ("new.rs", 4)]));
        assert_ne!(changes.selected_path(), Some("new.rs"));
    }
}
