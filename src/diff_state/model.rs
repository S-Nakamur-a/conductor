//! Diff モードのデータ型: ビューモード、フラット化した explorer 表示リスト、
//! 行/ハンク/ファイル単位の diff 構造体、そしてトップレベルの DiffState。

use std::collections::HashSet;

use crate::config::DiffView;

// ビューモード

/// diff の内容をどう表示するか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffViewMode {
    Unified,
    SideBySide,
}

impl From<DiffView> for DiffViewMode {
    fn from(v: DiffView) -> Self {
        match v {
            DiffView::Unified => DiffViewMode::Unified,
            DiffView::SideBySide => DiffViewMode::SideBySide,
        }
    }
}

// 表示リスト

/// explorer パネルに表示するフラット化リストの1エントリ。
#[derive(Debug, Clone)]
pub enum DiffListEntry {
    /// マージされた変更ツリー内のディレクトリノード(折りたたみ可能)。
    Directory {
        /// ディレクトリパス(例: "src/ui")。
        path: String,
        /// 表示名(最後の要素)。
        name: String,
        /// ネスト深度(0 がトップレベル)。
        depth: usize,
        /// このディレクトリが折りたたまれているか。
        collapsed: bool,
    },
    /// 変更のあったファイル。file_index は DiffState::files への添字。
    File {
        file_index: usize,
        /// ネスト深度(0 がトップレベルのファイル)。
        depth: usize,
    },
    /// リストの最上部に固定表示される、ブランチの変更サマリー用の疑似ファイル。
    /// 選択すると Viewer にサマリー全文が開く。将来メタデータ(鮮度など)を
    /// 追加しても既存の match アームを壊さないよう struct variant にしている。
    Summary {},
}

// 行レベルの型

/// diff の行がコンテキストか追加か削除かを示すタグ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineTag {
    Equal,
    Insert,
    Delete,
}

/// diff 行内のセグメント。変更箇所と非変更箇所を区別する。
#[derive(Debug, Clone)]
pub struct InlineSegment {
    /// このセグメントのテキスト内容。
    pub text: String,
    /// このセグメントを強調表示するか(実際に行内変更があった箇所かどうか)。
    pub emphasized: bool,
}

/// ハンク内の1行。
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub tag: DiffLineTag,
    /// 旧(ベース)ファイル側の行番号。存在する場合。
    pub old_line_no: Option<usize>,
    /// 新(HEAD)ファイル側の行番号。存在する場合。
    pub new_line_no: Option<usize>,
    /// 行内変更セグメント。空 Vec の場合は行全体をそのまま描画する。
    pub inline_segments: Vec<InlineSegment>,
    /// この行のテキスト内容(タブ展開済み)。
    pub content: String,
}

// ハンク

/// diff 行の連続したまとまり(コンテキスト + 変更)。
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// このハンクを構成する行。
    pub lines: Vec<DiffLine>,
    /// 検出できた場合の関数コンテキストヘッダー(例: "fn some_function()")。
    pub func_header: Option<String>,
}

// ファイル単位の diff

/// 単一ファイルの diff 情報。
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// ファイルパス(worktree ルートからの相対パス)。
    pub path: String,
    /// 全ハンクを通じた追加行数。
    pub added_lines: usize,
    /// 全ハンクを通じた削除行数。
    pub deleted_lines: usize,
    /// コンテキスト付きでパースしたハンク。
    pub hunks: Vec<DiffHunk>,
}

// トップレベルの diff state

/// Diff モード UI の全状態。
#[derive(Debug, Clone)]
pub struct DiffState {
    /// ブランチがベースに対して加えた変更(merge-base..workdir+index)。
    /// コミット済みと未コミットを1つの diff にまとめてあるので、コミット後に
    /// 再編集したファイルも1エントリのままになる。
    pub files: Vec<FileDiff>,
    /// explorer パネル用にフラット化した表示リスト。
    pub display_list: Vec<DiffListEntry>,
    /// 折りたたまれているディレクトリパスの集合(素のリポジトリ相対パスをキーにする)。
    pub collapsed_dirs: HashSet<String>,
    /// diff 内容ペイン内の垂直スクロールオフセット。
    pub scroll: usize,
    /// 現在の表示モード。
    pub view_mode: DiffViewMode,
    /// diff の比較対象となるベースブランチ(例: "main")。
    pub base_branch: String,
    /// diff の読み込みに失敗した場合の人間可読なエラーメッセージ。
    pub error: Option<String>,
    /// 現在のブランチに変更サマリーがあるか。true の場合、表示リストの先頭に
    /// DiffListEntry::Summary の疑似ファイルが固定表示される。App が
    /// ReviewState::change_summary から同期する(diff モデルは review state に
    /// 直接アクセスできないため、このフラグだけをキャッシュしている)。
    pub has_summary: bool,
}
