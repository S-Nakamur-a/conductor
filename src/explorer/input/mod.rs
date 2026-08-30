//! Explorer パネルのキー処理（ファイルツリー、diff リスト、コメントリスト）。
//!
//! エントリポイントは [tree::handle_explorer_key]。下部ペインにフォーカスがある
//! ときに委譲する2つのサブパネルとして [diff_list]（統合 diff リストのナビゲー
//! ション）と [comment_list]（レビューコメントリストのナビゲーション。
//! [comment_list::navigate_to_comment_with_focus] を含む）がある。
//! [viewer_actions] は、ツリーナビゲーション自体には属さない、Viewer パネル起点
//! のコメント操作（コメント追加入力を開く、コメント詳細モーダルを開く、送信された
//! コメントテキストのパース）を保持する。

mod comment_list;
mod diff_list;
mod tree;
mod viewer_actions;

// navigate_to_comment_with_focus は explorer 内 (mouse.rs) からしか呼ばれないので
// 公開範囲を crate::explorer に留める。残りは crate::event 側 (mouse/viewer_panel.rs,
// viewer/mod.rs, overlay/review.rs) からも呼ばれるため crate 全体に公開する。
pub use comment_list::handle_explorer_comment_list_key;
pub(in crate::explorer) use comment_list::navigate_to_comment_with_focus;
pub use tree::handle_explorer_key;
pub use viewer_actions::{open_viewer_comment, open_viewer_comment_detail, submit_new_comment};
