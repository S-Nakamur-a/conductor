//! マシン上の全 Claude を 1 枚に並べる一覧。
//!
//! 状態は Claude Code が自分で書く状態ファイルから取り、この窓のセッションとは会話の id で
//! 突き合わせる。この窓のセッションの行はパネルの id で持つ — 会話の id は `/clear` で変わる。

pub mod render;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use conductor_core::claude_log::summary::{Summary, Title};
use conductor_core::claude_sessions::registry::{LiveSession, Owner, Status, Waiting};
use conductor_core::keymap::Action;

use crate::effect::Effect;
use crate::workspace::{Ctx, Focus};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowKey {
    Panel(String),
    Pid(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Waiting(Ask),
    Working,
    /// 止まってから、まだ一覧で選んでも区画に映してもいない。
    Finished,
    Idle,
    /// 状態ファイルが知らない値をそのまま持つ (実物に `shell` があった)。
    Other(String),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Question,
    Permission,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Mine,
    OtherConductor,
    Outside,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub key: RowKey,
    pub title: Option<String>,
    pub state: State,
    pub since: Option<SystemTime>,
    pub context_tokens: Option<u64>,
    pub place: Place,
    pub working_dir: PathBuf,
    /// 入力待ちの欄に重ねた方の行。
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub rows: Vec<Row>,
}

/// この窓の PtyStore にある Claude。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnSession {
    pub panel_id: String,
    pub working_dir: PathBuf,
    pub claude_session_id: Option<String>,
}

/// worker が 1 回で読んだもの。題は会話ログの mtime が変わったものだけ入る。
#[derive(Debug, Default)]
pub struct FleetStatus {
    pub live: Vec<LiveSession>,
    pub summaries: Vec<(String, SystemTime, Summary)>,
}

const OUTSIDE_GROUP: &str = "外の端末";

#[derive(Debug, Default)]
pub struct FleetPanel {
    /// 同じ行が入力待ちの欄と repo の欄の両方にいるので、どちらかも持つ。
    selected: Option<(RowKey, bool)>,
    own: Vec<OwnSession>,
    live: Vec<LiveSession>,
    summaries: HashMap<String, (SystemTime, Summary)>,
    seen: HashMap<RowKey, SystemTime>,
    repos: HashMap<PathBuf, String>,
    polling: bool,
    came_from: Option<Focus>,
}

impl FleetPanel {
    pub fn is_polling(&self) -> bool {
        self.polling
    }

    pub fn note_spawned(&mut self, task: &crate::task::Task) {
        if matches!(task, crate::task::Task::FleetStatus { .. }) {
            self.polling = true;
        }
    }

    /// 結果は repo の切り替えで世代ごと捨てられるので、ガードもそこで下ろす。
    pub fn forget_in_flight(&mut self) {
        self.polling = false;
    }

    /// 会話ログを読み直さなくていいもの。会話の id → 前に読んだときの mtime。
    pub fn known_transcripts(&self) -> HashMap<String, SystemTime> {
        self.summaries
            .iter()
            .map(|(id, (mtime, _))| (id.clone(), *mtime))
            .collect()
    }

    pub fn open(&mut self, from: Focus, own: Vec<OwnSession>) {
        if !matches!(from, Focus::Fleet | Focus::FleetSession) {
            self.came_from = Some(from);
        }
        self.set_own(own);
        self.reselect_if_gone();
        self.mark_selected(SystemTime::now());
    }

    pub fn came_from(&self) -> Focus {
        self.came_from.unwrap_or(Focus::Explorer)
    }

    pub fn defer_focus(&mut self, focus: Focus) {
        self.came_from = Some(focus);
    }

    /// 一覧を開いている間に見られているのは、Claude 区画ではなく一覧で選んだ行。
    pub fn apply(
        &mut self,
        status: FleetStatus,
        own: Vec<OwnSession>,
        visible: Option<&str>,
        listing: bool,
        now: SystemTime,
    ) {
        self.polling = false;
        self.live = status.live;
        for (id, mtime, summary) in status.summaries {
            self.summaries.insert(id, (mtime, summary));
        }
        self.set_own(own);
        if listing {
            self.mark_selected(now);
        } else if let Some(panel) = visible {
            self.seen.insert(RowKey::Panel(panel.to_string()), now);
        }
        self.reselect_if_gone();
    }

    fn set_own(&mut self, own: Vec<OwnSession>) {
        for dir in own
            .iter()
            .map(|s| s.working_dir.clone())
            .chain(self.live.iter().map(|s| s.cwd.clone()))
        {
            self.repos
                .entry(dir.clone())
                .or_insert_with(|| repo_name(&dir));
        }
        self.own = own;
    }

    /// 選び直しは人が選んだものではないので、見たことにはしない。
    fn reselect_if_gone(&mut self) {
        let rows = self.visual_rows();
        if rows.iter().any(|r| self.is_selected(r)) {
            return;
        }
        let same_session = self
            .selected
            .as_ref()
            .and_then(|(key, _)| rows.iter().find(|r| r.key == *key));
        self.selected = same_session
            .or(rows.first())
            .map(|r| (r.key.clone(), r.pinned));
    }

    fn select(&mut self, row: &Row, now: SystemTime) {
        self.selected = Some((row.key.clone(), row.pinned));
        self.mark_selected(now);
    }

    fn mark_selected(&mut self, now: SystemTime) {
        if let Some((key, _)) = &self.selected {
            self.seen.insert(key.clone(), now);
        }
    }

    pub fn is_selected(&self, row: &Row) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|(key, pinned)| *key == row.key && *pinned == row.pinned)
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.visual_rows().into_iter().find(|r| self.is_selected(r))
    }

    /// 入力待ちを一番上に重ね、残りは repo ごと。外の端末は最後。
    pub fn groups(&self) -> (Vec<Row>, Vec<Group>) {
        let rows = self.rows();
        let pinned: Vec<Row> = rows
            .iter()
            .filter(|r| matches!(r.state, State::Waiting(_)))
            .map(|r| Row {
                pinned: true,
                ..r.clone()
            })
            .collect();
        let mut groups: Vec<Group> = Vec::new();
        for row in rows {
            let name = self.group_of(&row);
            match groups.iter_mut().find(|g| g.name == name) {
                Some(group) => group.rows.push(row),
                None => groups.push(Group {
                    name,
                    rows: vec![row],
                }),
            }
        }
        groups.sort_by(|a, b| {
            (a.name == OUTSIDE_GROUP)
                .cmp(&(b.name == OUTSIDE_GROUP))
                .then_with(|| a.name.cmp(&b.name))
        });
        (pinned, groups)
    }

    fn group_of(&self, row: &Row) -> String {
        if row.place == Place::Outside {
            return OUTSIDE_GROUP.to_string();
        }
        self.repos
            .get(&row.working_dir)
            .cloned()
            .unwrap_or_else(|| dir_name(&row.working_dir))
    }

    pub fn visual_rows(&self) -> Vec<Row> {
        let (pinned, groups) = self.groups();
        pinned
            .into_iter()
            .chain(groups.into_iter().flat_map(|g| g.rows))
            .collect()
    }

    fn rows(&self) -> Vec<Row> {
        let by_id: Vec<Option<&LiveSession>> = self
            .own
            .iter()
            .map(|own| {
                own.claude_session_id
                    .as_deref()
                    .and_then(|id| self.live.iter().find(|l| l.session_id == id))
            })
            .collect();
        let paired: Vec<Option<&LiveSession>> = self
            .own
            .iter()
            .zip(&by_id)
            .map(|(own, found)| found.or_else(|| self.lone_stray(&own.working_dir, &by_id)))
            .collect();
        let matched: Vec<&str> = paired
            .iter()
            .flatten()
            .map(|l| l.session_id.as_str())
            .collect();
        let mut rows: Vec<Row> = self
            .own
            .iter()
            .zip(&paired)
            .map(|(own, live)| {
                self.row(
                    RowKey::Panel(own.panel_id.clone()),
                    Place::Mine,
                    &own.working_dir,
                    live.map(|l| l.session_id.as_str())
                        .or(own.claude_session_id.as_deref()),
                    *live,
                )
            })
            .collect();
        rows.extend(
            self.live
                .iter()
                .filter(|l| !matched.contains(&l.session_id.as_str()))
                .map(|live| {
                    let place = match live.owner {
                        Owner::Conductor => Place::OtherConductor,
                        Owner::Elsewhere => Place::Outside,
                    };
                    self.row(
                        RowKey::Pid(live.pid),
                        place,
                        &live.cwd,
                        Some(live.session_id.as_str()),
                        Some(live),
                    )
                }),
        );
        rows.sort_by(|a, b| {
            a.working_dir
                .cmp(&b.working_dir)
                .then_with(|| key_order(&a.key, &b.key))
        });
        rows
    }

    /// `/clear` の後、パネルの会話の id はフックが知らせるまで古い。同じ場所で id の合わない
    /// この窓のセッションと conductor の Claude が 1 本ずつなら、それを同じものとみなす。
    fn lone_stray<'a>(
        &'a self,
        cwd: &Path,
        by_id: &[Option<&LiveSession>],
    ) -> Option<&'a LiveSession> {
        let orphans = self
            .own
            .iter()
            .zip(by_id)
            .filter(|(own, found)| found.is_none() && own.working_dir == cwd)
            .count();
        let mut strays = self.live.iter().filter(|l| {
            l.owner == Owner::Conductor
                && l.cwd == cwd
                && !by_id.iter().flatten().any(|m| m.session_id == l.session_id)
        });
        match (orphans, strays.next(), strays.next()) {
            (1, Some(stray), None) => Some(stray),
            _ => None,
        }
    }

    fn row(
        &self,
        key: RowKey,
        place: Place,
        working_dir: &Path,
        session_id: Option<&str>,
        live: Option<&LiveSession>,
    ) -> Row {
        let summary = session_id
            .and_then(|id| self.summaries.get(id))
            .map(|(_, s)| s);
        let since = live.and_then(|l| l.status_since);
        let state = match live.map(|l| &l.status) {
            None => State::Unknown,
            Some(Status::Working) => State::Working,
            Some(Status::Waiting(Waiting::Question)) => State::Waiting(Ask::Question),
            Some(Status::Waiting(Waiting::Permission)) => State::Waiting(Ask::Permission),
            Some(Status::Waiting(Waiting::Other(raw))) => State::Waiting(Ask::Other(raw.clone())),
            Some(Status::Other(raw)) => State::Other(raw.clone()),
            Some(Status::Idle) => {
                let seen_after = self
                    .seen
                    .get(&key)
                    .zip(since)
                    .is_some_and(|(seen, since)| *seen >= since);
                if seen_after {
                    State::Idle
                } else {
                    State::Finished
                }
            }
        };
        Row {
            title: summary.and_then(|s| match &s.title {
                Title::Custom(t) | Title::Ai(t) | Title::FirstPrompt(t) => Some(t.clone()),
                Title::None => None,
            }),
            context_tokens: summary.and_then(|s| s.context_tokens),
            key,
            state,
            since,
            place,
            working_dir: working_dir.to_path_buf(),
            pinned: false,
        }
    }

    pub fn update(&mut self, action: Action, ctx: &Ctx) -> Option<Vec<Effect>> {
        if ctx.focus == Focus::FleetSession {
            return (action == Action::LeaveTerminal).then(|| vec![Effect::Focus(Focus::Fleet)]);
        }
        let rows = self.visual_rows();
        let at = rows.iter().position(|r| self.is_selected(r));
        let last = rows.len().saturating_sub(1);
        let target = match action {
            Action::NavigateDown => at.map_or(0, |i| (i + 1).min(last)),
            Action::NavigateUp => at.map_or(0, |i| i.saturating_sub(1)),
            Action::GoToTop => 0,
            Action::GoToBottom => last,
            Action::Select => {
                return Some(
                    self.selected
                        .as_ref()
                        .map(|(key, _)| vec![Effect::EnterSession(key.clone())])
                        .unwrap_or_default(),
                );
            }
            Action::ExitSubPanel => return Some(vec![Effect::LeaveFleet(self.came_from())]),
            Action::ExpandOrRight => return Some(vec![Effect::Focus(Focus::FleetSession)]),
            _ => return None,
        };
        if let Some(row) = rows.get(target) {
            self.select(row, SystemTime::now());
        }
        Some(Vec::new())
    }
}

fn key_order(a: &RowKey, b: &RowKey) -> std::cmp::Ordering {
    match (a, b) {
        (RowKey::Panel(a), RowKey::Panel(b)) => a.cmp(b),
        (RowKey::Pid(a), RowKey::Pid(b)) => a.cmp(b),
        (RowKey::Panel(_), RowKey::Pid(_)) => std::cmp::Ordering::Less,
        (RowKey::Pid(_), RowKey::Panel(_)) => std::cmp::Ordering::Greater,
    }
}

fn dir_name(dir: &Path) -> String {
    dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// linked worktree でも main worktree の名前。git で引けなければディレクトリの名前。
fn repo_name(dir: &Path) -> String {
    conductor_core::git_engine::GitEngine::open(dir)
        .and_then(|git| git.main_worktree_path())
        .map_or_else(|_| dir_name(dir), |main| dir_name(&main))
}

#[cfg(test)]
mod tests;
