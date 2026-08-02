//! worktree の一覧と、そこへの選択。

use std::ops::Deref;

use crate::app::types::WorktreeListRow;
use crate::git_engine::WorktreeInfo;

/// 発見済みの worktree 一覧と、いま選択しているもの。
///
/// 一覧と添字を 1 つの型にまとめてあるのは、添字が一覧に対してしか意味を
/// 持たないから。以前は worktrees と selected_worktree が別々のフィールドで、
/// 一覧を差し替えるたびに添字のクランプを呼び出し側が思い出す必要があった。
/// [Self::replace] と [Self::select] を通す限り、添字は常に一覧の範囲内に
/// 収まる (一覧が空のときを除く)。
///
/// スライスへ [Deref] するので、iter() / get() / len() / 添字アクセスは
/// これまでどおり書ける。
#[derive(Default)]
pub struct WorktreeList {
    /// リポジトリで見つかった worktree。
    items: Vec<WorktreeInfo>,
    /// items への添字。items が空でない限り常に有効。
    selected: usize,
    /// worktree 行と、その下にぶら下がるセッション行を平坦化したもの。
    /// items から導出され、セッションが増減するたびに作り直される。
    pub rows: Vec<WorktreeListRow>,
    /// rows への添字。
    pub row_selected: usize,
}

impl Deref for WorktreeList {
    type Target = [WorktreeInfo];

    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

impl WorktreeList {
    /// 一覧をまるごと差し替える。選択は範囲内に収め直す。
    pub fn replace(&mut self, items: Vec<WorktreeInfo>) {
        self.items = items;
        self.clamp_selected();
    }

    /// 選択中の worktree。一覧が空なら None。
    pub fn selected(&self) -> Option<&WorktreeInfo> {
        self.items.get(self.selected)
    }

    /// 選択中の添字。
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// 選択を移す。範囲外は末尾に丸められる。
    pub fn select(&mut self, index: usize) {
        self.selected = index;
        self.clamp_selected();
    }

    /// 行一覧を差し替える。行の選択は範囲内に収め直す。
    pub fn set_rows(&mut self, rows: Vec<WorktreeListRow>) {
        self.rows = rows;
        if !self.rows.is_empty() && self.row_selected >= self.rows.len() {
            self.row_selected = self.rows.len() - 1;
        }
    }

    fn clamp_selected(&mut self) {
        if self.selected >= self.items.len() {
            self.selected = self.items.len().saturating_sub(1);
        }
    }
}
