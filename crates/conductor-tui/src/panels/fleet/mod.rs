//! マシン上の全 Claude を 1 枚に並べる一覧。
//!
//! 状態は Claude Code が自分で書く状態ファイルから取り、この窓のセッションとは会話の id で
//! 突き合わせる。この窓のセッションの行はパネルの id で持つ — 会話の id は `/clear` で変わる。

pub mod changes;
pub mod render;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use conductor_core::cc_hook::statusline::Usage;
use conductor_core::claude_log::summary::{Summary, Title};
use conductor_core::claude_sessions::registry::{LiveSession, Owner, Status, Waiting};
use conductor_core::git_engine::{Head, WorktreeHead};
use conductor_core::keymap::Action;

use crate::click::ClickTracker;
use crate::effect::Effect;
use crate::modal::popup::{Choices, Popup};
use crate::modal::{Confirm, Modal};
use crate::workspace::{Ctx, Focus, StatusLevel};

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
    /// statusLine の写しがあるセッションだけ。
    pub context_percent: Option<u8>,
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

/// 一覧の本文で押した 1 点。
pub struct ListPress<'a> {
    pub hit: Option<&'a render::Slot>,
    pub button: Option<&'a render::Slot>,
    pub in_button_columns: bool,
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

/// この窓の PtyStore にあるセッション。消す worktree の中のものを worker が選ぶ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnPty {
    pub panel_id: String,
    pub kind: conductor_svc::pty::SessionKind,
    pub working_dir: PathBuf,
    pub claude_session_id: Option<String>,
}

/// 消す worktree の中で動いている Claude。own はこの窓のパネルのもの (タブを閉じて終わりを待つ)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doomed {
    pub pid: u32,
    pub started: SystemTime,
    pub own: bool,
}

/// worktree を消す前の点検の結果。確認文はこれから作り、消す直前の数え直しもこれと比べる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    pub repo: PathBuf,
    pub path: PathBuf,
    pub label: String,
    pub check: conductor_core::git_engine::DeletionCheck,
    pub claudes: Vec<Doomed>,
    /// この窓で閉じるパネル (Claude とシェル)。
    pub panels: Vec<String>,
    /// panels のうち Claude。状態ファイルをまだ書いていない Claude も数える。
    pub own_claudes: usize,
    pub shells: usize,
    pub in_window_repo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inspected {
    Ready(Box<Inspection>),
    Refused(String),
    Failed(String),
}

/// worker が 1 回で読んだもの。題は会話ログの mtime が変わったものだけ入る。
#[derive(Debug, Default)]
pub struct FleetStatus {
    pub live: Vec<LiveSession>,
    pub summaries: Vec<(String, SystemTime, Summary)>,
    pub trees: Vec<RepoTree>,
    /// セッションの working_dir → それを含む worktree のパス。
    pub placement: HashMap<PathBuf, PathBuf>,
    pub usage: Usage,
}

const OUTSIDE_GROUP: &str = "外の端末";

/// 一覧の左下に映すもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lower {
    #[default]
    Changes,
    Shell,
}

#[derive(Debug, Default)]
pub struct FleetPanel {
    selected: Option<Selected>,
    own: Vec<OwnSession>,
    live: Vec<LiveSession>,
    summaries: HashMap<String, (SystemTime, Summary)>,
    seen: HashMap<RowKey, SystemTime>,
    trees: Vec<RepoTree>,
    placement: HashMap<PathBuf, PathBuf>,
    usage: Usage,
    /// 一度木に出た repo は、セッションが無くなっても窓を閉じるまで出し続ける。そこへ起動できるように。
    kept_roots: BTreeSet<PathBuf>,
    /// Smart Worktree の Prompt を開いた repo。Prompt の on_submit は関数ポインタで掴めない。
    smart_target: Option<PathBuf>,
    /// 消す前の点検を待っている worktree。点検は 1 本だけ。
    inspecting: Option<PathBuf>,
    /// 消すと確定し、結果がまだ届いていない worktree。結果は repo を切り替えても届くので、
    /// [Self::forget_in_flight] では下ろさない。
    deleting: BTreeSet<PathBuf>,
    polling: bool,
    /// gg や押しっぱなしでタブが何枚も開かないように、開き終わるまで次の g を捨てる。
    opening_page: bool,
    came_from: Option<Focus>,
    row_clicks: ClickTracker<(u16, bool, Option<Selected>)>,
    pub changes: changes::BranchChanges,
    lower: Lower,
    /// 窓が低いと左下の区画ごと無い。レイアウトから毎フレーム写す。
    lower_shown: bool,
    /// 左下のシェルに入ったときに映っていたもの。打ち先がこれから変わったら打たせない。
    typing_shell: Option<String>,
}

impl FleetPanel {
    pub fn is_polling(&self) -> bool {
        self.polling
    }

    pub fn note_spawned(&mut self, task: &crate::task::Task) {
        match task {
            crate::task::Task::FleetStatus { .. } => self.polling = true,
            crate::task::Task::FleetChanges { .. } => self.changes.note_fetching(),
            _ => {}
        }
    }

    /// 結果は repo の切り替えで世代ごと捨てられるので、ガードもそこで下ろす。
    pub fn forget_in_flight(&mut self) {
        self.polling = false;
        self.inspecting = None;
        self.opening_page = false;
        self.changes.forget_in_flight();
    }

    /// 会話ログを読み直さなくていいもの。会話の id → 前に読んだときの mtime。
    pub fn known_transcripts(&self) -> HashMap<String, SystemTime> {
        self.summaries
            .iter()
            .map(|(id, (mtime, _))| (id.clone(), *mtime))
            .collect()
    }

    pub fn lower(&self) -> Lower {
        self.lower
    }

    pub fn set_lower_shown(&mut self, shown: bool) {
        self.lower_shown = shown;
    }

    pub fn shows_shell(&self) -> bool {
        self.lower == Lower::Shell && self.lower_shown
    }

    pub fn show_shell(&mut self) {
        self.lower = Lower::Shell;
    }

    pub fn enter_shell(&mut self, id: String) {
        self.typing_shell = Some(id);
    }

    pub fn typing_shell(&self) -> Option<&str> {
        self.typing_shell.as_deref()
    }

    fn show_changes(&mut self) -> Vec<Effect> {
        self.lower = Lower::Changes;
        vec![Effect::Focus(Focus::Fleet)]
    }

    /// 左下にシェルを起こしてよい dir。木に載らない dir に起こすと、元の行が消えた後に辿れなくなる。
    pub fn shell_dir(&self) -> Result<PathBuf, &'static str> {
        if !self.lower_shown {
            return Err("The window is too short to show a shell under the list.");
        }
        let dir = self.selected_dir().ok_or("Select a worktree first.")?;
        if self.tree_root_of(&dir).is_none() {
            return Err("A shell opens only in a git worktree listed here.");
        }
        if self.inspecting.as_deref() == Some(dir.as_path()) {
            return Err("Still checking this worktree to delete it.");
        }
        if self.deleting.contains(&dir) {
            return Err("Still deleting this worktree.");
        }
        // 無い dir でもシェルの起動は失敗せず、すぐ終わるシェルが残る。
        if !dir.is_dir() {
            return Err("This worktree's directory is gone.");
        }
        Ok(dir)
    }

    pub fn note_deleting(&mut self, path: PathBuf) {
        self.deleting.insert(path);
    }

    pub fn note_deleted(&mut self, path: &Path) {
        self.deleting.remove(path);
    }

    pub fn open(&mut self, from: Focus, own: Vec<OwnSession>) {
        if !from.in_fleet() {
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

    pub fn usage(&self) -> &Usage {
        &self.usage
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

    /// 一覧から起こしたセッションを選ぶ。行は次の読み直しを待たずに own から出る。
    pub fn note_launched(&mut self, panel_id: String, own: Vec<OwnSession>) {
        self.set_own(own);
        self.selected = Some(Selected::Session(RowKey::Panel(panel_id), false));
    }

    pub fn set_smart_target(&mut self, repo: PathBuf) {
        self.smart_target = Some(repo);
    }

    pub fn take_smart_target(&mut self) -> Option<PathBuf> {
        self.smart_target.take()
    }

    /// 起動する dir と、その dir が属する git の repo の root。選んだ行の dir、何も無ければ fallback。
    pub fn launch_target(&self, fallback: &Path) -> (PathBuf, Option<PathBuf>) {
        let dir = self
            .selected_dir()
            .unwrap_or_else(|| fallback.to_path_buf());
        let root = self.tree_root_of(&dir);
        (dir, root)
    }

    /// 選んだ worktree、選んだセッションの worktree (外の端末はその cwd)。
    pub fn selected_dir(&self) -> Option<PathBuf> {
        match &self.selected {
            Some(Selected::Worktree(path)) => Some(path.clone()),
            Some(Selected::Session(..)) => self.selected_row().map(|row| {
                self.placement
                    .get(&row.working_dir)
                    .cloned()
                    .unwrap_or(row.working_dir)
            }),
            None => None,
        }
    }

    pub fn note_page_opened(&mut self) {
        self.opening_page = false;
    }

    fn tree_root_of(&self, dir: &Path) -> Option<PathBuf> {
        self.trees
            .iter()
            .find(|t| t.root == dir || t.worktrees.iter().any(|w| w.path == dir))
            .map(|t| t.root.clone())
    }

    pub fn is_tree_root(&self, root: &Path) -> bool {
        self.trees.iter().any(|t| t.root == root)
    }

    /// 一覧の [+] / [✧] を押した。行き先は押した行から取り、選択は動かさない。
    fn click_button(&mut self, slot: &render::Slot, at: (u16, u16)) -> Vec<Effect> {
        match slot {
            render::Slot::Worktree(w) => vec![Effect::PushModal(Modal::Popup(Popup::numbered(
                "New session",
                launch_choices(&w.path, None),
                Some(at),
            )))],
            render::Slot::Repo(Some(root)) => vec![Effect::FleetSmartPrompt(root.clone())],
            render::Slot::Repo(None) | render::Slot::Session(_) | render::Slot::Other => Vec::new(),
        }
    }

    /// 一覧は選んだ行に合わせて送られるので、二度目は同じ画面の行に別の行やボタンが来ている。
    /// 二度目はボタンも含めて引き直さず、一度目に選んだ行がまだ選ばれていればそこへ入る。
    pub fn click_list(&mut self, at: (u16, u16), press: ListPress) -> Vec<Effect> {
        let key = (at.1, press.in_button_columns, self.selected.clone());
        if self.row_clicks.is_double(key) {
            return match self.selected {
                Some(Selected::Session(..)) => vec![Effect::Focus(Focus::FleetSession)],
                _ => Vec::new(),
            };
        }
        if let Some(button) = press.button {
            self.row_clicks = ClickTracker::default();
            return self.click_button(button, at);
        }
        let item = match press.hit {
            Some(render::Slot::Worktree(w)) => Item::Worktree(w.clone()),
            Some(render::Slot::Session(row)) => Item::Session(row.clone()),
            _ => {
                self.row_clicks = ClickTracker::default();
                return Vec::new();
            }
        };
        self.select(&item, SystemTime::now());
        self.row_clicks
            .arm((at.1, press.in_button_columns, self.selected.clone()));
        Vec::new()
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
        self.usage = status.usage;
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
        self.sections().into_iter().flatten().collect()
    }

    fn sections(&self) -> Vec<Vec<Item>> {
        let (pinned, groups) = self.groups();
        let pinned: Vec<Item> = pinned.into_iter().map(Item::Session).collect();
        let repos = groups.into_iter().map(|group| {
            group
                .nodes
                .into_iter()
                .flat_map(|node| {
                    node.worktree
                        .map(Item::Worktree)
                        .into_iter()
                        .chain(node.rows.into_iter().map(Item::Session))
                })
                .collect::<Vec<_>>()
        });
        std::iter::once(pinned)
            .chain(repos)
            .filter(|section| !section.is_empty())
            .collect()
    }

    fn section_starts(&self) -> Vec<usize> {
        self.sections()
            .iter()
            .scan(0, |start, section| {
                let this = *start;
                *start += section.len();
                Some(this)
            })
            .collect()
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
            context_percent: session_id
                .and_then(|id| self.usage.context.get(id))
                .map(|pct| pct.round().clamp(0.0, 100.0) as u8),
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
        if ctx.focus == Focus::FleetShell {
            return match action {
                Action::LeaveTerminal => Some(vec![Effect::Focus(Focus::Fleet)]),
                // シェルの中の ctrl+g は端末層ではファイルを開くキー。左下には開く先が無い。
                Action::OpenFileFromTerminal => Some(self.show_changes()),
                Action::ScrollbackUp
                | Action::ScrollbackDown
                | Action::ScrollbackTop
                | Action::SnapToLive
                | Action::NextSession
                | Action::PrevSession => Some(Vec::new()),
                _ => None,
            };
        }
        let items = self.items();
        let at = items.iter().position(|item| self.is_selected_item(item));
        let last = items.len().saturating_sub(1);
        let target = match action {
            Action::NavigateDown => at.map_or(0, |i| (i + 1).min(last)),
            Action::NavigateUp => at.map_or(0, |i| i.saturating_sub(1)),
            Action::GoToTop => 0,
            Action::GoToBottom => last,
            Action::NextRepo => {
                let starts = self.section_starts();
                at.and_then(|i| starts.into_iter().find(|&s| s > i))
                    .or(at)
                    .unwrap_or(0)
            }
            Action::PrevRepo => {
                let starts = self.section_starts();
                at.and_then(|i| starts.into_iter().rev().find(|&s| s < i))
                    .unwrap_or(0)
            }
            Action::GoToWaiting => {
                let waiting = items
                    .iter()
                    .position(|item| matches!(item, Item::Session(row) if row.pinned));
                let Some(i) = waiting else {
                    return Some(vec![Effect::Status(
                        StatusLevel::Info,
                        "No session is waiting for input.".to_string(),
                    )]);
                };
                i
            }
            Action::Select => {
                return Some(match &self.selected {
                    Some(Selected::Session(key, _)) => vec![Effect::EnterSession(key.clone())],
                    _ => Vec::new(),
                });
            }
            Action::ExitSubPanel => return Some(vec![Effect::LeaveFleet(self.came_from())]),
            Action::ExpandOrRight => return Some(vec![Effect::Focus(Focus::FleetSession)]),
            Action::StopSession => {
                return Some(match self.selected_worktree() {
                    Some(worktree) => vec![self.ask_delete(worktree)],
                    None => self.ask_stop().into_iter().collect(),
                });
            }
            Action::OpenBranchPage => return Some(self.open_page().into_iter().collect()),
            Action::NewShell => return Some(vec![Effect::OpenFleetShell]),
            Action::ShowFleetChanges => return Some(self.show_changes()),
            Action::NewSessionMenu => {
                let (dir, root) = self.launch_target(&ctx.repo.root);
                return Some(vec![Effect::PushModal(Modal::Popup(Popup::numbered(
                    "New session",
                    launch_choices(&dir, root),
                    None,
                )))]);
            }
            _ => return None,
        };
        if let Some(item) = items.get(target) {
            self.select(item, SystemTime::now());
        }
        Some(Vec::new())
    }

    /// 消せるかと失うものは worker が調べる。結果は [Self::inspected] に届く。
    fn ask_delete(&mut self, worktree: Worktree) -> Effect {
        let warn = |text: &str| Effect::Status(StatusLevel::Warning, text.to_string());
        if self.inspecting.is_some() {
            return warn("Still checking the worktree to delete.");
        }
        let Some(repo) = self.tree_root_of(&worktree.path) else {
            return warn("This line is not a git worktree.");
        };
        if repo == worktree.path {
            return warn("The main worktree cannot be deleted.");
        }
        self.inspecting = Some(worktree.path.clone());
        Effect::InspectFleetWorktree {
            repo,
            path: worktree.path,
            label: worktree.label,
        }
    }

    /// 押したときのまま一覧にいて同じ worktree を選んでいるときだけ確認を出す。点検の間に
    /// 右の区画へ入って打ち始めた文の y が、後から出た確認に答えてしまわないように。
    pub fn inspected(&mut self, path: &Path, outcome: Inspected, undisturbed: bool) -> Vec<Effect> {
        if self.inspecting.as_deref() != Some(path) {
            return Vec::new();
        }
        self.inspecting = None;
        let still = undisturbed && self.selected == Some(Selected::Worktree(path.to_path_buf()));
        vec![match outcome {
            Inspected::Refused(why) => Effect::Status(StatusLevel::Warning, why),
            Inspected::Failed(why) => Effect::Status(StatusLevel::Error, why),
            Inspected::Ready(_) if !still => Effect::Status(
                StatusLevel::Info,
                "The selection moved, so nothing was deleted. Press D again.".into(),
            ),
            Inspected::Ready(inspection) => {
                Effect::PushModal(Modal::Confirm(render::delete_confirm(*inspection)))
            }
        }]
    }

    fn open_page(&mut self) -> Option<Effect> {
        if self.opening_page {
            return None;
        }
        let dir = self.selected_dir()?;
        self.opening_page = true;
        Some(Effect::Spawn(crate::task::Task::OpenBranchPage { dir }))
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

/// Smart Worktree は repo の root が分かるときだけ並べる。
pub fn launch_choices(dir: &Path, smart: Option<PathBuf>) -> Choices {
    let mut choices: Choices = vec![
        (
            "Claude Code",
            Effect::NewClaudeIn {
                dir: dir.to_path_buf(),
                chrome: false,
            },
        ),
        (
            "Claude Code + Chrome",
            Effect::NewClaudeIn {
                dir: dir.to_path_buf(),
                chrome: true,
            },
        ),
    ];
    choices.extend(smart.map(|root| ("Smart Worktree…", Effect::FleetSmartPrompt(root))));
    choices
}

/// HEAD のブランチ名で gh に PR を聞いた答え。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrLookup {
    Found(String),
    Missing,
    Failed(String),
}

/// 開く URL と、ステータスに出す文。`pr` が None なのは HEAD がブランチでなく、gh に聞いていないとき。
pub fn branch_page(
    head: &Head,
    pr: Option<PrLookup>,
    repo: Option<String>,
) -> Result<(String, String), String> {
    let branch = head.label();
    if let Some(PrLookup::Found(url)) = pr {
        return Ok((url, format!("Opened the pull request for '{branch}'.")));
    }
    let reason = match (pr, head) {
        (Some(PrLookup::Missing), _) => format!("No pull request for '{branch}'"),
        (Some(PrLookup::Failed(why)), _) => format!("Could not ask gh for a pull request ({why})"),
        (_, Head::Unborn(_)) => format!("No commit on '{branch}' yet"),
        _ => format!("Not on a branch {branch}"),
    };
    let repo = repo.ok_or_else(|| format!("{reason}, and origin has no web URL to open."))?;
    Ok((repo, format!("{reason}; opened the repository.")))
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
