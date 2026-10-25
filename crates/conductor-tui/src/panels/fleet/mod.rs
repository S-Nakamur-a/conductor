//! マシン上の全 Claude を 1 枚に並べる一覧。
//!
//! 状態は Claude Code が自分で書く状態ファイルから取り、この窓のセッションとは会話の id で
//! 突き合わせる。この窓のセッションの行はパネルの id で持つ — 会話の id は `/clear` で変わる。

pub mod render;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use conductor_core::claude_log::summary::{Summary, Title};
use conductor_core::claude_sessions::registry::{LiveSession, Owner, Status, Waiting};
use conductor_core::git_engine::WorktreeHead;
use conductor_core::keymap::Action;

use crate::effect::Effect;
use crate::modal::{Confirm, Modal};
use crate::workspace::{Ctx, Focus};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowKey {
    Panel(String),
    Pid(u32),
}

/// 外のプロセスは、一覧に載せたときの起動時刻で同じものかを確かめてから止める。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopTarget {
    Panel(String),
    Process { pid: u32, started: SystemTime },
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

/// repo の見出し 1 つ分。外の端末は root が無く、worktree の段も無い。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub root: Option<PathBuf>,
    pub nodes: Vec<Node>,
}

impl Group {
    pub fn rows(&self) -> Vec<&Row> {
        self.nodes.iter().flat_map(|n| &n.rows).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub worktree: Option<Worktree>,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub label: String,
}

/// 一覧で j / k が止まるもの。並びは描画と同じ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Worktree(Worktree),
    Session(Row),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Selected {
    Worktree(PathBuf),
    /// 同じ行が入力待ちの欄と repo の欄の両方にいるので、どちらかも持つ。
    Session(RowKey, bool),
}

/// worker が引いた repo 1 つ分。worktree は main が先頭、残りはラベル順。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoTree {
    pub root: PathBuf,
    pub name: String,
    pub worktrees: Vec<WorktreeHead>,
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
    pub trees: Vec<RepoTree>,
    /// セッションの working_dir → それを含む worktree のパス。
    pub placement: HashMap<PathBuf, PathBuf>,
}

const OUTSIDE_GROUP: &str = "外の端末";

#[derive(Debug, Default)]
pub struct FleetPanel {
    selected: Option<Selected>,
    own: Vec<OwnSession>,
    live: Vec<LiveSession>,
    summaries: HashMap<String, (SystemTime, Summary)>,
    seen: HashMap<RowKey, SystemTime>,
    trees: Vec<RepoTree>,
    placement: HashMap<PathBuf, PathBuf>,
    /// 一度木に出た repo は、セッションが無くなっても窓を閉じるまで出し続ける。そこへ起動できるように。
    kept_roots: BTreeSet<PathBuf>,
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

    /// セッションが無くても木を引き続ける repo。
    pub fn kept_roots(&self) -> Vec<PathBuf> {
        self.kept_roots.iter().cloned().collect()
    }

    pub fn came_from(&self) -> Focus {
        self.came_from.unwrap_or(Focus::Explorer)
    }

    /// この窓のセッションを止めた直後に、読み直しを待たずに行を消す。組んでいた状態ファイルの
    /// 写しも捨てないと、読み直すまで同じ Claude が pid の行で出る。
    pub fn forget_stopped(&mut self, panel_id: &str, own: Vec<OwnSession>) {
        let paired = self
            .own
            .iter()
            .zip(self.pairing())
            .find(|(own, _)| own.panel_id == panel_id)
            .and_then(|(_, live)| live.map(|l| l.session_id.clone()));
        if let Some(session) = paired {
            self.live.retain(|l| l.session_id != session);
        }
        self.set_own(own);
        self.reselect_if_gone();
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
        self.kept_roots
            .extend(status.trees.iter().map(|t| t.root.clone()));
        self.trees = status.trees;
        self.placement = status.placement;
        self.set_own(own);
        if listing {
            self.mark_selected(now);
        } else if let Some(panel) = visible {
            self.seen.insert(RowKey::Panel(panel.to_string()), now);
        }
        self.reselect_if_gone();
    }

    fn set_own(&mut self, own: Vec<OwnSession>) {
        self.own = own;
    }

    /// 選び直しは人が選んだものではないので、見たことにはしない。同じセッションが入力待ちの欄と
    /// repo の欄を行き来しただけなら同じセッションに留まる — 右で返事をしている最中に動かさない。
    fn reselect_if_gone(&mut self) {
        let items = self.items();
        if items.iter().any(|item| self.is_selected_item(item)) {
            return;
        }
        let sessions = || {
            items.iter().filter_map(|item| match item {
                Item::Session(row) => Some(row),
                Item::Worktree(_) => None,
            })
        };
        let same_session = match &self.selected {
            Some(Selected::Session(key, _)) => sessions().find(|r| r.key == *key),
            _ => None,
        };
        self.selected = match same_session.or_else(|| sessions().next()) {
            Some(row) => Some(Selected::Session(row.key.clone(), row.pinned)),
            None => items.iter().find_map(|item| match item {
                Item::Worktree(w) => Some(Selected::Worktree(w.path.clone())),
                Item::Session(_) => None,
            }),
        };
    }

    fn select(&mut self, item: &Item, now: SystemTime) {
        self.selected = Some(match item {
            Item::Worktree(w) => Selected::Worktree(w.path.clone()),
            Item::Session(row) => Selected::Session(row.key.clone(), row.pinned),
        });
        self.mark_selected(now);
    }

    fn mark_selected(&mut self, now: SystemTime) {
        if let Some(Selected::Session(key, _)) = &self.selected {
            self.seen.insert(key.clone(), now);
        }
    }

    pub fn is_selected(&self, row: &Row) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|s| *s == Selected::Session(row.key.clone(), row.pinned))
    }

    pub fn is_selected_worktree(&self, worktree: &Worktree) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|s| *s == Selected::Worktree(worktree.path.clone()))
    }

    fn is_selected_item(&self, item: &Item) -> bool {
        match item {
            Item::Worktree(w) => self.is_selected_worktree(w),
            Item::Session(row) => self.is_selected(row),
        }
    }

    pub fn selected_worktree(&self) -> Option<Worktree> {
        self.items().into_iter().find_map(|item| match item {
            Item::Worktree(w) if self.is_selected_worktree(&w) => Some(w),
            _ => None,
        })
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.visual_rows().into_iter().find(|r| self.is_selected(r))
    }

    /// 入力待ちを一番上に重ね、残りは repo → worktree の木。外の端末は最後。
    ///
    /// worker がまだ置き場を返していない dir (起動した直後など) と git の外の dir は、その dir
    /// 自身を repo とも worktree ともみなす。
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
        let mut groups: Vec<Group> = self
            .trees
            .iter()
            .map(|tree| Group {
                name: tree.name.clone(),
                root: Some(tree.root.clone()),
                nodes: tree
                    .worktrees
                    .iter()
                    .map(|w| Node {
                        worktree: Some(Worktree {
                            path: w.path.clone(),
                            label: w.head.label(),
                        }),
                        rows: Vec::new(),
                    })
                    .collect(),
            })
            .collect();
        let mut outside = Vec::new();
        for row in rows {
            if row.place == Place::Outside {
                outside.push(row);
                continue;
            }
            let placed = self.placement.get(&row.working_dir).and_then(|path| {
                groups
                    .iter_mut()
                    .flat_map(|g| &mut g.nodes)
                    .find(|n| n.worktree.as_ref().is_some_and(|w| w.path == *path))
            });
            if let Some(node) = placed {
                node.rows.push(row);
                continue;
            }
            match groups
                .iter_mut()
                .find(|g| g.root.as_deref() == Some(row.working_dir.as_path()))
            {
                Some(group) => group.nodes[0].rows.push(row),
                None => {
                    let name = dir_name(&row.working_dir);
                    groups.push(Group {
                        root: Some(row.working_dir.clone()),
                        nodes: vec![Node {
                            worktree: Some(Worktree {
                                path: row.working_dir.clone(),
                                label: name.clone(),
                            }),
                            rows: vec![row],
                        }],
                        name,
                    });
                }
            }
        }
        groups.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.root.cmp(&b.root)));
        if !outside.is_empty() {
            groups.push(Group {
                name: OUTSIDE_GROUP.to_string(),
                root: None,
                nodes: vec![Node {
                    worktree: None,
                    rows: outside,
                }],
            });
        }
        (pinned, groups)
    }

    pub fn group_of(&self, row: &Row) -> String {
        let (_, groups) = self.groups();
        groups
            .iter()
            .find(|g| g.rows().iter().any(|r| r.key == row.key))
            .map_or_else(|| dir_name(&row.working_dir), |g| g.name.clone())
    }

    pub fn items(&self) -> Vec<Item> {
        let (pinned, groups) = self.groups();
        let mut items: Vec<Item> = pinned.into_iter().map(Item::Session).collect();
        for node in groups.into_iter().flat_map(|g| g.nodes) {
            items.extend(node.worktree.map(Item::Worktree));
            items.extend(node.rows.into_iter().map(Item::Session));
        }
        items
    }

    pub fn visual_rows(&self) -> Vec<Row> {
        self.items()
            .into_iter()
            .filter_map(|item| match item {
                Item::Session(row) => Some(row),
                Item::Worktree(_) => None,
            })
            .collect()
    }

    /// own と同じ並びで、それぞれと組になった状態ファイルの写し。
    fn pairing(&self) -> Vec<Option<&LiveSession>> {
        let by_id: Vec<Option<&LiveSession>> = self
            .own
            .iter()
            .map(|own| {
                own.claude_session_id
                    .as_deref()
                    .and_then(|id| self.live.iter().find(|l| l.session_id == id))
            })
            .collect();
        self.own
            .iter()
            .zip(&by_id)
            .map(|(own, found)| found.or_else(|| self.lone_stray(&own.working_dir, &by_id)))
            .collect()
    }

    fn rows(&self) -> Vec<Row> {
        let paired = self.pairing();
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
        let items = self.items();
        let at = items.iter().position(|item| self.is_selected_item(item));
        let last = items.len().saturating_sub(1);
        let target = match action {
            Action::NavigateDown => at.map_or(0, |i| (i + 1).min(last)),
            Action::NavigateUp => at.map_or(0, |i| i.saturating_sub(1)),
            Action::GoToTop => 0,
            Action::GoToBottom => last,
            Action::Select => {
                return Some(match &self.selected {
                    Some(Selected::Session(key, _)) => vec![Effect::EnterSession(key.clone())],
                    _ => Vec::new(),
                });
            }
            Action::ExitSubPanel => return Some(vec![Effect::LeaveFleet(self.came_from())]),
            Action::ExpandOrRight => return Some(vec![Effect::Focus(Focus::FleetSession)]),
            Action::StopSession => return Some(self.ask_stop().into_iter().collect()),
            _ => return None,
        };
        if let Some(item) = items.get(target) {
            self.select(item, SystemTime::now());
        }
        Some(Vec::new())
    }

    /// 止める相手は押した時点の行で決める。確認の間に並びが変わっても相手は動かない。
    fn ask_stop(&self) -> Option<Effect> {
        let row = self.selected_row()?;
        let target = match &row.key {
            RowKey::Panel(id) => StopTarget::Panel(id.clone()),
            RowKey::Pid(pid) => StopTarget::Process {
                pid: *pid,
                started: self.live.iter().find(|l| l.pid == *pid)?.started,
            },
        };
        let confirm = render::stop_detail(&row).into_iter().fold(
            Confirm::yes_only(
                render::stop_question(&row),
                vec![Effect::StopFleetSession(target)],
            )
            .title("Stop session"),
            Confirm::detail,
        );
        Some(Effect::PushModal(Modal::Confirm(confirm)))
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

#[cfg(test)]
mod tests;
