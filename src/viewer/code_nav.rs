//! コードナビゲーション: カーソル位置のシンボル検索、定義へのジャンプ、ジャンプ履歴、
//! 画面上のシンボルヒントを扱う。
//!
//! シンボルインデックス・意味索引のバックグラウンド構築の起点だけは
//! [crate::app::code_nav] に残っている（App のライフサイクル入口から呼ばれるため）。

use crate::app::{App, StatusLevel};
use crate::overlay::{HintAction, SymbolHint, SymbolHintOverlay};
use crate::symbol_index::is_rust_keyword;
use crate::types::Focus;
use crate::viewer::hover_info::HoverInfo;
use sheaf_core::{Definition, Location, References};

/// gd / gr がカーソル行から決めた対象。
pub enum LinePick {
    /// 行に候補が 1 つだけあった (行番号・出現番号・綴り)。
    One(usize, usize, String),
    /// 候補が複数あったのでヒントを出した。続きは選択後に走る。
    Asked,
    /// 候補が無い。
    None,
}

/// どの層が答えたか。
///
/// 飛んだ先の見た目からは区別が付かない。索引が答えたのか構文層に落ちたのかで
/// 行番号の信頼度が違うので、答えるたびに名乗る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsweredBy {
    /// SCIP 索引。聞かれた位置のファイルも飛び先も索引生成時のまま。
    Index,
    /// 構文層。索引に無い語、生成後に変わったファイル、索引そのものが無い場合。
    TreeSitter,
}

impl AnsweredBy {
    fn label(self) -> &'static str {
        match self {
            AnsweredBy::Index => "index",
            AnsweredBy::TreeSitter => "tree-sitter",
        }
    }
}

/// 索引の宣言をポップアップに載せる最大行数。超えたら … で切り詰める。
/// 索引の宣言は where 節を畳まずに書くので、長いものは実際に長い。
const MAX_INDEX_SIGNATURE_LINES: usize = 8;

/// 同じ位置に複数の符号が乗ることがある (`Struct { file_path }` の省略記法なら
/// フィールドとローカル束縛の両方) ので、説明を持っている方を先に採る。
fn indexed_detail(
    described: &[sheaf_core::SymbolDetail],
) -> crate::viewer::hover_info::IndexedDetail {
    let detail = described
        .iter()
        .find(|d| d.signature.is_some())
        .or_else(|| described.first());
    let Some(detail) = detail else {
        return Default::default();
    };
    let mut signature_lines: Vec<String> = detail
        .signature
        .iter()
        .flat_map(|s| s.lines())
        .map(str::to_string)
        .collect();
    if signature_lines.len() > MAX_INDEX_SIGNATURE_LINES {
        signature_lines.truncate(MAX_INDEX_SIGNATURE_LINES);
        signature_lines.push("…".to_string());
    }
    crate::viewer::hover_info::IndexedDetail {
        kind: crate::semantic_index::kind_label(detail.kind).to_string(),
        signature_lines,
        doc_lines: detail
            .documentation
            .iter()
            .flat_map(|d| d.lines())
            .map(str::to_string)
            .collect(),
    }
}

/// 意味索引への 1 回の問い合わせに要るもの。タブ展開前の座標で持つ。
struct SemanticSite {
    rel: std::path::PathBuf,
    abs: std::path::PathBuf,
    source: String,
    line: u32,
    col: u32,
}

impl App {
    /// 意味索引に位置で聞き、答えなければシンボルインデックスを名前で引く。索引の
    /// 答えは gd の飛び先と同じなので、両者の説明がずれない。
    fn hover_info_at(&self, symbol: &str, line_1: usize, start_col: usize) -> Option<HoverInfo> {
        let current_file = self.viewer.content.current_file.clone();
        // 索引への問い合わせは 1 回にまとめる。所属と説明で別々に聞くと、同じ位置に
        // 2 回 Document をデコードすることになる。
        let described = self.semantic_description(line_1, start_col);
        let site = self
            .semantic_def_site(line_1, start_col, described.as_deref())
            .or_else(|| {
                crate::viewer::hover_info::resolve_def_site(
                    &self.code_nav.index,
                    symbol,
                    current_file.as_deref(),
                )
            })?;
        let same_line =
            Some(site.file_path.as_str()) == current_file.as_deref() && site.line == line_1;
        let mut info =
            crate::viewer::hover_info::build_hover_info(&self.code_nav.index, symbol, site)?;
        info.on_definition_line = same_line;
        info.container = described
            .as_ref()
            .and_then(|d| d.iter().find_map(|s| s.container.clone()));
        Some(info)
    }

    /// top が中にいるシンボルのうち、宣言が画面の外にあるいちばん内側のもの。
    pub fn sticky_declaration_line(&mut self, top: usize) -> Option<usize> {
        let key = (self.viewer.content.current_file.clone()?, top);
        if self.code_nav.sticky.asked.as_ref() != Some(&key) {
            let found = self.enclosing_declaration_out_of_view(&key.0, key.1);
            self.code_nav.sticky.declaration = found;
            self.code_nav.sticky.asked = Some(key);
        }
        self.code_nav.sticky.declaration
    }

    fn enclosing_declaration_out_of_view(&self, rel: &str, top: usize) -> Option<usize> {
        let tree_root = self.selected_worktree_path();
        let store = self.code_nav.semantic.store(&tree_root)?;
        let sheaf_core::Enclosures::Exact(found) =
            sheaf_core::enclosures_at(store, std::path::Path::new(rel), top as u32)
        else {
            return None;
        };
        found
            .iter()
            .map(|e| e.declaration.line as usize)
            .find(|line| *line < top)
    }

    /// 意味索引に、その位置の語について書いてあることを聞く。
    fn semantic_description(
        &self,
        line_1: usize,
        start_col: usize,
    ) -> Option<Vec<sheaf_core::SymbolDetail>> {
        let tree_root = self.selected_worktree_path();
        let store = self.code_nav.semantic.store(&tree_root)?;
        let line_idx = line_1.checked_sub(1)?;
        let occurrence = self.occurrence_at_rendered_column(line_idx, start_col)?;
        let site = self.semantic_site(&tree_root, line_idx, occurrence)?;
        let bridge = self.bridge(&site);
        Some(sheaf_core::describe_at(
            store, &bridge, &site.rel, site.line, site.col,
        ))
    }

    /// 意味索引に定義位置を聞く。Exact のときだけ採る -- Enclosing は「囲んでいる
    /// 型の定義」であって、聞かれた語の定義ではない。
    fn semantic_def_site(
        &self,
        line_1: usize,
        start_col: usize,
        described: Option<&[sheaf_core::SymbolDetail]>,
    ) -> Option<crate::viewer::hover_info::DefSite> {
        let line_idx = line_1.checked_sub(1)?;
        let occurrence = self.occurrence_at_rendered_column(line_idx, start_col)?;
        let Definition::Exact(locations) = self.semantic_definition(line_idx, occurrence)? else {
            return None;
        };
        let first = locations.first()?;
        Some(crate::viewer::hover_info::DefSite {
            file_path: first.path.to_string_lossy().into_owned(),
            line: first.line as usize + 1,
            // 索引が種別を答えるので、tree-sitter から借りるのはもうやめている。
            kind: String::new(),
            def_count: locations.len(),
            detail: described.map(indexed_detail),
        })
    }

    /// ビューアのカーソル行から選んだシンボルについて、ホバーのポップアップを出す (K キー)。
    /// ユーザが意図して押した操作なので、出せない場合もステータスでフィードバックを返す。
    /// 受動的な自動ホバーとの違いはそこ。
    pub fn show_hover_info_at(&mut self, line_idx: usize, symbol: &str) {
        use crate::app::StatusLevel;

        let current_file = self.viewer.content.current_file.clone();
        let start_col = self
            .viewer
            .content
            .file_content
            .get(line_idx)
            .and_then(|line| {
                crate::symbol_index::code_identifiers_on_line(
                    line,
                    line_idx + 1,
                    &self.viewer.content.code_mask,
                )
                .find(|(_, _, word)| word == symbol)
                .map(|(_, start, _)| start)
            });
        let info = start_col.and_then(|col| self.hover_info_at(symbol, line_idx + 1, col));
        match info {
            Some(info) => {
                self.code_nav.hover_info.shown_file = current_file;
                self.code_nav.hover_info.info = Some(info);
            }
            None => {
                self.set_status(
                    format!("No definition indexed for '{symbol}'"),
                    StatusLevel::Info,
                );
            }
        }
    }

    /// フォーカスが Viewer にあり、ファイルが開いていて、オーバーレイやサマリーが
    /// 画面を占有していないこと。
    fn hover_auto_allowed(&self) -> bool {
        self.focus.current() == Focus::Viewer
            && !self.viewer.is_summary()
            && self.overlays.active == crate::overlay::ActiveOverlay::None
            && !self.code_nav.references.active
            && !self.code_nav.symbol_action.active
            && !self.code_nav.symbol_hint.active
            && self.viewer.content.current_file.is_some()
    }

    /// ホバーのモーダルスタック全体（ポップアップ、保留中の候補、参照リスト、
    /// プレビュー、ピン留め）をクリアする。何か表示されていたかを返す。
    pub fn clear_hover(&mut self) -> bool {
        let had = self.code_nav.hover_info.info.is_some()
            || self.code_nav.hover_info.pending.is_some()
            || self.code_nav.hover_info.pinned;
        self.code_nav.hover_info.reset();
        had
    }

    /// マウスホバーに関する状態 (ジャンプ用の下線、ポップアップスタック、Explorer の行
    /// ハイライト) をまとめてクリアする。crossterm はマウスが端末ウィンドウの外に出た
    /// ことを報告しないので、「どこにも乗っていない」と確実に言えるイベントから呼ぶ。
    pub fn clear_all_hover(&mut self) {
        self.clear_pointer_hover();
        // ピン留めされたモーダルはキーボード操作によるもので、フォーカスやアイドルによる
        // 消失を免れる (HoverInfoOverlay::pinned と tick_hover の早期リターン)。
        if !self.code_nav.hover_info.pinned {
            self.clear_hover();
        }
    }

    /// ポインタ操作によるハイライト (ジャンプ用の下線と、行・チップ・タブのホバー) だけを
    /// クリアする。
    ///
    /// ホバーポップアップのスタックにあえて触れないのは、キー入力ごとの状態遷移を
    /// handle_key_event が解決しており、ここで先に pinned を戻すとモーダルのキーボード
    /// 経路が到達不能になり Esc が二重に発火するため。
    ///
    /// ここでクリアする対象には、キー入力に対する他の解除経路が無い。crossterm は
    /// ポインタがウィンドウの外に出たことを報告しないので、点いたままになる。
    pub fn clear_pointer_hover(&mut self) {
        self.viewer.click.hover_symbol = None;
        self.viewer.click.underline_pending = None;
        self.list_hover.clear();
        self.wtbar.hover = None;
        self.terminal.claude.tab_hover = None;
        self.terminal.shell.tab_hover = None;
        self.revidere.badge_hover = false;
    }

    /// マウスが現在乗っているシンボルを記録する (マウス移動イベントから)。cand は
    /// (symbol, 1始まりの行, anchor_row, anchor_col, start_col, end_col) で、anchor は
    /// 画面上の絶対座標、col は h_scroll 適用前の 0 始まりのコンテンツ列。新しいシンボルに
    /// 移るとアイドルカウントダウンがリセットされ、前のポップアップは破棄される。
    pub fn set_mouse_hover_candidate(
        &mut self,
        cand: Option<(String, usize, u16, u16, usize, usize)>,
    ) {
        match cand {
            Some((symbol, line, anchor_row, anchor_col, start_col, end_col)) => {
                let same = self
                    .code_nav
                    .hover_info
                    .pending
                    .as_ref()
                    .is_some_and(|c| c.symbol == symbol && c.line == line);
                if same {
                    return;
                }
                let file = self.viewer.content.current_file.clone();
                self.code_nav.hover_info.pending = Some(crate::overlay::HoverCandidate {
                    symbol,
                    line,
                    file,
                    anchor_row,
                    anchor_col,
                    start_col,
                    end_col,
                    since: std::time::Instant::now(),
                    resolved: false,
                });
                self.code_nav.hover_info.leave_at = None;
                if self.code_nav.hover_info.info.take().is_some() {
                    self.request_redraw();
                }
            }
            None => {
                // ポップアップが表示中なら即座には消さず、猶予を置く (tick_hover)。カーソルを
                // ポップアップまで移動してクリックできるようにするため。
                if self.code_nav.hover_info.info.is_some() {
                    self.code_nav.hover_info.pending = None;
                    if self.code_nav.hover_info.leave_at.is_none() {
                        self.code_nav.hover_info.leave_at = Some(std::time::Instant::now());
                    }
                } else if self.code_nav.hover_info.pending.take().is_some() {
                    self.request_redraw();
                }
            }
        }
    }

    /// ジャンプ用の下線のために、マウスが乗っているシンボルを記録する。cand は
    /// (symbol, 1始まりの行, start_col, end_col)。has_jump_modifier はこの移動時点の
    /// Cmd/Ctrl の状態で、下線の色を決める。解決済みのシンボルの上でも都度更新するので、
    /// デバウンスをやり直さずに修飾キーの押下・解放で色が切り替わる。
    ///
    /// ポップアップと違い離脱時の猶予は無い。シンボルから離れると下線は即座に消える。
    pub fn set_underline_candidate(
        &mut self,
        cand: Option<(String, usize, usize, usize)>,
        has_jump_modifier: bool,
    ) {
        match cand {
            Some((symbol, line, start_col, end_col)) => {
                if let Some(hs) = self.viewer.click.hover_symbol.as_mut()
                    && hs.line == line
                    && hs.start_col == start_col
                    && hs.end_col == end_col
                {
                    hs.has_jump_modifier = has_jump_modifier;
                    return;
                }
                let same_pending = self
                    .viewer
                    .click
                    .underline_pending
                    .as_ref()
                    .is_some_and(|p| {
                        p.line == line && p.start_col == start_col && p.end_col == end_col
                    });
                if same_pending {
                    if let Some(p) = self.viewer.click.underline_pending.as_mut() {
                        p.has_jump_modifier = has_jump_modifier;
                    }
                    return;
                }
                self.viewer.click.underline_pending = Some(crate::viewer::PendingUnderline {
                    symbol,
                    line,
                    start_col,
                    end_col,
                    since: std::time::Instant::now(),
                    resolved: false,
                    has_jump_modifier,
                });
                self.viewer.click.hover_symbol = None;
            }
            None => {
                self.viewer.click.underline_pending = None;
                self.viewer.click.hover_symbol = None;
            }
        }
    }

    /// 指定した画面上の絶対座標が、ホバーモーダルスタック（基本のポップアップ、
    /// 参照リスト、プレビューのいずれか）の内側にあれば true。マウスが上に
    /// あるあいだポップアップを維持したり、クリックを振り分けたりするのに使う。
    pub fn hover_point_hit(&self, col: u16, row: u16) -> bool {
        let hv = &self.code_nav.hover_info;
        let in_rect = |r: ratatui::layout::Rect| {
            r.width > 0
                && r.height > 0
                && col >= r.x
                && col < r.x + r.width
                && row >= r.y
                && row < r.y + r.height
        };
        if in_rect(hv.info_rect) {
            return true;
        }
        if let Some(refs) = &hv.refs {
            if in_rect(refs.rect) {
                return true;
            }
            if let Some(p) = &refs.preview
                && in_rect(p.rect)
            {
                return true;
            }
        }
        false
    }

    /// 毎フレーム呼ばれる自動ホバーの駆動処理。マウスがシンボルの上でデバウンス時間を
    /// 超えて静止したらポップアップを解決する。猶予期間の管理や、ファイル切り替え・
    /// フォーカス喪失による無効化も担う。
    pub fn tick_hover(&mut self) {
        /// マウスがシンボルの上で静止してからポップアップが現れるまでの時間。
        const HOVER_IDLE: std::time::Duration = std::time::Duration::from_millis(350);
        /// マウスがシンボルから離れた後も、一時的なポップアップを維持しておく
        /// 猶予期間。カーソルをポップアップまで移動してクリックできるようにする。
        const HOVER_GRACE: std::time::Duration = std::time::Duration::from_millis(700);

        // ピン留めされたモーダルはユーザ操作によるもので、フォーカスやアイドル
        // による消失を免れ、Esc か外側のクリックでのみ閉じる（イベント層で処理）。
        if self.code_nav.hover_info.pinned {
            return;
        }

        // ポップアップ表示中にビューアが別ファイルへ切り替わったら、猶予期間中でも破棄する。
        // 無関係なコードの上に残り続けないようにするため。
        if self.code_nav.hover_info.info.is_some()
            && self.code_nav.hover_info.shown_file != self.viewer.content.current_file
        {
            if self.clear_hover() {
                self.request_redraw();
            }
            return;
        }

        // 猶予期間: マウスが離れたシンボルのポップアップはしばらく残るが、それは
        // マウスが実際にポップアップの上にあるか、タイマーが切れていない間だけ。
        if let Some(left) = self.code_nav.hover_info.leave_at
            && left.elapsed() >= HOVER_GRACE
        {
            if self.clear_hover() {
                self.request_redraw();
            }
            return;
        }

        if !self.hover_auto_allowed() {
            // 猶予期間内のポップアップは消さない。ユーザがポップアップに向かって
            // マウスを動かしている途中かもしれず、その間は一時的にコンテンツ領域を
            // 外れる。
            if self.code_nav.hover_info.leave_at.is_some() {
                return;
            }
            if self.clear_hover() {
                self.request_redraw();
            }
            return;
        }

        // マウス静止だけで駆動する。最上行やキーボードに基づくヒューリスティクスは、行単位の
        // テキストカーソルが無い以上「カーソル行」が常に画面最上行になり、指していない
        // コードで発火するので採らなかった。

        // 十分な時間静止した候補を解決する。
        let ready = self
            .code_nav
            .hover_info
            .pending
            .as_ref()
            .is_some_and(|c| !c.resolved && c.since.elapsed() >= HOVER_IDLE);
        if ready {
            let (symbol, file, anchor_row, anchor_col, start_col, end_col, line) = {
                let c = self.code_nav.hover_info.pending.as_ref().unwrap();
                (
                    c.symbol.clone(),
                    c.file.clone(),
                    c.anchor_row,
                    c.anchor_col,
                    c.start_col,
                    c.end_col,
                    c.line,
                )
            };
            let info = self.hover_info_at(&symbol, line, start_col);
            if let Some(c) = self.code_nav.hover_info.pending.as_mut() {
                c.resolved = true;
            }
            self.code_nav.hover_info.anchor_row = anchor_row;
            self.code_nav.hover_info.anchor_col = anchor_col;
            // 対象ファイルを覚えておき、ビューアが切り替わった瞬間に古いファイルガードが破棄できるようにする。
            self.code_nav.hover_info.shown_file = if info.is_some() { file } else { None };
            // info が表示されている間はハイライトし続ける。離脱の猶予が無い
            // PointerState::hover_symbol とは独立。
            if info.is_some() {
                self.code_nav.hover_info.target_line = line;
                self.code_nav.hover_info.target_start_col = start_col;
                self.code_nav.hover_info.target_end_col = end_col;
            }
            self.code_nav.hover_info.info = info;
            self.request_redraw();
        }
    }

    /// 毎フレーム呼ばれるジャンプ下線の駆動処理。マウスがシンボルの上で
    /// （ポップアップより短い）専用のデバウンス時間を超えて静止したら、
    /// ジャンプ可能かどうかを解決し、それに応じて下線を表示・非表示する
    /// （ジャンプ不可能な単語には下線を出さない）。
    pub fn tick_underline_hover(&mut self) {
        // 閾値は 150ms。0ms だと横切るシンボルすべてに下線が点滅する。ポップアップの
        // 350ms より明らかに速いのは、下線が意図的な間を置かずに現れるべきものだから。
        let ready = self
            .viewer
            .click
            .underline_pending
            .as_ref()
            .is_some_and(|p| underline_debounce_ready(p.since.elapsed(), p.resolved));
        if !ready {
            return;
        }

        let (symbol, line, start_col, end_col, has_jump_modifier) = {
            let p = self.viewer.click.underline_pending.as_ref().unwrap();
            (
                p.symbol.clone(),
                p.line,
                p.start_col,
                p.end_col,
                p.has_jump_modifier,
            )
        };
        let jumpable = self.can_jump_to_symbol(&symbol);
        if let Some(p) = self.viewer.click.underline_pending.as_mut() {
            p.resolved = true;
        }
        self.viewer.click.hover_symbol = jumpable.then_some(crate::viewer::HoverSymbol {
            line,
            start_col,
            end_col,
            has_jump_modifier,
        });
        self.request_redraw();
    }

    /// マウスがポップアップ自体の上に来たので、猶予期間を打ち切る。
    pub fn hover_keep_alive(&mut self) {
        self.code_nav.hover_info.leave_at = None;
    }

    /// 現在表示中のシンボルについて参照リスト（レベル1）を開き、ポップアップを
    /// ピン留めする。何も表示されていないか、シンボルに参照がなければ何もしない。
    pub fn open_hover_refs(&mut self) {
        let symbol = match self.code_nav.hover_info.info.as_ref() {
            Some(info) if info.ref_count > 0 => info.symbol_name.clone(),
            _ => return,
        };
        let root = self.code_nav.index.root();
        let results = self.code_nav.index.find_references(&symbol, &root);
        if results.is_empty() {
            return;
        }
        self.code_nav.hover_info.pinned = true;
        self.code_nav.hover_info.leave_at = None;
        self.code_nav.hover_info.refs = Some(crate::overlay::HoverRefs {
            symbol,
            results,
            selected: 0,
            scroll: 0,
            rect: ratatui::layout::Rect::default(),
            row_hits: Vec::new(),
            preview: None,
        });
        self.request_redraw();
    }

    /// ホバーが説明している定義へ飛び、ポップアップを畳む。
    pub fn jump_to_hover_definition(&mut self) {
        let Some((file, line)) = self
            .code_nav
            .hover_info
            .info
            .as_ref()
            .map(|i| (i.file_path.clone(), i.line))
        else {
            return;
        };
        self.clear_hover();
        self.jump_to_location(&file, line, 0);
    }

    /// リスト中の参照行 idx について、コードプレビュー（レベル2）を開く。
    pub fn open_hover_preview(&mut self, idx: usize) {
        let (file, line) = match self.code_nav.hover_info.refs.as_mut() {
            Some(refs) => match refs.results.get(idx) {
                Some(r) => {
                    refs.selected = idx;
                    (r.file_path.clone(), r.line)
                }
                None => return,
            },
            None => return,
        };
        let root = self.code_nav.index.root();
        let preview = build_hover_preview(&root, &file, line);
        if let Some(refs) = self.code_nav.hover_info.refs.as_mut() {
            refs.preview = preview;
        }
        self.request_redraw();
    }

    /// 開いているプレビューの位置へジャンプし、ホバースタック全体を閉じる。
    pub fn hover_jump_to_preview(&mut self) {
        let target = self
            .code_nav
            .hover_info
            .refs
            .as_ref()
            .and_then(|r| r.preview.as_ref())
            .map(|p| (p.file.clone(), p.center_line));
        if let Some((file, line)) = target {
            self.clear_hover();
            self.jump_to_location(&file, line, 0);
        }
    }

    /// 参照リストの選択位置を delta だけ移動する（キーボード操作用）。範囲外には出ない。
    pub fn hover_refs_move(&mut self, delta: isize) {
        if let Some(refs) = self.code_nav.hover_info.refs.as_mut() {
            let n = refs.results.len();
            if n == 0 {
                return;
            }
            let cur = refs.selected as isize;
            refs.selected = (cur + delta).clamp(0, n as isize - 1) as usize;
            self.request_redraw();
        }
    }

    /// ホバースタックでの Esc: 開いている最も深いレベルを閉じる
    /// （プレビュー → リスト → ポップアップ全体の順）。レベルを閉じたかどうかを返す。
    pub fn hover_pop_level(&mut self) -> bool {
        if let Some(refs) = self.code_nav.hover_info.refs.as_mut() {
            if refs.preview.take().is_some() {
                self.request_redraw();
                return true;
            }
            self.code_nav.hover_info.refs = None;
            self.code_nav.hover_info.pinned = false;
            self.request_redraw();
            return true;
        }
        if self.clear_hover() {
            self.request_redraw();
            return true;
        }
        false
    }

    /// その答えが「聞かれた位置そのもの」を指しているか。
    ///
    /// 定義の上でジャンプを押したときに参照一覧へ切り替えるための判定。索引の答えは
    /// 位置なので、行の近さで当てにいく必要が無い。
    pub fn definition_is_here(&self, answer: &Definition, line_idx: usize) -> bool {
        let Some(current) = self.viewer.content.current_file.as_deref() else {
            return false;
        };
        answer_points_at(answer, current, line_idx)
    }

    pub fn is_cursor_at_definition(&self, symbol: &str) -> bool {
        let cur_file = match &self.viewer.content.current_file {
            Some(f) => f,
            None => return false,
        };
        // カーソル行は1始まり（file_scroll は0始まり）。
        let cursor_line = self.viewer.content.file_scroll + 1;
        let defs = self
            .code_nav
            .index
            .find_definitions(symbol, std::path::Path::new(cur_file));
        defs.iter().any(|d| {
            d.file_path == *cur_file && (d.line as isize - cursor_line as isize).unsigned_abs() <= 2
        })
    }

    /// ファイル上の位置へジャンプし、現在位置を履歴に積む。
    ///
    /// source_screen_row は、ジャンプ元のシンボルが表示されていた画面上の
    /// 行（0始まり）。ジャンプ先の行も同じ画面行に配置することで、ユーザの
    /// 視線位置を維持する。
    pub fn jump_to_location(&mut self, file_path: &str, line: usize, source_screen_row: usize) {
        let target_line_0 = line.saturating_sub(1);
        if let Some(ref cur_file) = self.viewer.content.current_file {
            let current_line_0 = self.viewer.content.file_scroll + source_screen_row;
            if cur_file == file_path && current_line_0 == target_line_0 {
                return;
            }
        }

        if let Some(ref cur_file) = self.viewer.content.current_file.clone() {
            let loc = crate::viewer::jump_history::Location {
                file_path: cur_file.clone(),
                line: self.viewer.content.file_scroll,
                h_scroll: self.viewer.content.h_scroll,
            };
            self.code_nav.history.push(loc);
        }

        // 対象ファイルを開く。根は Viewer が表示中のツリーのもの。ツリーの行を
        // 選び直す reveal と同じ根でないと、本文と選択行が別ツリーを指す。
        self.show_file(file_path, crate::app::OpenAs::Persistent);

        // ジャンプ先の行が、ジャンプ元のシンボルと同じ画面行に来るようスクロールする。
        let target_0 = line.saturating_sub(1);
        let total = self.viewer.content.file_content.len();
        let scroll = target_0
            .saturating_sub(source_screen_row)
            .min(total.saturating_sub(1));
        self.viewer.content.file_scroll = scroll;
        self.viewer.content.h_scroll = 0;
        self.viewer.show_raw_for_line_target();
        self.set_focus(Focus::Viewer);
    }

    /// ジャンプ履歴を1つ戻る。
    pub fn jump_back(&mut self) {
        let current = match self.viewer.content.current_file.clone() {
            Some(f) => crate::viewer::jump_history::Location {
                file_path: f,
                line: self.viewer.content.file_scroll,
                h_scroll: self.viewer.content.h_scroll,
            },
            None => return,
        };

        if let Some(loc) = self.code_nav.history.go_back(current) {
            self.show_file(&loc.file_path, crate::app::OpenAs::Persistent);
            let total = self.viewer.content.file_content.len();
            self.viewer.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer.content.h_scroll = loc.h_scroll;
            self.viewer.show_raw_for_line_target();
        }
    }

    /// ジャンプ履歴を1つ進める。
    pub fn jump_forward(&mut self) {
        let current = match self.viewer.content.current_file.clone() {
            Some(f) => crate::viewer::jump_history::Location {
                file_path: f,
                line: self.viewer.content.file_scroll,
                h_scroll: self.viewer.content.h_scroll,
            },
            None => return,
        };

        if let Some(loc) = self.code_nav.history.go_forward(current) {
            self.show_file(&loc.file_path, crate::app::OpenAs::Persistent);
            let total = self.viewer.content.file_content.len();
            self.viewer.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer.content.h_scroll = loc.h_scroll;
            self.viewer.show_raw_for_line_target();
        }
    }

    /// 一覧のポップアップを開き直す。
    fn show_reference_list(&mut self, title: String, results: Vec<crate::symbol_index::Reference>) {
        self.code_nav.references.show(title, results);
    }

    /// 意味索引が返した定義を画面に反映する。
    ///
    /// [`Definition::Exact`] と [`Definition::Enclosing`] は主張の強さが違うので混ぜない。
    /// 後者は聞かれた語の定義ではなく、それを囲んでいる型の定義なので、飛び先が何なのかを
    /// 名乗ってから見せる。
    pub fn apply_semantic_definition(
        &mut self,
        symbol: &str,
        answer: Definition,
        source_screen_row: usize,
    ) {
        let root = self.selected_worktree_path();
        match answer {
            Definition::NotCode => {
                self.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
            }
            Definition::Exact(locations) => self.jump_or_list(
                &root,
                symbol,
                &locations,
                source_screen_row,
                AnsweredBy::Index,
                "definition",
                "definitions",
            ),
            Definition::Syntactic(locations) if locations.is_empty() => {
                self.set_status(
                    format!("No definition found for '{symbol}' [tree-sitter]"),
                    StatusLevel::Warning,
                );
            }
            Definition::Syntactic(locations) => self.jump_or_list(
                &root,
                symbol,
                &locations,
                source_screen_row,
                AnsweredBy::TreeSitter,
                "definition",
                "definitions",
            ),
            Definition::Enclosing(found) => {
                let locations: Vec<_> = found.iter().map(|e| e.definition.clone()).collect();
                let types: Vec<&str> = found.iter().map(|e| e.ty.as_str()).collect();
                let results = locations_to_references(&root, &locations);
                match results.len() {
                    0 => self.set_status(
                        format!("No definition found for '{symbol}'"),
                        StatusLevel::Warning,
                    ),
                    1 => {
                        let (file, line) = (results[0].file_path.clone(), results[0].line);
                        self.jump_to_location(&file, line, source_screen_row);
                        self.set_status(
                            format!(
                                "'{symbol}' itself is not indexed — jumped to its enclosing type [index] {file}:{line}"
                            ),
                            StatusLevel::Info,
                        );
                    }
                    n => {
                        self.show_reference_list(format!("{symbol} (enclosing types)"), results);
                        self.set_status(
                            format!(
                                "'{symbol}' itself is not indexed — {n} enclosing types [index]"
                            ),
                            StatusLevel::Info,
                        );
                        log::debug!("enclosing types for {symbol}: {types:?}");
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn jump_or_list(
        &mut self,
        root: &std::path::Path,
        symbol: &str,
        locations: &[Location],
        source_screen_row: usize,
        by: AnsweredBy,
        one: &str,
        many: &str,
    ) {
        let results = locations_to_references(root, locations);
        match results.len() {
            0 => self.set_status(
                format!("No {one} found for '{symbol}'"),
                StatusLevel::Warning,
            ),
            1 => {
                let (file, line) = (results[0].file_path.clone(), results[0].line);
                self.jump_to_location(&file, line, source_screen_row);
                self.set_status(
                    format!(
                        "Jumped to {one} of '{symbol}' [{}] {file}:{line}",
                        by.label()
                    ),
                    StatusLevel::Success,
                );
            }
            n => {
                self.show_reference_list(format!("{symbol} ({many}, {})", by.label()), results);
                self.set_status(
                    format!("{n} {many} found for '{symbol}' [{}]", by.label()),
                    StatusLevel::Info,
                );
            }
        }
    }

    /// 意味索引が返した参照を画面に反映する。
    ///
    /// [`sheaf_core::Found::via_interface`] は直接参照と分けて後ろに並べる。そこへ実行が
    /// 届くとは限らない (実測で 9 件中 8 件が静的に別の実装へ解決される例がある) ので、
    /// 直接参照と同じ確信度で読まれると誤りになる。
    pub fn apply_semantic_references(&mut self, symbol: &str, answer: References) {
        let root = self.selected_worktree_path();
        let (results, title, by) = match answer {
            References::NotCode => {
                self.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
                return;
            }
            References::Syntactic(locations) => (
                locations_to_references(&root, &locations),
                symbol.to_string(),
                AnsweredBy::TreeSitter,
            ),
            References::Exact(found) => {
                let mut results = locations_to_references(&root, &found.direct);
                let indirect: Vec<_> = found
                    .via_interface
                    .iter()
                    .map(|v| v.reference.clone())
                    .collect();
                let via = locations_to_references(&root, &indirect);
                let title = if via.is_empty() {
                    format!("{symbol} (index)")
                } else {
                    format!(
                        "{symbol} (index: {} direct, {} via interface)",
                        results.len(),
                        via.len()
                    )
                };
                results.extend(via);
                (results, title, AnsweredBy::Index)
            }
        };

        if results.is_empty() {
            self.set_status(
                format!("No references found for '{symbol}' [{}]", by.label()),
                StatusLevel::Warning,
            );
            return;
        }
        let title = if by == AnsweredBy::TreeSitter {
            format!("{title} (tree-sitter)")
        } else {
            title
        };
        self.set_status(
            format!(
                "{} references for '{symbol}' [{}]",
                results.len(),
                by.label()
            ),
            StatusLevel::Info,
        );
        self.show_reference_list(title, results);
    }

    /// 意味索引を `tree_root` に向けて読み直す。
    ///
    /// パースは実測 67ms なのでフレームに置かない。読み終わるまでの間に選択中の
    /// worktree が動いていたら [`Slot::accept`] が取り込みを拒むので、その場合は
    /// [`Self::poll_semantic_index`] が読み直しを起こす。
    /// いま読んでいるファイルの索引ルートを作り直させる。
    ///
    /// 索引はファイル単位で鮮度を持つので、別のツリーで生成された索引は、
    /// そのツリーと内容が違うファイルについてだけ答えなくなる。読むだけの
    /// worktree では編集が起きず作り直しの引き金が引かれないので、手で頼む口が要る。
    pub(crate) fn cmd_rebuild_code_index(&mut self) {
        if self.code_nav.semantic.rebuild_reading() {
            self.set_status_info("Rebuilding the code index for this file…".to_string());
        } else {
            self.set_status(
                "No indexable file open".to_string(),
                crate::app::StatusLevel::Warning,
            );
        }
    }

    /// gd / gr がカーソル行のどの語を対象にするか決める。
    ///
    /// 行内カーソルが無いので、候補が複数ある行では選ばせるしかない。先頭を黙って選ぶと
    /// `pub use model::MenuItem;` のような行で意図しない語に飛ぶ。
    pub fn pick_line_identifier(&mut self, action: HintAction) -> LinePick {
        let line_idx = self.viewer.content.file_scroll;
        let Some(line) = self.viewer.content.file_content.get(line_idx) else {
            return LinePick::None;
        };
        // ラベルは 1 文字なので 26 個で頭打ちになる。
        let choices: Vec<_> = crate::symbol_index::code_identifiers_on_line(
            line,
            line_idx + 1,
            &self.viewer.content.code_mask,
        )
        .take(26)
        .collect();

        if choices.len() <= 1 {
            return match choices.into_iter().next() {
                Some((occurrence, _, word)) => LinePick::One(line_idx, occurrence, word),
                None => LinePick::None,
            };
        }

        let what = match action {
            HintAction::Definition => "definition",
            HintAction::Implementation => "implementation",
            HintAction::References => "references",
            HintAction::Hover => "hover info",
        };
        self.set_status(
            format!("Pick a symbol for {what} (Esc to cancel)"),
            StatusLevel::Info,
        );
        self.code_nav.symbol_hint = SymbolHintOverlay {
            active: true,
            hints: choices
                .iter()
                .enumerate()
                .map(|(i, (_, start, word))| SymbolHint {
                    label: ((b'a' + i as u8) as char).to_string(),
                    symbol_name: word.clone(),
                    line: line_idx + 1,
                    start_col: *start,
                })
                .collect(),
            input: String::new(),
            pending: Some(action),
        };
        LinePick::Asked
    }

    /// 描画された行の `col` 列に重なっている識別子が、その行の何番目の出現か。
    pub fn occurrence_at_rendered_column(&self, line_idx: usize, col: usize) -> Option<usize> {
        let line = self.viewer.content.file_content.get(line_idx)?;
        crate::symbol_index::identifier_occurrences(line)
            .position(|(start, end, _)| col >= start && col < end)
    }

    /// 意味索引に定義を聞く。索引が無い(まだ作っていない、別のツリーを見ている、
    /// Rust ではない)なら `None` を返し、呼び出し側は従来の経路へ落ちる。
    ///
    /// `Some` のときは構文層への切り替えまで含めて sheaf 側で済んでいる
    /// ([`Definition::Syntactic`] が tree-sitter の答え)。
    pub fn semantic_definition(&self, line_idx: usize, occurrence: usize) -> Option<Definition> {
        let tree_root = self.selected_worktree_path();
        let store = self.code_nav.semantic.store(&tree_root)?;
        let site = self.semantic_site(&tree_root, line_idx, occurrence)?;
        let bridge = self.bridge(&site);
        Some(sheaf_core::definition_at(
            store, &bridge, &site.rel, site.line, site.col,
        ))
    }

    /// 意味索引に参照を聞く。返り値の読み方は [`Self::semantic_definition`] と同じ。
    ///
    /// 構文層に落ちるとツリー全体を歩く(ありふれた名前で実測 200 ファイル約 157ms)。
    /// 描画のたびに走る経路からは呼ばない。
    pub fn semantic_references(&self, line_idx: usize, occurrence: usize) -> Option<References> {
        let tree_root = self.selected_worktree_path();
        let store = self.code_nav.semantic.store(&tree_root)?;
        let site = self.semantic_site(&tree_root, line_idx, occurrence)?;
        let bridge = self.bridge(&site);
        Some(sheaf_core::references_at(
            store, &bridge, &site.rel, site.line, site.col,
        ))
    }

    /// 意味索引に、その位置の trait を実装しているものを聞く。返り値の読み方は
    /// [`Self::semantic_definition`] と同じだが、こちらは構文層に落ちない
    /// ([`sheaf_core::Implementations::Unknown`] が「索引が答えられない」)。
    pub fn semantic_implementations(
        &self,
        line_idx: usize,
        occurrence: usize,
    ) -> Option<sheaf_core::Implementations> {
        let tree_root = self.selected_worktree_path();
        let store = self.code_nav.semantic.store(&tree_root)?;
        let site = self.semantic_site(&tree_root, line_idx, occurrence)?;
        let bridge = self.bridge(&site);
        Some(sheaf_core::implementations_at(
            store, &bridge, &site.rel, site.line, site.col,
        ))
    }

    /// viewer の行はタブ展開済み、索引の列は展開前。展開は識別子の数も並びも変えない
    /// ので、出現番号を経由すれば列だけ戻せる。
    fn semantic_site(
        &self,
        tree_root: &std::path::Path,
        line_idx: usize,
        occurrence: usize,
    ) -> Option<SemanticSite> {
        let rel = self.viewer.content.current_file.clone()?;
        let abs = tree_root.join(&rel);
        let source = std::fs::read_to_string(&abs).ok()?;
        let (col, _) = crate::symbol_index::occurrence_span_in_source(
            source.lines().nth(line_idx)?,
            occurrence,
        )?;
        Some(SemanticSite {
            rel: std::path::PathBuf::from(rel),
            abs,
            source,
            line: line_idx as u32,
            col: col as u32,
        })
    }

    fn bridge<'a>(&'a self, site: &'a SemanticSite) -> crate::semantic_index::Bridge<'a> {
        crate::semantic_index::Bridge {
            abs_path: &site.abs,
            source: &site.source,
            mask: &self.viewer.content.code_mask,
            index: &self.code_nav.index,
        }
    }

    /// いま Viewer で読んでいるファイル (worktree からの相対パス)。開いていなければ空。
    ///
    /// 名前でしか引けない検索の絞り込みに要る。空のパスは分類できないので絞らない。
    pub fn reading_file(&self) -> &str {
        self.viewer.content.current_file.as_deref().unwrap_or("")
    }

    /// シンボルインデックス中にそのシンボルの定義があるかを調べる。
    pub fn can_jump_to_symbol(&self, name: &str) -> bool {
        if !self.code_nav.index.is_available() {
            return false;
        }
        !self
            .code_nav
            .index
            .find_definitions(name, std::path::Path::new(self.reading_file()))
            .is_empty()
    }

    /// ビューアに表示中の行についてシンボルヒントを構築する。
    /// 画面上のジャンプ可能なシンボルに2文字ラベルを付けたヒントを返す。
    pub fn build_symbol_hints(&self, inner_height: usize) -> Vec<crate::overlay::SymbolHint> {
        let scroll = self.viewer.content.file_scroll;
        let total = self.viewer.content.file_content.len();
        let end = (scroll + inner_height).min(total);

        let mask = &self.viewer.content.code_mask;
        let mut seen = std::collections::HashSet::new();
        let mut candidates = Vec::new();

        for line_idx in scroll..end {
            let line = &self.viewer.content.file_content[line_idx];
            let line_1 = line_idx + 1;
            // マスクを構築したのと同じスキャンで列挙している。マスクはこの並びの
            // 位置をキーにしているため、必ずこのスキャンでなければならない。
            for (k, (start, stop, word)) in
                crate::symbol_index::identifier_occurrences(line).enumerate()
            {
                if !mask.is_code(line_1, k) {
                    continue;
                }
                if word.len() <= 1 || is_rust_keyword(word) {
                    continue;
                }
                if !seen.insert(word.to_string()) {
                    continue;
                }
                if !self.can_jump_to_symbol(word) {
                    continue;
                }
                candidates.push((word.to_string(), line_1, start, stop));
            }
        }

        candidates
            .into_iter()
            .enumerate()
            .map(|(i, (name, line, start, _end))| {
                let first = (b'a' + (i / 26) as u8) as char;
                let second = (b'a' + (i % 26) as u8) as char;
                crate::overlay::SymbolHint {
                    label: format!("{first}{second}"),
                    symbol_name: name,
                    line,
                    start_col: start,
                }
            })
            .collect()
    }
}

/// Exact 以外は false。構文層の答えは名前一致でしかなく、同名の別物を「ここが
/// 定義」と判定してしまう。line_idx も [`Location::line`] も 0 始まり。
fn answer_points_at(answer: &Definition, file: &str, line_idx: usize) -> bool {
    let Definition::Exact(locations) = answer else {
        return false;
    };
    locations
        .iter()
        .any(|loc| loc.path == std::path::Path::new(file) && loc.line as usize == line_idx)
}

/// 索引が返した位置に、その行の本文を添えて参照一覧の形にする。
///
/// 索引は行と列しか返さないが、一覧は本文を見せる。同じファイルを何度も読まないよう
/// まとめて読む — 参照は 1 ファイルに固まって出ることが多い。
///
/// 本文を読めなかった行は落とさずに空文字で残す。索引がその位置を答えたという事実は、
/// こちらがファイルを読めたかどうかとは別で、黙って消すと件数が合わなくなる。
pub fn locations_to_references(
    root: &std::path::Path,
    locations: &[sheaf_core::Location],
) -> Vec<crate::symbol_index::Reference> {
    use std::collections::HashMap;

    let mut sources: HashMap<&std::path::Path, Option<Vec<String>>> = HashMap::new();
    locations
        .iter()
        .map(|loc| {
            let lines = sources.entry(&loc.path).or_insert_with(|| {
                std::fs::read_to_string(root.join(&loc.path))
                    .ok()
                    .map(|text| text.lines().map(str::to_string).collect())
            });
            let content = lines
                .as_ref()
                .and_then(|l| l.get(loc.line as usize))
                .cloned()
                .unwrap_or_default();
            crate::symbol_index::Reference {
                file_path: loc.path.to_string_lossy().to_string(),
                // sheaf の Location::line は 0 始まり、Reference::line は 1 始まり。
                line: loc.line as usize + 1,
                content,
            }
        })
        .collect()
}

/// ファイルが読めない、または行が範囲外なら None。
fn build_hover_preview(
    root: &std::path::Path,
    rel_path: &str,
    line_1: usize,
) -> Option<crate::overlay::HoverPreview> {
    /// 参照行の前後に表示するコンテキストの行数。
    const CONTEXT: usize = 3;

    let source = std::fs::read_to_string(root.join(rel_path)).ok()?;
    let all: Vec<&str> = source.lines().collect();
    if line_1 == 0 || line_1 > all.len() {
        return None;
    }
    let idx = line_1 - 1;
    let start = idx.saturating_sub(CONTEXT);
    let end = (idx + CONTEXT + 1).min(all.len());
    let lines = (start..end)
        .map(|i| (i + 1, all[i].to_string()))
        .collect::<Vec<_>>();
    Some(crate::overlay::HoverPreview {
        file: rel_path.to_string(),
        center_line: line_1,
        lines,
        rect: ratatui::layout::Rect::default(),
    })
}

/// 2色ある下線のどちらを描くか、あるいは何も描かないなら `None`。
///
/// 下線は静止するだけで表示される。色は修飾キーの状態を伝えるだけで、Hint は
/// 「ここに定義がある」、Accent は「今押せばジャンプできる」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineColorKind {
    Hint,
    Accent,
}

/// 静止中のシンボルに対する下線の色。ジャンプ不可能な単語 (キーワード、未解決の識別子)
/// には出さない。ポップアップが同じ単語に何も出さないのと揃えている。
pub fn underline_color_kind(
    is_jumpable: bool,
    has_jump_modifier: bool,
) -> Option<UnderlineColorKind> {
    if !is_jumpable {
        return None;
    }
    Some(if has_jump_modifier {
        UnderlineColorKind::Accent
    } else {
        UnderlineColorKind::Hint
    })
}

/// ポップアップの対象シンボルが query_line を含んでいれば、そのハイライト範囲。非表示か
/// 別の行を説明している間は `None` で、レンダラは下線側の情報に落ちる。
pub fn popup_highlight_range(
    popup_shown: bool,
    target_line: usize,
    target_start_col: usize,
    target_end_col: usize,
    query_line: usize,
) -> Option<(usize, usize)> {
    if popup_shown && target_line == query_line {
        Some((target_start_col, target_end_col))
    } else {
        None
    }
}

/// tick_underline_hover から切り出してあるのは、App を構築せずにテストするため。
fn underline_debounce_ready(elapsed: std::time::Duration, resolved: bool) -> bool {
    const HOVER_UNDERLINE_MS: u64 = 150;
    !resolved && elapsed >= std::time::Duration::from_millis(HOVER_UNDERLINE_MS)
}

/// 行中の `col` 列にある識別子を (symbol_text, start_col, end_col) で返す。列は 0 始まり。
pub fn extract_symbol_at_column(line: &str, col: usize) -> Option<(String, usize, usize)> {
    if col >= line.len() {
        return None;
    }
    let ch = line.as_bytes().get(col).copied()?;
    if !(ch.is_ascii_alphanumeric() || ch == b'_') {
        return None;
    }
    let start = line[..col]
        .bytes()
        .rev()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let start_col = col - start;
    let end = line[col..]
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let end_col = col + end;
    let word = &line[start_col..end_col];
    if word.len() <= 1 || is_rust_keyword(word) {
        return None;
    }
    if !word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    Some((word.to_string(), start_col, end_col))
}

/// [extract_symbol_at_column] を mask でゲートし、コメントや文字列リテラル内の単語が
/// ジャンプ先として解決されないようにする。抽出処理は「この列にある識別子は何か」を
/// 調べる純粋な検索のままなので、地の文かどうかの判定はこちらに置く。マウスの列を
/// シンボルに変換する呼び出し元はすべてこちらを経由する。
pub fn masked_symbol_at_column(
    line: &str,
    col: usize,
    line_1: usize,
    mask: &crate::symbol_index::CodeMask,
) -> Option<(String, usize, usize)> {
    let (symbol, start_col, end_col) = extract_symbol_at_column(line, col)?;
    if !mask.is_code_at_column(line, line_1, col) {
        return None;
    }
    Some((symbol, start_col, end_col))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol_index::code_identifiers_on_line;

    /// 識別子はその上のどの桁からでも同じ範囲に解決され、キーワード・空白・
    /// 1 文字の名前・行外は解決しない。
    #[test]
    fn 桁はその下の識別子か何も無いかに解決する() {
        let decl = "    let foo = AppState::new();";
        /// 1 ケース: 行、桁、期待する (名前, 開始, 終了)、なぜそうなるか。
        type Case = (
            &'static str,
            usize,
            Option<(&'static str, usize, usize)>,
            &'static str,
        );
        let cases: &[Case] = &[
            (decl, 14, Some(("AppState", 14, 22)), "先頭の桁"),
            (
                decl,
                17,
                Some(("AppState", 14, 22)),
                "途中の桁でも範囲は同じ",
            ),
            ("    let foo = bar;", 4, None, "キーワードは対象外"),
            ("fn main() {}", 2, None, "空白の上"),
            ("short", 100, None, "行の外"),
            ("x + y", 0, None, "1 文字の名前は対象外"),
            (
                "    _handler.call();",
                5,
                Some(("_handler", 4, 12)),
                "先頭の _ も名前の一部",
            ),
        ];
        for (line, col, expected, why) in cases {
            let got = extract_symbol_at_column(line, *col);
            let want = expected.map(|(n, a, b)| (n.to_string(), a, b));
            assert_eq!(got, want, "{why}: {line:?} の col {col}");
        }
    }

    fn at(path: &str, line: u32) -> Location {
        Location {
            path: std::path::PathBuf::from(path),
            line,
            col: 0,
        }
    }

    #[test]
    fn 定義が聞かれた行そのものを指していれば真() {
        let answer = Definition::Exact(vec![at("src/app/focus.rs", 53)]);
        assert!(answer_points_at(&answer, "src/app/focus.rs", 53));
        // 1 行ずれただけでも別の場所。Location::line も line_idx も 0 始まり。
        assert!(!answer_points_at(&answer, "src/app/focus.rs", 54));
        assert!(!answer_points_at(&answer, "src/app/mod.rs", 53));
    }

    #[test]
    fn 構文層の答えは定義位置の判定に使わない() {
        // 名前一致でしかないので、同名の別物を「ここが定義」と誤判定する。
        let answer = Definition::Syntactic(vec![at("src/app/focus.rs", 53)]);
        assert!(!answer_points_at(&answer, "src/app/focus.rs", 53));
    }

    #[test]
    fn 再輸出の行はモジュール名と型名の両方を候補に出す() {
        // gd が先頭の 1 つを黙って選んでいた頃は、この行では必ず model が
        // 対象になり、MenuItem には辿り着けなかった。
        let src = "pub use model::MenuItem;\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let words: Vec<String> = code_identifiers_on_line(src.lines().next().unwrap(), 1, &mask)
            .map(|(_, _, word)| word)
            .collect();
        assert_eq!(words, vec!["model", "MenuItem"]);
    }

    #[test]
    fn 候補の開始桁から出現番号に戻せる() {
        // 選んだラベルの桁は occurrence_at_rendered_column で番号に戻され、
        // その番号で索引を引く。桁と番号がずれると別の語の定義に飛ぶ。
        let src = "pub use model::MenuItem;\n";
        let line = src.lines().next().unwrap();
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        for (occurrence, start, word) in code_identifiers_on_line(line, 1, &mask) {
            let back = crate::symbol_index::identifier_occurrences(line)
                .position(|(s, e, _)| start >= s && start < e);
            assert_eq!(back, Some(occurrence), "{word} の桁から番号に戻せない");
        }
    }

    #[test]
    fn コメントの中の語は候補にしない() {
        let src = "fn f() {\n    let x = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let words: Vec<String> = code_identifiers_on_line(src.lines().nth(1).unwrap(), 2, &mask)
            .map(|(_, _, word)| word)
            .collect();
        assert!(words.is_empty(), "{words:?}");
    }
    /// extract_symbol_at_column 単体はコメントという概念を持たないので、地の文の
    /// "build" をそのまま返す。それをジャンプ先にしないのがこのマスク。
    #[test]
    fn 桁からの語の取り出しはコードマスクで門を通す() {
        let cases = [
            ("    let x = 1; // build the index", "build", None),
            (r#"    let s = "index";"#, "index", None),
            (
                "    let value = 1; // build the index",
                "value",
                Some(("value".to_string(), 8, 13)),
            ),
        ];
        for (body, word, want) in cases {
            let src = format!("fn f() {{\n{body}\n}}\n");
            let mask = crate::symbol_index::CodeMask::compute(&src, "lib.rs");
            let line = src.lines().nth(1).unwrap();
            let col = line.find(word).unwrap();
            assert_eq!(masked_symbol_at_column(line, col, 2, &mask), want, "{body}");
        }
    }

    #[test]
    fn ホバーのプレビューは行の前後を窓にする() {
        let dir = std::env::temp_dir().join(format!("hover_prev_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = (1..=10)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("f.rs"), src).unwrap();

        // 中心行5 → 前後3行のコンテキスト (2..=8)。
        let p = build_hover_preview(&dir, "f.rs", 5).expect("preview");
        assert_eq!(p.center_line, 5);
        assert_eq!(p.file, "f.rs");
        assert_eq!(
            p.lines,
            vec![
                (2, "line2".to_string()),
                (3, "line3".to_string()),
                (4, "line4".to_string()),
                (5, "line5".to_string()),
                (6, "line6".to_string()),
                (7, "line7".to_string()),
                (8, "line8".to_string()),
            ]
        );

        // ファイル先頭付近では、ウィンドウはファイルの先頭でクランプされる。
        let p = build_hover_preview(&dir, "f.rs", 1).expect("preview");
        assert_eq!(p.lines.first().unwrap().0, 1);

        // 範囲外 / ファイルが存在しない場合は None。
        assert!(build_hover_preview(&dir, "f.rs", 999).is_none());
        assert!(build_hover_preview(&dir, "nope.rs", 1).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 行内候補の先頭。マスク・キーワード・1 文字の除外が効いているかを見るための短縮形。
    fn first_candidate(
        line: &str,
        line_1: usize,
        mask: &crate::symbol_index::CodeMask,
    ) -> Option<String> {
        code_identifiers_on_line(line, line_1, mask)
            .next()
            .map(|(_, _, word)| word)
    }

    #[test]
    fn マスク越しにコメント内の語を飛ばす() {
        // doc/行/ブロックコメントは、実在の型名とたまたま同じ英単語("Building"
        // バグ)を返してはならない。これは行頭プレフィックスによる推測ではなく
        // マスクによって判定するようになったため、ブロックコメントを1つとして
        // パースさせるには実際に複数行のソースが必要になる。
        let src = "\
//! Building and navigating
/// Create a new state
// Building the list
/* Building */
fn f() {
/* Building
 * Building (block cont.)
 */
}
#[derive(Debug)]
struct Marker;
fn g() {
    let state = DiffState::new();
}
pub struct Building {
    x: i32,
}
";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = |n: usize| src.lines().nth(n - 1).unwrap();

        assert_eq!(first_candidate(line(1), 1, &mask), None);
        assert_eq!(first_candidate(line(2), 2, &mask), None);
        assert_eq!(first_candidate(line(3), 3, &mask), None);
        assert_eq!(first_candidate(line(4), 4, &mask), None);
        // 上の複数行ブロックコメントの継続行。
        assert_eq!(first_candidate(line(7), 7, &mask), None);

        // #[derive(Debug)] はもはや特別扱いで除外されない。アトリビュートは
        // 地の文ではなく本物の構文であり、マスクがマスクするのはコメントと
        // 文字列だけなので、derive はコード位置にあり他の識別子と同様に
        // 返ってくる。以前のプレフィックスチェックは # で始まる行をすべて
        // 解決不能扱いしていたが、マスクは「コメントか文字列か」で線引きして
        // おり、アトリビュートはそのどちらでもない。
        assert_eq!(
            first_candidate(line(10), 10, &mask),
            Some("derive".to_string())
        );

        // 実際のコード行は引き続き最初の識別子として解決される。
        assert_eq!(
            first_candidate(line(13), 13, &mask),
            Some("state".to_string())
        );
        assert_eq!(
            first_candidate(line(15), 15, &mask),
            Some("Building".to_string())
        );
    }

    #[test]
    fn 行末コメントと文字列の一致を飛ばす() {
        // 実際の文の後ろにトレイリングコメントが続くケース。x は 1 文字なので単独では除外され、
        // build/the/index はコメントの中にあるため、行全体が何も解決しない。行の「始まり方」
        // だけを見ると "build" を返してしまう。
        let src = "fn f() {\n    let x = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(first_candidate(line, 2, &mask), None);

        // 同じ形だが、コメントの前に実際の識別子がある場合。修正が行全体を
        // 過剰にマスクしていないことを確認するため、これは解決されなければならない。
        let src = "fn f() {\n    let value = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(first_candidate(line, 2, &mask), Some("value".to_string()));

        // 文字列リテラルも同様にその中身を隠す。
        let src = "fn f() {\n    let s = \"index\";\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(first_candidate(line, 2, &mask), None);
    }

    #[test]
    fn 下線の色とデバウンス() {
        use std::time::Duration;

        // ジャンプ不可能な単語には、修飾キーの有無にかかわらず下線が付かない。Hint は
        // 「ここに定義がある」、Accent は「今押せばジャンプできる」。
        assert_eq!(underline_color_kind(false, false), None);
        assert_eq!(underline_color_kind(false, true), None);
        assert_eq!(
            underline_color_kind(true, false),
            Some(UnderlineColorKind::Hint)
        );
        assert_eq!(
            underline_color_kind(true, true),
            Some(UnderlineColorKind::Accent)
        );

        // 閾値は 150ms。解決済みの候補は、静止し続けても再解決されない。
        assert!(!underline_debounce_ready(Duration::from_millis(149), false));
        assert!(underline_debounce_ready(Duration::from_millis(150), false));
        assert!(!underline_debounce_ready(Duration::from_millis(500), true));
    }

    #[test]
    fn ポップアップの範囲は対象行に一致する() {
        // ポップアップ自身の対象行・列は、マウスが現在どこにあるかに関係なく
        // 返される。
        assert_eq!(popup_highlight_range(true, 42, 4, 10, 42), Some((4, 10)));
    }

    #[test]
    fn 対象行の外では範囲を返さない() {
        assert_eq!(popup_highlight_range(true, 42, 4, 10, 43), None);
    }

    #[test]
    fn 非表示なら範囲を返さない() {
        assert_eq!(popup_highlight_range(false, 42, 4, 10, 42), None);
    }
}
