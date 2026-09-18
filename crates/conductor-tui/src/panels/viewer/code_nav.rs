//! 定義・実装・参照へのジャンプと、その戻り道。
//!
//! 行の中にカーソルが無いので、候補が複数ある行では選ばせるしかない。先頭を黙って選ぶと
//! `pub use model::MenuItem;` のような行で必ず `model` に飛ぶ。

use std::path::{Path, PathBuf};

use conductor_core::syntax::{
    CodeMask, code_identifiers_on_line, identifier_occurrences, occurrence_span_in_source,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Position;
use sheaf_core::{Definition, Found, Implementations, Location, References, Store, SymbolDetail};

use conductor_core::semantic_index::{Bridge, kind_label};

use super::hover::{self, DefSite, Hover, Indexed, Pending};
use super::{ViewerPanel, render};
use crate::effect::Effect;
use crate::modal::Modal;
use crate::modal::references::Reference;
use crate::review::ReviewState;
use crate::workspace::{Ctx, StatusLevel};

/// ラベルは 1 文字なので、行内の候補はここで頭打ちになる。
const MAX_LABELS: usize = 26;

/// 索引がその答えをどこから出したか。
///
/// 飛んだ先の見た目からは区別が付かない。`relationships` に書いてあったのか符号の綴りから
/// 導いたのかで信頼度が違うので、答えるたびに名乗る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// 索引が書いているとおり。聞かれた位置のファイルも飛び先も索引生成時のまま。
    Index,
    /// 索引が符号の綴りから導いた。同名の trait が 2 つあると混ざる。
    Derived,
}

impl By {
    pub fn label(self) -> &'static str {
        match self {
            By::Index => "index",
            By::Derived => "index, by name",
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
}

impl Jump {
    fn from_key(c: char) -> Option<Self> {
        match c {
            'd' => Some(Jump::Definition),
            'i' => Some(Jump::Implementation),
            'r' => Some(Jump::References),
            'K' | 'h' => Some(Jump::Hover),
            _ => None,
        }
    }

    fn what(self) -> &'static str {
        match self {
            Jump::Definition => "definition",
            Jump::Implementation => "implementation",
            Jump::References => "references",
            Jump::Hover => "hover info",
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
                self.word_anchor(line_idx, occurrence, ctx),
            ),
        }
    }

    fn go_to_definition(
        &mut self,
        line_idx: usize,
        occurrence: usize,
        word: &str,
        ctx: &Ctx,
    ) -> Vec<Effect> {
        let Some(answer) = self.ask(ctx, line_idx, occurrence, sheaf_core::definition_at) else {
            return unresolved(ctx, word);
        };
        // 定義の上で押したなら、行きたいのは定義ではなく使われている場所。
        if self.answer_is_here(&answer, line_idx) {
            return self.find_references(line_idx, occurrence, word, ctx);
        }
        match definition_answer(answer) {
            Answered::Found(at, by, what) => self.land(word, locations(ctx.root, &at), by, what),
            Answered::NotCode => no_symbol(),
            Answered::Unresolved => unresolved(ctx, word),
        }
    }

    fn go_to_implementation(
        &mut self,
        line_idx: usize,
        occurrence: usize,
        word: &str,
        ctx: &Ctx,
    ) -> Vec<Effect> {
        let Some(answer) = self.ask(ctx, line_idx, occurrence, sheaf_core::implementations_at)
        else {
            return unresolved(ctx, word);
        };
        let (found, by, what) = match implementation_answer(answer) {
            Answered::Found(found, by, what) => (found, by, what),
            Answered::NotCode => return no_symbol(),
            Answered::Unresolved => return unresolved(ctx, word),
        };
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
        self.land(word, hits, by, what)
    }

    fn find_references(
        &mut self,
        line_idx: usize,
        occurrence: usize,
        word: &str,
        ctx: &Ctx,
    ) -> Vec<Effect> {
        let answer = self.ask(ctx, line_idx, occurrence, sheaf_core::references_at);
        let (hits, note) = match answer {
            None | Some(References::Unresolved) => return unresolved(ctx, word),
            Some(References::NotCode) => return no_symbol(),
            Some(References::Exact(found)) => reference_hits(ctx.root, &found),
        };
        let by = By::Index;
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

    /// 索引のどの答えかを必ず名乗る。
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
        let site = self.indexed_def_site(ctx, line_idx, occurrence, described.as_deref())?;
        let same_line =
            Some(site.path.as_str()) == self.content.path.as_deref() && site.line == line_idx + 1;
        let mut hover = hover::build(ctx.root, word, site, anchor)?;
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

    /// ポップアップの上では下に透けている語を拾わず、出ているものを居座らせる。
    pub fn pointer_moved(&mut self, col: u16, row: u16, ctx: &Ctx) {
        let on_popup = self.nav.hover.as_ref().is_some_and(|hover| {
            hover::rect(hover, ctx.theme, self.body).contains(Position::new(col, row))
        });
        if on_popup {
            if let Some(hover) = &mut self.nav.hover {
                hover.left_at = None;
            }
            return;
        }
        let over = self.word_at_screen(col, row, ctx);
        self.note_pointer(over);
    }

    /// ポップアップを押した。宣言か定義位置なら飛ぶ。
    pub fn click_hover(&mut self, col: u16, row: u16, ctx: &Ctx) -> Option<Vec<Effect>> {
        let hover = self.nav.hover.as_ref()?;
        let popup = hover::popup(hover, ctx.theme, self.body, self.highlighter_ref());
        let at = Position::new(col, row);
        let hit = |r: ratatui::layout::Rect| r.contains(at);
        if hit(popup.def_block) || hit(popup.def_row) {
            let (path, line) = (hover.path.clone(), hover.line);
            self.nav.hover = None;
            return Some(vec![Effect::JumpTo {
                path: PathBuf::from(path),
                line,
            }]);
        }
        // ポップアップの中の空振りは飲み込む。外側なら呼び出し側の通常処理へ。
        hit(popup.rect).then(Vec::new)
    }

    // ── 索引への問い合わせ ────────────────────────────────────────────────

    /// 索引に位置で聞く。索引が無ければ `None` (呼び出し側は `unresolved` を出す)。
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
    /// 囲んでいる型の定義は、聞かれた語の定義ではない。
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

    // ── 画面の 1 点を本文の座標へ ────────────────────────────────────────

    pub fn word_at_screen(&self, col: u16, row: u16, ctx: &Ctx) -> Option<Spotted> {
        if self.diff.active || row < self.body.y {
            return None;
        }
        let line_1 = self.line_at_screen(row, ctx)?;
        let text_col = self.text_col_at(col, ctx.review)?;
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
        let spot = self.word_at_screen(col, row, ctx)?;
        Some(self.run_jump(
            Jump::Definition,
            spot.line - 1,
            spot.occurrence,
            &spot.word,
            ctx,
        ))
    }

    /// 画面行が指している本文の行 (1 始まり)。
    fn line_at_screen(&self, row: u16, ctx: &Ctx) -> Option<usize> {
        let offset = row.checked_sub(self.body.y)? as usize;
        match render::origin_at(
            self,
            ctx.review,
            ctx.theme,
            ctx.config.ui.icon_set(),
            self.body.width,
            self.body.height as usize,
            offset,
        ) {
            render::Origin::Line(line_1) => Some(line_1),
            _ => None,
        }
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
    fn word_anchor(&self, line_idx: usize, occurrence: usize, ctx: &Ctx) -> (u16, u16) {
        let start = self
            .content
            .lines
            .get(line_idx)
            .and_then(|line| identifier_occurrences(line).nth(occurrence))
            .map_or(0, |(start, _, _)| start);
        let col = self.body.x as usize
            + self.gutter_width(ctx.review)
            + start.saturating_sub(self.scroll.column);
        let row = render::offset_of(
            self,
            ctx.review,
            ctx.theme,
            ctx.config.ui.icon_set(),
            self.body.width,
            self.body.height as usize,
            line_idx + 1,
        )
        .unwrap_or(0);
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

/// 索引の答えを、飛び先と名乗りに直したもの。
#[derive(Debug, PartialEq, Eq)]
enum Answered<T> {
    Found(T, By, &'static str),
    NotCode,
    Unresolved,
}

/// `Enclosing` は聞かれた語の定義ではなく、それを囲んでいる型の定義。`Exact` と
/// 同じ言い回しにすると、弱い主張が強い主張に紛れる。
fn definition_answer(answer: Definition) -> Answered<Vec<Location>> {
    match answer {
        Definition::NotCode => Answered::NotCode,
        Definition::Unresolved => Answered::Unresolved,
        Definition::Exact(at) => Answered::Found(at, By::Index, "definition"),
        Definition::Enclosing(found) => Answered::Found(
            found.iter().map(|e| e.definition.clone()).collect(),
            By::Index,
            "enclosing type (the symbol itself is not indexed)",
        ),
    }
}

fn implementation_answer(answer: Implementations) -> Answered<Vec<sheaf_core::Implementation>> {
    match answer {
        Implementations::NotCode => Answered::NotCode,
        Implementations::Unknown => Answered::Unresolved,
        Implementations::Exact(found) => Answered::Found(found, By::Index, "implementation"),
        Implementations::Derived(found) => Answered::Found(found, By::Derived, "implementation"),
    }
}

/// 索引が答えられないことを、理由と、いつなら答えられるかとともに言う。
fn unresolved(ctx: &Ctx, word: &str) -> Vec<Effect> {
    let index = &ctx.index.semantic;
    let why = if index.is_generating() {
        "indexing now"
    } else if index.is_pending() {
        "indexing starts when edits settle"
    } else if index.store(ctx.root).is_none() {
        "not indexed yet \u{2014} Repo \u{25b8} Rebuild Code Index"
    } else {
        "this file is not in the index \u{2014} Repo \u{25b8} Rebuild Code Index"
    };
    warn(format!("No indexed answer for '{word}' ({why})"))
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
pub(super) fn locations(root: &Path, at: &[Location]) -> Vec<Reference> {
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

/// 直接参照のうしろにインタフェース経由を並べ、内訳を見出しに添える。
pub(super) fn reference_hits(root: &Path, found: &Found) -> (Vec<Reference>, String) {
    let mut hits = locations(root, &found.direct);
    let indirect: Vec<Location> = found
        .via_interface
        .iter()
        .map(|v| v.reference.clone())
        .collect();
    let via = locations(root, &indirect);
    let note = if via.is_empty() {
        String::new()
    } else {
        format!(": {} direct, {} via interface", hits.len(), via.len())
    };
    hits.extend(via);
    (hits, note)
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

    /// SOURCE を 1 ファイル置いたツリーと、それを説明する索引を .conductor/ に書く。
    ///
    /// producer は動かさない。検査したいのは索引の作られ方ではなく、答えの読まれ方。
    fn indexed_tree() -> tempfile::TempDir {
        use protobuf::{EnumOrUnknown, Message, MessageField};
        use scip::types::{Document, Index, Metadata, Occurrence, TextEncoding};

        let dir = tempfile::TempDir::new().unwrap();
        // .conductor/ の置き場は commondir() から辿るので、git のツリーでないと見つからない。
        git2::Repository::init(dir.path()).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("lib.rs"), SOURCE).unwrap();

        let occurrence = |range: [i32; 3], name: &str, roles: i32| Occurrence {
            range: range.to_vec(),
            symbol: format!("scip-test cargo demo 0.1.0 {name}()."),
            symbol_roles: roles,
            ..Default::default()
        };
        let index = Index {
            metadata: MessageField::some(Metadata {
                project_root: format!("file://{}", dir.path().display()),
                text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
                ..Default::default()
            }),
            documents: vec![Document {
                relative_path: "lib.rs".to_string(),
                occurrences: vec![
                    occurrence([0, 7, 13], "target", 1),
                    occurrence([1, 7, 13], "caller", 1),
                    occurrence([1, 18, 24], "target", 0),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        // 鍵は調査が計算したものを使う。手で組むと、読む側が探す名前とずれる。
        let survey = conductor_core::semantic_index::survey(
            dir.path(),
            None,
            Some(Path::new("lib.rs")),
            &[],
        );
        let (root, key) = survey.roots.first().expect("Cargo.toml のルートが無い");
        let conductor_dir = dir.path().join(".conductor");
        std::fs::create_dir_all(&conductor_dir).unwrap();
        let target = root.target(&conductor_dir, dir.path(), key);
        std::fs::write(&target.index, index.write_to_bytes().unwrap()).unwrap();
        // 書式は sheaf の持ち物。手で綴ると読み書きが別々にずれる。
        let provenance = ["Cargo.toml", "lib.rs"]
            .into_iter()
            .map(|rel| {
                let bytes = std::fs::read(dir.path().join(rel)).unwrap();
                (PathBuf::from(rel), sheaf_core::blob_hash(&bytes))
            })
            .collect();
        sheaf_core::write_provenance(&target.hashes, &*root.lang.producer(), &provenance).unwrap();
        dir
    }

    struct Harness {
        ws: Workspace,
        svc: Services<crate::task::TaskResult>,
        _dir: tempfile::TempDir,
    }

    impl Harness {
        fn new() -> Self {
            let dir = indexed_tree();
            let mut ws = Workspace::for_test();
            let mut svc = Services::new();
            select_only_worktree(&mut ws, &mut svc, dir.path());
            ws.repo.root = dir.path().to_path_buf();
            ws.focus = Focus::Viewer;
            let mut harness = Self { ws, svc, _dir: dir };
            let effects = harness
                .ws
                .panels
                .viewer
                .open(Path::new("lib.rs"), None, None, false);
            crate::effect::apply(&mut harness.ws, &mut harness.svc, effects);
            pump(&mut harness.ws, &mut harness.svc);
            harness.load_index();
            harness.ws.focus = Focus::Viewer;
            harness
        }

        /// 調査と読み込みは背景の Task なので、届くまで frame を進める。
        fn load_index(&mut self) {
            let root = self.ws.panels.viewer.root().to_path_buf();
            for _ in 0..8 {
                if self.ws.index.semantic.store(&root).is_some() {
                    return;
                }
                let effects = crate::index::tick(&mut self.ws);
                crate::effect::apply(&mut self.ws, &mut self.svc, effects);
                pump(&mut self.ws, &mut self.svc);
            }
            panic!("索引が読み込まれない");
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

        fn word_at(&mut self, col: u16, row: u16) -> Option<Spotted> {
            let root = self.ws.panels.viewer.root().to_path_buf();
            let (panels, _, ctx) = self.ws.split(&root);
            panels.viewer.word_at_screen(col, row, &ctx)
        }

        fn install(&mut self, comments: Vec<conductor_core::review_store::ReviewComment>) {
            self.ws.review.install(Ok(crate::review::Snapshot {
                branch: "main".into(),
                comments,
                ..crate::review::Snapshot::default()
            }));
        }

        /// caller の行の target にマウスが止まった体でポップアップを出す。
        fn hover_by_mouse(&mut self) -> hover::Popup {
            let gutter = 1 + render::GUTTER_FIXED as u16;
            let spot = self.word_at(gutter + 18, 6).expect("target の上");
            assert_eq!((spot.word.as_str(), spot.line), ("target", 2));
            self.ws.panels.viewer.note_pointer(Some(spot));
            self.ws.panels.viewer.nav.pending.as_mut().unwrap().since =
                std::time::Instant::now() - hover::IDLE;
            let root = self.ws.panels.viewer.root().to_path_buf();
            let (panels, _, ctx) = self.ws.split(&root);
            assert!(panels.viewer.tick_hover(&ctx));
            let hover = panels
                .viewer
                .nav
                .hover
                .as_ref()
                .expect("ポップアップが出る");
            assert!(!hover.pinned);
            hover::popup(hover, ctx.theme, panels.viewer.body, None)
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
    fn 誰も呼んでいない関数は作り直しを勧めない() {
        let mut h = Harness::new();
        // caller はどこからも参照されていない。その定義行で gd を押すと参照一覧へ回る。
        h.ws.panels.viewer.scroll.line = 1;
        h.ch('g');
        h.ch('d');
        h.ch('a');

        let status = h.ws.chrome.status.as_ref().expect("何も言わずに黙った");
        assert!(
            status.text.contains("No references found"),
            "索引が 0 件と答えたのに別のことを言っている: {}",
            status.text
        );
        assert!(
            !status.text.contains("Rebuild"),
            "効かない作り直しを勧めている: {}",
            status.text
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

    #[test]
    fn 実装の答えはexactとderivedで名乗りが変わる() {
        let found = vec![implementation("a.rs", 4, "Foo")];
        let named = |answer| match implementation_answer(answer) {
            Answered::Found(_, by, _) => Some(by),
            _ => None,
        };
        assert_eq!(
            named(Implementations::Exact(found.clone())),
            Some(By::Index)
        );
        assert_eq!(named(Implementations::Derived(found)), Some(By::Derived));
        assert_eq!(named(Implementations::Unknown), None);
        assert_eq!(named(Implementations::NotCode), None);
        assert_ne!(By::Index.label(), By::Derived.label());
    }

    /// Exact と同じ言い回しにすると、「囲んでいる型に飛んだ」ことが画面から読めなくなる。
    #[test]
    fn 定義の答えは主張の強さで言い回しが変わる() {
        // Enclosing の中身は組み立てられない (SymbolId に公開の作り口が無い)。
        // ここで見たいのは飛び先ではなく言い回しなので、空で足りる。
        let cases: [(Definition, Answered<Vec<Location>>); 3] = [
            (
                Definition::Exact(vec![at("a.rs", 1)]),
                Answered::Found(vec![at("a.rs", 1)], By::Index, "definition"),
            ),
            (
                Definition::Enclosing(Vec::new()),
                Answered::Found(
                    Vec::new(),
                    By::Index,
                    "enclosing type (the symbol itself is not indexed)",
                ),
            ),
            (Definition::Unresolved, Answered::Unresolved),
        ];
        for (answer, expected) in cases {
            let label = format!("{answer:?}");
            assert_eq!(definition_answer(answer), expected, "{label}");
        }
    }

    /// 囲んでいる型の定義を「ここが定義」と読むと、押しただけで参照一覧に化ける。
    #[test]
    fn 定義位置の判定はexactの答えだけを見る() {
        let mut panel = ViewerPanel::new(&conductor_core::config::Config::default());
        panel.content.path = Some("a.rs".into());
        assert!(panel.answer_is_here(&Definition::Exact(vec![at("a.rs", 3)]), 3));
        assert!(!panel.answer_is_here(&Definition::Exact(vec![at("a.rs", 3)]), 4));
        assert!(!panel.answer_is_here(&Definition::Exact(vec![at("b.rs", 3)]), 3));
        assert!(!panel.answer_is_here(&Definition::Enclosing(Vec::new()), 3));
    }

    /// ずれると、画面で指した語と索引に聞く語が食い違う。
    #[test]
    fn 画面の桁は本文の語に解決する() {
        let mut h = Harness::new();
        h.ws.panels.viewer.body = ratatui::layout::Rect::new(0, 5, 80, 20);
        // 行番号 1 桁 + GUTTER_FIXED 4 = 5 桁ぶんがガター。
        let gutter = 1 + render::GUTTER_FIXED as u16;
        let spot = h.word_at(gutter + 7, 6).expect("caller の上");
        assert_eq!((spot.word.as_str(), spot.line), ("caller", 2));

        assert!(h.word_at(gutter - 1, 6).is_none(), "ガターの上は語ではない");
    }

    #[test]
    fn 開いたスレッドの下の語を指せる() {
        let mut h = Harness::new();
        h.ws.panels.viewer.body = ratatui::layout::Rect::new(0, 5, 80, 20);
        h.install(vec![crate::review::tests::comment("a", "lib.rs", 1, None)]);
        let gutter = (render::MARK + 1 + render::GUTTER_FIXED) as u16;

        assert!(h.word_at(gutter + 7, 6).is_none(), "6 行目はスレッドの本文");
        let rows = {
            let root = h.ws.panels.viewer.root().to_path_buf();
            let (panels, _, ctx) = h.ws.split(&root);
            render::offset_of(
                &panels.viewer,
                ctx.review,
                ctx.theme,
                ctx.config.ui.icon_set(),
                80,
                20,
                2,
            )
            .expect("2 行目は窓の中")
        };
        assert!(rows > 1, "スレッドが割り込んでいる: {rows}");
        let spot = h.word_at(gutter + 7, 5 + rows as u16).expect("caller の上");
        assert_eq!((spot.word.as_str(), spot.line), ("caller", 2));
    }

    #[test]
    fn ポップアップの上にマウスを運んでも下に透けている語に乗り換えない() {
        let mut h = Harness::new();
        h.ws.panels.viewer.body = ratatui::layout::Rect::new(0, 5, 80, 20);
        let popup = h.hover_by_mouse();
        let root = h.ws.panels.viewer.root().to_path_buf();
        let (panels, _, ctx) = h.ws.split(&root);

        // ポップアップは target の定義行に被さっている。素通しなら target を拾い直す。
        panels
            .viewer
            .pointer_moved(popup.rect.x + 1, popup.rect.y + 1, &ctx);
        let hover = panels.viewer.nav.hover.as_ref().expect("居座る");
        assert!(hover.left_at.is_none());

        let gutter = 1 + render::GUTTER_FIXED as u16;
        assert!(!popup.rect.contains(Position::new(gutter + 7, 6)));
        panels.viewer.pointer_moved(gutter + 7, 6, &ctx);
        assert!(
            panels.viewer.nav.hover.is_none(),
            "ポップアップの外の別の語に乗れば消える"
        );
    }

    #[test]
    fn 宣言の行を押すと定義へ飛ぶ() {
        let mut h = Harness::new();
        h.ws.panels.viewer.body = ratatui::layout::Rect::new(0, 5, 80, 20);
        let popup = h.hover_by_mouse();
        assert!(popup.def_block.height > 0, "宣言が出ている");
        let root = h.ws.panels.viewer.root().to_path_buf();
        let (panels, _, ctx) = h.ws.split(&root);
        let effects = panels
            .viewer
            .click_hover(popup.def_block.x + 3, popup.def_block.y, &ctx)
            .expect("ポップアップの中");
        assert!(
            matches!(&effects[..], [Effect::JumpTo { path, line: 1 }] if path == Path::new("lib.rs")),
            "{effects:?}"
        );
    }
}
