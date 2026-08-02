//! Viewer の状態 — ファイルツリーのモデルとファイル内容バッファ。
//!
//! Viewer モードの状態を管理する: ファイルシステムから構築する階層的なファイルツリー
//! （.git ディレクトリはスキップ）と、現在選択中のファイルの内容。
//!
//! [ViewerState] とそのサブ構造体は [state] にある。振る舞いは関心事ごとに
//! 他のサブモジュールへ分割されている（[content] はファイルを開く処理、
//! [tree] はファイルツリーの走査・展開、[search] はファイル内検索とファイル名検索、
//! [diff_view] は unified diff 表示、[highlight] は syntect によるシンタックスハイライト、
//! [selection] はガター行選択）。

mod content;
mod diff_view;
mod file_tree;
mod file_view;
mod highlight;
mod search;
mod selection;
mod state;
mod tree;

pub use file_tree::{FileTreeEntry, file_icon};
pub use file_view::UnifiedDiffEntry;
pub use state::*;
