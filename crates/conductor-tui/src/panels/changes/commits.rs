//! Commits タブ。左は base に無く HEAD にあるコミットの一覧、中央は選んだコミットの縦並びの
//! 差分 (読むだけの [ChangesPanel])。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use conductor_core::diff_state::{DiffSource, DiffState};
use conductor_core::git_engine::CommitInfo;
use conductor_core::keymap::Action;

use super::ChangesPanel;
use crate::effect::Effect;
use crate::list::{ListCursor, Viewport};
use crate::task::{RangePage, Task};
use crate::workspace::Ctx;

/// 巨大リポジトリで全履歴を読まないための 1 ページの件数。
pub(crate) const PAGE: usize = 100;

#[derive(Debug)]
pub struct CommitsTab {
    /// 出している worktree。出していなければ None。
    shown: Option<PathBuf>,
    base: String,
    seq: u64,
    listing: Option<usize>,
    listed: Result<Option<(git2::Oid, git2::Oid)>, String>,
    commits: Vec<CommitInfo>,
    exhausted: bool,
    cursor: ListCursor,
    view: Viewport,
    /// worktree ごとに選んでいたコミット。
    picked: HashMap<PathBuf, String>,
    /// 中央に出す (出している) コミット。
    showing: Option<String>,
    /// 差分を読んでいるコミット。1 本ずつ読む。
    waiting: Option<String>,
    pub page: ChangesPanel,
}

/// 一覧の行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListRow {
    Commit(usize),
    LoadMore,
}

impl CommitsTab {
    pub fn new(side_by_side: bool) -> Self {
        Self {
            shown: None,
            base: String::new(),
            seq: 0,
            listing: None,
            listed: Ok(None),
            commits: Vec::new(),
            exhausted: false,
            cursor: ListCursor::default(),
            view: Viewport::default(),
            picked: HashMap::new(),
            showing: None,
            waiting: None,
            page: ChangesPanel::read_only(side_by_side),
        }
    }

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
        self.cursor.clamp(self.len(), view);
    }

    /// 出している worktree が root でなければ、入ったとして一覧を先頭から読む。
    pub fn show(&mut self, root: &Path, base: &str) -> Option<Effect> {
        if self.shown.as_deref() == Some(root) {
            return None;
        }
        self.shown = Some(root.to_path_buf());
        self.base = base.to_string();
        self.seq += 1;
        self.listing = Some(0);
        self.listed = Ok(None);
        self.commits.clear();
        self.exhausted = false;
        self.cursor = ListCursor::default();
        self.showing = None;
        self.waiting = None;
        self.page.follow(root);
        Some(self.request(None))
    }

    /// タブを離れた・リポジトリを開き直した。次に出すときは読み直す。
    pub fn hide(&mut self) {
        self.shown = None;
    }

    fn request(&self, ends: Option<(git2::Oid, git2::Oid)>) -> Effect {
        Effect::Spawn(Task::RangeLog {
            seq: self.seq,
            worktree: self.shown.clone().unwrap_or_default(),
            base: self.base.clone(),
            ends,
            skip: self.commits.len(),
            limit: PAGE,
        })
    }

    pub fn len(&self) -> usize {
        let more = matches!(self.listed, Ok(Some(_))) && !self.exhausted;
        self.commits.len() + usize::from(more)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn line(&self, index: usize) -> Option<ListRow> {
        match index {
            i if i < self.commits.len() => Some(ListRow::Commit(i)),
            i if i < self.len() => Some(ListRow::LoadMore),
            _ => None,
        }
    }

    /// 中央に出す差分の代わりの一文。読み込み中・失敗・空のとき。
    pub fn notice(&self) -> Option<Result<String, String>> {
        match &self.listed {
            Err(error) => Some(Err(error.clone())),
            Ok(None) => Some(Ok("loading\u{2026}".into())),
            Ok(Some(_)) if self.commits.is_empty() => {
                Some(Ok(format!("No commits ahead of {}", self.base)))
            }
            Ok(Some(_)) if self.page.diff().is_none() => Some(Ok("loading\u{2026}".into())),
            Ok(Some(_)) => None,
        }
    }

    pub fn accept_log(
        &mut self,
        seq: u64,
        root: &Path,
        skip: usize,
        page: Result<RangePage, String>,
    ) -> Vec<Effect> {
        if seq != self.seq || self.shown.as_deref() != Some(root) || self.listing != Some(skip) {
            return Vec::new();
        }
        self.listing = None;
        let page = match page {
            Ok(page) => page,
            Err(error) => {
                self.listed = Err(error);
                return Vec::new();
            }
        };
        self.listed = Ok(Some(page.ends));
        self.exhausted = page.commits.len() < PAGE;
        self.commits.extend(page.commits);
        if skip == 0 {
            let picked = self.picked.get(root);
            let at = picked
                .and_then(|oid| self.commits.iter().position(|c| &c.oid == oid))
                .unwrap_or(0);
            self.cursor.select(at, self.len(), self.view);
            return self.pick(
                picked.is_some_and(|oid| self.commits.get(at).is_some_and(|c| &c.oid == oid)),
            );
        }
        self.cursor.clamp(self.len(), self.view);
        self.pick(false)
    }

    /// カーソルのコミットを中央に出す。keep は戻ってきたときで、預けた位置を捨てない。
    fn pick(&mut self, keep: bool) -> Vec<Effect> {
        let Some(ListRow::Commit(i)) = self.line(self.cursor.selected()) else {
            return Vec::new();
        };
        let oid = self.commits[i].oid.clone();
        if self.showing.as_ref() == Some(&oid) {
            return Vec::new();
        }
        if !keep {
            self.page.forget();
        }
        if let Some(root) = &self.shown {
            self.picked.insert(root.clone(), oid.clone());
        }
        self.showing = Some(oid);
        self.ask().into_iter().collect()
    }

    fn ask(&mut self) -> Option<Effect> {
        if self.waiting.is_some() {
            return None;
        }
        let oid = self.showing.clone()?;
        self.waiting = Some(oid.clone());
        Some(Effect::Spawn(Task::ComputeDiff {
            worktree: self.shown.clone()?,
            source: DiffSource::Commit { oid },
        }))
    }

    /// 待っているコミットの差分なら受ける。読み終えたとき選びが動いていれば次を頼む。
    pub fn accept_diff(
        &mut self,
        root: &Path,
        diff: &DiffState,
        ctx: &Ctx,
        effects: &mut Vec<Effect>,
    ) {
        let DiffSource::Commit { oid } = &diff.source else {
            return;
        };
        if self.shown.as_deref() != Some(root) || self.waiting.as_ref() != Some(oid) {
            return;
        }
        self.waiting = None;
        if self.showing.as_ref() == Some(oid) {
            self.page.install(root, diff, ctx);
        } else {
            effects.extend(self.ask());
        }
    }

    pub fn update_list(&mut self, action: Action) -> Option<Vec<Effect>> {
        let len = self.len();
        match action {
            Action::NavigateDown => self.cursor.step(1, len, self.view),
            Action::NavigateUp => self.cursor.step(-1, len, self.view),
            Action::GoToTop => self.cursor.select(0, len, self.view),
            Action::GoToBottom => self.cursor.select(usize::MAX, len, self.view),
            Action::Select => return Some(self.load_more().into_iter().collect()),
            _ => return None,
        }
        Some(self.pick(false))
    }

    fn load_more(&mut self) -> Option<Effect> {
        let Some(ListRow::LoadMore) = self.line(self.cursor.selected()) else {
            return None;
        };
        if self.listing.is_some() {
            return None;
        }
        let ends = self.listed.clone().ok().flatten()?;
        self.listing = Some(self.commits.len());
        Some(self.request(Some(ends)))
    }

    pub fn click(&mut self, y: u16) -> Option<Vec<Effect>> {
        let at = self.cursor.index_at(y, self.len(), self.view)?;
        self.cursor.select(at, self.len(), self.view);
        Some(match self.line(at)? {
            ListRow::LoadMore => self.load_more().into_iter().collect(),
            ListRow::Commit(_) => self.pick(false),
        })
    }

    pub fn scroll_list(&mut self, delta: isize) {
        self.cursor.pan(delta, self.len(), self.view);
    }
}
