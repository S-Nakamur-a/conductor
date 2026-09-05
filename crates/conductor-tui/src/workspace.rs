//! 画面全体の状態。旧 App の代わりだが、パネルの update はここを受け取らない。

use std::path::{Path, PathBuf};

use conductor_core::config::{self, Config};
use conductor_core::keymap::{Action, KeyContext, KeyMap};
use conductor_core::theme::Theme;
use conductor_core::update_checker::UpdateInfo;

use crate::effect::Effect;
use crate::index::Index;
use crate::modal::Modal;
use crate::panels::explorer::{BottomView, ExplorerPanel};
use crate::panels::revidere::RevidereState;
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
    Revidere,
}

impl Focus {
    pub fn is_pty(self) -> bool {
        matches!(
            self,
            Self::TerminalClaude | Self::TerminalShell | Self::Editor
        )
    }

    /// Tab の輪。Revidere は輪に入らず Explorer へ抜ける。Editor は開いている間だけ通る。
    pub fn next(self) -> Self {
        match self {
            Self::Worktree => Self::Explorer,
            Self::Explorer => Self::Viewer,
            Self::Viewer | Self::Editor => Self::TerminalClaude,
            Self::TerminalClaude => Self::TerminalShell,
            Self::TerminalShell | Self::Revidere => Self::Explorer,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Worktree | Self::Explorer | Self::Editor => Self::TerminalShell,
            Self::Viewer => Self::Explorer,
            Self::TerminalClaude => Self::Viewer,
            Self::TerminalShell => Self::TerminalClaude,
            Self::Revidere => Self::Explorer,
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
            Self::Revidere => "Review",
        }
    }

    pub fn key_context(self) -> KeyContext {
        match self {
            Self::Worktree => KeyContext::Worktree,
            Self::Explorer => KeyContext::Explorer,
            Self::Viewer => KeyContext::Viewer,
            Self::TerminalClaude | Self::TerminalShell => KeyContext::Terminal,
            Self::Editor => KeyContext::Editor,
            Self::Revidere => KeyContext::Revidere,
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
    /// 走っているものより新しいリリース。タイトルバーのバッジが読む。
    pub update: Option<UpdateInfo>,
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
    /// リポジトリを開いて名前を決める。
    pub fn open(root: &Path, main_branch: &str) -> anyhow::Result<Self> {
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
            main_branch: main_branch.to_string(),
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
    pub worktree: WorktreePanel,
    pub explorer: ExplorerPanel,
    pub viewer: ViewerPanel,
    pub terminal: TerminalPanel,
    pub revidere: RevidereState,
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
            worktree: WorktreePanel::default(),
            explorer: ExplorerPanel::default(),
            viewer: ViewerPanel::new(&config),
            terminal: TerminalPanel::new(&config),
            revidere: RevidereState::default(),
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
        }
    }

    /// panels と modals を可変で借りたまま [Ctx] を組む。`root` は panels から引くので、
    /// 呼び出し側が先に取り出しておく。
    pub(crate) fn split<'a>(
        &'a mut self,
        root: &'a Path,
    ) -> (&'a mut Panels, &'a mut Vec<Modal>, Ctx<'a>) {
        let key_context = self.key_context();
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
            Focus::Revidere => panels.revidere.update(action, &ctx),
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
            TaskResult::Tree(_) | TaskResult::Diff(_) => self.panels.explorer.apply_result(result),
            TaskResult::FileLoaded { .. } | TaskResult::MediaRendered { .. } => {
                self.panels.viewer.apply_result(result)
            }
            TaskResult::IndexLoaded(_) | TaskResult::SymbolsBuilt(_) => {
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
            TaskResult::Publishable(loaded) => self.accept_publishable(loaded),
            TaskResult::Published(outcome) => published(outcome),
            TaskResult::RevidereLoaded(outcome) => self.panels.revidere.install(*outcome),
            TaskResult::Analyzed { branch, outcome } => {
                let worktree = self.worktree_path();
                let selected = self.branch().to_string();
                self.panels
                    .revidere
                    .finished(&branch, outcome, worktree, &selected)
            }
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
    fn accept_pr_intake(&mut self, outcome: Result<(u64, PathBuf), String>) -> Vec<Effect> {
        match outcome {
            Ok((pr_number, worktree)) => {
                self.panels.worktree.select_when_listed(worktree);
                self.panels.explorer.show(BottomView::Comments);
                let mut effects = vec![Effect::Spawn(crate::task::Task::ListWorktrees)];
                if matches!(self.modals.last(), Some(Modal::PrInput(_))) {
                    effects.push(Effect::PopModal);
                }
                effects.push(Effect::Focus(Focus::Explorer));
                effects.push(Effect::Status(
                    StatusLevel::Success,
                    format!("PR #{pr_number} ready for review."),
                ));
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

    /// 差分の外に出たコメントを落としてから確認を出す。GitHub は 1 件でも
    /// ハンク外が混ざると一括投稿を丸ごと拒む。
    fn accept_publishable(
        &mut self,
        loaded: Result<Box<crate::task::Publishable>, String>,
    ) -> Vec<Effect> {
        let mut request = match loaded {
            Ok(request) => request,
            Err(e) => return vec![Effect::Status(StatusLevel::Warning, e)],
        };
        let total = request.comments.len();
        if total == 0 {
            return vec![Effect::Status(
                StatusLevel::Info,
                "No unpublished comments on this branch.".into(),
            )];
        }
        let (comments, skipped) = conductor_core::review_publish::filter_publishable(
            std::mem::take(&mut request.comments),
            self.panels.explorer.diff(),
        );
        request.comments = comments;
        request.skipped = skipped;
        if request.comments.is_empty() {
            return vec![Effect::Status(
                StatusLevel::Warning,
                format!(
                    "All {skipped} unpublished comment(s) are outside the current diff \u{2014} nothing to publish."
                ),
            )];
        }
        vec![Effect::PushModal(Modal::Publish(
            crate::modal::publish::Publish::new(request),
        ))]
    }

    /// 描く直前に、本文から導かれる重い成果物を整える。
    pub fn prepare(&mut self) -> Vec<Effect> {
        if self.focus == Focus::Revidere {
            self.panels.revidere.prepare(&self.theme, &self.config);
        }
        let effects = self.panels.viewer.prepare(&self.config, &self.theme);
        // Highlighter は初回参照で SyntaxSet を構築する。読んでいない間は触らせない。
        if self.panels.terminal.transcript().is_none() {
            return effects;
        }
        let overlay_open = !self.modals.is_empty();
        let Self {
            panels,
            theme,
            config,
            ..
        } = self;
        let Panels {
            viewer, terminal, ..
        } = panels;
        terminal.prepare(theme, viewer.highlighter(config), overlay_open);
        effects
    }

    /// レイアウトから区画の窓を引き直す。描画より前に呼ぶ。
    pub fn sync_layout(&mut self, layout: &crate::layout::Layout) {
        self.panels.explorer.sync_layout(layout);
        self.panels.viewer.sync_layout(layout);
        self.panels.terminal.sync_sizes(layout);
        self.panels.revidere.sync_layout(layout);
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

#[cfg(test)]
mod tests {
    use super::*;

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
        new.general.main_branch = "trunk".into();

        let effects = ws.adopt_config(&new);
        assert_ne!(ws.config.general.main_branch, "trunk");
        assert!(matches!(
            effects.as_slice(),
            [Effect::Status(StatusLevel::Warning, _)]
        ));
    }
}
