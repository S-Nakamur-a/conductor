//! Claude 区画に重ねる読み取り専用のトランスクリプト。
//!
//! ライブ PTY のスクロールバックは vt100 の行数で頭打ちになるので、上へ遡る操作は
//! セッションの .jsonl そのものを読むこのビューに入る。

mod build;
mod render;
mod style;
#[cfg(test)]
mod tests;
mod tool;
mod wrap;

use std::time::Instant;

use conductor_core::claude_log::LogEntry;
use conductor_core::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::panels::viewer::syntax::Highlighter;

use build::LineMeta;

pub(crate) use render::render;

/// 入場の枠色遷移にかける時間。これ以上長いと読みモードへ入る操作がもたつく。
const TRANSITION_MS: u64 = 500;

/// キーを処理した結果。ビューを畳むかどうかだけを持ち主へ返す。
pub(super) enum Handled {
    Consumed,
    Close,
}

pub(crate) struct Reflow {
    /// 映しているログの session id。届いた結果がこれと違えば捨てる。
    session: String,
    entries: Vec<LogEntry>,
    loading: bool,
    scroll: usize,
    /// 最新ターンに張り付いているか。組み直したときの着地点がこれで決まる。
    follow: bool,
    expanded: bool,
    /// 幅の変化以外での無効化。今のところ expanded の切り替えだけ。
    needs_rebuild: bool,
    size: (u16, u16),
    lines: Vec<Line<'static>>,
    meta: Vec<LineMeta>,
    sweep: Option<Instant>,
    /// 直前のフレームでモーダルが開いていたか。
    overlay_was_open: bool,
    wants_clear: bool,
}

impl Reflow {
    pub(super) fn opening(session: String) -> Self {
        Self {
            session,
            entries: Vec::new(),
            loading: true,
            scroll: 0,
            follow: true,
            // Claude Code 自身の既定に合わせて折りたたみで開く。
            expanded: false,
            needs_rebuild: false,
            size: (0, 0),
            lines: Vec::new(),
            meta: Vec::new(),
            sweep: Some(Instant::now()),
            overlay_was_open: false,
            wants_clear: true,
        }
    }

    #[cfg(test)]
    pub(super) fn is_loading(&self) -> bool {
        self.loading
    }

    pub(super) fn session(&self) -> &str {
        &self.session
    }

    /// 読み終えたログを載せる。session id の照合は呼び出し側が済ませている。
    pub(super) fn install(&mut self, entries: Vec<LogEntry>) {
        self.entries = entries;
        self.loading = false;
        self.follow = true;
        self.size.1 = 0;
        self.wants_clear = true;
    }

    pub(crate) fn border_color(&self, theme: &Theme) -> ratatui::style::Color {
        let complement = Theme::complement(theme.accent);
        match self.sweep {
            Some(start) => Theme::lerp(theme.accent, complement, eased(progress(start))),
            None => complement,
        }
    }

    pub(super) fn take_clear_request(&mut self) -> bool {
        std::mem::take(&mut self.wants_clear)
    }

    /// 描く前に、行の組み直しとスクロール位置を確定させる。
    pub(super) fn prepare(
        &mut self,
        theme: &Theme,
        highlighter: &Highlighter,
        size: (u16, u16),
        overlay_open: bool,
    ) {
        // モーダルは未書き込みのセルを塗ってしまうので、閉じた後は強制再描画でしか消せない。
        self.wants_clear |= self.overlay_was_open && !overlay_open;
        self.overlay_was_open = overlay_open;
        if self.sweep.is_some_and(|start| progress(start) >= 1.0) {
            self.sweep = None;
        }

        let (height, width) = size;
        let mut anchored = None;
        if self.size.1 != width || self.needs_rebuild {
            let ctx = build::Ctx {
                theme,
                highlighter,
                expanded: self.expanded,
            };
            let built = build::build(&ctx, &self.entries, width as usize);
            // 組み直す前にビューポートの先頭が何だったかを覚える。生の行番号は幅ごとに
            // 意味が変わるので、そのまま引き継ぐと無関係な位置に飛ぶ。
            let anchor = self.meta.get(self.scroll).copied();
            self.lines = built.lines;
            self.meta = built.meta;
            self.needs_rebuild = false;
            self.wants_clear = true;
            anchored = anchor.map(|a| render::anchor_index(&self.meta, a));
        }
        self.size = size;
        self.scroll = scroll_after_reflow(
            self.follow,
            anchored,
            self.scroll,
            self.lines.len(),
            height as usize,
        );
    }

    pub(super) fn key(&mut self, key: KeyEvent) -> Handled {
        let inner = self.size.0 as usize;
        let total = self.lines.len();
        let page = self.page();
        let at_bottom_before = at_bottom(self.scroll, total, inner);
        let before = self.scroll;

        match key.code {
            // 最下部でさらに下へ押したらライブ PTY へ戻す。
            KeyCode::Char('j') | KeyCode::Down if at_bottom_before => return Handled::Close,
            KeyCode::Char('d') | KeyCode::PageDown if at_bottom_before && page_down(&key) => {
                return Handled::Close;
            }
            KeyCode::Esc => return Handled::Close,

            KeyCode::Char('j') | KeyCode::Down => self.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('d') | KeyCode::PageDown if page_down(&key) => self.scroll += page,
            KeyCode::Char('u') | KeyCode::PageUp if page_up(&key) => {
                self.scroll = self.scroll.saturating_sub(page)
            }
            KeyCode::Char('g') | KeyCode::Home => self.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => {
                self.jump_to_latest();
                return Handled::Consumed;
            }
            // Claude Code の ctrl+o に合わせる。カーソルが無いのでビュー全体で 1 つのトグル。
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.expanded = !self.expanded;
                self.needs_rebuild = true;
                return Handled::Consumed;
            }
            _ => return Handled::Consumed,
        }

        self.scroll = clamp_scroll(self.scroll, total, inner);
        // どのキーで動いたかではなく着地点から follow を決める。最新行が見えていれば追従。
        self.follow = at_bottom(self.scroll, total, inner);
        self.wants_clear |= self.scroll != before;
        Handled::Consumed
    }

    pub(super) fn page(&self) -> usize {
        (self.size.0 as usize / 2).max(1)
    }

    pub(super) fn scroll_to_top(&mut self) {
        self.wants_clear |= self.scroll != 0;
        self.scroll = 0;
        self.follow = false;
    }

    /// 上下どちらでも、着地点から follow を導出し直す。
    pub(super) fn scroll_by(&mut self, delta: isize) {
        let (inner, total) = (self.size.0 as usize, self.lines.len());
        let want = if delta < 0 {
            self.scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.scroll.saturating_add(delta as usize)
        };
        let before = self.scroll;
        self.scroll = clamp_scroll(want, total, inner);
        self.follow = at_bottom(self.scroll, total, inner);
        self.wants_clear |= self.scroll != before;
    }

    /// 最新ターンへ飛んで追従を再開する。G/End とチップのクリックが共有する唯一の入口。
    pub(super) fn jump_to_latest(&mut self) {
        self.scroll = bottom_scroll(self.lines.len(), self.size.0 as usize);
        self.follow = true;
        self.wants_clear = true;
    }

    /// 「最新へ」チップの矩形。描画とクリック判定が同じ答えを見る。
    pub(super) fn badge_rect(&self, area: Rect) -> Option<Rect> {
        render::badge(area, self.follow).map(|(rect, _)| rect)
    }
}

fn page_down(key: &KeyEvent) -> bool {
    key.code == KeyCode::PageDown || key.modifiers.contains(KeyModifiers::CONTROL)
}

fn page_up(key: &KeyEvent) -> bool {
    key.code == KeyCode::PageUp || key.modifiers.contains(KeyModifiers::CONTROL)
}

fn clamp_scroll(scroll: usize, total: usize, inner: usize) -> usize {
    scroll.min(total.saturating_sub(inner))
}

fn at_bottom(scroll: usize, total: usize, inner: usize) -> bool {
    scroll >= total.saturating_sub(inner)
}

fn bottom_scroll(total: usize, inner: usize) -> usize {
    total.saturating_sub(inner)
}

/// 幅や高さが動いた後にビューポートが居るべき位置。
///
/// 追従中は最下部へ固定し直す。区画が狭くなると同じ本文がより多くの行に折り返されるので、
/// 正しく再アンカーした古いオフセットでも末尾には届かない。追従していなければ anchored
/// (組み直した後の同じ論理行) を尊重し、組み直しの無かったフレームは previous に落ちる。
fn scroll_after_reflow(
    following: bool,
    anchored: Option<usize>,
    previous: usize,
    total: usize,
    inner: usize,
) -> usize {
    let target = if following {
        bottom_scroll(total, inner)
    } else {
        anchored.unwrap_or(previous)
    };
    clamp_scroll(target, total, inner)
}

fn progress(start: Instant) -> f64 {
    (start.elapsed().as_millis() as f64 / TRANSITION_MS as f64).clamp(0.0, 1.0)
}

/// 両端で傾きがゼロになる smoothstep。枠の色相がちらつかずに始まり収まる。
fn eased(progress: f64) -> f64 {
    let p = progress.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}
