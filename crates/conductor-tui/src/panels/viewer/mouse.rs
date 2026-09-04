//! Viewer のマウス。当たり判定は描画と同じ行の列 (render::origin_at) から引く。

use super::input::no_place_to_comment;
use super::{ViewerPanel, render};
use crate::effect::Effect;
use crate::review::anchor_for;
use crate::workspace::{Ctx, StatusLevel};

/// ガターの桁が持っている意味。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Comment,
    Number,
    Fold,
    /// テスト実行ボタン。
    Test,
    Text,
}

/// 行の選択。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Selection {
    range: Option<(usize, usize)>,
    /// shift 付きクリックの起点。0 は「まだ無い」。
    anchor: usize,
}

impl Selection {
    /// 1 始まり・両端含む・start <= end に正規化した範囲。
    pub fn range(&self) -> Option<(usize, usize)> {
        self.range
            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) })
    }

    pub fn contains(&self, line_1: usize) -> bool {
        self.range()
            .is_some_and(|(start, end)| line_1 >= start && line_1 <= end)
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_none()
    }

    pub fn clear(&mut self) {
        self.range = None;
    }

    /// 起点は shift 無しのクリックでしか動かない。連続する shift クリックは常に
    /// 同じ所から伸びる。
    pub fn click(&mut self, line_1: usize, extend: bool) {
        if extend && self.anchor != 0 {
            self.range = Some((self.anchor, line_1));
            return;
        }
        self.range = Some((line_1, line_1));
        self.anchor = line_1;
    }
}

impl ViewerPanel {
    /// 画面上の 1 点のクリック。shift 付きは起点から範囲を伸ばす。
    pub fn click(&mut self, x: u16, y: u16, extend: bool, ctx: &Ctx) -> Vec<Effect> {
        if y < self.body.y {
            return self.click_tab_row(x.saturating_sub(self.tab_row.x), self.tab_row.width);
        }
        if self.is_showing_rendered_markdown() {
            return Vec::new();
        }
        let comments = self
            .content
            .path
            .as_deref()
            .map_or_else(Vec::new, |path| ctx.review.for_file(path));
        let zone = self.gutter_zone(
            x.saturating_sub(self.body.x),
            self.content.lines.len(),
            comments.is_empty(),
        );
        let line = match render::origin_at(
            self,
            ctx.review,
            ctx.theme,
            ctx.config.ui.icon_set(),
            self.body.width,
            self.body.height as usize,
            (y - self.body.y) as usize,
        ) {
            render::Origin::Line(line) => line,
            render::Origin::Deleted if zone != Zone::Text => return no_place_to_comment(),
            _ => return Vec::new(),
        };
        if self.diff.active
            && let Some(idx) = self.diff.expandable_at(line)
        {
            self.diff.expand(idx, false, &self.content.lines);
            return Vec::new();
        }
        match zone {
            Zone::Comment => match anchor_for(&comments, line) {
                Some(anchor) => self.threads.flip(anchor),
                None => return self.click_comment(line),
            },
            Zone::Number => return self.click_comment(line),
            Zone::Fold => {
                self.fold.toggle(line);
                self.scroll.line = self.fold.visible_anchor(line) - 1;
            }
            Zone::Test => {
                return match self.content.tests.contains_key(&line) {
                    true => self.run_test_at(line),
                    false => self.click_comment(line),
                };
            }
            // 選択の帯は素の本文にしか描かないので、diff では見えない選択を残さない。
            Zone::Text => {
                if !self.diff.active {
                    self.selection.click(line, extend);
                }
            }
        }
        Vec::new()
    }

    /// ガターを押しての作成。狙いは押した行なので、残っている選択には引きずられない。
    fn click_comment(&mut self, line_1: usize) -> Vec<Effect> {
        self.selection.clear();
        self.start_comment(line_1)
    }

    /// その行のテストをシェルへ流す。
    fn run_test_at(&self, line_1: usize) -> Vec<Effect> {
        let Some(run) = self.content.tests.get(&line_1) else {
            return Vec::new();
        };
        vec![
            Effect::Status(StatusLevel::Info, format!("Running {}", run.label)),
            Effect::SendToShell(run.command.clone()),
        ]
    }

    /// ガターの桁割り。render の組み方と 1 対 1 で、印の下を押せば印の意味になる。
    fn gutter_zone(&self, column: u16, total: usize, no_comments: bool) -> Zone {
        let mark = if no_comments { 0 } else { render::MARK };
        let column = column as usize;
        if column < mark {
            return Zone::Comment;
        }
        if self.diff.active {
            let digits = render::digit_count(self.diff.max_line_no);
            // 畳みも実行ボタンも diff には出ないので、左マージンは符号と行番号だけ。
            return if column < mark + render::DIFF_SIGN + digits {
                Zone::Number
            } else {
                Zone::Text
            };
        }
        let digits = render::digit_count(total);
        if column < mark + digits {
            return Zone::Number;
        }
        if column == mark + digits {
            return Zone::Fold;
        }
        let badge = render::badge_width(self);
        if badge > 0 && (mark + digits + 1..mark + digits + 1 + badge).contains(&column) {
            return Zone::Test;
        }
        Zone::Text
    }
}
