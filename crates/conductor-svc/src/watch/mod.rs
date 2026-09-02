//! 外部の事象を聞く 4 つの watcher。
//!
//! それぞれ自前のスレッド (notify 系は notify 自身の内部スレッド) で待ち受け、
//! [crate::EventSender::send_watch] で 1 本の mpsc へ直接送る。UI 側は
//! [crate::Services::try_recv] で他の Event と同じ経路から受け取るだけでよい。

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CcState {
    Active,
    Waiting,
}
