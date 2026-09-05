//! チップ帯の可視範囲。worktree ストリップと端末のセッションタブが共有する。

/// 帯に出すチップの窓 `[start, end)`。
///
/// `slots[i]` はチップ i の描画幅、`sep_w` は 2 つ目以降の前に置く区切りの幅、
/// `avail` はチップと区切りに使える幅。`desired_start` は今のスクロール位置で、
/// `reveal` が立っていれば `selected` が入る最小限だけ窓を動かす。
pub fn visible_window(
    slots: &[u16],
    sep_w: u16,
    avail: u16,
    desired_start: usize,
    selected: usize,
    reveal: bool,
) -> (usize, usize) {
    let total = slots.len();
    if total == 0 {
        return (0, 0);
    }

    let fill = |start: usize| -> usize {
        let mut used = 0u16;
        let mut end = start;
        while end < total {
            let extra = slots[end] + if end > start { sep_w } else { 0 };
            if used + extra > avail && end > start {
                break;
            }
            used = used.saturating_add(extra);
            end += 1;
        }
        end.max(start + 1).min(total)
    };

    // 末尾まで届く最小の start。行き過ぎをここで止めないと、左を隠したまま
    // 右に空白が残る。start を進めると end は戻らないので、最初に届いた所が最小。
    let mut tail_start = 0;
    for s in 0..total {
        if fill(s) == total {
            tail_start = s;
            break;
        }
    }

    let mut start = desired_start.min(tail_start);
    if reveal {
        if selected < start {
            start = selected;
        } else {
            while start < selected && selected >= fill(start) {
                start += 1;
            }
        }
    }
    (start, fill(start))
}

#[cfg(test)]
mod tests {
    use super::visible_window;

    const W: &[u16] = &[10, 10, 10, 10, 10, 10, 10, 10, 10, 10];

    /// 使える幅、今のスクロール位置、選択、reveal、期待する窓。
    type Case = (u16, usize, usize, bool, (usize, usize));

    #[test]
    fn 窓は幅と選択で決まる() {
        let cases: [Case; 6] = [
            (1000, 0, 0, false, (0, 10)),
            // 32 = chip(10) + (sep1+chip10) * 2
            (32, 0, 0, false, (0, 3)),
            (4, 0, 0, false, (0, 1)),
            (32, 5, 0, false, (5, 8)),
            // 行き過ぎは末尾が見える最小の start まで戻る。
            (32, 9, 0, false, (7, 10)),
            (32, 0, 5, true, (3, 6)),
        ];
        for (avail, start, selected, reveal, expected) in cases {
            let got = visible_window(W, 1, avail, start, selected, reveal);
            assert_eq!(got, expected, "avail={avail} start={start} reveal={reveal}");
        }
    }

    #[test]
    fn 空の帯は空の窓を返す() {
        assert_eq!(visible_window(&[], 1, 80, 0, 0, true), (0, 0));
    }
}
