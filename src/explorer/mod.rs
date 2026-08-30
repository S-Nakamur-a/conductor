//! Explorer パネルの状態 — ファイルツリーと Changed files / コメント一覧。
//!
//! [ExplorerState] とその構成要素 [FileTreeState] / [ExplorerBottomView] を
//! ここに置く。振る舞い（メソッド）は隣接モジュールにあり、このファイルは
//! レイアウトの定義のみを行う。

pub mod hover;
pub mod input;
pub mod list_row;
pub mod mouse;
pub mod render;
pub mod tree;

use std::collections::HashSet;
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
    /// 読むのは [ExplorerState::root]、書くのは [ExplorerState::load_file_tree] /
    /// [ExplorerState::replace_tree] / [ExplorerState::set_root] だけに限る。
    /// 以前は根を持たず、ファイルを開くたびに呼び出し側が「今どの worktree か」
    /// を引き直して渡していたので、表示中のツリーと開く先が食い違っても誰も
    /// 気付けなかった (worktree 切り替えはツリーの走査を裏に回すため、古い
    /// エントリと新しい根が同時に存在する瞬間がある)。
    pub(in crate::explorer) root: PathBuf,
    /// フラット化したファイルツリー（ディレクトリ+ファイル、pre-order）。
    pub file_tree: Vec<FileTreeEntry>,
    /// 選択中の行のインデックス（フィルタ前の全ツリーにおける）。
    pub tree_selected: usize,
    /// ツリーペインの垂直スクロールオフセット。
    pub tree_scroll: usize,
    /// visible_indices() のキャッシュ結果。ツリー構造が変わると無効化される。
    pub cached_visible_indices: Option<Rc<Vec<usize>>>,
    /// 各エントリの git_state を支える git status のスナップショット。
    /// load_file_tree() の呼び出しごとに1回だけ更新され（フレームごとでも
    /// エントリごとでもない）、完全な再構築の合間に遅延読み込みされる子要素に対しては
    /// ensure_children_loaded() が再利用する。
    pub git_status: GitStatusMap,
}

/// Explorer の下部ペインが表示しているビュー。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExplorerBottomView {
    /// 変更ファイルの diff 一覧。
    #[default]
    DiffList,
    /// レビューコメント一覧。
    Comments,
}

/// Explorer パネルの状態（ファイルツリー、選択、スクロール）。
pub struct ExplorerState {
    /// ファイルツリー管理。
    pub tree: FileTreeState,
    /// diff 一覧中の選択中ファイルのインデックス。
    pub diff_list_selected: usize,
    /// diff 一覧の垂直スクロールオフセット。
    pub diff_list_scroll: usize,
    /// explorer パネルのフォーカスが diff 一覧（下半分）にあるか。
    pub focus_on_diff_list: bool,
    /// explorer ファイルツリーペインの直近の内側の高さ（render 中に更新される）。
    pub tree_height: usize,
    /// explorer diff 一覧ペインの直近の内側の高さ（render 中に更新される）。
    pub diff_list_height: usize,
    /// diff 一覧がベースエラーバナーに使う行数（0 か 1、render 中に更新される）。
    /// このバナーは display_list のエントリではないので、画面行をリストの
    /// インデックスへ逆変換する処理（マウスクリック）はこれを差し引かないと
    /// 違うファイルを選んでしまう。
    pub diff_banner_rows: usize,
    /// explorer の下部ペインが現在表示しているビュー。
    pub bottom_view: ExplorerBottomView,
    /// explorer コメント一覧中の選択中コメントのインデックス。
    pub comment_list_selected: usize,
    /// explorer コメント一覧の垂直スクロールオフセット。
    pub comment_list_scroll: usize,
    /// レビュアーが「viewed」を付けたファイルの相対パス。
    pub viewed: HashSet<String>,
}

impl Default for ExplorerState {
    fn default() -> Self {
        Self {
            tree: FileTreeState::default(),
            diff_list_selected: 0,
            diff_list_scroll: 0,
            focus_on_diff_list: false,
            tree_height: 20,
            diff_list_height: 20,
            diff_banner_rows: 0,
            bottom_view: ExplorerBottomView::default(),
            comment_list_selected: 0,
            comment_list_scroll: 0,
            viewed: HashSet::new(),
        }
    }
}
