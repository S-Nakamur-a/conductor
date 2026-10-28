//! 画面全体の状態。旧 App の代わりだが、パネルの update はここを受け取らない。

use std::path::{Path, PathBuf};
use std::time::Instant;

use conductor_core::config::{self, Config};
use conductor_core::keymap::{Action, KeyContext, KeyMap};
use conductor_core::pr_intake::IntakenPr;
use conductor_core::theme::Theme;
use conductor_core::update_checker::UpdateInfo;

use crate::effect::Effect;
use crate::index::Index;
use crate::modal::Modal;
use crate::panels::explorer::{BottomView, ExplorerPanel};
use crate::panels::terminal::TerminalPanel;
use crate::panels::viewer::ViewerPanel;
use crate::panels::worktree::WorktreePanel;
use crate::review::ReviewState;
use crate::task::{Task, TaskEnv, TaskResult, UpdateCheck, UpdateStage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Focus {
    Worktree,
    Explorer,
    Viewer,
    TerminalClaude,
    TerminalShell,
    Editor,
    /// マシン上の全 Claude の一覧。
    Fleet,
    /// 一覧の右に映した、この窓のセッション。
    FleetSession,
    /// 一覧の左下に映した、選んだ worktree のシェル。
    FleetShell,
}

impl Focus {
    pub fn is_pty(self) -> bool {
        matches!(
            self,
            Self::TerminalClaude
                | Self::TerminalShell
                | Self::Editor
                | Self::FleetSession
                | Self::FleetShell
        )
    }

    pub fn in_fleet(self) -> bool {
        matches!(self, Self::Fleet | Self::FleetSession | Self::FleetShell)
    }

    /// Tab の輪。Editor は開いている間だけ通る。
    pub fn next(self) -> Self {
        match self {
            Self::Worktree => Self::Explorer,
            Self::Explorer => Self::Viewer,
            Self::Viewer | Self::Editor => Self::TerminalClaude,
            Self::TerminalClaude => Self::TerminalShell,
            Self::TerminalShell | Self::Fleet | Self::FleetSession | Self::FleetShell => {
                Self::Explorer
            }
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Worktree | Self::Explorer | Self::Editor => Self::TerminalShell,
            Self::Viewer => Self::Explorer,
            Self::TerminalClaude => Self::Viewer,
            Self::TerminalShell => Self::TerminalClaude,
            Self::Fleet | Self::FleetSession | Self::FleetShell => Self::Explorer,
        }
    }

    /// パレットのスコープ見出しに出す名前。
    pub fn label(self) -> &'static str {
        match self {
            Self::Worktree => "Worktree",
            Self::Explorer => "Explorer",
            Self::Viewer => "Viewer",
            Self::TerminalClaude => "Claude Code",
            Self::TerminalShell => "Shell",
            Self::Editor => "Editor",
            Self::Fleet => "Sessions",
            Self::FleetSession => "Session",
            Self::FleetShell => "Shell",
        }
    }

    pub fn key_context(self) -> KeyContext {
        match self {
            Self::Worktree => KeyContext::Worktree,
            Self::Fleet => KeyContext::Fleet,
            Self::Explorer => KeyContext::Explorer,
            Self::Viewer => KeyContext::Viewer,
            Self::TerminalClaude | Self::TerminalShell | Self::FleetSession | Self::FleetShell => {
                KeyContext::Terminal
            }
            Self::Editor => KeyContext::Editor,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    Success,
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub level: StatusLevel,
    pub text: String,
    pub shown_at: std::time::Instant,
}

/// パネルの外側にある全幅の行と、その状態。
#[derive(Debug, Default)]
pub struct Chrome {
    pub status: Option<StatusMessage>,
    pub menu: crate::menu::MenuBar,
    pub maximized: bool,
    /// つかんでいる境界。離すまで持つ。
    pub drag: Option<crate::layout::Divider>,
    /// マウスで選んでいる区画内の範囲。次のクリック・スクロール・キーで消える。
    pub selection: Option<crate::select::Selection>,
    /// 離した選択をクリップボードへ渡す番。文字は描いた画面から拾うので、次の描画の後に済ませる。
    pub copy_requested: bool,
    /// 走っているものより新しいリリース。タイトルバーのバッジが読む。
    pub update: Option<UpdateInfo>,
    /// 一覧が main を占めている。開いているかはフォーカスから導かない — 今の画面の
    /// どこかがフォーカスを動かしても、一覧から落ちないように。
    pub fleet_open: bool,
    /// 端末が OSC 11 で答えた背景色。答えない端末 (tmux の中を含む) では None。
    pub terminal_bg: Option<ratatui::style::Color>,
}

#[derive(Debug, Clone)]
pub struct RepoState {
    pub root: PathBuf,
    /// main worktree のディレクトリ名。linked worktree から開いてもリポジトリの名前。
    pub name: String,
    pub main_branch: String,
    /// 切り替えられるリポジトリ。今開いているものも含む。
    pub known: Vec<PathBuf>,
}

impl RepoState {
    /// リポジトリを開いて名前と既定ブランチを決める。
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        let git = conductor_core::git_engine::GitEngine::open(root)?;
        let dir_name = |path: &Path| {
            path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        let name = git
            .main_worktree_path()
            .map_or_else(|_| dir_name(root), |main| dir_name(&main));
        Ok(Self {
            root: root.to_path_buf(),
            name,
            main_branch: git.default_branch(),
            known: vec![root.to_path_buf()],
        })
    }

    pub fn known_index(&self) -> usize {
        self.known.iter().position(|p| *p == self.root).unwrap_or(0)
    }

    /// 開いたリポジトリを一覧に入れる。設定で並べたものは順番を保つ。
    pub fn remember(&mut self, path: &Path) {
        if !self.known.iter().any(|p| p == path) {
            self.known.push(path.to_path_buf());
        }
    }
}

/// [Workspace::theme] を組み立てる元。テーマ切替と高コントラストの両方がここから作る。
#[derive(Debug, Clone, Default)]
pub struct Appearance {
    pub name: String,
    pub high_contrast: bool,
}

impl Appearance {
    pub fn build(&self) -> Theme {
        let theme = Theme::from_name(&self.name);
        if self.high_contrast {
            theme.high_contrast()
        } else {
            theme
        }
    }
}

/// パネルの状態はここに 1 つずつ。
pub struct Panels {
    pub fleet: crate::panels::fleet::FleetPanel,
    pub worktree: WorktreePanel,
    pub explorer: ExplorerPanel,
    pub viewer: ViewerPanel,
    pub terminal: TerminalPanel,
}

/// 読み取り専用の環境。パネルの update と render の両方に渡す。
pub struct Ctx<'a> {
    pub theme: &'a Theme,
    pub version: &'static str,
    pub keymap: &'a KeyMap,
    pub config: &'a Config,
    pub repo: &'a RepoState,
    pub review: &'a ReviewState,
    /// 問い合わせるだけなので共有で足りる。
    pub index: &'a Index,
    /// 相対パスと検索範囲の基準。Viewer が持つ根と同じ。
    pub root: &'a Path,
    /// 1 つのパネルが 2 つの区画を持つことがあるので、どちらが受けたかを添える。
    pub focus: Focus,
    /// フォーカス中の区画とモードで決まる層。キーの案内とスコープが読む。
    pub key_context: KeyContext,
    /// 一覧の右に映っているセッション ([Workspace::fleet_session])。
    pub fleet_session: Option<String>,
}

pub struct Workspace {
    pub repo: RepoState,
    pub version: &'static str,
    pub focus: Focus,
    pub panels: Panels,
    pub modals: Vec<Modal>,
    pub review: ReviewState,
    pub chrome: Chrome,
    pub fx: crate::fx::Fx,
    pub should_quit: bool,
    /// 更新を入れ終えた。抜けたあと main が同じ引数で自分を exec し直す。
    pub relaunch: bool,
    pub index: Index,
    /// まだ前回のセッションを探しに行っていない。worktree の一覧が要るので、
    /// 起動時ではなくそれが最初に届いたときに落とす。
    pending_auto_resume: bool,
    pub theme: Theme,
    pub appearance: Appearance,
    pub keymap: KeyMap,
    pub config: Config,
    review_requests: ReviewRequests,
}

/// repo を切り替えても持ち越す。他の repo で消した worktree を作り直さないため。
#[derive(Default)]
struct ReviewRequests {
    in_flight: bool,
    /// 依頼が続いている間、もう取り込まない (main worktree, PR 番号)。試したものと worktree を見たもの。
    tried: std::collections::HashSet<(PathBuf, u64)>,
}

impl Workspace {
    pub fn new(
        repo: RepoState,
        config: Config,
        keymap: KeyMap,
        theme: Theme,
        version: &'static str,
    ) -> Self {
        let panels = Panels {
            fleet: crate::panels::fleet::FleetPanel::default(),
            worktree: WorktreePanel::default(),
            explorer: ExplorerPanel::default(),
            viewer: ViewerPanel::new(&config),
            terminal: TerminalPanel::new(&config),
        };
        let mut fx = crate::fx::Fx::default();
        if config.ui.startup_animation {
            fx.play(crate::fx::Kind::assemble(), crate::fx::Target::Panels);
        }
        Self {
            repo,
            version,
            focus: Focus::Explorer,
            panels,
            modals: Vec::new(),
            review: ReviewState::default(),
            chrome: Chrome::default(),
            fx,
            should_quit: false,
            relaunch: false,
            index: Index::default(),
            pending_auto_resume: config.general.auto_resume,
            appearance: Appearance {
                name: theme.name.to_string(),
                high_contrast: config.ui.high_contrast,
            },
            theme,
            keymap,
            config,
            review_requests: ReviewRequests::default(),
        }
    }

    pub fn ctx(&self) -> Ctx<'_> {
        Ctx {
            theme: &self.theme,
            version: self.version,
            keymap: &self.keymap,
            config: &self.config,
            repo: &self.repo,
            review: &self.review,
            index: &self.index,
            root: self.panels.viewer.root(),
            focus: self.focus,
            key_context: self.key_context(),
            fleet_session: self.fleet_session(),
        }
    }

    /// 1 つのパネルが 2 つの層を持つことがあるので、区画やモードは持ち主に訊く。
    pub fn key_context(&self) -> KeyContext {
        match self.focus {
            Focus::Explorer => self.panels.explorer.key_context(),
            Focus::Viewer => self.panels.viewer.key_context(),
            focus => focus.key_context(),
        }
    }

    /// 選択中の worktree。一覧が届くまではリポジトリの根。
    pub fn worktree_path(&self) -> PathBuf {
        self.panels
            .worktree
            .selected()
            .map_or_else(|| self.repo.root.clone(), |w| w.path.clone())
    }

    /// 今のブランチ。worktree 一覧が届くまでは設定の main ブランチ。
    pub fn branch(&self) -> &str {
        self.panels
            .worktree
            .selected()
            .map_or(self.repo.main_branch.as_str(), |w| w.branch.as_str())
    }

    pub fn task_env(&self) -> TaskEnv {
        TaskEnv {
            root: self.repo.root.clone(),
            version: self.version,
            main_branch: self.repo.main_branch.clone(),
            worktree_dir: self.config.general.worktree_dir.clone(),
            word_diff: self.config.diff.word_diff,
            tab_width: self.config.viewer.tab_width,
            branch: self.branch().to_string(),
            claude_dirs: self.config.claude_dirs(),
        }
    }

    /// panels と modals を可変で借りたまま [Ctx] を組む。`root` は panels から引くので、
    /// 呼び出し側が先に取り出しておく。
    pub(crate) fn split<'a>(
        &'a mut self,
        root: &'a Path,
    ) -> (&'a mut Panels, &'a mut Vec<Modal>, Ctx<'a>) {
        let key_context = self.key_context();
        let fleet_session = self.fleet_session();
        let Self {
            panels,
            modals,
            version,
            theme,
            keymap,
            config,
            repo,
            review,
            index,
            focus,
            ..
        } = self;
        let ctx = Ctx {
            theme,
            version,
            keymap,
            config,
            repo,
            review,
            index,
            root,
            focus: *focus,
            key_context,
            fleet_session,
        };
        (panels, modals, ctx)
    }

    /// Action をフォーカス中のパネルへ渡す。消費しなければ `None` で、
    /// 呼び出し側は [crate::route::global_effects] の既定の解釈に落とす。
    pub fn dispatch(&mut self, action: Action) -> Option<Vec<Effect>> {
        self.dispatch_to(self.focus, action)
    }

    /// フォーカスの外にあるパネルへ渡す。コマンドの宛先が選択で決まるとき用。
    pub fn dispatch_to(&mut self, target: Focus, action: Action) -> Option<Vec<Effect>> {
        let root = self.panels.viewer.root().to_path_buf();
        let (panels, _, ctx) = self.split(&root);
        match target {
            Focus::Worktree => panels.worktree.update(action, &ctx),
            Focus::Explorer => panels.explorer.update(action, &ctx),
            Focus::Viewer => panels.viewer.update(action, &ctx),
            Focus::TerminalClaude | Focus::TerminalShell => panels.terminal.update(action, &ctx),
            Focus::Fleet | Focus::FleetSession => panels
                .terminal
                .update(action, &ctx)
                .or_else(|| panels.fleet.update(action, &ctx)),
            // 端末パネルの読みビューとスクロールは右の Claude のもの。左下のシェルには無い。
            Focus::FleetShell => panels.fleet.update(action, &ctx),
            _ => None,
        }
    }

    pub fn tick_top_modal(&mut self) -> Vec<Effect> {
        let root = self.panels.viewer.root().to_path_buf();
        let (_, modals, ctx) = self.split(&root);
        modals
            .last_mut()
            .map(|top| top.tick(&ctx))
            .unwrap_or_default()
    }

    /// ホバーの締切を 1 押しする。
    pub fn tick_viewer(&mut self) -> bool {
        let root = self.panels.viewer.root().to_path_buf();
        let (panels, _, ctx) = self.split(&root);
        panels.viewer.tick_hover(&ctx)
    }

    /// svc から届いた結果を持ち主のパネルへ渡す。[Self::dispatch] と同じ理由でここに置く。
    pub fn accept(&mut self, result: TaskResult) -> Vec<Effect> {
        match result {
            TaskResult::FleetStatus(status) => {
                let own = self.panels.terminal.own_claude_sessions();
                self.apply_fleet(*status, own);
                Vec::new()
            }
            TaskResult::SessionStopped(stopped) => vec![stopped_status(stopped)],
            TaskResult::FleetWorktreeInspected { path, outcome } => {
                let undisturbed = self.focus == Focus::Fleet && self.modals.is_empty();
                self.panels.fleet.inspected(&path, outcome, undisturbed)
            }
            TaskResult::Reopened { reopen, ended } => {
                self.panels.fleet.reopened(&reopen.session_id);
                vec![if ended {
                    Effect::LaunchReopened(reopen)
                } else {
                    Effect::Status(
                        StatusLevel::Warning,
                        format!(
                            "The session did not end within {}s: once it has, resume it from the Resume list as {}",
                            crate::task::fleet_delete::OWN_WAIT.as_secs(),
                            reopen.account
                        ),
                    )
                }]
            }
            TaskResult::FleetWorktreeDeleted {
                path,
                in_window_repo,
                outcome,
            } => {
                self.panels.fleet.note_deleted(&path);
                let mut effects = vec![match outcome {
                    Ok(done) => Effect::Status(StatusLevel::Success, done),
                    Err(why) => Effect::Status(StatusLevel::Error, why),
                }];
                if in_window_repo {
                    effects.push(Effect::Spawn(Task::ListWorktrees));
                }
                effects
            }
            TaskResult::FleetChanges { dir, diff, status } => {
                let fleet = &mut self.panels.fleet;
                let target = fleet.selected_dir();
                fleet
                    .changes
                    .install(dir, *diff, status, target.as_deref(), Instant::now());
                Vec::new()
            }
            TaskResult::BranchPageOpened(opened) => {
                self.panels.fleet.note_page_opened();
                vec![match opened {
                    Ok(message) => Effect::Status(StatusLevel::Info, message),
                    Err(error) => Effect::Status(StatusLevel::Error, error),
                }]
            }
            TaskResult::Tree(_)
            | TaskResult::FileIndex { .. }
            | TaskResult::GitStatus { .. }
            | TaskResult::Diff { .. }
            | TaskResult::HeadLog { .. }
            | TaskResult::CommitOverview(_) => self.panels.explorer.apply_result(result),
            TaskResult::FileLoaded { .. } | TaskResult::MediaRendered { .. } => {
                self.panels.viewer.apply_result(result)
            }
            TaskResult::IndexLoaded(_) => {
                crate::index::accept(self, result);
                Vec::new()
            }
            TaskResult::Transcript {
                session_id,
                entries,
            } => self
                .panels
                .terminal
                .install_transcript(&session_id, entries),
            TaskResult::Grep { .. }
            | TaskResult::Sessions(_)
            | TaskResult::History { .. }
            | TaskResult::RemoteBranches(_)
            | TaskResult::Commits(_) => self.accept_in_modal(result),
            TaskResult::UpdateCheck { outcome, announce } => {
                self.accept_update_check(outcome, announce)
            }
            TaskResult::UpdateProgress(stage) => self.accept_update_progress(stage),
            TaskResult::PrIntake(outcome) => self.accept_pr_intake(outcome),
            TaskResult::ReviewRound {
                round,
                in_window_repo,
            } => self.accept_review_round(*round, in_window_repo),
            TaskResult::Publishable(loaded) => self.accept_publishable(loaded),
            TaskResult::Published(outcome) => published(outcome),
            TaskResult::Review(loaded) => {
                self.review.install(loaded.map(|s| *s));
                match self.review.error.clone() {
                    Some(e) => vec![Effect::Status(StatusLevel::Warning, e)],
                    None => Vec::new(),
                }
            }
            TaskResult::Resumable {
                sessions,
                main_grabbed,
            } => crate::resume::accept(self, sessions, main_grabbed),
            result @ TaskResult::Worktrees(Ok(_)) => {
                let mut effects = self.in_worktree_panel(result);
                effects.extend(self.start_auto_resume());
                effects
            }
            _ => self.in_worktree_panel(result),
        }
    }

    fn in_worktree_panel(&mut self, result: TaskResult) -> Vec<Effect> {
        let root = self.panels.viewer.root().to_path_buf();
        let (panels, _, ctx) = self.split(&root);
        panels.worktree.apply_result(result, &ctx)
    }

    fn start_auto_resume(&mut self) -> Vec<Effect> {
        if !self.pending_auto_resume {
            return Vec::new();
        }
        let paths: Vec<PathBuf> = self
            .panels
            .worktree
            .list()
            .iter()
            .map(|w| w.path.clone())
            .collect();
        if paths.is_empty() {
            return Vec::new();
        }
        self.pending_auto_resume = false;
        vec![Effect::Spawn(Task::FindResumable { paths })]
    }

    fn accept_update_check(&mut self, outcome: UpdateCheck, announce: bool) -> Vec<Effect> {
        let (level, message) = match outcome {
            UpdateCheck::Newer(info) => {
                let text = format!(
                    "Update available: v{} \u{2014} run \u{201c}App: Update and Restart\u{201d}",
                    info.latest_version
                );
                self.chrome.update = Some(*info);
                (StatusLevel::Success, text)
            }
            UpdateCheck::UpToDate => {
                self.chrome.update = None;
                (
                    StatusLevel::Info,
                    format!("Already up to date (v{}).", self.version),
                )
            }
            // 届かなかっただけ。すでに出ているバッジは消さない。
            UpdateCheck::Unreachable => (
                StatusLevel::Warning,
                String::from("Update check failed \u{2014} could not reach GitHub"),
            ),
        };
        if !announce {
            return Vec::new();
        }
        vec![Effect::Status(level, message)]
    }

    /// 差し替えの報告は開いているモーダルへ。閉じられていても再起動と失敗は届ける。
    fn accept_update_progress(&mut self, stage: UpdateStage) -> Vec<Effect> {
        self.relaunch |= matches!(stage, UpdateStage::Installed);
        match self.modals.last_mut() {
            Some(Modal::Update(modal)) => modal.accept(stage).unwrap_or_default(),
            _ => match stage {
                UpdateStage::Installed => vec![Effect::Quit],
                UpdateStage::Failed(reason) => vec![Effect::Status(StatusLevel::Error, reason)],
                UpdateStage::Step(_) => Vec::new(),
            },
        }
    }

    /// 頼んだモーダルがまだ開いていれば届ける。閉じたあとの結果は捨てる。
    fn accept_in_modal(&mut self, result: TaskResult) -> Vec<Effect> {
        let Some(modal) = self.modals.last_mut() else {
            return Vec::new();
        };
        match (modal, result) {
            (Modal::Grep(grep), TaskResult::Grep { seq, found }) => grep.install(seq, found),
            (Modal::BranchPicker(picker), TaskResult::RemoteBranches(Ok(branches))) => {
                picker.install(branches);
                Vec::new()
            }
            (Modal::CherryPick(picker), TaskResult::Commits(Ok(commits))) => {
                picker.install(commits);
                Vec::new()
            }
            (Modal::Resume(picker), TaskResult::Sessions(Ok(sessions))) => {
                picker.install(sessions);
                Vec::new()
            }
            (Modal::History(browser), TaskResult::History { saved, records }) => {
                let mut effects = Vec::new();
                match records {
                    Ok(records) => browser.install(records),
                    Err(e) => effects.push(Effect::Status(StatusLevel::Error, e)),
                }
                if saved {
                    effects.push(Effect::Status(
                        StatusLevel::Success,
                        "saved the terminal output".into(),
                    ));
                }
                effects
            }
            (
                _,
                TaskResult::Sessions(Err(e))
                | TaskResult::History {
                    records: Err(e), ..
                }
                | TaskResult::RemoteBranches(Err(e))
                | TaskResult::Commits(Err(e)),
            ) => {
                vec![Effect::Status(StatusLevel::Error, e)]
            }
            _ => Vec::new(),
        }
    }

    /// 取り込みは閉じたあとでも効かせる。gh も fetch も済んでいるので捨てない。
    /// 失敗のときだけ入力へ戻し、打ち直さずに直せるようにする。
    fn accept_pr_intake(&mut self, outcome: Result<Box<IntakenPr>, String>) -> Vec<Effect> {
        match outcome {
            Ok(pr) => {
                let IntakenPr {
                    pr_number,
                    worktree_path,
                    pulled,
                    ..
                } = *pr;
                self.panels
                    .worktree
                    .select_when_listed(worktree_path.clone());
                self.panels.explorer.show(BottomView::Comments);
                let mut effects = vec![Effect::Spawn(crate::task::Task::ListWorktrees)];
                if matches!(self.modals.last(), Some(Modal::PrInput(_))) {
                    effects.push(Effect::PopModal);
                }
                effects.push(Effect::Focus(Focus::Explorer));
                if let Some(pulled) = pulled {
                    effects.push(Effect::Status(
                        StatusLevel::Info,
                        format!("PR #{pr_number}: {pulled}"),
                    ));
                }
                effects
            }
            Err(e) => match self.modals.last_mut() {
                Some(Modal::PrInput(prompt)) => {
                    prompt.failed(e);
                    Vec::new()
                }
                _ => vec![Effect::Status(StatusLevel::Error, e)],
            },
        }
    }

    /// 前の問い合わせが返るまで次を投げない。取り込みは checkout を伴い、1 分で返る保証が無い。
    pub fn ask_review_requests(&mut self) -> Option<crate::task::Task> {
        let repos = &self.config.review.auto_review_repos;
        if repos.is_empty() || self.review_requests.in_flight {
            return None;
        }
        self.review_requests.in_flight = true;
        Some(crate::task::Task::ReviewRequests {
            repos: repos.clone(),
            tried: self.review_requests.tried.clone(),
            window_root: self.repo.root.clone(),
            worktree_dir: self.config.general.worktree_dir.clone(),
        })
    }

    /// 頼まれて取り込んだ PR では、見ている worktree から引き剥がさないよう選択もフォーカスも動かさない。
    /// Status は 1 枠なので、1 周で起きたことを 1 つの文にまとめる。
    fn accept_review_round(
        &mut self,
        round: conductor_core::pr_intake::ReviewRound,
        in_window_repo: bool,
    ) -> Vec<Effect> {
        self.review_requests.in_flight = false;
        let tried = &mut self.review_requests.tried;
        for queried in round.queried {
            tried.retain(|(repo, number)| *repo != queried.main || queried.listed.contains(number));
            tried.extend(
                queried
                    .with_worktree
                    .into_iter()
                    .map(|number| (queried.main.clone(), number)),
            );
        }
        let mut effects = Vec::new();
        let mut parts = Vec::new();
        let mut failed = !round.failures.is_empty();
        if let Some(intake) = round.intaken {
            let name = intake.repo.file_name().map_or_else(
                || intake.repo.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let number = intake.number;
            self.review_requests.tried.insert((intake.repo, number));
            match intake.outcome {
                Ok(pr) => {
                    parts.push(format!(
                        "Review requested on {name} PR #{number}: checked out '{}'. Run /revidere there.",
                        pr.meta.branch
                    ));
                    if in_window_repo {
                        effects.push(Effect::Spawn(crate::task::Task::ListWorktrees));
                    }
                }
                Err(e) => {
                    failed = true;
                    parts.push(format!("Review requested on {name} PR #{number}: {e}"));
                }
            }
        }
        parts.extend(
            round
                .failures
                .into_iter()
                .map(|f| format!("review requests: {f}")),
        );
        if !parts.is_empty() {
            let level = if failed {
                StatusLevel::Warning
            } else {
                StatusLevel::Info
            };
            effects.push(Effect::Status(level, parts.join(" \u{b7} ")));
        }
        effects
    }

    fn accept_publishable(
        &mut self,
        loaded: Result<Box<crate::task::Publishable>, String>,
    ) -> Vec<Effect> {
        let request = match loaded {
            Ok(request) => request,
            Err(e) => return vec![Effect::Status(StatusLevel::Warning, e)],
        };
        if request.comments.is_empty() {
            return vec![match request.skipped {
                0 => Effect::Status(
                    StatusLevel::Info,
                    "No unpublished comments on this branch.".into(),
                ),
                skipped => Effect::Status(
                    StatusLevel::Warning,
                    format!(
                        "All {skipped} unpublished comment(s) are outside the current diff \u{2014} nothing to publish."
                    ),
                ),
            }];
        }
        vec![Effect::PushModal(Modal::Publish(
            crate::modal::publish::Publish::new(request),
        ))]
    }

    /// 一覧に隠れている間も、その下の今の画面のフォーカス。
    pub fn screen_focus(&self) -> Focus {
        if self.chrome.fleet_open {
            self.panels.fleet.came_from()
        } else {
            self.focus
        }
    }

    /// 一覧の上では一覧を出たときの行き先にする。打っている先が見えない区画に変わるので。
    pub fn set_screen_focus(&mut self, focus: Focus) {
        if self.chrome.fleet_open {
            self.panels.fleet.defer_focus(focus);
        } else {
            self.focus = focus;
        }
    }

    /// 打っていたセッションの行が消えて選び直したら、選び直した先へは打たせない。
    pub(crate) fn apply_fleet(
        &mut self,
        status: crate::panels::fleet::FleetStatus,
        own: Vec<crate::panels::fleet::OwnSession>,
    ) {
        let typing_into = self.fleet_session();
        let visible = self.panels.terminal.visible_claude().map(str::to_string);
        self.panels.fleet.apply(
            status,
            own,
            visible.as_deref(),
            self.chrome.fleet_open,
            std::time::SystemTime::now(),
        );
        let shown = self.fleet_session();
        if self.focus == Focus::FleetSession && (shown.is_none() || shown != typing_into) {
            self.focus = Focus::Fleet;
        }
    }

    /// 一覧の右に映っている、この窓のセッション。
    pub fn fleet_session(&self) -> Option<String> {
        if !self.chrome.fleet_open {
            return None;
        }
        match self.panels.fleet.selected_row()?.key {
            crate::panels::fleet::RowKey::Panel(id) => {
                self.panels.terminal.has_session(&id).then_some(id)
            }
            crate::panels::fleet::RowKey::Pid(_) => None,
        }
    }

    /// 一覧の左下に映っている、選んだ worktree のシェル。
    pub fn fleet_shell(&self) -> Option<String> {
        let fleet = &self.panels.fleet;
        if !self.chrome.fleet_open || !fleet.shows_shell() {
            return None;
        }
        self.panels.terminal.live_shell_in(&fleet.selected_dir()?)
    }

    /// キーと貼り付けを受け取るセッション。
    pub fn pty_session(&self) -> Option<String> {
        match self.focus {
            Focus::TerminalClaude | Focus::TerminalShell | Focus::Editor => {
                self.panels.terminal.session_in(self.focus)
            }
            Focus::FleetSession => self.fleet_session(),
            Focus::FleetShell => self
                .fleet_shell()
                .filter(|id| self.panels.fleet.typing_shell() == Some(id.as_str())),
            Focus::Worktree | Focus::Explorer | Focus::Viewer | Focus::Fleet => None,
        }
    }

    /// 描く直前に、本文から導かれる重い成果物を整える。
    pub fn prepare(&mut self) -> Vec<Effect> {
        let effects = self.panels.viewer.prepare(&self.config, &self.theme);
        // Highlighter は初回参照で SyntaxSet を構築する。読んでいない間は触らせない。
        if !self.panels.terminal.has_transcript() {
            return effects;
        }
        let overlay_open = !self.modals.is_empty();
        let fleet_session = self.fleet_session();
        let Self {
            panels,
            theme,
            config,
            ..
        } = self;
        let Panels {
            viewer, terminal, ..
        } = panels;
        terminal.prepare(
            theme,
            viewer.highlighter(config),
            overlay_open,
            fleet_session.as_deref(),
        );
        effects
    }

    /// レイアウトから区画の窓を引き直す。描画より前に呼ぶ。
    pub fn sync_layout(&mut self, layout: &crate::layout::Layout) {
        self.panels.explorer.sync_layout(layout);
        self.panels.viewer.sync_layout(layout);
        let lower = layout.rect(crate::layout::Region::FleetChanges);
        self.panels.fleet.set_lower_shown(lower.is_some());
        // 打ち先が消えた・入れ替わった・見えなくなったときは、次のシェルへ打たせず一覧へ戻す。
        if self.focus == Focus::FleetShell && self.pty_session().is_none() {
            self.focus = Focus::Fleet;
        }
        let fleet_session = self.fleet_session();
        let fleet_shell = self.fleet_shell();
        self.panels
            .terminal
            .sync_sizes(layout, fleet_session.as_deref(), fleet_shell.as_deref());
        if let Some(rect) = lower {
            self.panels
                .fleet
                .changes
                .set_viewport(crate::list::Viewport::inside(rect, 0));
        }
        let modal = crate::render::comment_list_rect(layout.area);
        for open in &mut self.modals {
            if let Modal::CommentList(list) = open {
                list.set_viewport(crate::list::Viewport::inside(modal, 0));
            }
        }
    }

    /// 設定ファイルを読み直して外観を入れ替える。
    pub fn reload_config(&mut self) -> Vec<Effect> {
        // アトミック保存は remove を挟むので、無い瞬間に読むと Config::load が
        // 既定のファイルを書き出して編集中の内容を潰す。
        if !config::config_file_path().exists() {
            return Vec::new();
        }
        match Config::load() {
            Ok(new) => self.adopt_config(&new),
            Err(e) => vec![Effect::Status(
                StatusLevel::Error,
                format!("Config error \u{2014} kept previous settings: {e}"),
            )],
        }
    }

    /// 差が無ければ何もしない。テーマピッカー自身の書き戻しもここへ届くため。
    fn adopt_config(&mut self, new: &Config) -> Vec<Effect> {
        let appearance_changed = new.appearance_snapshot() != self.config.appearance_snapshot();
        let restart_changed = config::has_restart_changes(&self.config, new);
        let mut effects = Vec::new();
        if restart_changed {
            effects.push(Effect::Status(
                StatusLevel::Warning,
                "Config updated \u{2014} some changes require a restart to take effect".into(),
            ));
        }
        if appearance_changed {
            self.config.adopt_appearance(new);
            self.appearance.name = self.config.theme_name().to_string();
            self.appearance.high_contrast = self.config.ui.high_contrast;
            self.theme = self.appearance.build();
            if !restart_changed {
                effects.push(Effect::Status(StatusLevel::Info, "Config reloaded".into()));
            }
        }
        effects
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::for_test_with(Config::default())
    }

    #[cfg(test)]
    pub(crate) fn for_test_with(mut config: Config) -> Self {
        let repo = RepoState {
            root: PathBuf::from("/tmp/repo"),
            name: "repo".into(),
            main_branch: "main".into(),
            known: vec![PathBuf::from("/tmp/repo")],
        };
        let (keymap, _) = KeyMap::with_warnings(&toml::Table::new());
        // 起動演出は画面を伏せるので、描画とフレームの理由を見るテストでは切っておく。
        config.ui.startup_animation = false;
        Self::new(repo, config, keymap, Theme::default(), "0.0.0-test")
    }
}

/// 投稿の結果。通ったものはワーカーが既に published にしているので、
/// 見えている件数を合わせるためにレビューを読み直す。
fn published(outcome: conductor_core::review_publish::PublishOutcome) -> Vec<Effect> {
    use conductor_core::review_publish::PublishOutcome;
    let (level, message) = match outcome {
        PublishOutcome::Succeeded { published_ids } => (
            StatusLevel::Success,
            format!("Published {} comment(s) to GitHub.", published_ids.len()),
        ),
        PublishOutcome::PartialFailure {
            published_ids,
            failed,
        } => {
            for (id, error) in &failed {
                log::warn!("failed to publish comment {id}: {error}");
            }
            (
                StatusLevel::Warning,
                format!(
                    "Published {} comment(s); {} failed \u{2014} see the log.",
                    published_ids.len(),
                    failed.len()
                ),
            )
        }
        PublishOutcome::Failed { error } => (
            StatusLevel::Error,
            format!("Failed to publish comments: {error}"),
        ),
    };
    vec![
        Effect::Status(level, message),
        Effect::Spawn(crate::task::Task::LoadReview),
    ]
}

fn stopped_status(stopped: conductor_core::claude_sessions::registry::Stopped) -> Effect {
    use conductor_core::claude_sessions::registry::Stopped;
    let (level, message) = match stopped {
        Stopped::Terminated => (StatusLevel::Success, "Stopped the session.".to_string()),
        Stopped::Killed => (
            StatusLevel::Success,
            "Stopped the session (it ignored SIGTERM, so it was killed).".to_string(),
        ),
        Stopped::AlreadyGone => (
            StatusLevel::Info,
            "That session had already ended.".to_string(),
        ),
        Stopped::Failed(error) => (
            StatusLevel::Error,
            format!("Could not stop the session: {error}"),
        ),
    };
    Effect::Status(level, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::pr_intake::{FetchedPr, RequestedIntake, ReviewRound};

    fn intake(repo: &str, number: u64, outcome: Result<(), &str>) -> ReviewRound {
        let outcome = outcome.map_err(String::from).map(|()| IntakenPr {
            pr_number: number,
            worktree_path: PathBuf::from(repo).join("feature-a"),
            meta: FetchedPr {
                branch: String::from("feature/a"),
                title: String::new(),
                base_ref: String::from("main"),
                head_ref: String::from("feature/a"),
                url: String::new(),
                head_owner_login: None,
            },
            pulled: None,
        });
        ReviewRound {
            intaken: Some(RequestedIntake {
                repo: PathBuf::from(repo),
                number,
                outcome,
            }),
            queried: Vec::new(),
            failures: Vec::new(),
        }
    }

    fn statuses(effects: &[Effect]) -> Vec<(StatusLevel, String)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Status(level, text) => Some((*level, text.clone())),
                _ => None,
            })
            .collect()
    }

    fn tried_of(task: Option<crate::task::Task>) -> std::collections::HashSet<(PathBuf, u64)> {
        match task {
            Some(crate::task::Task::ReviewRequests { tried, .. }) => tried,
            other => panic!("問い合わせない: {other:?}"),
        }
    }

    #[test]
    fn 設定が空ならレビュー依頼を問い合わせない() {
        let mut ws = Workspace::for_test();
        assert!(ws.ask_review_requests().is_none());
    }

    #[test]
    fn 問い合わせが返るまで次を投げない() {
        let mut ws = Workspace::for_test();
        ws.config.review.auto_review_repos = vec!["/r".into()];
        assert!(ws.ask_review_requests().is_some());
        assert!(ws.ask_review_requests().is_none(), "投げ中に次を投げた");

        ws.accept(TaskResult::ReviewRound {
            round: Box::default(),
            in_window_repo: false,
        });
        assert!(
            ws.ask_review_requests().is_some(),
            "返ってもガードが下りない"
        );
    }

    #[test]
    fn 取り込んだprは選択もフォーカスも動かさずstatusで知らせる() {
        let mut ws = Workspace::for_test();
        ws.config.review.auto_review_repos = vec!["/r/webshop".into()];
        ws.modals.push(Modal::PrInput(Default::default()));
        let focus = ws.focus;
        ws.ask_review_requests();

        let effects = ws.accept(TaskResult::ReviewRound {
            round: Box::new(intake("/r/webshop", 7, Ok(()))),
            in_window_repo: false,
        });
        assert_eq!(
            statuses(&effects),
            [(
                StatusLevel::Info,
                String::from(
                    "Review requested on webshop PR #7: checked out 'feature/a'. Run /revidere there."
                )
            )]
        );
        assert_eq!(effects.len(), 1, "{effects:?}");
        assert_eq!(ws.focus, focus);
        assert!(matches!(ws.modals.last(), Some(Modal::PrInput(_))));
        assert!(
            tried_of(ws.ask_review_requests()).contains(&(PathBuf::from("/r/webshop"), 7)),
            "取り込んだ PR を次の周でも試す"
        );
    }

    #[test]
    fn 窓のリポジトリへ取り込んだら一覧を取り直す() {
        let mut ws = Workspace::for_test();
        let effects = ws.accept(TaskResult::ReviewRound {
            round: Box::new(intake("/r", 7, Ok(()))),
            in_window_repo: true,
        });
        assert!(matches!(
            effects.first(),
            Some(Effect::Spawn(crate::task::Task::ListWorktrees))
        ));
    }

    #[test]
    fn 取り込めなかったprも次の周で試し直さない() {
        let mut ws = Workspace::for_test();
        ws.config.review.auto_review_repos = vec!["/r".into()];
        ws.ask_review_requests();
        let effects = ws.accept(TaskResult::ReviewRound {
            round: Box::new(intake("/r", 7, Err("network down"))),
            in_window_repo: false,
        });
        assert_eq!(statuses(&effects)[0].0, StatusLevel::Warning);
        assert!(tried_of(ws.ask_review_requests()).contains(&(PathBuf::from("/r"), 7)));
    }

    #[test]
    fn 依頼から外れたprは忘れworktreeを見たprは覚える() {
        let mut ws = Workspace::for_test();
        ws.config.review.auto_review_repos = vec!["/r".into(), "/s".into()];
        for repo in ["/r", "/s"] {
            ws.accept(TaskResult::ReviewRound {
                round: Box::new(intake(repo, 7, Ok(()))),
                in_window_repo: false,
            });
        }
        let round = ReviewRound {
            queried: vec![conductor_core::pr_intake::QueriedRepo {
                main: PathBuf::from("/r"),
                listed: vec![8, 9],
                with_worktree: vec![8],
            }],
            ..Default::default()
        };
        ws.accept(TaskResult::ReviewRound {
            round: Box::new(round),
            in_window_repo: false,
        });
        let tried = tried_of(ws.ask_review_requests());
        assert_eq!(
            tried,
            std::collections::HashSet::from([(PathBuf::from("/r"), 8), (PathBuf::from("/s"), 7),]),
            "外れた /r の 7 を忘れ、別 repo の /s の 7 は残し、worktree のある 8 を覚える"
        );
    }

    #[test]
    fn 取り込みと別リポジトリの失敗は1つのstatusにまとめる() {
        let mut ws = Workspace::for_test();
        let mut round = intake("/r/webshop", 7, Ok(()));
        round.failures.push(String::from("/r/other: gh auth login"));
        let effects = ws.accept(TaskResult::ReviewRound {
            round: Box::new(round),
            in_window_repo: false,
        });
        let shown = statuses(&effects);
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert_eq!(shown[0].0, StatusLevel::Warning);
        assert!(shown[0].1.contains("PR #7"), "{}", shown[0].1);
        assert!(
            shown[0].1.contains("/r/other: gh auth login"),
            "{}",
            shown[0].1
        );
    }

    #[test]
    fn repoを切り替えても問い合わせの結果は届きガードが下りる() {
        let repo = crate::testing::TestRepo::new();
        let other = crate::testing::TestRepo::new();
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        ws.config.review.auto_review_repos = vec!["/nonexistent/repo".into()];
        let task = ws.ask_review_requests().expect("問い合わせない");
        crate::effect::apply(&mut ws, &mut svc, vec![Effect::Spawn(task)]);
        crate::effect::apply(&mut ws, &mut svc, vec![Effect::SwitchRepo(other.root())]);
        crate::testing::pump(&mut ws, &mut svc);
        assert!(
            ws.ask_review_requests().is_some(),
            "結果が捨てられてガードが上がったまま"
        );
    }

    #[test]
    fn 設定に差が無ければ何も起きない() {
        let mut ws = Workspace::for_test();
        let same = ws.config.clone();
        assert!(ws.adopt_config(&same).is_empty());
    }

    #[test]
    fn テーマの変更はその場で効く() {
        let mut ws = Workspace::for_test();
        let mut new = ws.config.clone();
        new.ui.theme = Some("nord".into());

        let effects = ws.adopt_config(&new);
        assert_eq!(ws.theme.name, "nord");
        assert_eq!(ws.appearance.name, "nord");
        assert!(matches!(
            effects.as_slice(),
            [Effect::Status(StatusLevel::Info, _)]
        ));
    }

    #[test]
    fn 再起動が要る設定は警告だけで写さない() {
        let mut ws = Workspace::for_test();
        let mut new = ws.config.clone();
        new.general.shell = "/bin/fish".into();

        let effects = ws.adopt_config(&new);
        assert_ne!(ws.config.general.shell, "/bin/fish");
        assert!(matches!(
            effects.as_slice(),
            [Effect::Status(StatusLevel::Warning, _)]
        ));
    }
}
