//! 画面全体の状態。旧 App の代わりだが、パネルの update はここを受け取らない。

use std::path::{Path, PathBuf};

use conductor_core::config::Config;
use conductor_core::keymap::{Action, KeyContext, KeyMap};
use conductor_core::theme::Theme;

use crate::effect::Effect;
use crate::modal::Modal;
use crate::panels::explorer::ExplorerPanel;
use crate::panels::terminal::TerminalPanel;
use crate::panels::viewer::ViewerPanel;
use crate::panels::worktree::WorktreePanel;
use crate::review::ReviewState;
use crate::task::{TaskEnv, TaskResult};

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
}

/// 読み取り専用の環境。パネルの update と render の両方に渡す。
pub struct Ctx<'a> {
    pub theme: &'a Theme,
    pub keymap: &'a KeyMap,
    pub config: &'a Config,
    pub repo: &'a RepoState,
    pub review: &'a ReviewState,
    /// 相対パスと検索範囲の基準。Viewer が持つ根と同じ。
    pub root: &'a Path,
    /// 1 つのパネルが 2 つの区画を持つことがあるので、どちらが受けたかを添える。
    pub focus: Focus,
    /// フォーカス中の区画とモードで決まる層。キーの案内とスコープが読む。
    pub key_context: KeyContext,
}

pub struct Workspace {
    pub repo: RepoState,
    pub focus: Focus,
    pub panels: Panels,
    pub modals: Vec<Modal>,
    pub review: ReviewState,
    pub chrome: Chrome,
    pub should_quit: bool,
    pub theme: Theme,
    pub appearance: Appearance,
    pub keymap: KeyMap,
    pub config: Config,
}

impl Workspace {
    pub fn new(repo: RepoState, config: Config, keymap: KeyMap, theme: Theme) -> Self {
        let panels = Panels {
            worktree: WorktreePanel::default(),
            explorer: ExplorerPanel::default(),
            viewer: ViewerPanel::new(&config),
            terminal: TerminalPanel::new(&config),
        };
        Self {
            repo,
            focus: Focus::Explorer,
            panels,
            modals: Vec::new(),
            review: ReviewState::default(),
            chrome: Chrome::default(),
            should_quit: false,
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
            keymap: &self.keymap,
            config: &self.config,
            repo: &self.repo,
            review: &self.review,
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
            main_branch: self.repo.main_branch.clone(),
            worktree_dir: self.config.general.worktree_dir.clone(),
            word_diff: self.config.diff.word_diff,
            tab_width: self.config.viewer.tab_width,
            branch: self.branch().to_string(),
        }
    }

    /// Action をフォーカス中のパネルへ渡す。消費しなければ `None` で、
    /// 呼び出し側は [crate::route::global_effects] の既定の解釈に落とす。
    pub fn dispatch(&mut self, action: Action) -> Option<Vec<Effect>> {
        self.dispatch_to(self.focus, action)
    }

    /// フォーカスの外にあるパネルへ渡す。コマンドの宛先が選択で決まるとき用。
    pub fn dispatch_to(&mut self, target: Focus, action: Action) -> Option<Vec<Effect>> {
        let key_context = self.key_context();
        let root = self.panels.viewer.root().to_path_buf();
        let Self {
            focus,
            panels,
            theme,
            keymap,
            config,
            repo,
            review,
            ..
        } = self;
        let ctx = Ctx {
            theme,
            keymap,
            config,
            repo,
            review,
            root: &root,
            focus: *focus,
            key_context,
        };
        match target {
            Focus::Worktree => panels.worktree.update(action, &ctx),
            Focus::Explorer => panels.explorer.update(action, &ctx),
            Focus::Viewer => panels.viewer.update(action, &ctx),
            Focus::TerminalClaude | Focus::TerminalShell => panels.terminal.update(action, &ctx),
            _ => None,
        }
    }

    pub fn tick_top_modal(&mut self) -> Vec<Effect> {
        let key_context = self.key_context();
        let root = self.panels.viewer.root().to_path_buf();
        let Self {
            modals,
            theme,
            keymap,
            config,
            repo,
            review,
            focus,
            ..
        } = self;
        let ctx = Ctx {
            theme,
            keymap,
            config,
            repo,
            review,
            root: &root,
            focus: *focus,
            key_context,
        };
        modals
            .last_mut()
            .map(|top| top.tick(&ctx))
            .unwrap_or_default()
    }

    /// svc から届いた結果を持ち主のパネルへ渡す。[Self::dispatch] と同じ理由でここに置く。
    pub fn accept(&mut self, result: TaskResult) -> Vec<Effect> {
        match result {
            TaskResult::Tree(_) | TaskResult::Diff(_) => self.panels.explorer.apply_result(result),
            TaskResult::FileLoaded { .. } => self.panels.viewer.apply_result(result),
            TaskResult::Grep { .. } | TaskResult::Sessions(_) | TaskResult::History { .. } => {
                self.accept_in_modal(result)
            }
            TaskResult::Review(loaded) => {
                self.review.install(loaded.map(|s| *s));
                match self.review.error.clone() {
                    Some(e) => vec![Effect::Status(StatusLevel::Warning, e)],
                    None => Vec::new(),
                }
            }
            _ => {
                let key_context = self.key_context();
                let root = self.panels.viewer.root().to_path_buf();
                let Self {
                    focus,
                    panels,
                    theme,
                    keymap,
                    config,
                    repo,
                    review,
                    ..
                } = self;
                let ctx = Ctx {
                    theme,
                    keymap,
                    config,
                    repo,
                    review,
                    root: &root,
                    focus: *focus,
                    key_context,
                };
                panels.worktree.apply_result(result, &ctx)
            }
        }
    }

    /// 頼んだモーダルがまだ開いていれば届ける。閉じたあとの結果は捨てる。
    fn accept_in_modal(&mut self, result: TaskResult) -> Vec<Effect> {
        let Some(modal) = self.modals.last_mut() else {
            return Vec::new();
        };
        match (modal, result) {
            (Modal::Grep(grep), TaskResult::Grep { seq, found }) => grep.install(seq, found),
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
                },
            ) => {
                vec![Effect::Status(StatusLevel::Error, e)]
            }
            _ => Vec::new(),
        }
    }

    /// レイアウトから区画の窓を引き直す。描画より前に呼ぶ。
    pub fn sync_layout(&mut self, layout: &crate::layout::Layout) {
        self.panels.explorer.sync_layout(layout);
        self.panels.viewer.sync_layout(layout);
        self.panels.terminal.sync_sizes(layout);
        let modal = crate::render::comment_list_rect(layout.area);
        for open in &mut self.modals {
            if let Modal::CommentList(list) = open {
                list.set_viewport(crate::list::Viewport::inside(modal, 0));
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        let repo = RepoState {
            root: PathBuf::from("/tmp/repo"),
            name: "repo".into(),
            main_branch: "main".into(),
            known: vec![PathBuf::from("/tmp/repo")],
        };
        let (keymap, _) = KeyMap::with_warnings(&toml::Table::new());
        Self::new(repo, Config::default(), keymap, Theme::default())
    }
}
