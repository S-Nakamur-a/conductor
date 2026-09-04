//! 定義・実装・参照へのジャンプと、その戻り道。
//!
//! 行の中にカーソルが無いので、候補が複数ある行では選ばせるしかない。先頭を黙って選ぶと
//! `pub use model::MenuItem;` のような行で必ず `model` に飛ぶ。

use std::path::{Path, PathBuf};

use conductor_core::symbol_index::{
    CodeMask, Reference, Symbol, SymbolIndex, code_identifiers_on_line, identifier_occurrences,
    occurrence_span_in_source,
};
use crossterm::event::{KeyCode, KeyEvent};
use sheaf_core::{Definition, Implementations, Location, References, Store, SymbolDetail};

use conductor_core::semantic_index::{Bridge, kind_label};

use super::hover::{self, DefSite, Hover, Indexed, Pending};
use super::{ViewerPanel, render};
use crate::effect::Effect;
use crate::modal::Modal;
use crate::review::ReviewState;
use crate::workspace::{Ctx, StatusLevel};

/// ラベルは 1 文字なので、行内の候補はここで頭打ちになる。
const MAX_LABELS: usize = 26;

/// どの層が答えたか。
///
/// 飛んだ先の見た目からは区別が付かない。索引が答えたのか構文層に落ちたのかで
/// 行番号の信頼度が違うので、答えるたびに名乗る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// SCIP 索引。聞かれた位置のファイルも飛び先も索引生成時のまま。
    Index,
    /// 索引が符号の綴りから導いた。同名の trait が 2 つあると混ざる。
    Derived,
    /// 構文層。索引に無い語、生成後に変わったファイル、索引そのものが無い場合。
    TreeSitter,
}

impl By {
    pub fn label(self) -> &'static str {
        match self {
            By::Index => "index",
            By::Derived => "index, by name",
            By::TreeSitter => "tree-sitter",
        }
    }
}

/// 語を選んだあとに走らせるもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jump {
    Definition,
    Implementation,
    References,
    Hover,
    /// その語にできることを並べる (構文層のみ)。
    Actions,
}

impl Jump {
    fn from_key(c: char) -> Option<Self> {
        match c {
            'd' => Some(Jump::Definition),
            'i' => Some(Jump::Implementation),
            'r' => Some(Jump::References),
            'K' | 'h' => Some(Jump::Hover),
            'a' => Some(Jump::Actions),
            _ => None,
        }
    }

    fn what(self) -> &'static str {
        match self {
            Jump::Definition => "definition",
            Jump::Implementation => "implementation",
            Jump::References => "references",
            Jump::Hover => "hover info",
            Jump::Actions => "actions",
        }
    }
}

/// 行内の語に付けたラベル。
#[derive(Debug)]
pub struct Labels {
    pub jump: Jump,
    /// 1 始まり。
    pub line: usize,
    pub picks: Vec<Pick>,
}

#[derive(Debug)]
pub struct Pick {
    pub label: char,
    pub word: String,
    pub start_col: usize,
}

/// 画面上の 1 点に見つかった語。
#[derive(Debug)]
pub struct Spotted {
    pub word: String,
    /// 1 始まり。
    pub line: usize,
    pub occurrence: usize,
    pub start_col: usize,
    /// 画面上の位置。ポップアップをここへ寄せる。
    pub anchor: (u16, u16),
}

/// ファイル上の位置。戻り道はここを覚えている。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spot {
    pub path: String,
    /// 0 始まり (スクロール位置)。
    pub line: usize,
    pub column: usize,
}

const MAX_HISTORY: usize = 200;

/// 戻る・進むの積み。
#[derive(Debug, Default)]
pub struct History {
    back: Vec<Spot>,
    forward: Vec<Spot>,
}

impl History {
    /// 新しい枝に入ったので、進む先は捨てる。
    pub fn push(&mut self, spot: Spot) {
        self.forward.clear();
        self.back.push(spot);
        if self.back.len() > MAX_HISTORY {
            self.back.remove(0);
        }
    }

    pub fn back(&mut self, current: Spot) -> Option<Spot> {
        let prev = self.back.pop()?;
        self.forward.push(current);
        Some(prev)
    }

    pub fn forward(&mut self, current: Spot) -> Option<Spot> {
        let next = self.forward.pop()?;
        self.back.push(current);
        Some(next)
    }

    #[cfg(test)]
    pub fn depth(&self) -> (usize, usize) {
        (self.back.len(), self.forward.len())
    }
}

/// Viewer が持つコードジャンプの状態。どれも入力を全部は奪わないので、
/// モーダルのスタックではなくパネルの中にいる。
#[derive(Debug, Default)]
pub struct CodeNav {
    /// 開いているファイルのマスク。読み込みと同じワーカーが作る。
    pub mask: CodeMask,
    pub labels: Option<Labels>,
    pub hover: Option<Hover>,
    /// マウスが止まるのを待っている候補。
    pub pending: Option<Pending>,
    pub history: History,
    /// g の 2 打鍵目を待っている。
    pub pending_g: bool,
}

impl CodeNav {
    pub fn reset_for_file(&mut self, mask: CodeMask) {
        self.mask = mask;
        self.labels = None;
        self.hover = None;
        self.pending = None;
        self.pending_g = false;
    }
}

/// 索引への 1 回の問い合わせに要るもの。タブ展開前の座標で持つ。
struct Site {
    rel: PathBuf,
    abs: PathBuf,
    source: String,
    line: u32,
    col: u32,
}

impl ViewerPanel {
    /// g の 2 打鍵目かラベルを待っている。route はこの間キーを丸ごとここへ渡す。
    pub fn awaiting_nav(&self) -> bool {
        self.nav.pending_g || self.nav.labels.is_some()
    }

    pub fn nav_chord(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        if self.nav.labels.is_some() {
            return self.label_key(key, ctx);
        }
        self.nav.pending_g = false;
        match key.code {
            KeyCode::Char('g') => {
                self.scroll.line = 0;
                Vec::new()
            }
            KeyCode::Char(c) => match Jump::from_key(c) {
                Some(jump) => self.start_jump(jump, ctx),
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn label_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        let Some(labels) = self.nav.labels.take() else {
            return Vec::new();
        };
        let KeyCode::Char(c) = key.code else {
            return Vec::new();
        };
        let Some(pick) = labels.picks.into_iter().find(|p| p.label == c) else {
            return Vec::new();
        };
        let line_idx = labels.line.saturating_sub(1);
        // ラベルは描画された桁に置いてある。索引は位置で引くので、桁から
        // 出現番号へ戻してから渡す。ずれると別の語の定義へ飛ぶ。
        let Some(occurrence) = self.occurrence_at(line_idx, pick.start_col) else {
            return Vec::new();
        };
        self.run_jump(labels.jump, line_idx, occurrence, &pick.word, ctx)
    }

    /// コードジャンプのキー。パネルが消費しなければ `None`。
    pub(super) fn nav_key(
        &mut self,
        action: conductor_core::keymap::Action,
        ctx: &Ctx,
    ) -> Option<Vec<Effect>> {
        use conductor_core::keymap::Action;
        match action {
            // g は前置。先頭へ飛ぶのは gg。
            Action::GoToTop if !self.diff.active => {
                self.nav.pending_g = true;
                Some(Vec::new())
            }
            Action::ShowHoverInfo => Some(self.start_jump(Jump::Hover, ctx)),
            Action::JumpBack => Some(self.history_step(false)),
            Action::JumpForward => Some(self.history_step(true)),
            _ => None,
        }
    }

    /// カーソル行の対象を決める。
    pub fn start_jump(&mut self, jump: Jump, ctx: &Ctx) -> Vec<Effect> {
        let line_idx = self.scroll.line;
        let Some(line) = self.content.lines.get(line_idx) else {
            return no_symbol();
        };
        let choices: Vec<(usize, usize, String)> =
            code_identifiers_on_line(line, line_idx + 1, &self.nav.mask)
                .take(MAX_LABELS)
                .collect();
        match choices.len() {
            0 => no_symbol(),
            1 => {
                let (occurrence, _, word) = &choices[0];
                self.run_jump(jump, line_idx, *occurrence, word, ctx)
            }
            _ => {
                self.nav.labels = Some(Labels {
                    jump,
                    line: line_idx + 1,
                    picks: choices
                        .iter()
                        .enumerate()
                        .map(|(i, (_, start, word))| Pick {
                            label: (b'a' + i as u8) as char,
                            word: word.clone(),
                            start_col: *start,
                        })
                        .collect(),
                });
                vec![Effect::Status(
                    StatusLevel::Info,
                    format!("Pick a symbol for {} (esc to cancel)", jump.what()),
                )]
            }
        }
    }

    /// 対象が決まったあと。ラベルで選んだ場合もここに合流する。
    fn run_jump(
        &mut self,
        jump: Jump,
        line_idx: usize,
        occurrence: usize,
        word: &str,
        ctx: &Ctx,
    ) -> Vec<Effect> {
        match jump {
            Jump::Definition => self.go_to_definition(line_idx, occurrence, word, ctx),
            Jump::Implementation => self.go_to_implementation(line_idx, occurrence, word, ctx),
            Jump::References => self.find_references(line_idx, occurrence, word, ctx),
            Jump::Hover => self.show_hover(
                line_idx,
                occurrence,
                word,
                ctx,
                self.word_anchor(line_idx, occurrence, ctx.review),
            ),
            Jump::Actions => self.open_actions(word, ctx),
        }
    }

    fn go_to_definition(
        &mut self,
        line_idx: usize,
        occurrence: usize,
        word: &str,
        ctx: &Ctx,
    ) -> Vec<Effect> {
        // 索引が引ければそちらが答える。構文層への切り替えまで sheaf 側で済むので、
        // 下の名前ベースの経路は索引が無いときだけ走る。
        if let Some(answer) = self.ask(ctx, line_idx, occurrence, sheaf_core::definition_at) {
            // 定義の上で押したなら、行きたいのは定義ではなく使われている場所。
            if self.answer_is_here(&answer, line_idx) {
                return self.find_references(line_idx, occurrence, word, ctx);
            }
            let Some((at, by, what)) = definition_answer(answer) else {
                return no_symbol();
            };
            return self.land(word, locations(ctx.root, &at), by, what);
        }
        let Some(symbols) = available(ctx) else {
            return not_ready();
        };
        let hits = from_symbols(&symbols.find_definitions(word, Path::new(self.reading())));
        if self.on_own_definition(&hits, line_idx) {
            return self.find_references(line_idx, occurrence, word, ctx);
        }
        self.land(word, hits, By::TreeSitter, "definition")
    }

    /// 名前で引いた答えなので、同名の別物を掴んでいないことまでは言えない。
    fn on_own_definition(&self, hits: &[Reference], line_idx: usize) -> bool {
        matches!(hits, [only]
            if Some(only.file_path.as_str()) == self.content.path.as_deref()
                && only.line == line_idx + 1)
    }

    fn go_to_implementation(
        &mut self,
        line_idx: usize,
        occurrence: usize,
        word: &str,
        ctx: &Ctx,
    ) -> Vec<Effect> {
        let answer = self.ask(ctx, line_idx, occurrence, sheaf_core::implementations_at);
        if let Some((found, by)) = implementation_answer(answer) {
            // impl ブロックの符号が無い形では着地点が最初のメソッドになる。行そのものより
            // 「どの型の実装か」が要る情報なので、一覧にはその型を並べる。
            let hits = found
                .iter()
                .map(|imp| Reference {
                    file_path: imp.site.path.to_string_lossy().into_owned(),
                    line: imp.site.line as usize + 1,
                    content: format!("impl {word} for {}", imp.ty),
                })
                .collect();
            return self.land(word, hits, by, "implementation");
        }
        let Some(symbols) = available(ctx) else {
            return not_ready();
        };
        let impls = symbols.find_implementations(word);
        self.land(word, from_symbols(&impls), By::TreeSitter, "implementation")
    }

    fn find_references(
        &mut self,
        line_idx: usize,
        occurrence: usize,
        word: &str,
        ctx: &Ctx,
    ) -> Vec<Effect> {
        let (hits, by, note) = match self.ask(ctx, line_idx, occurrence, sheaf_core::references_at)
        {
            Some(References::NotCode) => return no_symbol(),
            Some(References::Syntactic(at)) => {
                (locations(ctx.root, &at), By::TreeSitter, String::new())
            }
            // via_interface はそこへ実行が届くとは限らない (実測で 9 件中 8 件が静的に
            // 別の実装へ解決される例がある)。直接参照の後ろに、数を分けて並べる。
            Some(References::Exact(found)) => {
                let mut hits = locations(ctx.root, &found.direct);
                let indirect: Vec<Location> = found
                    .via_interface
                    .iter()
                    .map(|v| v.reference.clone())
                    .collect();
                let via = locations(ctx.root, &indirect);
                let note = if via.is_empty() {
                    String::new()
                } else {
                    format!(": {} direct, {} via interface", hits.len(), via.len())
                };
                hits.extend(via);
                (hits, By::Index, note)
            }
            None => match available(ctx) {
                Some(symbols) => (
                    symbols.find_references(word, ctx.root),
                    By::TreeSitter,
                    String::new(),
                ),
                None => return not_ready(),
            },
        };
        if hits.is_empty() {
            return warn(format!("No references found for '{word}' [{}]", by.label()));
        }
        let title = format!("{word} ({}{note})", by.label());
        vec![
            Effect::Status(
                StatusLevel::Info,
                format!("{} references for '{word}' [{}]", hits.len(), by.label()),
            ),
            Effect::PushModal(Modal::References(
                crate::modal::references::References::new(title, hits),
            )),
        ]
    }

    fn open_actions(&mut self, word: &str, ctx: &Ctx) -> Vec<Effect> {
        let Some(symbols) = available(ctx) else {
            return not_ready();
        };
        let actions = crate::modal::symbol_actions::SymbolActions::build(
            word,
            &symbols.find_definitions(word, Path::new(self.reading())),
            &symbols.find_implementations(word),
        );
        match actions {
            Some(actions) => vec![Effect::PushModal(Modal::SymbolActions(actions))],
            None => warn(format!("No navigation targets for '{word}'")),
        }
    }

    /// どの層が答えたかを必ず名乗る。
    fn land(&mut self, word: &str, hits: Vec<Reference>, by: By, what: &str) -> Vec<Effect> {
        match hits.as_slice() {
            [] => warn(format!("No {what} found for '{word}' [{}]", by.label())),
            [only] => {
                let (path, line) = (only.file_path.clone(), only.line);
                let mut effects = self.jump_to(&path, line);
                effects.push(Effect::Status(
                    StatusLevel::Success,
                    format!(
                        "Jumped to the {what} of '{word}' [{}] {path}:{line}",
                        by.label()
                    ),
                ));
                effects
            }
            many => {
                let n = many.len();
                vec![
                    Effect::Status(
                        StatusLevel::Info,
                        format!("{n} {what}s found for '{word}' [{}]", by.label()),
                    ),
                    Effect::PushModal(Modal::References(
                        crate::modal::references::References::new(
                            format!("{word} ({what}s, {})", by.label()),
                            hits,
                        ),
                    )),
                ]
            }
        }
    }

    fn jump_to(&self, path: &str, line: usize) -> Vec<Effect> {
        vec![Effect::JumpTo {
            path: PathBuf::from(path),
            line,
        }]
    }

    /// 戻り先として今の位置を積む。
    pub fn note_jump_from(&mut self) {
        if let Some(path) = self.content.path.clone() {
            self.nav.history.push(Spot {
                path,
                line: self.scroll.line,
                column: self.scroll.column,
            });
        }
    }

    fn history_step(&mut self, forward: bool) -> Vec<Effect> {
        let Some(path) = self.content.path.clone() else {
            return Vec::new();
        };
        let current = Spot {
            path,
            line: self.scroll.line,
            column: self.scroll.column,
        };
        let stepped = if forward {
            self.nav.history.forward(current)
        } else {
            self.nav.history.back(current)
        };
        let Some(spot) = stepped else {
            let which = if forward { "forward" } else { "back" };
            return vec![Effect::Status(
                StatusLevel::Info,
                format!("no further jump {which} in the history"),
            )];
        };
        // 積みは自分で動かしたので、開き直しで積み直させない。
        self.scroll.column = spot.column;
        vec![Effect::OpenFile {
            path: PathBuf::from(spot.path),
            line: Some(spot.line + 1),
            diff: None,
            preview: false,
        }]
    }

    // ── ホバー ────────────────────────────────────────────────────────────

    /// キーボードから即座に出す。出せない理由もステータスで返す —
    /// 受動的なマウスホバーと違い、押した本人が待っている。
    fn show_hover(
        &mut self,
        line_idx: usize,
        occurrence: usize,
        word: &str,
        ctx: &Ctx,
        anchor: (u16, u16),
    ) -> Vec<Effect> {
        match self.hover_at(line_idx, occurrence, word, ctx, anchor) {
            Some(mut hover) => {
                hover.pinned = true;
                self.nav.hover = Some(hover);
                Vec::new()
            }
            None => vec![Effect::Status(
                StatusLevel::Info,
                format!("No definition indexed for '{word}'"),
            )],
        }
    }

    /// 索引の答えは gd の飛び先と同じなので、ホバーの説明とジャンプ先がずれない。
    fn hover_at(
        &self,
        line_idx: usize,
        occurrence: usize,
        word: &str,
        ctx: &Ctx,
        anchor: (u16, u16),
    ) -> Option<Hover> {
        // 所属と説明は describe_at の 1 回で受ける。別々に聞くと同じ位置で 2 回
        // Document をデコードすることになる。
        let described = self.ask(ctx, line_idx, occurrence, sheaf_core::describe_at);
        let (site, by) =
            match self.indexed_def_site(ctx, line_idx, occurrence, described.as_deref()) {
                Some(site) => (site, By::Index),
                None => (
                    hover::resolve_def_site(
                        &ctx.index.symbols,
                        word,
                        self.content.path.as_deref(),
                    )?,
                    By::TreeSitter,
                ),
            };
        let same_line =
            Some(site.path.as_str()) == self.content.path.as_deref() && site.line == line_idx + 1;
        let mut hover = hover::build(&ctx.index.symbols, ctx.root, word, site, by, anchor)?;
        hover.on_definition_line = same_line;
        hover.container = described
            .as_ref()
            .and_then(|d| d.iter().find_map(|s| s.container.clone()));
        Some(hover)
    }

    /// 索引が位置で答えた定義。`Exact` のときだけ採る — `Enclosing` は囲んでいる型の
    /// 定義であって、聞かれた語の定義ではない。
    fn indexed_def_site(
        &self,
        ctx: &Ctx,
        line_idx: usize,
        occurrence: usize,
        described: Option<&[SymbolDetail]>,
    ) -> Option<DefSite> {
        let Definition::Exact(at) =
            self.ask(ctx, line_idx, occurrence, sheaf_core::definition_at)?
        else {
            return None;
        };
        let first = at.first()?;
        Some(DefSite {
            path: first.path.to_string_lossy().into_owned(),
            line: first.line as usize + 1,
            // 索引が種別を答えるので、tree-sitter から借りない。
            kind: String::new(),
            def_count: at.len(),
            detail: described.map(indexed_detail),
        })
    }

    /// マウスが乗った。前と同じ語なら数え直さない。
    pub fn note_pointer(&mut self, cand: Option<Spotted>) {
        match cand {
            Some(spot) => {
                let same = self
                    .nav
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.word == spot.word && p.line == spot.line);
                if same {
                    return;
                }
                self.nav.pending = Some(Pending {
                    word: spot.word,
                    line: spot.line,
                    occurrence: spot.occurrence,
                    start_col: spot.start_col,
                    anchor: spot.anchor,
                    since: std::time::Instant::now(),
                    resolved: false,
                });
                if let Some(hover) = &mut self.nav.hover {
                    hover.left_at = None;
                }
                self.nav.hover.take_if(|h| !h.pinned);
            }
            None => {
                self.nav.pending = None;
                // 出ているポップアップは猶予を置いてから消す。カーソルをポップアップまで
                // 運んでクリックできるようにするため。
                if let Some(hover) = &mut self.nav.hover
                    && hover.left_at.is_none()
                {
                    hover.left_at = Some(std::time::Instant::now());
                }
            }
        }
    }

    /// 毎フレーム。何か動いたら true。
    pub fn tick_hover(&mut self, ctx: &Ctx) -> bool {
        if let Some(hover) = &self.nav.hover
            && !hover.pinned
            && hover.left_at.is_some_and(|at| at.elapsed() >= hover::GRACE)
        {
            self.nav.hover = None;
            return true;
        }
        let ready = self
            .nav
            .pending
            .as_ref()
            .is_some_and(|p| !p.resolved && p.since.elapsed() >= hover::IDLE);
        if !ready {
            return false;
        }
        let (word, line, occurrence, anchor) = {
            let p = self.nav.pending.as_mut().expect("直前に見ている");
            p.resolved = true;
            (p.word.clone(), p.line, p.occurrence, p.anchor)
        };
        self.nav.hover = self.hover_at(line.saturating_sub(1), occurrence, &word, ctx, anchor);
        true
    }

    /// ポップアップのフッター行を押した。定義位置なら飛び、参照数なら一覧を開く。
    pub fn click_hover(&mut self, col: u16, row: u16, ctx: &Ctx) -> Option<Vec<Effect>> {
        let hover = self.nav.hover.as_ref()?;
        let popup = hover::popup(hover, ctx.theme, self.body, self.highlighter_ref());
        let hit = |r: ratatui::layout::Rect| {
            r.height > 0 && row == r.y && col >= r.x && col < r.x + r.width
        };
        if hit(popup.def_row) {
            let (path, line) = (hover.path.clone(), hover.line);
            self.nav.hover = None;
            return Some(vec![Effect::JumpTo {
                path: PathBuf::from(path),
                line,
            }]);
        }
        if hit(popup.refs_row) {
            let word = hover.word.clone();
            self.nav.hover = None;
            let hits = ctx.index.symbols.find_references(&word, ctx.root);
            if hits.is_empty() {
                return Some(warn(format!("No references found for '{word}'")));
            }
            return Some(vec![Effect::PushModal(Modal::References(
                crate::modal::references::References::new(
                    format!("{word} ({})", By::TreeSitter.label()),
                    hits,
                ),
            ))]);
        }
        // ポップアップの中の空振りは飲み込む。外側なら呼び出し側の通常処理へ。
        let r = popup.rect;
        (col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height).then(Vec::new)
    }

    // ── 索引への問い合わせ ────────────────────────────────────────────────

    /// 索引に位置で聞く。索引が無ければ `None` で、呼び出し側は構文層の経路へ落ちる。
    fn ask<T>(
        &self,
        ctx: &Ctx,
        line_idx: usize,
        occurrence: usize,
        query: fn(&Store, &dyn sheaf_core::SyntacticLayer, &Path, u32, u32) -> T,
    ) -> Option<T> {
        let store = ctx.index.semantic.store(ctx.root)?;
        let site = self.site(ctx.root, line_idx, occurrence)?;
        let bridge = Bridge {
            abs_path: &site.abs,
            source: &site.source,
            mask: &self.nav.mask,
            index: &ctx.index.symbols,
        };
        Some(query(store, &bridge, &site.rel, site.line, site.col))
    }

    /// 本文の行はタブ展開済み、索引の列は展開前。展開は識別子の数も並びも変えないので、
    /// 出現番号を経由すれば列だけ戻せる。
    fn site(&self, tree_root: &Path, line_idx: usize, occurrence: usize) -> Option<Site> {
        let rel = self.content.path.clone()?;
        let abs = tree_root.join(&rel);
        let source = std::fs::read_to_string(&abs).ok()?;
        let (col, _) = occurrence_span_in_source(source.lines().nth(line_idx)?, occurrence)?;
        Some(Site {
            rel: PathBuf::from(rel),
            abs,
            source,
            line: line_idx as u32,
            col: col as u32,
        })
    }

    /// その答えが聞かれた位置そのものを指しているか。`Exact` 以外は false —
    /// 構文層の答えは名前一致でしかなく、同名の別物を「ここが定義」と誤判定する。
    fn answer_is_here(&self, answer: &Definition, line_idx: usize) -> bool {
        let Some(current) = self.content.path.as_deref() else {
            return false;
        };
        let Definition::Exact(at) = answer else {
            return false;
        };
        at.iter()
            .any(|loc| loc.path == Path::new(current) && loc.line as usize == line_idx)
    }

    /// 描かれた行の `col` 桁に重なる識別子が、その行の何番目の出現か。
    pub fn occurrence_at(&self, line_idx: usize, col: usize) -> Option<usize> {
        let line = self.content.lines.get(line_idx)?;
        identifier_occurrences(line).position(|(start, end, _)| col >= start && col < end)
    }

    fn reading(&self) -> &str {
        self.content.path.as_deref().unwrap_or("")
    }

    // ── 画面の 1 点を本文の座標へ ────────────────────────────────────────

    /// 折りたたみは飛ばして数えるが、開いているスレッドの行はまだ数に入れていない。
    /// [ViewerPanel::click] と同じ数え方なので、ずれ方も同じになる。
    pub fn word_at_screen(&self, col: u16, row: u16, review: &ReviewState) -> Option<Spotted> {
        if self.diff.active || row < self.body.y {
            return None;
        }
        let line_1 = self.line_at_screen(row)?;
        let text_col = self.text_col_at(col, review)?;
        let line = self.content.lines.get(line_1 - 1)?;
        let (occurrence, (start, _, word)) = identifier_occurrences(line)
            .enumerate()
            .find(|(_, (start, end, _))| text_col >= *start && text_col < *end)?;
        if !self.nav.mask.is_code(line_1, occurrence) {
            return None;
        }
        Some(Spotted {
            word: word.to_string(),
            line: line_1,
            occurrence,
            start_col: start,
            anchor: (col, row),
        })
    }

    /// Cmd/Ctrl + クリック。桁を運んでいるので語を選ばせる必要がない。
    pub fn jump_at_screen(&mut self, col: u16, row: u16, ctx: &Ctx) -> Option<Vec<Effect>> {
        let spot = self.word_at_screen(col, row, ctx.review)?;
        Some(self.run_jump(
            Jump::Definition,
            spot.line - 1,
            spot.occurrence,
            &spot.word,
            ctx,
        ))
    }

    /// 画面行が指している本文の行 (1 始まり)。
    fn line_at_screen(&self, row: u16) -> Option<usize> {
        let offset = row.checked_sub(self.body.y)? as usize;
        self.fold
            .visible_from(self.scroll.line + 1, self.content.lines.len())
            .nth(offset)
    }

    /// 画面の桁を本文の桁へ直す。ガターの上なら `None`。
    fn text_col_at(&self, col: u16, review: &ReviewState) -> Option<usize> {
        let inside = col.checked_sub(self.body.x)? as usize;
        let gutter = self.gutter_width(review);
        inside
            .checked_sub(gutter)
            .map(|text| text + self.scroll.column)
    }

    /// 印 + 行番号 + 折りたたみ + 仕切りの幅。render の組み方と 1 対 1。
    fn gutter_width(&self, review: &ReviewState) -> usize {
        let has_comments = self
            .content
            .path
            .as_deref()
            .is_some_and(|path| !review.for_file(path).is_empty());
        let mark = if has_comments { render::MARK } else { 0 };
        mark + render::digit_count(self.content.lines.len()) + render::GUTTER_FIXED
    }

    /// 語を選んでいる最中の位置。ポップアップはそこへ寄せる。
    fn word_anchor(&self, line_idx: usize, occurrence: usize, review: &ReviewState) -> (u16, u16) {
        let start = self
            .content
            .lines
            .get(line_idx)
            .and_then(|line| identifier_occurrences(line).nth(occurrence))
            .map_or(0, |(start, _, _)| start);
        let col = self.body.x as usize
            + self.gutter_width(review)
            + start.saturating_sub(self.scroll.column);
        let row = self
            .fold
            .visible_index(line_idx + 1, self.content.lines.len())
            .saturating_sub(
                self.fold
                    .visible_index(self.scroll.line + 1, self.content.lines.len()),
            );
        (
            (col as u16).min(self.body.x + self.body.width.saturating_sub(1)),
            self.body.y + (row as u16).min(self.body.height.saturating_sub(1)),
        )
    }
}

/// ラベルを載せた行。候補の先頭 1 文字をラベルに差し替える。
///
/// 行そのものを描き直すのは、ラベルが本文の桁に重なるため。別の行に出すと、
/// どの語のラベルなのかが読めない。
pub fn label_line(
    labels: &Labels,
    text: &str,
    skip: usize,
    width: usize,
    theme: &conductor_core::theme::Theme,
) -> ratatui::text::Line<'static> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;

    let dim = Style::default().fg(theme.muted);
    let hot = Style::default()
        .fg(theme.selected_fg)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    let mut buffer = String::new();
    for (i, ch) in text.chars().enumerate().skip(skip).take(width) {
        match labels.picks.iter().find(|p| p.start_col == i) {
            Some(pick) => {
                if !buffer.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buffer), dim));
                }
                spans.push(Span::styled(pick.label.to_string(), hot));
            }
            None => buffer.push(ch),
        }
    }
    if !buffer.is_empty() {
        spans.push(Span::styled(buffer, dim));
    }
    ratatui::text::Line::from(spans)
}

/// `Enclosing` は聞かれた語の定義ではなく、それを囲んでいる型の定義。`Exact` と
/// 同じ言い回しにすると、弱い主張が強い主張に紛れる。
fn definition_answer(answer: Definition) -> Option<(Vec<Location>, By, &'static str)> {
    match answer {
        Definition::NotCode => None,
        Definition::Exact(at) => Some((at, By::Index, "definition")),
        Definition::Syntactic(at) => Some((at, By::TreeSitter, "definition")),
        Definition::Enclosing(found) => Some((
            found.iter().map(|e| e.definition.clone()).collect(),
            By::Index,
            "enclosing type (the symbol itself is not indexed)",
        )),
    }
}

/// 索引の実装の答えを、飛び先と名乗りに直す。
///
/// 空と `Unknown` は索引の答えにしない。呼び出し側が構文層へ落とす。
fn implementation_answer(
    answer: Option<Implementations>,
) -> Option<(Vec<sheaf_core::Implementation>, By)> {
    match answer? {
        Implementations::Exact(found) if !found.is_empty() => Some((found, By::Index)),
        Implementations::Derived(found) if !found.is_empty() => Some((found, By::Derived)),
        _ => None,
    }
}

/// 索引が答えないうちは名前でも引けない。
fn available<'a>(ctx: &'a Ctx) -> Option<&'a SymbolIndex> {
    ctx.index
        .symbols
        .is_available()
        .then_some(&ctx.index.symbols)
}

fn not_ready() -> Vec<Effect> {
    warn("The code index is not ready yet".to_string())
}

fn no_symbol() -> Vec<Effect> {
    warn("No symbol under the cursor".to_string())
}

fn warn(text: String) -> Vec<Effect> {
    vec![Effect::Status(StatusLevel::Warning, text)]
}

/// 同じ位置に複数の符号が乗ることがある (`Struct { file_path }` の省略記法ならフィールドと
/// ローカル束縛の両方) ので、説明を持っている方を先に採る。
fn indexed_detail(described: &[SymbolDetail]) -> Indexed {
    let Some(detail) = described
        .iter()
        .find(|d| d.signature.is_some())
        .or_else(|| described.first())
    else {
        return Indexed::default();
    };
    let mut signature: Vec<String> = detail
        .signature
        .iter()
        .flat_map(|s| s.lines())
        .map(str::to_string)
        .collect();
    if signature.len() > hover::MAX_SIGNATURE_LINES {
        signature.truncate(hover::MAX_SIGNATURE_LINES);
        signature.push("\u{2026}".to_string());
    }
    Indexed {
        kind: kind_label(detail.kind).to_string(),
        signature,
        doc: detail
            .documentation
            .iter()
            .flat_map(|d| d.lines())
            .map(str::to_string)
            .collect(),
    }
}

/// 索引が返した位置に、その行の本文を添えて一覧の形にする。
///
/// 読めなかった行は落とさずに空文字で残す。索引がその位置を答えた事実と、こちらが
/// ファイルを読めたかどうかは別で、黙って消すと件数が合わなくなる。
fn locations(root: &Path, at: &[Location]) -> Vec<Reference> {
    let mut sources: std::collections::HashMap<&Path, Option<Vec<String>>> = Default::default();
    at.iter()
        .map(|loc| {
            let lines = sources.entry(&loc.path).or_insert_with(|| {
                std::fs::read_to_string(root.join(&loc.path))
                    .ok()
                    .map(|text| text.lines().map(str::to_string).collect())
            });
            Reference {
                file_path: loc.path.to_string_lossy().into_owned(),
                // sheaf の Location::line は 0 始まり、Reference::line は 1 始まり。
                line: loc.line as usize + 1,
                content: lines
                    .as_ref()
                    .and_then(|l| l.get(loc.line as usize))
                    .cloned()
                    .unwrap_or_default(),
            }
        })
        .collect()
}

fn from_symbols(found: &[Symbol]) -> Vec<Reference> {
    found
        .iter()
        .map(|s| Reference {
            file_path: s.file_path.clone(),
            line: s.line,
            content: format!("{:?} {}", s.kind, s.name),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::on_key;
    use crate::testing::{pump, select_only_worktree};
    use crate::workspace::{Focus, Workspace};
    use conductor_svc::Services;
    use crossterm::event::{KeyCode, KeyModifiers};
    use sheaf_core::Implementation;

    const SOURCE: &str = "\
pub fn target() {}
pub fn caller() { target(); }
";

    struct Harness {
        ws: Workspace,
        svc: Services<crate::task::TaskResult>,
        _dir: tempfile::TempDir,
    }

    impl Harness {
        /// 索引まで組んだ Viewer。tree-sitter は同期で構築する — 検査したいのは
        /// 索引の作られ方ではなく、答えの読まれ方。
        fn new() -> Self {
            let dir = tempfile::TempDir::new().unwrap();
            std::fs::write(dir.path().join("lib.rs"), SOURCE).unwrap();
            let mut ws = Workspace::for_test();
            let mut svc = Services::new();
            select_only_worktree(&mut ws, &mut svc, dir.path());
            ws.index.symbols.set_root(dir.path().to_path_buf());
            ws.index.symbols.build();
            ws.focus = Focus::Viewer;
            let mut harness = Self { ws, svc, _dir: dir };
            let effects = harness
                .ws
                .panels
                .viewer
                .open(Path::new("lib.rs"), None, None, false);
            crate::effect::apply(&mut harness.ws, &mut harness.svc, effects);
            pump(&mut harness.ws, &mut harness.svc);
            harness.ws.focus = Focus::Viewer;
            harness
        }

        fn press(&mut self, key: KeyEvent) {
            on_key(&mut self.ws, &mut self.svc, key);
            pump(&mut self.ws, &mut self.svc);
        }

        fn ch(&mut self, c: char) {
            self.press(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }

        fn viewer(&self) -> &ViewerPanel {
            &self.ws.panels.viewer
        }
    }

    fn at(path: &str, line: u32) -> Location {
        Location {
            path: PathBuf::from(path),
            line,
            col: 0,
        }
    }

    fn implementation(path: &str, line: u32, ty: &str) -> Implementation {
        Implementation {
            site: at(path, line),
            ty: ty.to_string(),
        }
    }

    /// パネル単体では「ラベルの桁 → 出現番号 → 索引」の往復が出ない。
    #[test]
    fn gdでラベルを選んで飛びctrl_oで戻る() {
        let mut h = Harness::new();
        // caller の行。候補は caller と target の 2 つ。
        h.ws.panels.viewer.scroll.line = 1;

        h.ch('g');
        assert!(h.viewer().awaiting_nav(), "g が前置になっていない");
        h.ch('d');
        let labels = h.viewer().nav.labels.as_ref().expect("ラベルが出ていない");
        assert_eq!(
            labels
                .picks
                .iter()
                .map(|p| p.word.as_str())
                .collect::<Vec<_>>(),
            ["caller", "target"]
        );

        // 2 つ目のラベルが target。先頭を黙って選ぶ実装ならここで caller に飛ぶ。
        h.ch('b');
        assert!(h.viewer().nav.labels.is_none(), "選んだあともラベルが残る");
        assert_eq!(h.viewer().active_path(), Some("lib.rs"));
        assert_eq!(h.viewer().scroll.line, 0, "target の定義行");

        h.press(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert_eq!(h.viewer().scroll.line, 1, "飛ぶ前の行へ戻る");

        h.press(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::CONTROL));
        assert_eq!(h.viewer().scroll.line, 0, "進むと飛び先へ");
    }

    #[test]
    fn 候補が1つの行はラベルを出さずそのまま飛ぶ() {
        let mut h = Harness::new();
        // target の宣言行。候補は target だけ。
        h.ws.panels.viewer.scroll.line = 0;
        h.ch('g');
        h.ch('d');
        assert!(h.viewer().nav.labels.is_none());
        // 定義の上で押したので、行き先は定義ではなく参照の一覧。
        assert!(
            matches!(h.ws.modals.last(), Some(Modal::References(_))),
            "{:?}",
            h.ws.modals.last()
        );
    }

    #[test]
    fn ggは前置の2打鍵目として先頭へ飛ぶ() {
        let mut h = Harness::new();
        h.ws.panels.viewer.scroll.line = 1;
        h.ch('g');
        h.ch('g');
        assert_eq!(h.viewer().scroll.line, 0);
        assert!(!h.viewer().awaiting_nav());
    }

    #[test]
    fn 履歴の両端では戻り先が無いことを伝える() {
        let mut h = Harness::new();
        h.press(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        let status = h.ws.chrome.status.as_ref().expect("何も言わずに黙った");
        assert!(status.text.contains("no further jump back"), "{status:?}");
    }

    #[test]
    fn 履歴は上限で頭打ちになり新しい枝で進む先を捨てる() {
        let mut history = History::default();
        let spot = |line| Spot {
            path: "a.rs".into(),
            line,
            column: 0,
        };
        for line in 0..MAX_HISTORY + 50 {
            history.push(spot(line));
        }
        assert_eq!(history.depth(), (MAX_HISTORY, 0));

        assert_eq!(history.back(spot(999)), Some(spot(MAX_HISTORY + 49)));
        assert_eq!(history.depth().1, 1);
        history.push(spot(1));
        assert_eq!(history.depth().1, 0, "新しい枝で進む先が残っている");
        assert!(history.forward(spot(1)).is_none());
    }

    /// Exact と Derived を同じ言葉で見せると、綴りから導いただけの答えが
    /// producer の申告と同じ確度に見える。
    #[test]
    fn 実装の答えはexactとderivedで名乗りが変わる() {
        let found = vec![implementation("a.rs", 4, "Foo")];
        let cases = [
            (Some(Implementations::Exact(found.clone())), Some(By::Index)),
            (
                Some(Implementations::Derived(found.clone())),
                Some(By::Derived),
            ),
            (Some(Implementations::Exact(Vec::new())), None),
            (Some(Implementations::Derived(Vec::new())), None),
            (Some(Implementations::Unknown), None),
            (Some(Implementations::NotCode), None),
            (None, None),
        ];
        for (answer, expected) in cases {
            let label = format!("{answer:?}");
            assert_eq!(
                implementation_answer(answer).map(|(_, by)| by),
                expected,
                "{label}"
            );
        }
        assert_ne!(By::Index.label(), By::Derived.label());
    }

    /// Exact と同じ言い回しにすると、「囲んでいる型に飛んだ」ことが画面から読めなくなる。
    #[test]
    fn 定義の答えは層と主張の強さで言い回しが変わる() {
        // Enclosing の中身は組み立てられない (SymbolId に公開の作り口が無い)。
        // ここで見たいのは飛び先ではなく言い回しなので、空で足りる。
        let cases: [(Definition, Option<(By, &str)>); 4] = [
            (
                Definition::Exact(vec![at("a.rs", 1)]),
                Some((By::Index, "definition")),
            ),
            (
                Definition::Syntactic(vec![at("a.rs", 1)]),
                Some((By::TreeSitter, "definition")),
            ),
            (
                Definition::Enclosing(Vec::new()),
                Some((
                    By::Index,
                    "enclosing type (the symbol itself is not indexed)",
                )),
            ),
            (Definition::NotCode, None),
        ];
        for (answer, expected) in cases {
            let label = format!("{answer:?}");
            assert_eq!(
                definition_answer(answer).map(|(_, by, what)| (by, what)),
                expected,
                "{label}"
            );
        }
    }

    /// 構文層の答えを使うと、同名の別物の上で押しただけで参照一覧に化ける。
    #[test]
    fn 定義位置の判定はexactの答えだけを見る() {
        let mut panel = ViewerPanel::new(&conductor_core::config::Config::default());
        panel.content.path = Some("a.rs".into());
        assert!(panel.answer_is_here(&Definition::Exact(vec![at("a.rs", 3)]), 3));
        assert!(!panel.answer_is_here(&Definition::Exact(vec![at("a.rs", 3)]), 4));
        assert!(!panel.answer_is_here(&Definition::Exact(vec![at("b.rs", 3)]), 3));
        assert!(!panel.answer_is_here(&Definition::Syntactic(vec![at("a.rs", 3)]), 3));
    }

    /// ずれると、画面で指した語と索引に聞く語が食い違う。
    #[test]
    fn 画面の桁は本文の語に解決する() {
        let mut h = Harness::new();
        h.ws.panels.viewer.body = ratatui::layout::Rect::new(0, 5, 80, 20);
        let review = crate::review::ReviewState::default();
        // 行番号 1 桁 + GUTTER_FIXED 4 = 5 桁ぶんがガター。
        let gutter = 1 + render::GUTTER_FIXED as u16;
        let spot = h
            .viewer()
            .word_at_screen(gutter + 7, 6, &review)
            .expect("caller の上");
        assert_eq!((spot.word.as_str(), spot.line), ("caller", 2));

        assert!(
            h.viewer().word_at_screen(gutter - 1, 6, &review).is_none(),
            "ガターの上は語ではない"
        );
    }
}
