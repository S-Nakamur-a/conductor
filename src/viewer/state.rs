//! Viewer の状態の型定義のみ。振る舞いは隣接モジュールにある。

use std::collections::HashSet;

use crate::text_input::TextInput;
use crate::viewer::media::MediaState;

use super::file_tree::ScoredFile;
use super::file_view::UnifiedDiffEntry;

/// ファイル内容表示の状態。
#[derive(Default)]
pub struct FileContentState {
    pub file_content: Vec<String>,
    pub file_scroll: usize,
    /// 水平スクロールオフセット（文字単位）。
    pub h_scroll: usize,
    /// 表示中のファイルの、ツリーの根からの相対パス。
    pub current_file: Option<String>,
    /// 「未選択」「空ファイル」「読めなかった」はどれも file_content が空になる。
    /// これが無いと 3 つを見分けられず、失敗が黙って「未選択」に丸められる。
    pub load_error: Option<String>,
    pub highlighted_lines: Vec<Vec<(ratatui::style::Style, String)>>,
    /// (current_file, file_content) のハッシュ。冗長な再ハイライトを飛ばすために持つ。
    pub highlighted_cache_key: Option<u64>,
    /// line_no -> (tag, segments)。diff が変わるか別のファイルを開くと無効化する。
    pub cached_diff_annotations: Option<
        std::collections::HashMap<
            usize,
            (
                crate::diff_state::DiffLineTag,
                Vec<crate::diff_state::InlineSegment>,
            ),
        >,
    >,
    pub cached_diff_annotations_file: Option<String>,
    /// grep から飛んできた行（1始まり）。次にファイルを開くと消える。
    pub grep_highlight_line: Option<usize>,
    /// 画面上の位置をファイルの行やスレッドアクションへ戻すための対応表。
    pub screen_row_map: Vec<ScreenRow>,
    /// 「ファイルの行」と「画面に出る行」の食い違いはここだけが知っている。
    pub folds: crate::viewer::FoldState,
    /// ▶ 実行ボタンを駆動する。1始まりの行番号がキー。言語ごとのスキャナ
    /// ([crate::go_test] / [crate::rust_test]) が埋め、他の言語では空になる。
    pub test_runs: std::collections::HashMap<usize, crate::test_run::TestRun>,
    /// 識別子の出現のうちどれがコードで、どれがコメントや文字列中の地の文か。
    /// 開いたファイル自身のテキストから作るので、索引がどの根に対して構築されて
    /// いるかとは無関係に、常に画面にあるものを表す。
    pub code_mask: crate::symbol_index::CodeMask,
}

/// 画面行が何を表すか。マウスのクリック処理が読む。
#[derive(Debug, Clone)]
pub enum ScreenRow {
    /// ソース行（1始まり）。
    Code(usize),
    /// スレッド本文。行選択のクリック対象ではない。
    ThreadContent,
    ThreadActions {
        comment_id: String,
    },
}

/// ファイル内検索の状態。
#[derive(Default)]
pub struct SearchState {
    pub search_query: TextInput,
    pub search_matches: Vec<usize>,
    /// search_matches 内の、現在のマッチの位置。
    pub search_match_idx: usize,
    pub search_active: bool,
}

/// unified diff 表示の状態。
#[derive(Default)]
pub struct DiffViewState {
    pub diff_mode: bool,
    pub diff_view_lines: Vec<UnifiedDiffEntry>,
    pub diff_view_scroll: usize,
    /// フレームごとの O(n) スキャンを避けるためのキャッシュ。
    pub diff_view_max_line_no: usize,
    /// 画面行から diff_view_lines への逆引き。挿入されたスレッド行は None。
    /// この挿入が単純な scroll+offset の算術を壊すので、対応表が要る。
    pub screen_entry_map: Vec<Option<usize>>,
}

/// どの行のどのコメントに返信中かを表す、1 つの状態機械。
pub struct InlineThreadState {
    /// スレッドが展開されている行番号（1始まり）。
    pub expanded: HashSet<usize>,
    /// リプライ入力が有効な行番号。None ならリプライ中でない。
    pub reply_line: Option<usize>,
    pub reply_comment_id: Option<String>,
    pub reply_buffer: TextInput,
}

impl Default for InlineThreadState {
    fn default() -> Self {
        Self {
            expanded: HashSet::new(),
            reply_line: None,
            reply_comment_id: None,
            reply_buffer: TextInput::new_multiline(),
        }
    }
}

/// コメント向けの行選択状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LineSelection {
    #[default]
    None,
    /// 1始まりで両端を含む。start > end もあり得るので selected_range() で正規化する。
    Selected { start: usize, end: usize },
}

/// ファイル名のあいまい検索の状態。
#[derive(Default)]
pub struct FilenameSearchState {
    pub filename_search_active: bool,
    pub filename_search_query: TextInput,
    /// スコア順にソート済み。
    pub filename_search_results: Vec<ScoredFile>,
    pub filename_search_selected: usize,
    /// 検索開始時に一度だけ集める全ファイルパス。
    pub filename_search_all_files: Vec<String>,
}

/// ジャンプ下線の対象。修飾キーの有無に関わらず、カーソルが止まれば出る。
#[derive(Debug, Clone)]
pub struct HoverSymbol {
    /// 1始まりの行番号。
    pub line: usize,
    /// 0始まり、h_scroll 適用前の文字数単位。end は含まない。
    pub start_col: usize,
    pub end_col: usize,
    /// 下線の色だけを決める 2 段階の開示。false は「ここに定義がある」(hint)、
    /// true は「今押せば飛べる」(accent)。クリックの契約自体は変わらない。
    pub has_jump_modifier: bool,
}

/// 下線が確定する前の候補。ジャンプ下線のデバウンス (150ms) を待っている状態で、
/// code_nav のポップアップの 350ms とは独立に測る。
#[derive(Debug, Clone)]
pub struct PendingUnderline {
    pub symbol: String,
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub since: std::time::Instant,
    /// ジャンプ可能かの判定は索引を引くので重い。静止中に毎フレーム繰り返さない。
    pub resolved: bool,
    pub has_jump_modifier: bool,
}

/// ポインタの居場所 — ホバー、ジャンプ下線、gutter のドラッグ。行番号は 1 始まり。
pub struct PointerState {
    pub hover_line: Option<usize>,
    /// カーソルが gutter (行番号の領域) の上にあるときだけ入る。
    pub hover_gutter_line: Option<usize>,
    /// 確定した下線の対象。デバウンス待ちや、飛べない語の上では None。
    pub hover_symbol: Option<HoverSymbol>,
    pub underline_pending: Option<PendingUnderline>,
    pub last_line_click_time: std::time::Instant,
    pub last_line_click_line: usize,
    /// gutter ドラッグの開始行。範囲はドラッグ先まで伸び、mouse-up でコメントを開く。
    pub gutter_drag_anchor: Option<usize>,
}

impl Default for PointerState {
    fn default() -> Self {
        Self {
            hover_line: None,
            hover_gutter_line: None,
            hover_symbol: None,
            underline_pending: None,
            last_line_click_time: std::time::Instant::now(),
            last_line_click_line: 0,
            gutter_drag_anchor: None,
        }
    }
}

/// Viewer で開いているファイル 1 つ分のタブ。
pub struct ViewerTab {
    /// ツリーの根からの相対パス。
    pub path: String,
    /// 非アクティブな間の退避先。アクティブなタブの実体は ViewerState 側にあるので
    /// None になる。実体を 2 か所に置かないことで、どちらが本物かを迷わせない。
    pub(in crate::viewer) stashed: Option<TabView>,
    pub status: ViewerTabStatus,
}

/// タブをいつ閉じるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewerTabStatus {
    /// 明示的に閉じるまで残る。
    #[default]
    Persistent,
    /// ちょっと見るだけ。フォーカスが外れると閉じる。
    ///
    /// 高々 1 枚で、しかも必ずアクティブ。フォーカスが移る経路が移る前に閉じるので、
    /// 非アクティブな Preview は存在しない。
    Preview,
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

/// 行番号の左に置くスレッドマーカー (💬/│) の列幅。
///
/// 行番号の右の「+」バッジ列と分けているのは、既存スレッドの開閉と新規コメントの
/// 開始でクリック対象を共有させないため。gutter とバッジ側は常に新規コメントを開く。
pub const COMMENT_MARKER_W: u16 = 2;

/// ガターのうち行番号が使わない列数:
///   diff の +/-(1) + 空白(1) + 折りたたみマーカー(1) + 空白(1) + '│'(1) + 空白(1)
///
/// 描画もヒットテストもスレッドのインデントもこの幅だけ右にずれるので、別々の
/// 数字で持つと 1 列ずれて気づかれない。
pub const GUTTER_FIXED_W: usize = 6;

/// Viewer モードが保持する全ての状態。
#[derive(Default)]
pub struct ViewerState {
    /// 開いた順。
    pub tabs: Vec<ViewerTab>,
    /// tabs が空のときは意味を持たない。
    pub active_tab: usize,
    /// アクティブなタブの実体。
    pub content: FileContentState,
    pub search: SearchState,
    pub diff_view: DiffViewState,
    pub inline: InlineThreadState,
    pub selection: LineSelection,
    pub filename_search: FilenameSearchState,
    /// 画像・動画を ASCII アートとして出す。
    pub media_state: MediaState,
    pub click: PointerState,
    /// タブ行のクリック領域。描画が書き、マウス処理が幅を計算し直さずに同じ
    /// ジオメトリを引けるようにする。
    pub tab_row_hits: crate::hit_map::ColumnSpans<crate::ui::tab_bar::TabAction>,
    /// 最初に表示するタブ。描画が解決後の値を書き戻す。
    pub tab_scroll: usize,
    /// 次の描画で窓をアクティブなタブへ寄せるか。切り替えた側が立て、描画が下ろす。
    /// 常時立てておくと、隠れたタブを覗く操作がその場で巻き戻される。
    pub tab_reveal: bool,
    /// gd, gi, gr の 2 打目待ち。
    pub pending_g_key: bool,
    /// za, zc, zo, zm, zr, zR, zM の 2 打目待ち。
    pub pending_z_key: bool,
    /// ブランチの change summary を表示しているか。diff_view.diff_mode とは排他。
    pub show_summary: bool,
    pub summary_scroll: usize,
    /// 折り返し後の総行数。描画が書き、キーハンドラがクランプに読む。
    pub summary_total_lines: usize,
    /// markdown を生のソースではなくレンダリング済みで出すか。セッション内で持続し、
    /// markdown 以外を開いている間は無視される。判定は is_showing_rendered_markdown
    /// を通すこと。
    pub md_rendered: bool,
    /// md_rendered と違い、ファイルごとに open_file がリセットする。
    pub md_scroll: usize,
    /// 折り返し後の総行数。描画が書き、キーハンドラがクランプに読む。
    pub md_total_lines: usize,
}
