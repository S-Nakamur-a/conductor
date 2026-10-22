//! 外部の事象を聞く 4 つの watcher。
//!
//! それぞれ自前のスレッド (notify 系は notify 自身の内部スレッド) で待ち受け、
//! [crate::EventSender::send_watch] で 1 本の mpsc へ直接送る。UI 側は
//! [crate::Services::try_recv] で他の Event と同じ経路から受け取るだけでよい。
//! cc-notify だけは PTY ホストが持ち、ホストとの接続越しに同じ経路へ合流する。

mod cc_notify;
mod config_watcher;
mod file_watcher;
mod refresh_pipe;

pub use cc_notify::CcNotifyListener;
pub use config_watcher::ConfigWatcher;
pub use file_watcher::FileWatcher;
pub use refresh_pipe::RefreshPipe;

use std::path::PathBuf;

/// watcher が届ける事象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// worktree 配下でファイルが変更された。
    ///
    /// パスは .git/ .conductor/ を除いた変更先の 1 件。リネームイベントは (from, to) の
    /// 両方を運ぶことがあるので、送るのは先頭要素固定ではなく実際の変更先。
    FsChanged(PathBuf),
    /// worktree の git 側 (HEAD、index、refs) が動いた。commit や fetch は作業ツリーの
    /// ファイルを触らないので、FsChanged では届かない。
    GitChanged,
    /// 設定ファイルが作成・変更・置換された。
    ConfigChanged,
    /// Claude Code パネルの active/waiting 状態が変わった。
    CcState { kind: CcState, cwd: PathBuf },
    /// パネルが書き込んでいる Claude セッションの id が変わった (起動、/resume、/clear)。
    CcSessionRotated {
        panel_id: String,
        session_id: String,
    },
    /// MCP からレビューデータの refresh 要求が届いた。
    RefreshRequested,
    /// パネルの Claude が MCP 越しに画面操作を頼んだ。
    UiRequest {
        cwd: PathBuf,
        request: conductor_core::cc_hook::UiRequest,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CcState {
    Active,
    Waiting,
}

impl From<conductor_core::cc_hook::Notification> for WatchEvent {
    fn from(notification: conductor_core::cc_hook::Notification) -> Self {
        use conductor_core::cc_hook::Notification;
        match notification {
            Notification::Active { cwd } => Self::CcState {
                kind: CcState::Active,
                cwd,
            },
            Notification::Waiting { cwd } => Self::CcState {
                kind: CcState::Waiting,
                cwd,
            },
            Notification::Session {
                panel_id,
                session_id,
            } => Self::CcSessionRotated {
                panel_id,
                session_id,
            },
            Notification::Ui { cwd, request } => Self::UiRequest { cwd, request },
        }
    }
}
