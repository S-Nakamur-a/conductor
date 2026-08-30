//! Explorer と App のつなぎ目。
//!
//! パネルは [Ctx] / [Paint] / [Panes] しか受け取らず App 型を見ない。ここが
//! それらを組み立て、返ってきた [Intent] を適用する唯一の場所になる。

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use super::ctx::{Ctx, Paint};
use super::keys::Panes;
use crate::app::App;
use crate::types::Focus;

impl App {
    fn explorer_ctx(&self) -> Ctx<'_> {
        Ctx {
            theme: &self.theme,
            config: &self.config,
            keymap: &self.keymap,
            focused: self.focus == Focus::Explorer,
            diff: &self.diff_state,
            review: &self.review_state,
            revidere: self.revidere_artifact_state(),
        }
    }

    fn explorer_paint(&self) -> Paint<'_> {
        Paint {
            hover_tree: &self.list_hover.explorer_tree,
            hover_changes: &self.list_hover.diff_list,
            revidere_badge_hover: self.revidere.badge_hover,
            tick: self.ui_tick,
            search: self
                .viewer
                .search
                .search_active
                .then(|| self.viewer.search.search_query.text()),
            expanded: self.expanded_panel.is_some(),
            border: self.animated_border_color(Focus::Explorer),
        }
    }

    /// Explorer カラムの矩形。描画時に計算されたレイアウトから引く。
    pub fn explorer_area(&self) -> Rect {
        self.layout.cache.columns[1]
    }

    pub fn explorer_panes(&self, area: Rect) -> Panes {
        Panes::split(
            area,
            self.config.layout.explorer_split_pct,
            self.explorer.bottom(),
            self.diff_state.error.is_some(),
        )
    }

    pub fn render_explorer(&mut self, frame: &mut Frame, area: Rect) {
        let geometry = super::render::render(
            frame,
            area,
            &self.explorer,
            &self.explorer_ctx(),
            &self.explorer_paint(),
        );
        if let Some((x, y)) = geometry.search_cursor
            && self.overlays.active == crate::overlay::ActiveOverlay::None
        {
            frame.set_cursor_position(ratatui::layout::Position::new(x, y));
        }
    }

    pub fn render_explorer_overlay(&mut self, frame: &mut Frame, area: Rect) {
        super::render::render_comments_overlay(
            frame,
            area,
            &self.explorer,
            &self.explorer_ctx(),
            &self.explorer_paint(),
        );
    }

    pub fn explorer_key(&mut self, key: KeyEvent) -> Option<KeyEvent> {
        let panes = self.explorer_panes(self.explorer_area());
        let in_modal = self.overlays.active == crate::overlay::ActiveOverlay::CommentList;
        let intent = {
            let ctx = Ctx {
                theme: &self.theme,
                config: &self.config,
                keymap: &self.keymap,
                focused: true,
                diff: &self.diff_state,
                review: &self.review_state,
                revidere: crate::revidere::ArtifactState::None,
            };
            super::keys::handle_key(&mut self.explorer, key, &ctx, &panes, in_modal)
        };
        if let Some(intent) = intent {
            self.apply_explorer_intent(intent);
        }
        None
    }

    pub fn explorer_click(&mut self, x: u16, y: u16) {
        let panes = self.explorer_panes(self.explorer_area());
        self.set_focus(Focus::Explorer);
        let intent = {
            let ctx = Ctx {
                theme: &self.theme,
                config: &self.config,
                keymap: &self.keymap,
                focused: true,
                diff: &self.diff_state,
                review: &self.review_state,
                revidere: crate::revidere::ArtifactState::None,
            };
            super::pointer::click(&mut self.explorer, x, y, &ctx, &panes)
        };
        if let Some(intent) = intent {
            self.apply_explorer_intent(intent);
        }
    }

    /// ポインタが乗っている行。上ペインと下ペインで別々に返す。
    /// 列が Explorer の外なら両方 None になり、それがホバーの消灯になる。
    pub fn explorer_hover(&self, x: u16, y: u16) -> (Option<usize>, Option<usize>) {
        let area = self.explorer_area();
        if x < area.x || x >= area.x + area.width {
            return (None, None);
        }
        let panes = self.explorer_panes(area);
        let tree_len = self.explorer.tree.visible_indices().len();
        (
            self.explorer.tree_cursor.index_at(y, tree_len, panes.tree),
            self.explorer.changes_cursor.index_at(
                y,
                self.diff_state.display_list.len(),
                panes.bottom,
            ),
        )
    }

    pub fn explorer_scroll(&mut self, lines: isize, y: u16) {
        let panes = self.explorer_panes(self.explorer_area());
        let ctx = Ctx {
            theme: &self.theme,
            config: &self.config,
            keymap: &self.keymap,
            focused: true,
            diff: &self.diff_state,
            review: &self.review_state,
            revidere: crate::revidere::ArtifactState::None,
        };
        super::pointer::scroll(&mut self.explorer, lines, y, &ctx, &panes);
    }
}
