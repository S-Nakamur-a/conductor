//! ホストから届いた出力を写しへ流す: vt100 へ供給し、行バッファを保ち、
//! リフロー用の生履歴を残す。

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::Instant;

use super::{MAX_RAW_HISTORY_BYTES, SharedIo, lock};

pub(super) fn feed(io: &SharedIo, bytes: &[u8]) {
    *lock(&io.last_output) = Instant::now();
    io.output_notify.store(true, Ordering::Relaxed);

    {
        let mut parser = lock(&io.screen);
        parser.process(bytes);
        note_shown_cursor(parser.screen(), &mut lock(&io.shown_cursor));

        // 記録はパーサへの供給と同じロックの下で行う。並行するリサイズの再構築から
        // 見ても、履歴とパーサの内容が食い違わないようにするため。
        if let Some(raw_history) = &io.raw_history {
            let mut history = lock(raw_history);
            history.extend(bytes.iter().copied());
            trim_raw_history(&mut history, MAX_RAW_HISTORY_BYTES);
        }
    }

    let mut partial = lock(&io.partial);
    partial.push_str(&String::from_utf8_lossy(bytes));
    while let Some(pos) = partial.find('\n') {
        let line: String = partial.drain(..=pos).collect();
        push_line(
            io,
            line.trim_end_matches('\n')
                .trim_end_matches('\r')
                .to_string(),
        );
    }
}

/// 子が終わった。改行の来なかった最後の行も行バッファへ入れる。
pub(super) fn finish(io: &SharedIo) {
    let rest = std::mem::take(&mut *lock(&io.partial));
    if !rest.is_empty() {
        push_line(io, rest);
    }
    io.exited.store(true, Ordering::Relaxed);
    io.output_notify.store(true, Ordering::Relaxed);
}

pub(super) fn note_shown_cursor(screen: &vt100::Screen, shown: &mut (u16, u16)) {
    if !screen.hide_cursor() {
        *shown = screen.cursor_position();
    }
}

/// 生バイト履歴を高々 cap まで削り、可能なら行境界に揃える。
///
/// 改行の探索範囲に上限があるのは、エスケープだらけの TUI 出力には改行がほとんど無く、
/// 無制限に探すと追記のたび O(n) を払うか、削り続けてバッファを空にしてしまうため。
/// 近くに改行が無ければそのまま残す — 先頭行が欠けるのは、真っ白な画面よりましである。
pub(super) fn trim_raw_history(history: &mut VecDeque<u8>, cap: usize) {
    if history.len() <= cap {
        return;
    }
    for _ in 0..history.len() - cap {
        history.pop_front();
    }
    const ALIGN_SCAN_LIMIT: usize = 8 * 1024;
    if let Some(pos) = history
        .iter()
        .take(ALIGN_SCAN_LIMIT)
        .position(|&b| b == b'\n')
    {
        for _ in 0..=pos {
            history.pop_front();
        }
    }
}

fn push_line(io: &SharedIo, line: String) {
    let limit = *lock(&io.line_limit);
    let mut lines = lock(&io.lines);
    lines.push(line);
    if lines.len() > limit {
        let excess = lines.len() - limit;
        lines.drain(..excess);
    }
}
