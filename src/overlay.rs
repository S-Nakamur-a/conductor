//! オーバーレイの状態。表示中のものは [ActiveOverlay] が 1 つだけ指す。

use crate::background::BackgroundOp;
use crate::claude_sessions::ResumableSession;
use crate::git_engine::CommitInfo;
use crate::grep_search::GrepProgress;
use crate::review_store::SessionHistory;
use crate::search_result_tree::SearchResultTree;
use crate::text_input::TextInput;
use crate::types::Focus;

/// 現在アクティブなオーバーレイ (同時に高々 1 つ)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveOverlay {
    #[default]
    None,
    SwitchBranch,
    Grab,
    Prune,
    CherryPick,
    History,
    ResumeSession,
    RepoSelector,
    OpenRepo,
    /// 「Review: Review Pull Request…」用の PR 番号・URL の入力。取り込みに
    /// 失敗しても入力内容を保ったまま開いたままにするので、打ち直さずに
    /// 修正して再試行できる。
    PrInput,
    GrepSearch,
    Help,
    CommandPalette,
    /// worktree の切り替え。左の worktree カラムを置き換えたモーダル。
    /// 既存の worktree 一覧の状態と handle_worktree_key を再利用する。
    WorktreeSwitcher,
    /// 全画面のコメント一覧。ブランチ上の全レビューコメントの俯瞰と、
    /// 該当箇所へのジャンプ。コメント一覧の状態とハンドラを再利用する。
    CommentList,
    /// テーマピッカー。上下で切り替え、移動のたびにライブプレビュー、Enter で
    /// 確定、Esc でピッカーを開いた時点のテーマへ戻す。
    ThemePicker,
    /// レビューを作る前の確認。
    RevidereConfirm,
}

/// 確認を出した時点で、その worktree の成果物がどうなっているか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RevidereArtifact {
    /// まだ無い。
    #[default]
    None,
    /// あるが、解析したときのコミットから先へ進んでいる。
    Stale,
    /// いまのコミットを見て作られている。
    Current,
}

/// レビューを作る前の確認のオーバーレイ状態。
///
/// AI の呼び出しは数分と費用がかかるので、w / W の押し間違いや、メニューの
/// 隣の行を選んだだけで走り出さないようにしている。文言と、作り直しに
/// 貯めた応答を捨てるかどうかは artifact が決める。
#[derive(Default)]
pub struct RevidereConfirmOverlay {
    pub branch: String,
    /// 見る区間の呼び名 ([crate::revidere::scope_label])。区間はビューを
    /// 閉じても残るので、いまどちらを作ろうとしているかを名乗る。
    pub scope: &'static str,
    pub artifact: RevidereArtifact,
}

#[derive(Default)]
pub struct ThemePickerOverlay {
    /// 選べるテーマ名を表示順に並べたもの (Theme::all_names を参照)。
    pub themes: Vec<String>,
    /// themes 内で現在ハイライトしている添字。
    pub selected: usize,
    /// ピッカーを開いた時点で有効だった theme_name。Esc で戻すのに使う。
    pub original: String,
}

#[derive(Default)]
pub struct SwitchBranchOverlay {
    pub branches: Vec<String>,
    pub selected: usize,
    pub filter: TextInput,
}

#[derive(Default)]
pub struct GrabOverlay {
    pub branches: Vec<String>,
    pub selected: usize,
    pub filter: TextInput,
}

#[derive(Default)]
pub struct CherryPickOverlay {
    pub source_branch: String,
    pub commits: Vec<CommitInfo>,
    pub selected: usize,
}

#[derive(Default)]
pub struct PruneOverlay {
    pub stale: Vec<String>,
}

#[derive(Default)]
pub struct ResumeSessionOverlay {
    pub sessions: Vec<ResumableSession>,
    pub selected: usize,
    pub filter: TextInput,
    pub all_projects: bool,
}

#[derive(Default)]
pub struct GrepSearchOverlay {
    pub query: TextInput,
    pub result_tree: SearchResultTree,
    pub selected: usize,
    pub scroll: usize,
    pub running: bool,
    pub bg_op: BackgroundOp<GrepProgress>,
    pub regex_mode: bool,
    pub case_sensitive: bool,
    /// インクリメンタル検索のデバウンスタイマー。
    pub debounce_deadline: Option<std::time::Instant>,
    /// 第 1 段階 (最近変更されたファイルのみ) の結果を表示中かどうか。
    pub phase1_active: bool,
    /// 2 段階のインクリメンタル検索における、第 2 段階 (全体検索) の
    /// バックグラウンド処理。
    pub bg_op_phase2: BackgroundOp<GrepProgress>,
    /// バックグラウンド検索からの生のマッチを溜める。完了時にツリーへ組み直す。
    pub pending_matches: Vec<crate::grep_search::GrepMatch>,
    /// クエリの入力欄にフォーカスがあるか (true)、結果一覧にあるか (false)。
    /// オーバーレイを開いたとき入力欄にフォーカスが来るよう既定は true。
    pub input_focused: bool,
}

#[derive(Default)]
pub struct CommandPaletteOverlay {
    pub filter: TextInput,
    pub selected: usize,
}

#[derive(Default)]
pub struct HistoryOverlay {
    pub records: Vec<SessionHistory>,
    pub selected: usize,
    pub search_query: TextInput,
    pub search_active: bool,
}

#[derive(Default)]
pub struct RepoSelectorOverlay {
    pub selected: usize,
}

#[derive(Default)]
pub struct OpenRepoOverlay {
    pub buffer: TextInput,
}

/// PR 番号・URL 入力のオーバーレイ状態 (「Review: Review Pull Request…」)。
#[derive(Default)]
pub struct PrInputOverlay {
    pub buffer: TextInput,
    /// このオーバーレイに対する PR 取り込み (gh / git) がバックグラウンドで
    /// 動いているあいだ立つ。
    pub loading: bool,
    /// 取り込みに失敗したときに設定し、次の Enter または編集でクリアする。
    /// オーバーレイは開いたままで buffer にも触らないので、修正して
    /// 再試行できる。
    pub error: Option<String>,
    pub bg_op: BackgroundOp<crate::pr_intake::PrIntakeOutcome>,
}

/// 参照一覧の 1 行。ファイルの見出しか、その中の 1 件か。
pub enum RefRow<'a> {
    File {
        path: &'a str,
        count: usize,
        collapsed: bool,
    },
    Hit {
        index: usize,
    },
}

/// コードナビゲーション: 参照一覧のオーバーレイ状態 (gr = Find References)。
#[derive(Default)]
pub struct ReferencesOverlay {
    pub active: bool,
    pub symbol_name: String,
    pub results: Vec<crate::symbol_index::Reference>,
    /// [`Self::rows`] が返す行の番号。結果そのものの番号ではない。
    pub selected: usize,
    pub scroll: usize,
    /// 畳んでいるファイル。
    pub collapsed: std::collections::HashSet<String>,
}

impl ReferencesOverlay {
    /// 結果を差し替えて開く。
    ///
    /// ファイルが複数あるときは最初の 1 つだけ開いておく。実索引で 1 シンボルが
    /// 15 ファイル 63 箇所に散るので、全部開くと見出しが流れて数が読めない。
    pub fn show(&mut self, title: String, results: Vec<crate::symbol_index::Reference>) {
        self.collapsed = {
            let mut files: Vec<&str> = Vec::new();
            for r in &results {
                if !files.contains(&r.file_path.as_str()) {
                    files.push(&r.file_path);
                }
            }
            files.iter().skip(1).map(|f| f.to_string()).collect()
        };
        self.active = true;
        self.symbol_name = title;
        self.results = results;
        self.selected = 0;
        self.scroll = 0;
    }

    /// ファイルごとにまとめた表示行。畳んだファイルは見出しだけ。
    pub fn rows(&self) -> Vec<RefRow<'_>> {
        let mut order: Vec<&str> = Vec::new();
        let mut groups: std::collections::HashMap<&str, Vec<usize>> = Default::default();
        for (i, r) in self.results.iter().enumerate() {
            let path = r.file_path.as_str();
            groups
                .entry(path)
                .or_insert_with(|| {
                    order.push(path);
                    Vec::new()
                })
                .push(i);
        }
        let mut rows = Vec::new();
        for path in order {
            let hits = &groups[path];
            let collapsed = self.collapsed.contains(path);
            rows.push(RefRow::File {
                path,
                count: hits.len(),
                collapsed,
            });
            if !collapsed {
                rows.extend(hits.iter().map(|&index| RefRow::Hit { index }));
            }
        }
        rows
    }

    /// 検査用。行の並びを綴りで見る。
    #[cfg(test)]
    fn row_labels(&self) -> Vec<String> {
        self.rows()
            .iter()
            .map(|r| match r {
                RefRow::File {
                    path,
                    count,
                    collapsed,
                } => format!("{} {path} ({count})", if *collapsed { '+' } else { '-' }),
                RefRow::Hit { index } => format!("  {}", self.results[*index].line),
            })
            .collect()
    }

    /// 見出しの開閉を切り替える。選択行が消えないよう、その見出し自身に選択を寄せる。
    pub fn toggle(&mut self, path: &str) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_string());
        }
        self.select_file(path);
    }

    /// そのファイルの見出しへ選択を移す。
    pub fn select_file(&mut self, path: &str) {
        self.selected = self
            .rows()
            .iter()
            .position(|r| matches!(r, RefRow::File { path: p, .. } if *p == path))
            .unwrap_or(0);
    }
}

/// Vimium 風のナビゲーション中に表示するシンボルのヒント 1 件。
#[derive(Debug, Clone)]
pub struct SymbolHint {
    /// 2 文字のラベル (例: "aa", "ab")。
    pub label: String,
    /// シンボル名 (例: "AppState")。
    pub symbol_name: String,
    /// 1 始まりの行番号。
    pub line: usize,
    /// 内容中の開始桁 (0 始まり)。
    pub start_col: usize,
}

/// ヒントを選んだあとに走らせる操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintAction {
    Definition,
    Implementation,
    References,
    Hover,
}

/// Vimium 風のシンボルヒントのオーバーレイ。Viewer で g を押すと出る。
#[derive(Default)]
pub struct SymbolHintOverlay {
    pub active: bool,
    /// 見えているシンボルに対して生成した全ヒント。
    pub hints: Vec<SymbolHint>,
    /// ラベル照合のためにここまで入力された文字 (0〜2 文字)。
    pub input: String,
    /// 選択後に走らせる操作。None なら従来どおりアクションメニューを開く。
    ///
    /// gd / gr が行内の語を選ばせるために使う。この経路のラベルは 1 文字なので、
    /// 「入力が溜まったか」ではこの状態を判別できず、別の印が要る。
    pub pending: Option<HintAction>,
}

/// 選択したシンボルに対して実行できるアクション。
#[derive(Debug, Clone)]
pub struct SymbolAction {
    /// 押すキー (例: 'd', 'i', 'r')。
    pub key: char,
    /// 説明 (例: "Go to definition")。
    pub label: String,
    /// 対象のファイルパス。
    pub file_path: String,
    /// 対象の行番号 (1 始まり)。
    pub line: usize,
}

/// シンボルのヒントを選んだあとに出るアクション選択モーダル。
#[derive(Default)]
pub struct SymbolActionOverlay {
    pub active: bool,
    pub symbol_name: String,
    pub actions: Vec<SymbolAction>,
    pub selected: usize,
    /// 元のシンボルがあった画面上の行 (0 始まり)。ジャンプ時に縦位置を
    /// 保つために使う。
    pub source_screen_row: usize,
}

/// マウスやカーソルが乗っていて、待機のデバウンスが明けてホバーポップアップが
/// 解決されるのを待っているシンボル。resolved は (ポップアップが出たかどうかに
/// かかわらず) 検索を試みた時点で true になる。カーソルが止まっている間、
/// フレームごとの処理が毎フレーム計算し直さないようにするため。
pub struct HoverCandidate {
    /// カーソル・マウスの下にある識別子。
    pub symbol: String,
    /// シンボルがある内容行 (1 始まり)。
    pub line: usize,
    /// シンボルがあるファイル。定義の絞り込みに使う。
    pub file: Option<String>,
    /// シンボルの画面上の絶対行。ポップアップはこの行のすぐ下 (余白が無ければ上)
    /// に配置する。
    pub anchor_row: u16,
    /// シンボル先頭の画面上の絶対桁。横方向の配置に使う。
    pub anchor_col: u16,
    /// ソース行におけるシンボルの開始桁 (0 始まり、h_scroll 適用前の内容文字での桁)。
    /// 解決後に HoverInfoOverlay::target_* へ引き継がれ、マウスの現在位置とは
    /// 無関係にポップアップの対象をハイライトし続けられるようにする。
    pub start_col: usize,
    /// 終了桁 (この桁は含まない)。start_col を参照。
    pub end_col: usize,
    /// カーソル・マウスがこのシンボル上で止まった時刻。
    pub since: std::time::Instant,
    /// この候補に対する検索が既に走ったか。
    pub resolved: bool,
}

/// コードプレビュー (第 2 階層)。参照の周辺のソース行の窓で、参照一覧の行を
/// クリックすると表示される。
pub struct HoverPreview {
    /// プレビュー元のファイル (リポジトリ相対)。
    pub file: String,
    /// プレビューの中心となる参照行 (1 始まり)。
    pub center_line: usize,
    /// 表示する各行の (1 始まりの行番号, テキスト)。
    pub lines: Vec<(usize, String)>,
    /// 描画された矩形。当たり判定のために描画側が書き込む。
    pub rect: ratatui::layout::Rect,
}

/// 参照一覧 (第 1 階層)。基本のホバーポップアップで N refs をクリックすると開く。
/// マウス優先で、行をクリックすると [HoverPreview] が開く。
pub struct HoverRefs {
    /// これらの参照が属するシンボル (一覧のタイトル)。
    pub symbol: String,
    /// 見つかった全参照。
    pub results: Vec<crate::symbol_index::Reference>,
    /// ハイライトしている行 (キーボード操作とプレビューの対象)。
    pub selected: usize,
    /// 最初に見えている行の添字。
    pub scroll: usize,
    /// 描画された一覧ポップアップの矩形。描画側が書き込む。
    pub rect: ratatui::layout::Rect,
    /// 見えている各行の (結果の添字, 行の矩形)。描画側が書き込む。
    pub row_hits: Vec<(usize, ratatui::layout::Rect)>,
    /// 行がクリックされていれば、開いているプレビュー。
    pub preview: Option<HoverPreview>,
}

/// シンボルのホバー情報ポップアップ。Viewer のカーソル下にあるシンボルの
/// シグネチャ・doc・参照。マウスがシンボル上で止まるか、キーボードのカーソルが
/// 動かなくなると自動で表示される。info は解決済みのポップアップ
/// (None は非表示)、pending は待機のデバウンスを数えている候補。
/// anchor_row と anchor_col は配置に使う、解決したシンボルの画面上の位置。
///
/// ここから対話的なモーダルの階層へ発展し得る: N refs をクリックすると
/// ポップアップが固定され [HoverRefs] が開き、行をクリックすると
/// [HoverPreview] が開く。pinned のポップアップは Esc か外側のクリックまで
/// フォーカスや待機の解除を生き延びる。leave_at は、まだ一時的なポップアップを
/// マウスがシンボルから外れたあと少しだけ生かしておく猶予 (カーソルが
/// ポップアップまで移動してクリックできるように)。
#[derive(Default)]
pub struct HoverInfoOverlay {
    pub info: Option<crate::viewer::hover_info::HoverInfo>,
    pub pending: Option<HoverCandidate>,
    pub anchor_row: u16,
    pub anchor_col: u16,
    pub pinned: bool,
    pub leave_at: Option<std::time::Instant>,
    /// 現在の info を解決したときに表示していたファイル。(固定されていない)
    /// ポップアップの下で Viewer がファイルを切り替えると、これが
    /// content.current_file と一致しなくなり、古くなったポップアップが
    /// 毎フレームの処理で落とされる。
    pub shown_file: Option<String>,
    /// info が説明しているシンボルのソース行 (1 始まり)。ポップアップが
    /// 表示されているあいだ、PointerState::hover_symbol とは独立に描画側が
    /// そのシンボルをハイライトし続けられるようにする。マウスは既にそこから
    /// 外れているかもしれないし、ポップアップの離脱猶予の中にいるかもしれないが、
    /// 下線そのものにはそうした猶予が無いため。
    pub target_line: usize,
    /// target_line 上のハイライト対象シンボルの開始桁 (target_line を参照)。
    pub target_start_col: usize,
    /// ハイライト対象シンボルの終了桁 (この桁は含まない)。
    pub target_end_col: usize,
    /// 基本ポップアップの矩形。当たり判定のために描画側が書き込む。
    pub info_rect: ratatui::layout::Rect,
    /// 基本ポップアップ内の N refs のクリック可能領域 (シンボルに参照が
    /// 無ければ大きさ 0)。描画側が書き込む。
    pub refs_hit: ratatui::layout::Rect,
    /// 定義位置の行のクリック可能領域。押すとその定義へ飛ぶ。描画側が書き込む。
    pub def_hit: ratatui::layout::Rect,
    pub refs: Option<HoverRefs>,
}

impl HoverInfoOverlay {
    pub fn is_shown(&self) -> bool {
        self.info.is_some()
    }

    /// ホバーのモーダル階層全体を非表示・非固定に戻す。
    pub fn reset(&mut self) {
        self.info = None;
        self.pending = None;
        self.pinned = false;
        self.leave_at = None;
        self.shown_file = None;
        self.target_line = 0;
        self.target_start_col = 0;
        self.target_end_col = 0;
        self.refs = None;
        self.info_rect = ratatui::layout::Rect::default();
        self.refs_hit = ratatui::layout::Rect::default();
    }
}

/// ヘルプオーバーレイの状態。
pub struct HelpOverlay {
    pub context: Focus,
}

impl Default for HelpOverlay {
    fn default() -> Self {
        Self {
            context: Focus::Worktree,
        }
    }
}

/// オーバーレイの状態をまとめて持つ入れ物。App にあった個別のフィールドを置き換える。
#[derive(Default)]
pub struct OverlayManager {
    /// 現在アクティブなオーバーレイ。
    pub active: ActiveOverlay,
    pub switch_branch: SwitchBranchOverlay,
    pub grab: GrabOverlay,
    pub prune: PruneOverlay,
    pub cherry_pick: CherryPickOverlay,
    pub history: HistoryOverlay,
    pub resume_session: ResumeSessionOverlay,
    pub repo_selector: RepoSelectorOverlay,
    pub open_repo: OpenRepoOverlay,
    pub pr_input: PrInputOverlay,
    pub grep_search: GrepSearchOverlay,
    pub help: HelpOverlay,
    pub command_palette: CommandPaletteOverlay,
    pub theme_picker: ThemePickerOverlay,
    pub revidere_confirm: RevidereConfirmOverlay,
}

#[cfg(test)]
mod references_tests {
    use super::*;
    use crate::symbol_index::Reference;

    fn hit(file: &str, line: usize) -> Reference {
        Reference {
            file_path: file.to_string(),
            line,
            content: String::new(),
        }
    }

    #[test]
    fn 開いたときは最初のファイルだけ展開する() {
        let mut o = ReferencesOverlay::default();
        o.show(
            "sym".into(),
            vec![hit("a.rs", 1), hit("a.rs", 9), hit("b.rs", 4)],
        );
        assert_eq!(o.row_labels(), ["- a.rs (2)", "  1", "  9", "+ b.rs (1)"]);
    }

    #[test]
    fn 同じファイルが飛び飛びでも1つの見出しにまとまる() {
        // 索引と tree-sitter で並び順が違う。並びに依存すると片方だけ壊れる。
        let mut o = ReferencesOverlay::default();
        o.show(
            "sym".into(),
            vec![hit("a.rs", 1), hit("b.rs", 4), hit("a.rs", 9)],
        );
        assert_eq!(o.row_labels(), ["- a.rs (2)", "  1", "  9", "+ b.rs (1)"]);
    }

    #[test]
    fn 畳んでも選択はその見出しに残る() {
        let mut o = ReferencesOverlay::default();
        o.show(
            "sym".into(),
            vec![hit("a.rs", 1), hit("a.rs", 9), hit("b.rs", 4)],
        );
        o.selected = 2; // a.rs の 2 件目
        o.toggle("a.rs");
        assert_eq!(o.selected, 0, "消えた行に選択が残っている");
        assert_eq!(o.row_labels(), ["+ a.rs (2)", "+ b.rs (1)"]);
    }
}
