//! 画面全体の状態。旧 App の代わりだが、パネルの update はここを受け取らない。

use std::path::PathBuf;

use conductor_core::config::Config;
use conductor_core::keymap::{KeyContext, KeyMap};
use conductor_core::theme::Theme;

use crate::modal::Modal;

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
    /// root の HEAD ブランチ。worktree 一覧が入るフェーズ 2 で選択中のものに変わる。
    pub branch: String,
    pub main_branch: String,
}

/// パネルの状態はここに 1 つずつ。フェーズ 2 以降で中身が入る。
#[derive(Debug, Default)]
pub struct Panels {}

/// 読み取り専用の環境。パネルの update と render の両方に渡す。
pub struct Ctx<'a> {
    pub theme: &'a Theme,
    pub keymap: &'a KeyMap,
    pub config: &'a Config,
    pub repo: &'a RepoState,
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
        Self {
            repo,
            focus: Focus::Explorer,
            panels: Panels::default(),
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
        }
    }

    pub fn key_context(&self) -> KeyContext {
        self.focus.key_context()
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        let repo = RepoState {
            root: PathBuf::from("/tmp/repo"),
            name: "repo".into(),
            branch: "main".into(),
            main_branch: "main".into(),
        };
        let (keymap, _) = KeyMap::with_warnings(&toml::Table::new());
        Self::new(repo, Config::default(), keymap, Theme::default())
    }
}
