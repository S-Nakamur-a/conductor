//! コードナビゲーション: カーソル位置のシンボル検索、定義へのジャンプ、ジャンプ履歴、
//! バックグラウンドでのシンボルインデックス構築、画面上のシンボルヒントを扱う。

use super::App;
use super::focus::Focus;

impl App {
    // コードナビゲーションのヘルパー

    /// 現在のビューア行からカーソル位置のシンボルを抽出する。
    ///
    /// ここで言う「カーソル」は今も画面最上行を指しており、実際のテキストカーソルとは違う。
    /// その食い違いは別途把握済みの既知の問題であり、ここでは扱わない。
    pub fn get_symbol_at_cursor(&self) -> Option<String> {
        let scroll = self.viewer_state.content.file_scroll;
        let line = self.viewer_state.content.file_content.get(scroll)?;
        let line_1 = scroll + 1;
        extract_symbol_from_line(line, line_1, &self.viewer_state.content.code_mask)
    }

    /// ビューアのカーソル位置にあるシンボルについて、ホバー情報のポップアップを
    /// 明示的に表示する（最上行の最初の識別子を対象とし、gd と同じ検索を使う）。
    ///
    /// K キーに割り当てられた、待ち時間なしの即時トリガー。ユーザが意図して押した
    /// 操作なので、ポップアップを出せない場合でもステータス表示でフィードバックを返す。
    /// 何も表示しない受動的な自動ホバーとは異なる。
    pub fn show_hover_info(&mut self) {
        use crate::app::StatusLevel;

        let symbol = match self.get_symbol_at_cursor() {
            Some(s) => s,
            None => {
                self.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
                return;
            }
        };
        if !self.code_nav.index.is_available() {
            self.set_status(
                "Symbol index not ready yet".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        let current_file = self.viewer_state.content.current_file.clone();
        match crate::hover_info::build_hover_info(
            &self.code_nav.index,
            &symbol,
            current_file.as_deref(),
        ) {
            Some(info) => {
                self.code_nav.hover_info.shown_file = current_file.clone();
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

    /// 受動的な自動ホバーポップアップを今表示してよいかどうか。フォーカスが
    /// Viewer にあり、ファイル（通常またはdiff）が開かれていて、ブロッキングの
    /// オーバーレイやサマリー疑似ビューが画面を占有していないことが条件。
    fn hover_auto_allowed(&self) -> bool {
        self.focus == Focus::Viewer
            && !self.viewer_state.is_summary()
            && self.overlays.active == crate::overlay::ActiveOverlay::None
            && !self.code_nav.references.active
            && !self.code_nav.symbol_action.active
            && !self.code_nav.symbol_hint.active
            && self.viewer_state.content.current_file.is_some()
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

    /// マウスホバーに関する状態をまとめてすべてクリアする: ジャンプ用の下線、
    /// ポップアップスタック、Explorer の行ホバーハイライト。crossterm は
    /// マウスが端末ウィンドウの外に出たことを報告してくれないため、この関数は
    /// 「マウスが今どこにも乗っていない」と確実に言えるいくつかのイベント――
    /// 任意のキー入力、FocusLost、ブロッキングオーバーレイが開いたとき――から
    /// 呼ばれる（呼び出し箇所は event_loop.rs と event/mouse/mod.rs を参照）。
    pub fn clear_all_hover(&mut self) {
        self.clear_pointer_hover();
        // ポップアップスタックを破棄してよいのはピン留めされていないときだけ。
        // ピン留めされたモーダルはキーボード操作によるもので、以前からの慣習として
        // フォーカスやアイドルによる消失を免れる（HoverInfoOverlay::pinned と
        // tick_hover の早期リターンを参照）。
        if !self.code_nav.hover_info.pinned {
            self.clear_hover();
        }
    }

    /// ポインタ操作によるハイライトだけをクリアする: ジャンプ用の下線と
    /// 行・チップ・タブのホバー。
    ///
    /// あえてホバーポップアップのスタックには触れない。handle_key_event が
    /// キー入力ごとにポップアップの状態遷移を解決しており、その挙動はこの関数では
    /// 再現できない――ピン留めされたモーダルはキー入力を自分の操作として消費し、
    /// 一時的なポップアップは Esc で閉じられるが、その Esc はフォーカス中のパネル
    /// 側の Esc アクションを二重に起動しないよう飲み込まれる。以前の版ではここで
    /// スタックをクリアしていたが、それは handle_key_event より先に実行され
    /// pinned をfalseに戻してしまうため、モーダルのキーボード経路全体が
    /// 到達不能になり、Esc が二重に発火する不具合があった。
    ///
    /// ここでクリアする対象には、キー入力に対する他の解除経路が存在しない。
    /// crossterm はポインタがウィンドウの外に出たことを報告しないため、これが
    /// なければキーボード操作に切り替えた後もハイライトが点いたままになる。
    pub fn clear_pointer_hover(&mut self) {
        self.viewer_state.click.hover_symbol = None;
        self.viewer_state.click.underline_pending = None;
        self.list_hover.clear();
        // バー・タブバーのホバー状態（背景色ベースの表現に変更済み）。
        self.wtbar.hover = None;
        self.terminal.claude_tab_hover = None;
        self.terminal.shell_tab_hover = None;
    }

    /// マウスが現在乗っているシンボルを記録する（マウス移動イベントから呼ばれる）。
    /// cand は (symbol, 1始まりの行, anchor_row, anchor_col, start_col, end_col)。
    /// anchor は画面上の絶対座標、col は（h_scroll適用前の）0始まりのコンテンツ列で、
    /// 解決後は HoverInfoOverlay::target_* にそのまま引き継がれる。マウスが
    /// 空白や識別子以外の上にあるときは None。新しいシンボルに移ると、
    /// アイドルカウントダウンがリセットされ、前のシンボル用に表示されていた
    /// ポップアップは破棄される。
    pub fn set_mouse_hover_candidate(
        &mut self,
        cand: Option<(String, usize, u16, u16, usize, usize)>,
    ) {
        match cand {
            Some((symbol, line, anchor_row, anchor_col, start_col, end_col)) => {
                let same = self
                    .code_nav.hover_info
                    .pending
                    .as_ref()
                    .is_some_and(|c| c.symbol == symbol && c.line == line);
                if same {
                    return;
                }
                let file = self.viewer_state.content.current_file.clone();
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
                    self.dirty.mark_all();
                }
            }
            None => {
                // マウスがシンボルから離れて空白に移動した。ポップアップが表示中なら
                // 即座には消さず、短い猶予期間を設ける（tick_hover を参照）。これにより
                // カーソルをポップアップまで移動してクリックできるようにする。まだ何も
                // 表示されていなければ候補を単に破棄する。
                if self.code_nav.hover_info.info.is_some() {
                    self.code_nav.hover_info.pending = None;
                    if self.code_nav.hover_info.leave_at.is_none() {
                        self.code_nav.hover_info.leave_at = Some(std::time::Instant::now());
                    }
                } else if self.code_nav.hover_info.pending.take().is_some() {
                    self.dirty.mark_all();
                }
            }
        }
    }

    /// ジャンプ用の下線のために、マウスが乗っているシンボルを記録する。
    /// [set_mouse_hover_candidate] のポップアップ用デバウンスとは別系統。
    /// cand は (symbol, 1始まりの行, start_col, end_col)、シンボル外なら
    /// None。has_jump_modifier はこの移動時点での Cmd/Ctrl の状態で、
    /// 下線の色を決めるために解決後の [crate::viewer::HoverSymbol] に保存
    /// される。すでに解決済みのシンボルの上に乗ったままでも都度更新されるので、
    /// デバウンスをやり直さずに修飾キーの押下・解放で色が切り替わる。
    ///
    /// ポップアップと違い、こちらには離脱時の猶予がない。シンボルから離れる
    /// （あるいは別のシンボルに移る）と、表示中の下線は即座に消える。
    pub fn set_underline_candidate(
        &mut self,
        cand: Option<(String, usize, usize, usize)>,
        has_jump_modifier: bool,
    ) {
        match cand {
            Some((symbol, line, start_col, end_col)) => {
                if let Some(hs) = self.viewer_state.click.hover_symbol.as_mut()
                    && hs.line == line
                    && hs.start_col == start_col
                    && hs.end_col == end_col
                {
                    hs.has_jump_modifier = has_jump_modifier;
                    return;
                }
                let same_pending = self
                    .viewer_state
                    .click
                    .underline_pending
                    .as_ref()
                    .is_some_and(|p| {
                        p.line == line && p.start_col == start_col && p.end_col == end_col
                    });
                if same_pending {
                    if let Some(p) = self.viewer_state.click.underline_pending.as_mut() {
                        p.has_jump_modifier = has_jump_modifier;
                    }
                    return;
                }
                self.viewer_state.click.underline_pending = Some(crate::viewer::PendingUnderline {
                    symbol,
                    line,
                    start_col,
                    end_col,
                    since: std::time::Instant::now(),
                    resolved: false,
                    has_jump_modifier,
                });
                // 猶予なし。新しく乗った候補は、前の候補で表示されていた下線を
                // 即座に隠す。
                self.viewer_state.click.hover_symbol = None;
            }
            None => {
                self.viewer_state.click.underline_pending = None;
                self.viewer_state.click.hover_symbol = None;
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

    /// 毎フレーム呼ばれる自動ホバーの駆動処理。マウスがシンボルの上で
    /// デバウンス時間を超えて静止したらホバーポップアップを解決する。見つから
    /// なければ何も表示しない。猶予期間の管理や、ファイル切り替え・フォーカス
    /// 喪失による無効化も担う。
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

        // 古いファイルに対するガード。ポップアップ表示中にビューアが（ジャンプや
        // ファイルツリー操作、外部からのリロードで）別ファイルに切り替わった場合、
        // ポップアップはもはや画面にないファイルのシンボルを説明していることになる。
        // 猶予期間中であっても破棄し、無関係なコードの上に残り続けないようにする。
        if self.code_nav.hover_info.info.is_some()
            && self.code_nav.hover_info.shown_file != self.viewer_state.content.current_file
        {
            if self.clear_hover() {
                self.dirty.mark_all();
            }
            return;
        }

        // 猶予期間: マウスが離れたシンボルのポップアップはしばらく残るが、それは
        // マウスが実際にポップアップの上にあるか、タイマーが切れていない間だけ。
        if let Some(left) = self.code_nav.hover_info.leave_at
            && left.elapsed() >= HOVER_GRACE
        {
            if self.clear_hover() {
                self.dirty.mark_all();
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
                self.dirty.mark_all();
            }
            return;
        }

        // 自動ホバーはマウスがシンボルの上に静止していることだけで駆動する
        // （マウス移動ハンドラが設定する）。最上行やキーボードに基づくヒューリスティ
        // クも試したが採用しなかった。行単位のテキストカーソルがない以上「カーソル
        // 行」は常に画面最上行になってしまい、ユーザが指していないコードに対しても
        // 発火していた。マウス位置なら正確で、シンボルの上ならポップアップを出し、
        // 空白の上なら何も出さない。

        // 十分な時間静止した候補を解決する。
        let ready = self
            .code_nav.hover_info
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
            let info =
                crate::hover_info::build_hover_info(&self.code_nav.index, &symbol, file.as_deref());
            if let Some(c) = self.code_nav.hover_info.pending.as_mut() {
                c.resolved = true;
            }
            self.code_nav.hover_info.anchor_row = anchor_row;
            self.code_nav.hover_info.anchor_col = anchor_col;
            // このポップアップがどのファイルを対象にしているかを記憶しておき、
            // ビューアが別ファイルに切り替わった瞬間に古いファイルガードが破棄
            // できるようにする。
            self.code_nav.hover_info.shown_file = if info.is_some() { file } else { None };
            // 対象のシンボルは info が表示されている間はハイライトし続ける。これは
            // ClickTracker::hover_symbol（マウスがすでに離れているかもしれず、また
            // 離脱の猶予も一切ない）とは独立している。
            if info.is_some() {
                self.code_nav.hover_info.target_line = line;
                self.code_nav.hover_info.target_start_col = start_col;
                self.code_nav.hover_info.target_end_col = end_col;
            }
            self.code_nav.hover_info.info = info;
            self.dirty.mark_all();
        }
    }

    /// 毎フレーム呼ばれるジャンプ下線の駆動処理。マウスがシンボルの上で
    /// （ポップアップより短い）専用のデバウンス時間を超えて静止したら、
    /// ジャンプ可能かどうかを解決し、それに応じて下線を表示・非表示する
    /// （ジャンプ不可能な単語には下線を出さない）。
    pub fn tick_underline_hover(&mut self) {
        // 閾値は150ms（underline_debounce_ready が使う値）。マウスが通り
        // すぎるだけのコードで、横切るシンボルすべてに下線が点滅するのを防ぐには
        // 十分な長さがあり（0msも試したが「クリスマスツリー」のようにちらつく）、
        // 一方で上の tick_hover にあるポップアップの350msの HOVER_IDLE より
        // 明らかに速く、それとは独立に保てる程度には短い。下線はポップアップの
        // 意図的な間とは対照的に、瞬時に現れるべきものだからである。
        let ready = self
            .viewer_state
            .click
            .underline_pending
            .as_ref()
            .is_some_and(|p| underline_debounce_ready(p.since.elapsed(), p.resolved));
        if !ready {
            return;
        }

        let (symbol, line, start_col, end_col, has_jump_modifier) = {
            let p = self.viewer_state.click.underline_pending.as_ref().unwrap();
            (
                p.symbol.clone(),
                p.line,
                p.start_col,
                p.end_col,
                p.has_jump_modifier,
            )
        };
        let jumpable = self.can_jump_to_symbol(&symbol);
        if let Some(p) = self.viewer_state.click.underline_pending.as_mut() {
            p.resolved = true;
        }
        self.viewer_state.click.hover_symbol = jumpable.then_some(crate::viewer::HoverSymbol {
            text: symbol,
            line,
            start_col,
            end_col,
            has_jump_modifier,
        });
        self.dirty.mark_all();
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
        self.dirty.mark_all();
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
        self.dirty.mark_all();
    }

    /// 開いているプレビューの位置へジャンプし、ホバースタック全体を閉じる。
    pub fn hover_jump_to_preview(&mut self) {
        let target = self
            .code_nav.hover_info
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
            self.dirty.mark_all();
        }
    }

    /// ホバースタックでの Esc: 開いている最も深いレベルを閉じる
    /// （プレビュー → リスト → ポップアップ全体の順）。レベルを閉じたかどうかを返す。
    pub fn hover_pop_level(&mut self) -> bool {
        if let Some(refs) = self.code_nav.hover_info.refs.as_mut() {
            if refs.preview.take().is_some() {
                self.dirty.mark_all();
                return true;
            }
            self.code_nav.hover_info.refs = None;
            self.code_nav.hover_info.pinned = false;
            self.dirty.mark_all();
            return true;
        }
        if self.clear_hover() {
            self.dirty.mark_all();
            return true;
        }
        false
    }

    /// カーソルが現在、指定したシンボルの定義位置と一致する（あるいは非常に
    /// 近い）かどうかを調べる。現在のファイル+行がシンボルの定義位置のいずれか
    /// と一致すれば true を返す。
    pub fn is_cursor_at_definition(&self, symbol: &str) -> bool {
        let cur_file = match &self.viewer_state.content.current_file {
            Some(f) => f,
            None => return false,
        };
        // カーソル行は1始まり（file_scroll は0始まり）。
        let cursor_line = self.viewer_state.content.file_scroll + 1;
        let defs = self.code_nav.index.find_definitions(symbol);
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
        // 自分自身へのジャンプ（移動先==現在位置）はスキップする。
        let target_line_0 = line.saturating_sub(1);
        if let Some(ref cur_file) = self.viewer_state.content.current_file {
            let current_line_0 = self.viewer_state.content.file_scroll + source_screen_row;
            if cur_file == file_path && current_line_0 == target_line_0 {
                return;
            }
        }

        // 現在位置を履歴に保存する。
        if let Some(ref cur_file) = self.viewer_state.content.current_file.clone() {
            let loc = crate::jump_history::Location {
                file_path: cur_file.clone(),
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            };
            self.code_nav.history.push(loc);
        }

        // 対象ファイルを開く。根は Viewer が表示中のツリーのもの。ツリーの行を
        // 選び直す reveal と同じ根でないと、本文と選択行が別ツリーを指す。
        let tab_width = self.config.viewer.tab_width;
        self.viewer_state.open_file(file_path, tab_width);
        self.rehighlight_viewer();
        self.viewer_state.reveal_file_in_tree(file_path);

        // ジャンプ先の行が、ジャンプ元のシンボルと同じ画面行に来るようスクロールする。
        let target_0 = line.saturating_sub(1);
        let total = self.viewer_state.content.file_content.len();
        let scroll = target_0
            .saturating_sub(source_screen_row)
            .min(total.saturating_sub(1));
        self.viewer_state.content.file_scroll = scroll;
        self.viewer_state.content.h_scroll = 0;
        self.viewer_state.show_raw_for_line_target();
        self.set_focus(Focus::Viewer);
    }

    /// ジャンプ履歴を1つ戻る。
    pub fn jump_back(&mut self) {
        let current = match self.viewer_state.content.current_file.clone() {
            Some(f) => crate::jump_history::Location {
                file_path: f,
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            },
            None => return,
        };

        if let Some(loc) = self.code_nav.history.go_back(current) {
            let tab_width = self.config.viewer.tab_width;
            self.viewer_state.open_file(&loc.file_path, tab_width);
            self.rehighlight_viewer();
            self.viewer_state.reveal_file_in_tree(&loc.file_path);
            let total = self.viewer_state.content.file_content.len();
            self.viewer_state.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer_state.content.h_scroll = loc.h_scroll;
            self.viewer_state.show_raw_for_line_target();
        }
    }

    /// ジャンプ履歴を1つ進める。
    pub fn jump_forward(&mut self) {
        let current = match self.viewer_state.content.current_file.clone() {
            Some(f) => crate::jump_history::Location {
                file_path: f,
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            },
            None => return,
        };

        if let Some(loc) = self.code_nav.history.go_forward(current) {
            let tab_width = self.config.viewer.tab_width;
            self.viewer_state.open_file(&loc.file_path, tab_width);
            self.rehighlight_viewer();
            self.viewer_state.reveal_file_in_tree(&loc.file_path);
            let total = self.viewer_state.content.file_content.len();
            self.viewer_state.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer_state.content.h_scroll = loc.h_scroll;
            self.viewer_state.show_raw_for_line_target();
        }
    }

    /// 現在選択中のワークツリーを対象に、シンボルインデックスの構築を
    /// バックグラウンドで開始する。
    ///
    /// 各呼び出し元ではなくここでインデックスの対象を合わせ直すことで、両者が
    /// ずれないようにしている。インデックスはビューアが表示しているツリーを
    /// 説明していなければならず、ビルドを望むあらゆる経路――起動時、ワーク
    /// ツリー切り替え、ファイルシステム変更――は同じツリーを対象にしたい。
    /// [SymbolIndex::set_root] はルートが変わっていなければ何もしないので、
    /// ファイルシステム変更の経路もその場での再構築になる。
    ///
    /// すでに実行中のビルドは、置き換えずに完了まで走らせておく。ワークツリー
    /// の選択変更はユーザがリストをスクロールできる速さでどんどん届くため、
    /// この仕組みがないと10個のワークツリーをドラッグして通過するだけで10個の
    /// フルツリー解析が並行して走り出してしまう（BackgroundOp は置き換え対象
    /// を中断できず、join handle を破棄するだけでワーカーはそのまま完走する）。
    /// その間インデックスは使えないままになり、ちょうどユーザが操作している
    /// タイミングでナビゲーションが死ぬことになる。置き換えられたビルドは
    /// 世代チェックによって自分の結果を捨て、最終的に落ち着いたワークツリーの
    /// ビルドは下の呼び出し元から行われる。
    pub fn start_symbol_index_build(&mut self) {
        self.code_nav.index.set_root(self.selected_worktree_path());
        if self.bg.symbol_index.is_running() {
            return;
        }
        let index = self.code_nav.index.clone();
        self.bg.symbol_index.start(move |tx| {
            let result = match index.build() {
                Ok(count) => Ok(count),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });
    }

    /// シンボルインデックス中にそのシンボルの定義があるかを調べる。
    pub fn can_jump_to_symbol(&self, name: &str) -> bool {
        if !self.code_nav.index.is_available() {
            return false;
        }
        !self.code_nav.index.find_definitions(name).is_empty()
    }

    /// ビューアに表示中の行についてシンボルヒントを構築する。
    /// 画面上のジャンプ可能なシンボルに2文字ラベルを付けたヒントを返す。
    pub fn build_symbol_hints(&self, inner_height: usize) -> Vec<crate::overlay::SymbolHint> {
        let scroll = self.viewer_state.content.file_scroll;
        let total = self.viewer_state.content.file_content.len();
        let end = (scroll + inner_height).min(total);

        let mask = &self.viewer_state.content.code_mask;
        let mut seen = std::collections::HashSet::new();
        let mut candidates = Vec::new();

        for line_idx in scroll..end {
            let line = &self.viewer_state.content.file_content[line_idx];
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

        // 2文字ラベルを割り当てる: aa, ab, ..., az, ba, bb, ...
        candidates
            .into_iter()
            .enumerate()
            .map(|(i, (name, line, start, end))| {
                let first = (b'a' + (i / 26) as u8) as char;
                let second = (b'a' + (i % 26) as u8) as char;
                crate::overlay::SymbolHint {
                    label: format!("{first}{second}"),
                    symbol_name: name,
                    line,
                    start_col: start,
                    end_col: end,
                }
            })
            .collect()
    }
}

/// rel_path 内の line_1（1始まり）を中心に、前後数行を含むコードプレビュー
/// のウィンドウを構築する。ファイルが読めない、または行が範囲外なら None
/// を返す。
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

// ジャンプ下線の判定ヘルパー（純粋関数、直接ユニットテスト対象）

/// 2色ある下線のどちらを描くか、あるいは何も描かないなら None。
///
/// 下線は現在、Cmd/Ctrl を押しているときだけでなく、シンボルに静止するだけで
/// 表示される。色によって修飾キーの状態を伝える役割は残っており、Hint は
/// 「ここに定義がある」、Accent は「今押せばジャンプできる」を意味する
/// （実際のクリックには依然として修飾キーが必要で、下線はその約束の内容を
/// 変えているだけ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineColorKind {
    Hint,
    Accent,
}

/// 静止中のシンボルに対する下線の色を決める。ジャンプ不可能な単語――
/// キーワードや未解決の識別子――には下線をまったく出さない。これは
/// ポップアップが同じ単語に対して何も表示しないのと揃えている。
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

/// ホバー情報ポップアップの対象シンボルが query_line を含んでいるか、
/// 含んでいればそのハイライト範囲を返す。ポップアップが非表示か別の行を
/// 説明している間は None を返し、その場合レンダラはその行について
/// ClickTracker::hover_symbol（下線）側の情報にフォールバックする。
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

/// 下線用デバウンスの判定を tick_underline_hover から切り出したもので、
/// App を構築せずにユニットテストできる。elapsed が150msを超えていて、
/// かつ候補がまだ一度も解決されていなければ準備完了とみなす。
fn underline_debounce_ready(elapsed: std::time::Duration, resolved: bool) -> bool {
    const HOVER_UNDERLINE_MS: u64 = 150;
    !resolved && elapsed >= std::time::Duration::from_millis(HOVER_UNDERLINE_MS)
}

// シンボル抽出のための自由関数

/// カーソル位置にあたるソースコード行からシンボル名を抽出する。
/// line_1（1始まり）上で [mask] がコードだと判定した最初の識別子を
/// 返す――単に識別子の形をした最初の単語ではない。コメントや文字列
/// リテラル内の単語（//! Building … 内の Building や、
/// let x = 1; // build the index 内の index など）は、以前の
/// 「コメントのみの行を除外する」処理と同様にスキップされるが、行単位では
/// なく出現位置単位で判定するため、実コードの行末に付いたコメントが
/// その行前半のコードまで隠してしまうことはなくなった。
///
/// 以前は //、/*、*、# を見る単独のプレフィックスチェックだった。
/// これでは行頭から始まるコメントしか捉えられず、行の途中のコメントや
/// 文字列リテラルを見分ける手段がなく、現在マスクが一箇所で決めている
/// 判定を重複させていた。[App::build_symbol_hints](crate::app::App::build_symbol_hints)
/// と同じ「列挙してゲートする」形になっている。
pub fn extract_symbol_from_line(
    line: &str,
    line_1: usize,
    mask: &crate::symbol_index::CodeMask,
) -> Option<String> {
    for (k, (_, _, word)) in crate::symbol_index::identifier_occurrences(line).enumerate() {
        if !mask.is_code(line_1, k) {
            continue;
        }
        if word.len() > 1 && !is_rust_keyword(word) {
            return Some(word.to_string());
        }
    }
    None
}

/// 単語が Rust のキーワードかどうかを調べる（シンボルとして扱うべきではない）。
pub fn is_rust_keyword(word: &str) -> bool {
    matches!(
        word,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
    )
}

/// 行中の特定の列にあるシンボル（識別子）を抽出する。
/// (symbol_text, start_col, end_col) を返す。列は0始まりの文字オフセット。
pub fn extract_symbol_at_column(line: &str, col: usize) -> Option<(String, usize, usize)> {
    if col >= line.len() {
        return None;
    }
    // col の位置の文字が識別子の一部であることを確認する。
    let ch = line.as_bytes().get(col).copied()?;
    if !(ch.is_ascii_alphanumeric() || ch == b'_') {
        return None;
    }
    // 識別子の先頭を探すため後方に走査する。
    let start = line[..col]
        .bytes()
        .rev()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let start_col = col - start;
    // 識別子の末尾を探すため前方に走査する。
    let end = line[col..]
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let end_col = col + end;
    let word = &line[start_col..end_col];
    if word.len() <= 1 || is_rust_keyword(word) {
        return None;
    }
    // 識別子はアルファベットかアンダースコアで始まる必要がある。
    if !word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    Some((word.to_string(), start_col, end_col))
}

/// [extract_symbol_at_column] を mask でゲートし、コメントや文字列
/// リテラル内の単語がジャンプ先として解決されないようにする。
///
/// 抽出処理そのものとは分離してある。抽出処理は「この列にある識別子は何か」
/// を調べる純粋な検索のままであり、その出現がコードなのか地の文なのかを
/// 知る手段を持たない。マウスの列をジャンプ可能なシンボルに変換するすべての
/// 呼び出し元（ホバー下線、自動ホバーポップアップ、Cmd+クリック）は、
/// extract_symbol_at_column を直接呼ぶのではなくこちらを経由する。
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

    #[test]
    fn test_extract_symbol_at_column_basic() {
        let line = "    let foo = AppState::new();";
        // col 14 の AppState の 'A' をクリック
        let result = extract_symbol_at_column(line, 14);
        assert_eq!(result, Some(("AppState".to_string(), 14, 22)));
    }

    #[test]
    fn test_extract_symbol_at_column_middle() {
        let line = "    let foo = AppState::new();";
        // col 17 の AppState の 'S' をクリック
        let result = extract_symbol_at_column(line, 17);
        assert_eq!(result, Some(("AppState".to_string(), 14, 22)));
    }

    #[test]
    fn test_extract_symbol_at_column_on_keyword() {
        let line = "    let foo = bar;";
        // col 4 の let の 'l' をクリック
        let result = extract_symbol_at_column(line, 4);
        assert_eq!(result, None); // "let" はキーワード
    }

    #[test]
    fn test_extract_symbol_at_column_on_space() {
        let line = "fn main() {}";
        let result = extract_symbol_at_column(line, 2);
        assert_eq!(result, None); // 空白
    }

    #[test]
    fn test_extract_symbol_at_column_out_of_bounds() {
        let line = "short";
        let result = extract_symbol_at_column(line, 100);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_symbol_at_column_single_char() {
        let line = "x + y";
        // 1文字の識別子は除外される
        let result = extract_symbol_at_column(line, 0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_symbol_at_column_underscore_prefix() {
        let line = "    _handler.call();";
        let result = extract_symbol_at_column(line, 5);
        assert_eq!(result, Some(("_handler".to_string(), 4, 12)));
    }

    // masked_symbol_at_column

    #[test]
    fn masked_symbol_at_column_skips_trailing_line_comment() {
        // extract_symbol_at_column 単体では何の躊躇もなく "build" を返して
        // しまう――コメントという概念を持たないため。ホバーや Cmd+クリックが
        // 地の文をジャンプ先として扱わないようにしているのはこのマスクである。
        let src = "fn f() {\n    let x = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        let col = line.find("build").unwrap();
        assert_eq!(masked_symbol_at_column(line, col, 2, &mask), None);
    }

    #[test]
    fn masked_symbol_at_column_skips_string_literal() {
        let src = "fn f() {\n    let s = \"index\";\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        let col = line.find("index").unwrap();
        assert_eq!(masked_symbol_at_column(line, col, 2, &mask), None);
    }

    #[test]
    fn masked_symbol_at_column_allows_real_code() {
        // 上のコメントのケースと同じ形の行だが、コード側の識別子を指している。
        // マスクが過剰にマスクしていないことを確認する。
        let src = "fn f() {\n    let value = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        let col = line.find("value").unwrap();
        assert_eq!(
            masked_symbol_at_column(line, col, 2, &mask),
            Some(("value".to_string(), col, col + 5))
        );
    }

    #[test]
    fn build_hover_preview_windows_around_line() {
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

    #[test]
    fn extract_symbol_from_line_skips_comments_via_mask() {
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

        assert_eq!(extract_symbol_from_line(line(1), 1, &mask), None);
        assert_eq!(extract_symbol_from_line(line(2), 2, &mask), None);
        assert_eq!(extract_symbol_from_line(line(3), 3, &mask), None);
        assert_eq!(extract_symbol_from_line(line(4), 4, &mask), None);
        // 上の複数行ブロックコメントの継続行。
        assert_eq!(extract_symbol_from_line(line(7), 7, &mask), None);

        // #[derive(Debug)] はもはや特別扱いで除外されない。アトリビュートは
        // 地の文ではなく本物の構文であり、マスクがマスクするのはコメントと
        // 文字列だけなので、derive はコード位置にあり他の識別子と同様に
        // 返ってくる。以前のプレフィックスチェックは # で始まる行をすべて
        // 解決不能扱いしていたが、マスクは「コメントか文字列か」で線引きして
        // おり、アトリビュートはそのどちらでもない。
        assert_eq!(
            extract_symbol_from_line(line(10), 10, &mask),
            Some("derive".to_string())
        );

        // 実際のコード行は引き続き最初の識別子として解決される。
        assert_eq!(
            extract_symbol_from_line(line(13), 13, &mask),
            Some("state".to_string())
        );
        assert_eq!(
            extract_symbol_from_line(line(15), 15, &mask),
            Some("Building".to_string())
        );
    }

    #[test]
    fn extract_symbol_from_line_skips_trailing_comment_and_string_hits() {
        // プレフィックスチェックを置き換えた元の不具合: 実際の文の後ろに
        // トレイリングコメントが続くケース。x は1文字なので単独では除外され、
        // build/the/index はコメントの中にあるため、行全体が今は何も
        // 解決しない――以前の実装は行の「始まり方」しか見ておらず、行の途中の
        // // より後を見ていなかったため "build" を返してしまっていた。
        let src = "fn f() {\n    let x = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(extract_symbol_from_line(line, 2, &mask), None);

        // 同じ形だが、コメントの前に実際の識別子がある場合。修正が行全体を
        // 過剰にマスクしていないことを確認するため、これは解決されなければならない。
        let src = "fn f() {\n    let value = 1; // build the index\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(
            extract_symbol_from_line(line, 2, &mask),
            Some("value".to_string())
        );

        // 文字列リテラルも同様にその中身を隠す。
        let src = "fn f() {\n    let s = \"index\";\n}\n";
        let mask = crate::symbol_index::CodeMask::compute(src, "lib.rs");
        let line = src.lines().nth(1).unwrap();
        assert_eq!(extract_symbol_from_line(line, 2, &mask), None);
    }

    // ジャンプ下線の判定関数

    #[test]
    fn viewer_hover_symbol_color_none_when_not_jumpable() {
        // ジャンプ不可能な単語には、修飾キーの有無にかかわらず下線が付かない。
        assert_eq!(underline_color_kind(false, false), None);
        assert_eq!(underline_color_kind(false, true), None);
    }

    #[test]
    fn viewer_hover_symbol_color_hint_without_modifier() {
        // 修飾キー不要で、静止するだけで表示される――情報提供であって操作可能
        // という意味ではないことを示す色(Hint)。
        assert_eq!(
            underline_color_kind(true, false),
            Some(UnderlineColorKind::Hint)
        );
    }

    #[test]
    fn viewer_hover_symbol_color_accent_with_modifier() {
        // Cmd/Ctrl を押すと、同じ下線が「今押せばジャンプできる」という意味に昇格する。
        assert_eq!(
            underline_color_kind(true, true),
            Some(UnderlineColorKind::Accent)
        );
    }

    #[test]
    fn viewer_hover_symbol_popup_range_matches_target_line() {
        // ポップアップ自身の対象行・列は、マウスが現在どこにあるかに関係なく
        // 返される。
        assert_eq!(popup_highlight_range(true, 42, 4, 10, 42), Some((4, 10)));
    }

    #[test]
    fn viewer_hover_symbol_popup_range_none_off_target_line() {
        assert_eq!(popup_highlight_range(true, 42, 4, 10, 43), None);
    }

    #[test]
    fn viewer_hover_symbol_popup_range_none_when_hidden() {
        assert_eq!(popup_highlight_range(false, 42, 4, 10, 42), None);
    }

    #[test]
    fn viewer_hover_symbol_debounce_not_ready_before_150ms() {
        assert!(!underline_debounce_ready(
            std::time::Duration::from_millis(149),
            false
        ));
    }

    #[test]
    fn viewer_hover_symbol_debounce_ready_at_150ms() {
        assert!(underline_debounce_ready(
            std::time::Duration::from_millis(150),
            false
        ));
    }

    #[test]
    fn viewer_hover_symbol_debounce_not_ready_once_resolved() {
        // すでに解決済みの候補は、マウスが同じシンボルの上に静止している間、
        // tick のたびに再解決されることはない。
        assert!(!underline_debounce_ready(
            std::time::Duration::from_millis(500),
            true
        ));
    }
}
