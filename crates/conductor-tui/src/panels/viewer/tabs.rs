//! 開いているファイルのタブ。
//!
//! アクティブなタブの状態は [ViewerPanel] が実体を持ち、非アクティブなぶんだけが
//! [Tab::stashed] へ退避される。実体が「タブの中」と「パネルの直下」の 2 か所に
//! 現れると、どちらを書いたかで表示がずれる。

use super::{Content, DiffPane, FoldState, Scroll, Search, Selection, ViewerPanel};
use crate::effect::Effect;

/// タブをいつ閉じるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabStatus {
    /// 明示的に閉じるまで残る。
    #[default]
    Persistent,
    /// ちょっと見るだけ。高々 1 枚で、しかも必ずアクティブ。
    Preview,
}

impl TabStatus {
    pub fn is_preview(self) -> bool {
        self == Self::Preview
    }

    /// 永続で開き直すと固定される。クリックしたファイルを Enter で固定する経路がこれ。
    fn reopened(self, requested: Self) -> Self {
        match requested {
            Self::Persistent => Self::Persistent,
            Self::Preview => self,
        }
    }
}

#[derive(Debug)]
pub struct Tab {
    /// 根からの相対パス。
    pub path: String,
    pub status: TabStatus,
    stashed: Option<Stashed>,
}

/// 非アクティブな間の退避先。
#[derive(Debug, Default)]
struct Stashed {
    content: Content,
    diff: DiffPane,
    search: Search,
    fold: FoldState,
    selection: Selection,
    scroll: Scroll,
}

impl ViewerPanel {
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn active_tab(&self) -> usize {
        self.active
    }

    pub fn active_path(&self) -> Option<&str> {
        self.tabs.get(self.active).map(|t| t.path.as_str())
    }

    /// relative_path のタブを用意してアクティブにする。新しく作ったときだけ true。
    pub(super) fn activate_tab_for(&mut self, relative_path: &str, status: TabStatus) -> bool {
        if let Some(idx) = self.tabs.iter().position(|t| t.path == relative_path) {
            if idx == self.active {
                let tab = &mut self.tabs[idx];
                tab.status = tab.status.reopened(status);
                return false;
            }
            self.stash();
            let idx = self.drop_preview(idx);
            self.active = idx;
            self.restore(idx);
            return false;
        }
        self.stash();
        let len = self.tabs.len();
        let at = self.drop_preview(len);
        self.tabs.insert(
            at,
            Tab {
                path: relative_path.to_string(),
                status,
                stashed: None,
            },
        );
        self.active = at;
        true
    }

    /// 次/前のタブへ。端は巻き戻る。
    pub(super) fn step_tab(&mut self, delta: isize) -> Vec<Effect> {
        if self.tabs.len() < 2 {
            return Vec::new();
        }
        let len = self.tabs.len() as isize;
        let next = ((self.active as isize + delta).rem_euclid(len)) as usize;
        self.focus_tab(next)
    }

    /// idx のタブへ切り替える。中身はディスクから読み直すので、裏で書き換えられた
    /// ファイルが古いまま残らない。
    pub(super) fn focus_tab(&mut self, idx: usize) -> Vec<Effect> {
        if idx >= self.tabs.len() {
            return Vec::new();
        }
        if idx != self.active {
            self.stash();
            let idx = self.drop_preview(idx);
            self.active = idx;
            self.restore(idx);
        }
        let path = self.tabs[self.active].path.clone();
        self.request(path, None, None, true)
    }

    /// idx のタブを閉じる。アクティブなら右隣 (無ければ左隣) へ移り、最後の 1 枚を
    /// 閉じたらファイル未選択へ戻る。
    pub(super) fn close_tab(&mut self, idx: usize) -> Vec<Effect> {
        if idx >= self.tabs.len() {
            return Vec::new();
        }
        if idx != self.active {
            self.tabs.remove(idx);
            if idx < self.active {
                self.active -= 1;
            }
            return Vec::new();
        }

        self.tabs.remove(idx);
        self.clear_active();
        if self.tabs.is_empty() {
            self.active = 0;
            return Vec::new();
        }
        self.active = idx.min(self.tabs.len() - 1);
        self.restore(self.active);
        let path = self.tabs[self.active].path.clone();
        self.request(path, None, None, true)
    }

    /// 新しい根に無いファイルのタブを閉じる。
    pub(super) fn prune_tabs_to_root(&mut self) -> Vec<Effect> {
        let before = self.tabs.len();
        let active_path = self.active_path().map(str::to_string);
        let root = self.root().to_path_buf();
        self.tabs.retain(|t| root.join(&t.path).is_file());
        if self.tabs.len() == before {
            return Vec::new();
        }
        if let Some(idx) = active_path.and_then(|p| self.tabs.iter().position(|t| t.path == p)) {
            self.active = idx;
            return Vec::new();
        }
        self.clear_active();
        self.active = 0;
        let Some(tab) = self.tabs.first_mut() else {
            return Vec::new();
        };
        // 別の根で溜めた状態は捨て、新しい根のファイルとして開き直す。
        tab.stashed = None;
        let path = tab.path.clone();
        self.request(path, None, None, false)
    }

    /// preview は必ずアクティブなので、呼ぶのはフォーカスが他へ移る直前だけ。閉じたタブ
    /// より後ろの添字は 1 つ前へ動くので、行き先をここで詰めないと隣のファイルが開く。
    fn drop_preview(&mut self, dest: usize) -> usize {
        let Some(idx) = self.tabs.iter().position(|t| t.status.is_preview()) else {
            return dest;
        };
        self.tabs.remove(idx);
        self.clear_active();
        if dest > idx { dest - 1 } else { dest }
    }

    fn stash(&mut self) {
        let view = self.take_active();
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.stashed = Some(view);
        }
    }

    fn restore(&mut self, idx: usize) {
        let Some(view) = self.tabs.get_mut(idx).and_then(|t| t.stashed.take()) else {
            return;
        };
        self.content = view.content;
        self.diff = view.diff;
        self.search = view.search;
        self.fold = view.fold;
        self.selection = view.selection;
        self.scroll = view.scroll;
    }

    fn take_active(&mut self) -> Stashed {
        Stashed {
            content: std::mem::take(&mut self.content),
            diff: std::mem::take(&mut self.diff),
            search: std::mem::take(&mut self.search),
            fold: std::mem::take(&mut self.fold),
            selection: std::mem::take(&mut self.selection),
            scroll: std::mem::take(&mut self.scroll),
        }
    }

    /// アクティブなタブの状態を捨て、ファイル未選択の表示へ戻す。
    fn clear_active(&mut self) {
        let side_by_side = self.diff.side_by_side;
        self.take_active();
        self.diff.side_by_side = side_by_side;
    }
}

/// タブ帯の 1 行ぶんの割り付け。描画と当たり判定が同じ 1 回の計算を読む。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Strip {
    /// (タブの添字, タブ本体の列範囲, 閉じるチップの列範囲)。
    pub cells: Vec<(usize, std::ops::Range<u16>, std::ops::Range<u16>)>,
    /// 左右に隠れているタブがあるか。
    pub left: bool,
    pub right: bool,
}

pub const OVERFLOW_LEFT: char = '\u{2039}';
pub const OVERFLOW_RIGHT: char = '\u{203a}';
pub const CLOSE: &str = " [x]";

/// タブ 1 枚の表示文字列。長ければ先頭を省いてファイル名を残す。
pub fn label(path: &str) -> String {
    let budget = 28;
    let len = path.chars().count();
    if len <= budget {
        return format!(" {path} ");
    }
    let kept: String = path.chars().skip(len - budget + 1).collect();
    format!(" \u{2026}{kept} ")
}

/// scroll 枚目から描けるだけ並べる。
pub fn strip(tabs: &[Tab], scroll: usize, width: u16) -> Strip {
    let scroll = scroll.min(tabs.len().saturating_sub(1));
    let left = scroll > 0;
    let mut x = u16::from(left);
    let mut cells = Vec::new();
    let close_w = CLOSE.chars().count() as u16;
    for (i, tab) in tabs.iter().enumerate().skip(scroll) {
        let label_w = label(&tab.path).chars().count() as u16;
        let w = label_w + close_w;
        // 最後の 1 桁は次の印のために空ける。1 枚も入らないときだけ削ってでも出す。
        if x + w > width.saturating_sub(1) && !cells.is_empty() {
            return Strip {
                cells,
                left,
                right: true,
            };
        }
        let tab_end = (x + label_w).min(width);
        let close_end = (x + w).min(width);
        cells.push((i, x..tab_end, tab_end..close_end));
        x += w;
    }
    Strip {
        cells,
        left,
        right: false,
    }
}

/// タブ帯のどこを押したか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripHit {
    Tab(usize),
    Close(usize),
    ScrollLeft,
    ScrollRight,
}

impl Strip {
    pub fn hit(&self, x: u16, width: u16) -> Option<StripHit> {
        if self.left && x == 0 {
            return Some(StripHit::ScrollLeft);
        }
        if self.right && x + 1 >= width {
            return Some(StripHit::ScrollRight);
        }
        for (i, tab, close) in &self.cells {
            if close.contains(&x) {
                return Some(StripHit::Close(*i));
            }
            if tab.contains(&x) {
                return Some(StripHit::Tab(*i));
            }
        }
        None
    }
}

impl ViewerPanel {
    /// アクティブなタブが窓から外れていたら窓を寄せ直す。溢れの印で送っただけの
    /// スクロールは巻き戻さない — 見たくて送ったものが毎フレーム戻る。
    pub(super) fn reveal_tab(&mut self, width: u16) {
        if self.active < self.tab_scroll {
            self.tab_scroll = self.active;
            return;
        }
        while !strip(&self.tabs, self.tab_scroll, width)
            .cells
            .iter()
            .any(|(i, _, _)| *i == self.active)
            && self.tab_scroll < self.active
        {
            self.tab_scroll += 1;
        }
    }

    pub fn tab_scroll(&self) -> usize {
        self.tab_scroll
    }

    /// タブ帯のうちタブが使える幅。Raw/Rendered のトグルを出す間はその桁を譲る。
    pub(super) fn tab_strip_width(&self, width: u16) -> u16 {
        match super::render::toggle(width, self.markdown_toggle_available()) {
            Some(chip) => chip.raw.start,
            None => width,
        }
    }

    pub(super) fn click_tab_row(&mut self, x: u16, width: u16) -> Vec<Effect> {
        if let Some(chip) = super::render::toggle(width, self.markdown_toggle_available())
            && (chip.raw.contains(&x) || chip.rendered.contains(&x))
            && chip.rendered.contains(&x) != self.is_showing_rendered_markdown()
        {
            return self.toggle_markdown();
        }
        let width = self.tab_strip_width(width);
        let strip = strip(&self.tabs, self.tab_scroll, width);
        match strip.hit(x, width) {
            Some(StripHit::Tab(idx)) => self.focus_tab(idx),
            Some(StripHit::Close(idx)) => self.close_tab(idx),
            Some(StripHit::ScrollLeft) => {
                self.tab_scroll = self.tab_scroll.saturating_sub(1);
                Vec::new()
            }
            Some(StripHit::ScrollRight) => {
                self.tab_scroll = (self.tab_scroll + 1).min(self.tabs.len().saturating_sub(1));
                Vec::new()
            }
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::config::Config;

    fn panel(paths: &[&str]) -> ViewerPanel {
        let mut panel = ViewerPanel::new(&Config::default());
        for path in paths {
            panel.activate_tab_for(path, TabStatus::Persistent);
        }
        panel
    }

    /// ラベルはどれも " name " の 6 桁、閉じるチップは " [x]" の 4 桁で計 10 桁。
    /// 窓 34 桁は右端 1 桁を印に取るので 3 枚まで。
    const W: u16 = 34;

    #[test]
    fn 溢れると印が出て押した向きへ1枚ずつ送る() {
        let mut panel = panel(&["a.rs", "b.rs", "c.rs", "d.rs"]);
        panel.tab_scroll = 0;
        let strip = strip(panel.tabs(), 0, W);
        assert_eq!(strip.cells.len(), 3, "{strip:?}");
        assert!(strip.right && !strip.left);

        panel.click_tab_row(W - 1, W);
        assert_eq!(panel.tab_scroll(), 1);
        let sent = super::strip(panel.tabs(), panel.tab_scroll(), W);
        assert!(sent.left, "送ったぶん左にも印が出る");
        assert_eq!(sent.hit(0, W), Some(StripHit::ScrollLeft));

        panel.click_tab_row(0, W);
        assert_eq!(panel.tab_scroll(), 0);
    }

    #[test]
    fn 見えているタブは押した位置のものが選ばれる() {
        let mut panel = panel(&["a.rs", "b.rs", "c.rs"]);
        let strip = strip(panel.tabs(), 0, W);
        let (_, range, _) = strip.cells[1].clone();
        assert_eq!(strip.hit(range.start, W), Some(StripHit::Tab(1)));
        panel.click_tab_row(range.start, W);
        assert_eq!(panel.active_tab(), 1);
    }

    #[test]
    fn 閉じるチップを押すとそのタブが閉じる() {
        let mut panel = panel(&["a.rs", "b.rs", "c.rs"]);
        panel.focus_tab(0);
        let strip = strip(panel.tabs(), 0, W);
        let (_, _, close) = strip.cells[1].clone();
        panel.click_tab_row(close.start, W);
        assert!(
            !panel.tabs().iter().any(|t| t.path == "b.rs"),
            "{:?}",
            panel.tabs()
        );
        assert_eq!(panel.active_tab(), 0, "閉じたのはアクティブでないタブ");
    }

    #[test]
    fn 閉じるチップとタブ本体の当たり判定が重ならない() {
        let panel = panel(&["a.rs", "b.rs"]);
        let strip = strip(panel.tabs(), 0, W);
        for (i, tab, close) in &strip.cells {
            assert_eq!(tab.end, close.start, "タブ {i} の本体と閉じるチップの間");
            assert_eq!(strip.hit(tab.start, W), Some(StripHit::Tab(*i)));
            assert_eq!(strip.hit(close.start, W), Some(StripHit::Close(*i)));
        }
    }

    #[test]
    fn 窓はアクティブが外に出たときだけ寄せ直す() {
        let mut panel = panel(&["a.rs", "b.rs", "c.rs", "d.rs"]);
        assert_eq!(panel.active_tab(), 3);
        panel.reveal_tab(W);
        assert_eq!(panel.tab_scroll(), 1, "末尾が見える位置まで送る");

        panel.click_tab_row(W - 1, W);
        let sent = panel.tab_scroll();
        panel.reveal_tab(W);
        assert_eq!(panel.tab_scroll(), sent, "送った窓は戻らない");

        panel.focus_tab(0);
        panel.reveal_tab(W);
        assert_eq!(panel.tab_scroll(), 0);
    }

    #[test]
    fn 長いパスは先頭を省いてファイル名を残す() {
        let text = label("src/very/deep/directory/tree/name.rs");
        assert!(
            text.contains("name.rs") && text.contains('\u{2026}'),
            "{text}"
        );
        assert_eq!(label("a.rs"), " a.rs ");
    }
}
