//! コードナビゲーション (戻る・進む) のためのジャンプ履歴スタック。
//!
//! ファイル上の位置を記録し、IDE の「戻る」「進む」と同じように定義位置の
//! あいだを行き来できるようにする。

/// コードベース上の保存された位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// worktree ルートからの相対ファイルパス。
    pub file_path: String,
    /// 0 始まりの行番号 (スクロール位置)。
    pub line: usize,
    /// 水平スクロールのオフセット。
    pub h_scroll: usize,
}

/// 履歴スタックに保持する最大件数。
const MAX_HISTORY: usize = 200;

/// 戻る・進むのナビゲーション履歴。
pub struct JumpHistory {
    /// 過去の位置のスタック (末尾が最新)。
    back: Vec<Location>,
    /// 進む先の位置のスタック (戻ったときに積まれる)。
    forward: Vec<Location>,
}

impl JumpHistory {
    /// 空の履歴を作る。
    pub fn new() -> Self {
        Self {
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    /// 位置を「戻る」スタックへ積む。
    /// 「進む」スタックはクリアする (新しい分岐に入ったため)。
    pub fn push(&mut self, location: Location) {
        self.forward.clear();
        self.back.push(location);
        if self.back.len() > MAX_HISTORY {
            self.back.remove(0);
        }
    }

    /// 直前の位置へ戻る。
    /// current を「進む」スタックへ積み、直前の位置を返す。
    pub fn go_back(&mut self, current: Location) -> Option<Location> {
        let prev = self.back.pop()?;
        self.forward.push(current);
        Some(prev)
    }

    /// 次の位置へ進む。
    /// current を「戻る」スタックへ積み、次の位置を返す。
    pub fn go_forward(&mut self, current: Location) -> Option<Location> {
        let next = self.forward.pop()?;
        self.back.push(current);
        Some(next)
    }

    /// UI 描画用のパンくずリストを組み立てる。
    ///
    /// (entries, current_index) を返す。entries は「戻る」スタック +
    /// current + 「進む」スタックを順に並べたもので、current_index が
    /// current を指す。
    ///
    /// 返すのは現在位置の周辺 max_visible 件までで、先頭側を切り詰めた場合は
    /// 番兵として None を先頭に挿入する。
    pub fn breadcrumb_trail(
        &self,
        current: &Location,
        max_visible: usize,
    ) -> (Vec<Option<Location>>, usize) {
        // 全体の並び: back (古い→新しい) + current + forward (古い→新しいになるよう反転)。
        let total = self.back.len() + 1 + self.forward.len();
        let cur_idx = self.back.len(); // current の位置 (0 始まり)

        if total <= max_visible {
            let mut entries: Vec<Option<Location>> = self.back.iter().cloned().map(Some).collect();
            entries.push(Some(current.clone()));
            for loc in self.forward.iter().rev() {
                entries.push(Some(loc.clone()));
            }
            return (entries, cur_idx);
        }

        // 表示窓: 現在位置を中心に取りつつ、履歴 (back) 側を多めに見せる。
        let half = max_visible / 2;
        let start = if cur_idx <= half {
            0
        } else if cur_idx + half >= total {
            total.saturating_sub(max_visible)
        } else {
            cur_idx - half
        };
        let end = (start + max_visible).min(total);

        let mut all: Vec<Location> = self.back.to_vec();
        all.push(current.clone());
        for loc in self.forward.iter().rev() {
            all.push(loc.clone());
        }

        let mut entries: Vec<Option<Location>> =
            all[start..end].iter().cloned().map(Some).collect();
        let mut adjusted_idx = cur_idx - start;

        if start > 0 {
            entries.insert(0, None); // 「…」を表す番兵
            adjusted_idx += 1;
        }

        (entries, adjusted_idx)
    }
}

impl Default for JumpHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(file: &str, line: usize) -> Location {
        Location {
            file_path: file.to_string(),
            line,
            h_scroll: 0,
        }
    }

    #[test]
    fn going_back_walks_the_stack_then_stops_at_the_oldest_jump() {
        let mut h = JumpHistory::new();
        h.push(loc("a.rs", 10));
        h.push(loc("b.rs", 20));

        let prev = h.go_back(loc("c.rs", 30));
        assert_eq!(prev, Some(loc("b.rs", 20)));

        let prev = h.go_back(loc("b.rs", 20));
        assert_eq!(prev, Some(loc("a.rs", 10)));

        // これ以上の履歴は無い。
        assert!(h.go_back(loc("a.rs", 10)).is_none());
    }

    #[test]
    fn going_forward_returns_to_where_going_back_came_from() {
        let mut h = JumpHistory::new();
        h.push(loc("a.rs", 10));
        h.push(loc("b.rs", 20));

        // 戻る。
        let prev = h.go_back(loc("c.rs", 30)).unwrap();
        assert_eq!(prev, loc("b.rs", 20));

        // 進む。
        let next = h.go_forward(loc("b.rs", 20)).unwrap();
        assert_eq!(next, loc("c.rs", 30));
    }

    #[test]
    fn a_new_jump_discards_the_forward_stack() {
        // go_back した直後は forward に積まれていて、進める。
        let mut h = JumpHistory::new();
        h.push(loc("a.rs", 10));
        h.push(loc("b.rs", 20));
        h.go_back(loc("c.rs", 30));
        assert!(h.go_forward(loc("b.rs", 20)).is_some());

        // 新しく push したら forward はクリアされ、進めなくなるはず。
        let mut h = JumpHistory::new();
        h.push(loc("a.rs", 10));
        h.push(loc("b.rs", 20));
        h.go_back(loc("c.rs", 30));
        h.push(loc("d.rs", 40));
        assert!(h.go_forward(loc("d.rs", 40)).is_none());
    }

    #[test]
    fn the_back_stack_stops_growing_at_two_hundred_entries() {
        let mut h = JumpHistory::new();
        for i in 0..250 {
            h.push(loc("file.rs", i));
        }
        // MAX_HISTORY で頭打ちになるはず。
        assert_eq!(h.back.len(), 200);
    }
}
