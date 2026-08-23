//! Viewer の状態 — ファイルツリーのモデルとファイル内容バッファ。
//!
//! Viewer モードの状態を管理する: ファイルシステムから構築する階層的なファイルツリー
//! （.git ディレクトリはスキップ）と、現在選択中のファイルの内容。
//!
//! [ViewerState] とそのサブ構造体は [state] にある。振る舞いは関心事ごとに
//! 他のサブモジュールへ分割されている（[content] はファイルを開く処理、
//! [tree] はファイルツリーの走査・展開、[search] はファイル内検索とファイル名検索、
//! [diff_view] は unified diff 表示、[highlight] は syntect によるシンタックスハイライト、
//! [selection] はガター行選択、[fold] はコードブロックの折りたたみ、
//! [tabs] は複数ファイルを同時に開くためのタブ）。

mod content;
mod diff_view;
mod fold;
mod file_tree;
mod file_view;
mod highlight;
mod search;
mod selection;
mod state;
mod tabs;
mod tree;

pub use file_tree::FileTreeEntry;
pub use fold::{FoldRule, FoldState};
// revidere の 2 列ビューは開いているファイルではなく diff の行を直接ハイライト
// するので、ViewerState を経由せず構文定義だけを引く。拡張子のエイリアス表を
// 2 つ持つと、片方だけ直したときに同じファイルが場所によって色付いたり
// 付かなかったりする。
pub use file_view::UnifiedDiffEntry;
pub(crate) use highlight::find_syntax;
pub use state::*;
