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

// これらのアイテムが1階層深いディレクトリに移動した後も、event の兄弟サブモジュール
// (mouse, viewer, overlay, event::mod 自身) が既存の super::explorer::X /
// crate::event::explorer::X という参照をそのまま解決できるよう再エクスポートする。
pub(in crate::event) use comment_list::{
    handle_explorer_comment_list_key, navigate_to_comment_with_focus,
};
pub(in crate::event) use tree::handle_explorer_key;
pub(in crate::event) use viewer_actions::{
    open_viewer_comment, open_viewer_comment_detail, submit_new_comment,
};
