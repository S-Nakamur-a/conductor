//! 画面全体の状態。旧 App の代わりだが、パネルの update はここを受け取らない。

use std::path::PathBuf;

use conductor_core::config::Config;
use conductor_core::keymap::{Action, KeyContext, KeyMap};
use conductor_core::theme::Theme;

use crate::effect::Effect;
use crate::modal::Modal;
use crate::panels::terminal::TerminalPanel;
use crate::panels::worktree::WorktreePanel;
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
    pub menu_open: bool,
    pub maximized: bool,
}

#[derive(Debug, Clone)]
pub struct RepoState {
    pub root: PathBuf,
    /// main worktree のディレクトリ名。linked worktree から開いてもリポジトリの名前。
    pub name: String,
    pub main_branch: String,
}

/// パネルの状態はここに 1 つずつ。
pub struct Panels {
    pub worktree: WorktreePanel,
    pub terminal: TerminalPanel,
}

/// 読み取り専用の環境。パネルの update と render の両方に渡す。
pub struct Ctx<'a> {
    pub theme: &'a Theme,
    pub keymap: &'a KeyMap,
    pub config: &'a Config,
    pub repo: &'a RepoState,
    /// 1 つのパネルが 2 つの区画を持つことがあるので、どちらが受けたかを添える。
    pub focus: Focus,
}

pub struct Workspace {
    pub repo: RepoState,
    pub focus: Focus,
    pub panels: Panels,
    pub modals: Vec<Modal>,
    pub chrome: Chrome,
    pub should_quit: bool,
    pub theme: Theme,
    pub keymap: KeyMap,
    pub config: Config,
}

impl Workspace {
    pub fn new(repo: RepoState, config: Config, keymap: KeyMap, theme: Theme) -> Self {
        let panels = Panels {
            worktree: WorktreePanel::default(),
            terminal: TerminalPanel::new(&config),
        };
        Self {
            repo,
            focus: Focus::Explorer,
            panels,
            modals: Vec::new(),
            chrome: Chrome::default(),
            should_quit: false,
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
            focus: self.focus,
        }
    }

    pub fn key_context(&self) -> KeyContext {
        self.focus.key_context()
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
        }
    }

    /// Action をフォーカス中のパネルへ渡す。消費しなければ `None` で、
    /// 呼び出し側は [crate::route::global_effects] の既定の解釈に落とす。
    pub fn dispatch(&mut self, action: Action) -> Option<Vec<Effect>> {
        let Self {
            focus,
            panels,
            theme,
            keymap,
            config,
            repo,
            ..
        } = self;
        let ctx = Ctx {
            theme,
            keymap,
            config,
            repo,
            focus: *focus,
        };
        match focus {
            Focus::Worktree => panels.worktree.update(action, &ctx),
            Focus::TerminalClaude | Focus::TerminalShell => panels.terminal.update(action, &ctx),
            _ => None,
        }
    }

    /// svc から届いた結果をパネルへ渡す。[Self::dispatch] と同じ理由でここに置く。
    pub fn accept(&mut self, result: TaskResult) -> Vec<Effect> {
        let Self {
            focus,
            panels,
            theme,
            keymap,
            config,
            repo,
            ..
        } = self;
        let ctx = Ctx {
            theme,
            keymap,
            config,
            repo,
            focus: *focus,
        };
        panels.worktree.apply_result(result, &ctx)
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        let repo = RepoState {
            root: PathBuf::from("/tmp/repo"),
            name: "repo".into(),
            main_branch: "main".into(),
        };
        let (keymap, _) = KeyMap::with_warnings(&toml::Table::new());
        Self::new(repo, Config::default(), keymap, Theme::default())
    }
}
