//! チップ/タブ帯の可視範囲の計算。worktree の帯、Viewer のタブ行、
//! Claude Code / Shell のタブ帯が、同じ貪欲詰めとクランプを共有する。

/// バーに表示するチップのウィンドウ [start, end) を計算する。
///
/// slots[i] はチップ i の描画幅全体（チップ本体＋削除ボタン）、sep_w は
/// 最初に表示するチップ以外の各チップの前に描く区切り文字の幅。avail は
/// チップと区切り文字に使える残り幅（呼び出し側がオーバーフローヒントの
/// 分を既に確保済み）。desired_start は現在のスクロール位置。reveal が
/// 立っている場合、selected を含むために必要な最小限だけウィンドウを
/// パンする。
pub(crate) fn visible_window(
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

    // start から貪欲に前方へ埋めていく。少なくとも1つのチップは常に表示する。
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

    // 最後のチップまで到達できる最小の start — オーバースクロールをクランプ
    // することで、左側にチップが隠れたまま右側に空白が残る事態を防ぐ。
    // start を大きくするとウィンドウの end は同じか後ろにしか動かないので、
    // 昇順に走査して最初に end に到達した start がそのまま最小の start になる。
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

    #[test]
    fn 幅が広ければ全部収まる() {
        // 十分な余白がある → ウィンドウ全体を表示、パンなし。
        let (start, end) = visible_window(W, 1, 1000, 0, 0, false);
        assert_eq!((start, end), (0, 10));
    }

    #[test]
    fn 詰め込みは使える幅で止まる() {
        // avail 32: chip(10) + sep1+chip(10) + sep1+chip(10) = 32 → チップ3個。
        let (start, end) = visible_window(W, 1, 32, 0, 0, false);
        assert_eq!((start, end), (0, 3));
    }

    #[test]
    fn 狭すぎてもチップは1つは出る() {
        let (start, end) = visible_window(W, 1, 4, 0, 0, false);
        assert_eq!((start, end), (0, 1));
    }

    #[test]
    fn スクロール位置が窓を動かす() {
        let (start, end) = visible_window(W, 1, 32, 4, 4, false);
        assert_eq!((start, end), (4, 7));
    }

    #[test]
    fn 行き過ぎたスクロールは右端が埋まる位置でクランプする() {
        // desired start が 9 だと空間が無駄になるので、最後のチップが右端に
        // 来るようクランプする（末尾で終わる3個分のウィンドウになる）。
        let (start, end) = visible_window(W, 1, 32, 9, 0, false);
        assert_eq!((start, end), (7, 10));
    }

    #[test]
    fn 選択が窓より前ならrevealは左へ寄せる() {
        // ウィンドウは現在 [4,7) にある; チップ1を選択 → 見えるよう後方にパンする。
        let (start, end) = visible_window(W, 1, 32, 4, 1, true);
        assert_eq!(start, 1);
        assert!((start..end).contains(&1));
    }

    #[test]
    fn 選択が窓より後ろならrevealは右へ寄せる() {
        // ウィンドウは [0,3) にある; チップ8を選択 → 見えるようになるまで進める。
        let (start, end) = visible_window(W, 1, 32, 0, 8, true);
        assert!((start..end).contains(&8));
    }

    #[test]
    fn 選択が既に見えていればrevealは動かさない() {
        let (start, end) = visible_window(W, 1, 32, 3, 4, true);
        assert_eq!((start, end), (3, 6));
    }

    #[test]
    fn 空の一覧でも落ちない() {
        assert_eq!(visible_window(&[], 1, 100, 0, 0, true), (0, 0));
    }
}
