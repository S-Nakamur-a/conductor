//! Explorer パネルの状態 — ファイルツリーと Changed files / コメント一覧。
//!
//! [Explorer] とその構成要素 [FileTreeState] / [BottomView] を
//! ここに置く。振る舞い（メソッド）は隣接モジュールにあり、このファイルは
//! レイアウトの定義のみを行う。

pub mod apply;
pub mod ctx;
pub mod hover;
pub mod intent;
pub mod keys;
pub mod panel;
pub mod pointer;
pub mod render;
pub mod state;

pub use state::{BottomView, Explorer, Pane};
pub mod tree;

use std::path::PathBuf;
use std::rc::Rc;

use crate::git_engine::status_map::GitStatusMap;
use crate::viewer::FileTreeEntry;

/// ファイルツリー管理の状態。
#[derive(Default)]
pub struct FileTreeState {
    /// このツリーを歩いた根。エントリの相対パスはすべてここからの相対で、
    /// 絶対パスに戻せるのはこの値だけ。
    ///
    /// 読むのは [Explorer::root]、書くのは [Explorer::load_file_tree] /
    /// [Explorer::replace_tree] / [Explorer::set_root] だけに限る。
    /// 以前は根を持たず、ファイルを開くたびに呼び出し側が「今どの worktree か」
    /// を引き直して渡していたので、表示中のツリーと開く先が食い違っても誰も
    /// 気付けなかった (worktree 切り替えはツリーの走査を裏に回すため、古い
    /// エントリと新しい根が同時に存在する瞬間がある)。
    pub(in crate::explorer) root: PathBuf,
    /// フラット化したファイルツリー（ディレクトリ+ファイル、pre-order）。
    pub file_tree: Vec<FileTreeEntry>,
    /// visible_indices() のキャッシュ結果。ツリー構造が変わると無効化される。
    pub cached_visible_indices: std::cell::RefCell<Option<Rc<Vec<usize>>>>,
    /// 各エントリの git_state を支える git status のスナップショット。
    /// load_file_tree() の呼び出しごとに1回だけ更新され（フレームごとでも
    /// エントリごとでもない）、完全な再構築の合間に遅延読み込みされる子要素に対しては
    /// ensure_children_loaded() が再利用する。
    pub git_status: GitStatusMap,
}
