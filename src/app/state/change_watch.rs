//! 選択中 worktree に変更が入ったかの検知。

use std::collections::HashMap;

/// 前回のポーリングで見た姿。これと今を比べて差分の読み直しを決める。
#[derive(Default)]
pub struct ChangeWatch {
    /// 選択中 worktree の HEAD。
    pub head_oid: Option<String>,
    /// (追加, 変更, 削除, 未追跡) の件数。
    pub status: Option<(usize, usize, usize, usize)>,
    /// ブランチ名 -> HEAD oid。こちらは全 worktree ぶんで、コミットの検知に使う。
    pub heads: HashMap<String, String>,
}

impl ChangeWatch {
    /// 前回から動いたか。動いていれば今の姿を覚える。
    pub fn record(&mut self, head: Option<String>, status: (usize, usize, usize, usize)) -> bool {
        let moved = self.head_oid.as_ref() != head.as_ref() || self.status != Some(status);
        self.head_oid = head;
        self.status = Some(status);
        moved
    }
}
