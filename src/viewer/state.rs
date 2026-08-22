//! Viewer state の構造体定義。
//!
//! [ViewerState] を構成する全てのサブ構造体と、それらが保持する小さな enum
//! （ScreenRow, ExplorerBottomView, LineSelection）。振る舞い（メソッド）は
//! 隣接モジュールにあり、このファイルはレイアウトの定義のみを行う。

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use crate::git_engine::status_map::GitStatusMap;
use crate::media_state::MediaState;
use crate::text_input::TextInput;

use super::file_tree::{FileTreeEntry, ScoredFile};
use super::file_view::UnifiedDiffEntry;

// サブ構造体

/// ファイルツリー管理の状態。
#[derive(Default)]
pub struct FileTreeState {
    /// このツリーを歩いた根。エントリの相対パスはすべてここからの相対で、
    /// 絶対パスに戻せるのはこの値だけ。
    ///
    /// 読むのは [ViewerState::root]、書くのは [ViewerState::load_file_tree] /
    /// [ViewerState::replace_tree] / [ViewerState::set_root] だけに限る。
    /// 以前は根を持たず、ファイルを開くたびに呼び出し側が「今どの worktree か」
    /// を引き直して渡していたので、表示中のツリーと開く先が食い違っても誰も
    /// 気付けなかった (worktree 切り替えはツリーの走査を裏に回すため、古い
    /// エントリと新しい根が同時に存在する瞬間がある)。
    pub(in crate::viewer) root: PathBuf,
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

/// ファイル内容表示の状態。
#[derive(Default)]
pub struct FileContentState {
    /// 現在開いているファイルの各行。
    pub file_content: Vec<String>,
    /// ファイル内容ペインの垂直スクロールオフセット。
    pub file_scroll: usize,
    /// ファイル内容ペインの水平スクロールオフセット（文字単位）。
    pub h_scroll: usize,
    /// 現在表示中のファイルの相対パス（あれば）。
    pub current_file: Option<String>,
    /// なぜ file_content が空なのかの理由。読み込みに失敗したときだけ入る。
    ///
    /// 「未選択」「中身が空のファイル」「読めなかった」はどれも file_content が
    /// 空になるので、これが無いと Viewer は 3 つを見分けられず、失敗が黙って
    /// 「ファイル未選択」に丸められる。open_file が成功したら必ず消す。
    pub load_error: Option<String>,
    /// 行ごとにキャッシュしたシンタックスハイライト済みトークン（syntect の出力を
    /// ratatui のスタイルへ変換したもの）。
    pub highlighted_lines: Vec<Vec<(ratatui::style::Style, String)>>,
    /// (current_file, file_content) のハッシュ。冗長な再ハイライトをスキップするために使う。
    pub highlighted_cache_key: Option<u64>,
    /// 現在表示中のファイルの diff 注釈のキャッシュ（line_no -> (tag, segments)）。
    /// diff データが変わるか別のファイルを開くと無効化される。
    pub cached_diff_annotations: Option<
        std::collections::HashMap<
            usize,
            (
                crate::diff_state::DiffLineTag,
                Vec<crate::diff_state::InlineSegment>,
            ),
        >,
    >,
    /// cached_diff_annotations を構築した対象のファイルパス。
    pub cached_diff_annotations_file: Option<String>,
    /// grep 検索結果からハイライトされた行番号（1始まり）。次にファイルを開くとクリアされる。
    pub grep_highlight_line: Option<usize>,
    /// render 中に構築される画面行のマッピング。マウスイベントハンドラが
    /// 画面上の位置をファイルの行/スレッドアクションへ変換するのに使う。
    pub screen_row_map: Vec<ScreenRow>,
    /// 開いているファイルの折りたたみ範囲と、そのうち今閉じているもの。
    /// 「ファイルの行」と「画面に出る行」の食い違いはここだけが知っている
    /// （[crate::viewer::FoldState] のモジュールコメントを参照）。
    pub folds: crate::viewer::FoldState,
    /// 現在のファイル中の実行可能なテスト。1始まりの行番号をキーにする。
    /// 言語ごとのスキャナ（*_test.go には [crate::go_test]、*.rs には
    /// [crate::rust_test]）が埋める。それ以外のファイルでは空。▶ 実行ボタンを
    /// 駆動する。
    pub test_runs: std::collections::HashMap<usize, crate::test_run::TestRun>,
    /// 開いているファイル中の識別子の出現のうち、どれがコードであり、コメントや
    /// 文字列中の地の文ではないかを示す。開いたときにそのファイル自身のテキストから
    /// 構築するので、常に画面に実際にあるものを表す — symbol index がたまたま
    /// どの根に対して構築されているかとは無関係。文法を持たない言語では空になり、
    /// それらは誤ったナビゲーションではなくナビゲーションが無い状態になる。
    pub code_mask: crate::symbol_index::CodeMask,
}

/// 画面行が何を表すか（マウスクリック処理向け）。
#[derive(Debug, Clone)]
pub enum ScreenRow {
    /// ソースコードの行（1始まりの行番号）。
    Code(usize),
    /// スレッド本文の行（行選択のクリック対象ではない）。
    ThreadContent,
    /// 特定のコメント向けのクリック可能なボタンを持つアクション行。
    ThreadActions { comment_id: String },
}

/// ファイル内検索の状態。
#[derive(Default)]
pub struct SearchState {
    /// 現在の検索クエリ（空 = 検索が行われていない）。
    pub search_query: TextInput,
    /// 現在の検索クエリに一致する行インデックス。
    pub search_matches: Vec<usize>,
    /// 現在のマッチを指す search_matches 内のインデックス。
    pub search_match_idx: usize,
    /// 検索入力ボックスが表示されているか。
    pub search_active: bool,
}

/// unified diff 表示の状態。
#[derive(Default)]
pub struct DiffViewState {
    /// viewer が unified diff モードかどうか。
    pub diff_mode: bool,
    /// unified diff 表示のエントリ（diff モードに入ったときに埋まる）。
    pub diff_view_lines: Vec<UnifiedDiffEntry>,
    /// diff 表示の垂直スクロールオフセット。
    pub diff_view_scroll: usize,
    /// diff 表示の最大行番号のキャッシュ（フレームごとの O(n) スキャンを避ける）。
    pub diff_view_max_line_no: usize,
    /// 描画された各画面行（diff ビューポート内）を diff_view_lines 内の
    /// インデックスへ逆引きするマップ。挿入されたインラインスレッド行は
    /// None になる。render 中に書き込まれ、マウス処理（expand-context など）が
    /// 使う — 挿入されたスレッド行は単純な scroll+offset の算術を壊すため。
    pub screen_entry_map: Vec<Option<usize>>,
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

/// Explorer パネルの状態（選択、スクロール）。
pub struct ExplorerState {
    /// diff 一覧中の選択中ファイルのインデックス。
    pub diff_list_selected: usize,
    /// diff 一覧の垂直スクロールオフセット。
    pub diff_list_scroll: usize,
    /// explorer パネルのフォーカスが diff 一覧（下半分）にあるか。
    pub explorer_focus_on_diff_list: bool,
    /// explorer ファイルツリーペインの直近の内側の高さ（render 中に更新される）。
    pub explorer_tree_height: usize,
    /// explorer diff 一覧ペインの直近の内側の高さ（render 中に更新される）。
    pub explorer_diff_list_height: usize,
    /// diff 一覧がベースエラーバナーに使う行数（0 か 1、render 中に更新される）。
    /// このバナーは display_list のエントリではないので、画面行をリストの
    /// インデックスへ逆変換する処理（マウスクリック）はこれを差し引かないと
    /// 違うファイルを選んでしまう。
    pub explorer_diff_banner_rows: usize,
    /// explorer の下部ペインが現在表示しているビュー。
    pub explorer_bottom_view: ExplorerBottomView,
    /// explorer コメント一覧中の選択中コメントのインデックス。
    pub comment_list_selected: usize,
    /// explorer コメント一覧の垂直スクロールオフセット。
    pub comment_list_scroll: usize,
    /// インラインコメントスレッドが展開されている行番号（1始まり）の集合。
    pub expanded_inline_threads: HashSet<usize>,
    /// インラインリプライ入力が有効な行番号（None = リプライ中でない）。
    pub inline_reply_line: Option<usize>,
    /// インラインリプライの対象となるコメント ID。
    pub inline_reply_comment_id: Option<String>,
    /// インラインリプライ入力のテキストバッファ。
    pub inline_reply_buffer: TextInput,
    /// レビュアーが「viewed」を付けたファイルの相対パス。
    pub viewed: HashSet<String>,
}

impl Default for ExplorerState {
    fn default() -> Self {
        Self {
            diff_list_selected: 0,
            diff_list_scroll: 0,
            explorer_focus_on_diff_list: false,
            explorer_tree_height: 20,
            explorer_diff_list_height: 20,
            explorer_diff_banner_rows: 0,
            explorer_bottom_view: ExplorerBottomView::default(),
            comment_list_selected: 0,
            comment_list_scroll: 0,
            expanded_inline_threads: HashSet::new(),
            inline_reply_line: None,
            inline_reply_comment_id: None,
            inline_reply_buffer: TextInput::new_multiline(),
            viewed: HashSet::new(),
        }
    }
}

/// コメント向けの行選択状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LineSelection {
    /// 行が選択されていない。
    #[default]
    None,
    /// 範囲が完全に選択されている（start と end は1始まりで両端を含む）。
    /// start が end より大きいこともある — 呼び出し側は selected_range() で正規化する。
    Selected { start: usize, end: usize },
}

/// ファイル名のあいまい検索の状態。
#[derive(Default)]
pub struct FilenameSearchState {
    /// ファイル名検索オーバーレイが有効か。
    pub filename_search_active: bool,
    /// 現在のファイル名検索クエリ。
    pub filename_search_query: TextInput,
    /// スコア付けしてソートしたあいまい検索の結果。
    pub filename_search_results: Vec<ScoredFile>,
    /// 検索結果一覧内の選択中インデックス。
    pub filename_search_selected: usize,
    /// ファイル名検索向けの全ファイルパスのキャッシュ一覧（検索開始時に埋まる）。
    pub filename_search_all_files: Vec<String>,
}

/// ジャンプ用の下線のための symbol ホバー情報（Cmd/Ctrl+hover のときだけでなく、
/// カーソルが止まればいつでも表示される — 下の has_jump_modifier を参照）。
#[derive(Debug, Clone)]
pub struct HoverSymbol {
    /// symbol が存在する行番号（1始まり）。
    pub line: usize,
    /// 開始列（0始まり、h_scroll 適用前のコンテンツ文字数単位）。
    pub start_col: usize,
    /// 終了列（0始まり、含まない）。
    pub end_col: usize,
    /// この symbol 上での直近のマウス移動時に Cmd/Ctrl が押されていたか。
    /// 下線の色を決める（2段階の開示）: false なら theme.hint（「ここに定義がある」）、
    /// true なら theme.accent（「今押せばジャンプできる」）を描く。クリックの
    /// 契約自体は変わらない — これは下線がどちらの約束をするかだけを制御する。
    pub has_jump_modifier: bool,
}

/// マウスが乗っている symbol。ClickTracker::hover_symbol に昇格する前に、
/// ジャンプ下線独自のデバウンス（150ms — code_nav.rs のポップアップの
/// 350ms HOVER_IDLE とは独立）を待っている状態。
#[derive(Debug, Clone)]
pub struct PendingUnderline {
    pub symbol: String,
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub since: std::time::Instant,
    /// この静止位置に対して、ジャンプ可能かどうかの（コストの高い、index
    /// 参照を伴う）チェックが既に実行済みかどうか。マウスが静止している間、
    /// フレームごとの tick が毎フレーム繰り返さないようにする。
    pub resolved: bool,
    pub has_jump_modifier: bool,
}

/// ダブルクリック追跡の状態。
pub struct ClickTracker {
    /// viewer パネルで現在マウスカーソル下にある行番号（1始まり）。
    pub hover_line: Option<usize>,
    /// マウスカーソルが特に gutter（行番号領域）上にあるときの行番号（1始まり）。
    pub hover_gutter_line: Option<usize>,
    /// 確定したジャンプ下線の対象。マウスがジャンプ可能な symbol の上で
    /// デバウンス時間を超えて静止したときに表示される（待機中、またはジャンプ
    /// できない単語の上にあるときは None）。
    pub hover_symbol: Option<HoverSymbol>,
    /// デバウンス中の、静止候補（hover_symbol が確定する前の状態）。
    pub underline_pending: Option<PendingUnderline>,
    /// ダブルクリック判定用の、直近の行番号クリックのタイムスタンプ（ms）。
    pub last_line_click_time: std::time::Instant,
    /// 最後にクリックされた行番号（1始まり）。
    pub last_line_click_line: usize,
    /// gutter のドラッグが進行中の間、その開始行（1始まり、アンカー）。
    /// 範囲はドラッグ先の行まで伸び、コメントは mouse-up で開く。ドラッグ中で
    /// なければ None。
    pub gutter_drag_anchor: Option<usize>,
    /// ダブルクリック判定用の、直近のファイルツリークリックのタイムスタンプ。
    pub last_tree_click_time: std::time::Instant,
    /// ファイルツリーで最後にクリックされたツリーインデックス。
    pub last_tree_click_idx: usize,
    /// ダブルクリック判定用の、直近のコメント一覧クリックのタイムスタンプ。
    pub last_comment_click_time: std::time::Instant,
    /// コメント一覧で最後にクリックされたインデックス。
    pub last_comment_click_idx: usize,
}

impl Default for ClickTracker {
    fn default() -> Self {
        Self {
            hover_line: None,
            hover_gutter_line: None,
            hover_symbol: None,
            underline_pending: None,
            last_line_click_time: std::time::Instant::now(),
            last_line_click_line: 0,
            gutter_drag_anchor: None,
            last_tree_click_time: std::time::Instant::now(),
            last_tree_click_idx: usize::MAX,
            last_comment_click_time: std::time::Instant::now(),
            last_comment_click_idx: usize::MAX,
        }
    }
}

// メイン構造体

/// Viewer で開いているファイル 1 つ分のタブ。
pub struct ViewerTab {
    /// 表示中のツリーの根からの相対パス。
    pub path: String,
    /// 非アクティブな間の退避先。アクティブなタブの状態は ViewerState 側の
    /// content/search/diff_view/selection が実体を持つので None になる。
    /// 実体を 2 か所に置かないことで、どちらが本物かを迷う余地を無くしている。
    pub(in crate::viewer) stashed: Option<TabView>,
}

/// タブ 1 つ分の、ファイルに紐づく状態のまとまり。
#[derive(Default)]
pub(in crate::viewer) struct TabView {
    pub(in crate::viewer) content: FileContentState,
    pub(in crate::viewer) search: SearchState,
    pub(in crate::viewer) diff_view: DiffViewState,
    pub(in crate::viewer) selection: LineSelection,
    pub(in crate::viewer) md_scroll: usize,
}


/// Viewer の最も左にあるコメントマーカー列の幅（列数） — 💬/│ の
/// スレッドマーカーが存在する場所で、行番号の左側にある。「+」バッジ列
/// （行番号の右側）とは別に保つことで、既存スレッドの開閉と新規コメントの
/// 開始がクリック対象を共有しないようにしている: gutter+バッジ側全体は
/// 常に新規コメントを開始する。
pub const COMMENT_MARKER_W: u16 = 2;

/// ガターのうち行番号が使わない列数:
///   diff の +/-(1) + 空白(1) + 折りたたみマーカー(1) + 空白(1) + '│'(1) + 空白(1)
///
/// 行番号の桁数と足して [ViewerState::gutter_total_width] になる。描画も
/// マウスのヒットテストもインラインスレッドのインデントもこの幅ぶんだけ
/// 右にずれるので、別々の数字で持つと1列ずれて気づかれない。
pub const GUTTER_FIXED_W: usize = 6;

/// Viewer モードが保持する全ての状態。
#[derive(Default)]
pub struct ViewerState {
    /// ファイルツリー管理。
    pub tree: FileTreeState,
    /// 開いているファイルのタブ。開いた順に並ぶ。
    pub tabs: Vec<ViewerTab>,
    /// tabs 内のアクティブなタブの位置。tabs が空のときは意味を持たない。
    pub active_tab: usize,
    /// ファイル内容表示 — アクティブなタブの実体。
    pub content: FileContentState,
    /// ファイル内検索。
    pub search: SearchState,
    /// unified diff 表示。
    pub diff_view: DiffViewState,
    /// Explorer パネルの状態（選択、スクロール）。
    pub explorer: ExplorerState,
    /// コメント向けの行選択。
    pub selection: LineSelection,
    /// ファイル名のあいまい検索。
    pub filename_search: FilenameSearchState,
    /// メディア描画の状態（画像/動画を ASCII アートとして表示）。
    pub media_state: MediaState,
    /// ダブルクリック追跡。
    pub click: ClickTracker,
    /// タブ行のクリック領域（render 中に更新される）。マウス処理が幅を
    /// 計算し直さず、描画とまったく同じジオメトリを引けるようにする。
    pub tab_row_hits: Vec<crate::ui::tab_bar::TabHit>,
    /// 'g' が押されて2つ目のキー（gd, gi, gr）待ちかどうか。
    pub pending_g_key: bool,
    /// 'z' が押されて2つ目のキー（za, zc, zo, zR, zM）待ちかどうか。
    pub pending_z_key: bool,
    /// viewer がファイル内容/diff の代わりにブランチの change summary 疑似
    /// ファイル（"SUMMARY" エントリ）を表示しているか。diff_view.diff_mode とは
    /// 排他。enter_summary_view / exit_summary_view を参照。
    pub show_summary: bool,
    /// summary 表示内での垂直スクロールオフセット。
    pub summary_scroll: usize,
    /// summary 表示の折り返し後の総行数。render 中に書き込まれ、キーハンドラが
    /// summary_scroll をクランプするために読む。
    pub summary_total_lines: usize,
    /// markdown ファイルを生のソースの代わりにレンダリング済み（SUMMARY 形式の
    /// 本文）で表示しているか。セッション内で持続する: 別の markdown ファイルを
    /// 開いても引き継がれ、markdown 以外のファイルを開いている間は単に無視される。
    /// 素のファイル表示でのみ効果を持つ — is_showing_rendered_markdown を参照。
    /// あらゆるレンダラとイベントハンドラはこれで判定しなければならない。
    pub md_rendered: bool,
    /// レンダリング済み markdown 表示内での垂直スクロールオフセット。
    /// md_rendered と違い、ファイルごとにリセットされる（open_file 内で）。
    pub md_scroll: usize,
    /// レンダリング済み markdown 表示の折り返し後の総行数。render 中に
    /// 書き込まれ、キーハンドラが md_scroll をクランプするために読む。
    pub md_total_lines: usize,
}
