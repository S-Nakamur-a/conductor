//! 「ユーザーがどこを見ていたか」の保存と復元。

use crate::app::types::PendingViewRestore;

/// worktree を切り替えても、再起動しても、元の場所に戻すための状態。
#[derive(Default)]
pub struct ViewRestore {
    /// いまメモリ上の `viewer_state` がどの worktree のものか。
    ///
    /// 切り替える前に現在の状態を保存するための「持ち主」の記録で、
    /// 最初の worktree を読むまでは `None`。
    pub current_branch: Option<String>,
    /// ファイルツリーが揃ったら開き直すファイルとスクロール位置 (一度きり)。
    /// [`crate::app::App::consume_pending_view_restore`] が消費する。
    pub pending: Option<PendingViewRestore>,
}
