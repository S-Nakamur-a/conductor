//! バックグラウンド PTY reader スレッド: 生バイトを vt100 パーサへ供給し、
//! Claude Code 出力解析に使う行バッファを維持し、Cursor Position Report クエリに
//! 応答し、リフロー時の再生に使う生バイト履歴を記録する。

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::{MAX_RAW_HISTORY_BYTES, PtyManager};

impl PtyManager {
    /// バックグラウンド reader スレッドの本体。
    ///
    /// PTY の reader から継続的に読み取り、正しい端末描画のため生バイトを
    /// vt100 パーサへ供給しつつ、Claude Code 出力解析に使う行バッファのために
    /// 行単位にも分割する。
    ///
    /// writer ハンドルは、カーソル位置レポート(CSI 6 n)のような端末クエリへ
    /// 応答するのに使う。多くのプログラム(fzf、シェルなど)が UI の描画位置を
    /// 決めるためにこれを送ってくる。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn reader_thread(
        mut reader: Box<dyn Read + Send>,
        buffer: Arc<Mutex<Vec<String>>>,
        buffer_limit: Arc<Mutex<usize>>,
        screen: Arc<Mutex<vt100::Parser>>,
        raw_history: Option<Arc<Mutex<VecDeque<u8>>>>,
        last_output_time: Arc<Mutex<Instant>>,
        alt_screen_entered: Arc<AtomicBool>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        output_notify: Arc<AtomicBool>,
    ) {
        let mut read_buf = [0u8; 4096];
        // 部分行の蓄積用('\n' で終わらないデータのため)。
        let mut partial = String::new();
        // 直前のオルタネート画面状態を保持し、遷移を検出する。
        let mut prev_alt_screen = false;

        loop {
            match reader.read(&mut read_buf) {
                Ok(0) => {
                    // EOF — PTY マスターが閉じられた。
                    // 残っている部分行をフラッシュする。
                    if !partial.is_empty() {
                        let line = std::mem::take(&mut partial);
                        Self::push_line(&buffer, &buffer_limit, line);
                    }
                    break;
                }
                Ok(n) => {
                    let bytes = &read_buf[..n];

                    // 最終出力時刻を更新し、メインループへ通知する。
                    {
                        let mut t = last_output_time.lock().unwrap_or_else(|e| e.into_inner());
                        *t = Instant::now();
                    }
                    output_notify.store(true, Ordering::Relaxed);

                    // パーサへ供給する前に、応答が必要な端末クエリの数を
                    // 数えておく(パーサはバイトを消費してしまう)。
                    let cpr_count = count_csi_dsr(bytes);

                    // 正しい描画のため生バイトを vt100 へ供給する。
                    {
                        let mut parser = screen.lock().unwrap_or_else(|e| e.into_inner());
                        parser.process(bytes);

                        // リフロー時の再生用に同じバイトを記録するが、生履歴を
                        // 有効にしたセッション(シェル)に限る。screen ロック
                        // 下で行うことで、記録されたストリームがパーサの処理
                        // 内容と正確に同期し続け、並行して走る
                        // resize_session の再構築からも一貫した履歴として
                        // 見える。内側のスコープは、以降の CPR / オルタネート
                        // 画面処理の前に履歴のガードを解放する。
                        if let Some(raw_history) = &raw_history {
                            let mut history = raw_history.lock().unwrap_or_else(|e| e.into_inner());
                            history.extend(bytes.iter().copied());
                            Self::trim_raw_history(&mut history, MAX_RAW_HISTORY_BYTES);
                        }

                        // Cursor Position Report リクエスト(CSI 6 n)に応答する。
                        // fzf、zsh、bash などのプログラムは、インライン描画のため
                        // 現在のカーソル位置を知ろうとしてこれを送ってくる。
                        // 応答しないと、タイムアウトするかユーザーが何か入力
                        // するまでブロックされる。
                        if cpr_count > 0 {
                            let cursor = parser.screen().cursor_position();
                            // 端末座標は 1-based。
                            let response = format!("\x1b[{};{}R", cursor.0 + 1, cursor.1 + 1,);
                            log::debug!(
                                "CPR: responding to {} query(ies) with cursor ({}, {})",
                                cpr_count,
                                cursor.0 + 1,
                                cursor.1 + 1,
                            );
                            if let Ok(mut w) = writer.lock() {
                                for _ in 0..cpr_count {
                                    let _ = w.write_all(response.as_bytes());
                                }
                                let _ = w.flush();
                            }
                        }

                        // オルタネート画面モードへの遷移を検出する。
                        let is_alt = parser.screen().alternate_screen();
                        if is_alt && !prev_alt_screen {
                            log::debug!(
                                "ALT_SCREEN reader: entered alternate screen, chunk_size={n}"
                            );
                            alt_screen_entered.store(true, Ordering::Relaxed);
                        }
                        prev_alt_screen = is_alt;
                    }

                    // CC 解析用の行バッファも維持する。
                    let chunk = String::from_utf8_lossy(bytes);
                    partial.push_str(&chunk);

                    // 改行で分割し、完成した行を push する。
                    while let Some(pos) = partial.find('\n') {
                        let line: String = partial.drain(..=pos).collect();
                        // 末尾の '\n' (と任意の '\r') を取り除く。
                        let line = line
                            .trim_end_matches('\n')
                            .trim_end_matches('\r')
                            .to_string();
                        Self::push_line(&buffer, &buffer_limit, line);
                    }
                }
                Err(_) => {
                    // 読み取りエラー — PTY はおそらく閉じられている。スレッドを終了する。
                    break;
                }
            }
        }
    }

    /// 生バイト履歴を先頭から削って高々 cap バイトまで切り詰める。上限に
    /// 達した後は、可能なら次の改行まで追加で削り、残った履歴が行の境界から
    /// きれいに始まるようにする — こうすることで、リフロー再構築後に不完全な
    /// 行が誤って描画されるのを防ぐ。
    ///
    /// 改行の探索範囲には上限を設けている: エスケープシーケンスだらけの
    /// TUI ストリームは改行がごく少ないことがあり、無制限の探索は追記の
    /// たびに O(n) スキャンのコストがかかるか、(削り続けた場合)バッファ
    /// 全体を空にしてしまう。近くに改行が見つからない場合はバイトをそのまま
    /// 残す — 多少不完全な先頭行の方が、真っ白な画面よりはるかにましである。
    pub(super) fn trim_raw_history(history: &mut VecDeque<u8>, cap: usize) {
        if history.len() <= cap {
            return;
        }
        let excess = history.len() - cap;
        for _ in 0..excess {
            history.pop_front();
        }
        // 探索範囲内に次の改行があれば、そのすぐ後ろに揃える。
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

    /// 現在の上限を守りながら、共有バッファへ1行を push する。
    fn push_line(buffer: &Arc<Mutex<Vec<String>>>, buffer_limit: &Arc<Mutex<usize>>, line: String) {
        let limit = {
            let l = buffer_limit.lock().unwrap_or_else(|e| e.into_inner());
            *l
        };

        let mut buf = buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.push(line);

        // 上限を超えていたら先頭から削る。
        if buf.len() > limit {
            let excess = buf.len() - limit;
            buf.drain(..excess);
        }
    }
}

/// プログラムは端末に「カーソルはどこか」を尋ねるためにこれを送り、CSI row ; col R を
/// 期待する。
fn count_csi_dsr(bytes: &[u8]) -> usize {
    if bytes.len() < 4 {
        return 0;
    }
    bytes.windows(4).filter(|w| *w == b"\x1b[6n").count()
}
