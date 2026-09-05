//! バックグラウンドの reader スレッド。生バイトを vt100 へ供給し、行バッファを保ち、
//! カーソル位置クエリに答え、リフロー用の生履歴を残す。

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{MAX_RAW_HISTORY_BYTES, SharedIo, lock};

/// PTY が閉じる (EOF か読み取りエラー) まで回る。
///
/// writer を持つのは、カーソル位置レポート (CSI 6 n) にこのスレッドから即答するため。
/// fzf やシェルは描画位置を決めるためにこれを送り、返事が無いとタイムアウトするか
/// ユーザが何か打つまで止まる。
pub(super) fn run(
    mut reader: Box<dyn Read + Send>,
    io: SharedIo,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
) {
    let mut read_buf = [0u8; 4096];
    let mut partial = String::new();
    let mut prev_alt_screen = false;

    loop {
        let n = match reader.read(&mut read_buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let bytes = &read_buf[..n];

        *lock(&io.last_output) = Instant::now();
        io.output_notify.store(true, Ordering::Relaxed);

        // パーサはバイトを消費してしまうので、供給する前に数える。
        let cpr_count = count_csi_dsr(bytes);

        {
            let mut parser = lock(&io.screen);
            parser.process(bytes);

            // 記録はパーサへの供給と同じロックの下で行う。並行するリサイズの再構築から
            // 見ても、履歴とパーサの内容が食い違わないようにするため。
            if let Some(raw_history) = &io.raw_history {
                let mut history = lock(raw_history);
                history.extend(bytes.iter().copied());
                trim_raw_history(&mut history, MAX_RAW_HISTORY_BYTES);
            }

            if cpr_count > 0 {
                let cursor = parser.screen().cursor_position();
                // 端末座標は 1 始まり。
                let response = format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1);
                if let Ok(mut w) = writer.lock() {
                    for _ in 0..cpr_count {
                        let _ = w.write_all(response.as_bytes());
                    }
                    let _ = w.flush();
                }
            }

            let is_alt = parser.screen().alternate_screen();
            if is_alt && !prev_alt_screen {
                io.alt_screen_entered.store(true, Ordering::Relaxed);
            }
            prev_alt_screen = is_alt;
        }

        partial.push_str(&String::from_utf8_lossy(bytes));
        while let Some(pos) = partial.find('\n') {
            let line: String = partial.drain(..=pos).collect();
            push_line(
                &io,
                line.trim_end_matches('\n')
                    .trim_end_matches('\r')
                    .to_string(),
            );
        }
    }

    if !partial.is_empty() {
        push_line(&io, partial);
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

/// プログラムは「カーソルはどこか」を尋ねるためにこれを送り、CSI row ; col R を待つ。
fn count_csi_dsr(bytes: &[u8]) -> usize {
    if bytes.len() < 4 {
        return 0;
    }
    bytes.windows(4).filter(|w| *w == b"\x1b[6n").count()
}
