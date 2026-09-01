//! Explorer が所有する状態。
//!
//! 木の走査そのもの ([super::tree]) は据え置いた。gitignore の尊重、子の遅延
//! 読み込み、git 状態の解決は本質的な複雑さで、書き直しても縮まない。ここで
//! 畳んだのは、その周りにあった選択・スクロール・高さ・フラグの方である。

use std::collections::HashSet;

use crate::widget::click::ClickTracker;
use crate::widget::list::ListCursor;

use super::FileTreeState;

/// 下ペインが並べているもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BottomView {
    /// 変更のあったファイル。
    Changes,
    /// レビューコメント。
    Comments,
}

/// Explorer の中でキーを受け取るペイン。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Tree,
    Bottom,
}

pub struct Explorer {
    /// ファイルツリーの中身と根。走査と遅延読み込みを持つ。
    pub tree: FileTreeState,
    /// 木の中での位置。数えるのは可視エントリなので、畳まれた部分木は含まない。
    pub tree_cursor: ListCursor,

    bottom: BottomView,
    /// 変更ファイル一覧とコメント一覧で別々に覚える。切り替えて戻ったとき、
    /// 見ていた場所に戻るため。
    pub changes_cursor: ListCursor,
    pub comments_cursor: ListCursor,

    focus: Pane,

    /// レビュアーが「viewed」を付けたファイルの相対パス。
    pub viewed: HashSet<String>,

    pub tree_clicks: ClickTracker,
    pub changes_clicks: ClickTracker,
    pub comment_clicks: ClickTracker,
}

impl Explorer {
    pub fn bottom(&self) -> BottomView {
        self.bottom
    }

    pub fn focus(&self) -> Pane {
        self.focus
    }

    /// 下ペインに指定のものを出し、フォーカスもそこへ移す。
    pub fn show(&mut self, view: BottomView) {
        self.bottom = view;
        self.focus = Pane::Bottom;
    }

    pub fn focus_pane(&mut self, pane: Pane) {
        self.focus = pane;
    }
}

impl Default for Explorer {
    fn default() -> Self {
        Self {
            tree: FileTreeState::default(),
            tree_cursor: ListCursor::default(),
            bottom: BottomView::Changes,
            changes_cursor: ListCursor::default(),
            comments_cursor: ListCursor::default(),
            focus: Pane::Tree,
            viewed: HashSet::new(),
            tree_clicks: ClickTracker::default(),
            changes_clicks: ClickTracker::default(),
            comment_clicks: ClickTracker::default(),
        }
    }
}
