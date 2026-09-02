//! セッションへの入力: 生バイト、チャンク書き込み、サニタイズ済みペースト、
//! マウスホイールの転送。

use std::io::Write;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use super::locale::utf8_chunks;
use super::{PtyStore, lock};

/// カーネルの PTY 入力バッファ (通常 4096 バイト) に触れないための分割単位。
const CHUNK_SIZE: usize = 1024;
const CHUNK_DELAY: Duration = Duration::from_millis(5);

impl PtyStore {
    /// idx のセッションの PTY へ入力を送る。
    pub fn write_to_session(&self, idx: usize, data: &[u8]) -> Result<()> {
        let session = self
            .sessions
            .get(idx)
            .context("session index out of bounds")?;
        let mut writer = lock(&session.writer);
        writer.write_all(data).context("failed to write to PTY")?;
        writer.flush().context("failed to flush PTY writer")
    }

    /// 大きなテキストを、通常のタイプ入力として (bracketed paste は使わずに) 送る。
    /// 受け手のアプリに全文を表示させたいプロンプト注入で使う。
    pub fn write_chunked_to_session(&self, idx: usize, text: &str) -> Result<()> {
        let session = self
            .sessions
            .get(idx)
            .context("session index out of bounds")?;
        write_chunks(&mut lock(&session.writer), text)
    }

    /// クリップボードの内容をサニタイズしてから送る。
    ///
    /// bracketed paste のマーカーは、フォアグラウンドのアプリが DECSET 2004 を有効に
    /// している場合だけ付ける。本物の端末と同じゲートで、無条件に包むと要求していない
    /// アプリにリテラルの [200~ が流れ込む。
    pub fn write_paste_to_session(&self, idx: usize, text: &str) -> Result<()> {
        let cleaned = sanitize_pasted_text(text);
        let session = self
            .sessions
            .get(idx)
            .context("session index out of bounds")?;

        // writer を取る前に screen のロックを手放す。
        let bracketed = lock(&session.io.screen).screen().bracketed_paste();
        let mut writer = lock(&session.writer);

        if bracketed {
            writer
                .write_all(b"\x1b[200~")
                .context("failed to write paste-start to PTY")?;
        }
        write_chunks(&mut writer, &cleaned)?;
        if bracketed {
            writer
                .write_all(b"\x1b[201~")
                .context("failed to write paste-end to PTY")?;
            writer.flush().context("failed to flush PTY writer")?;
        }
        Ok(())
    }

    /// マウスホイールを、画面を持つセッションへ転送する。処理したら true で、
    /// 呼び出し側はローカルのスクロールバックを動かして**はいけない**。
    ///
    /// tmux / iTerm2 に合わせた 3 通り。子がマウスレポートを要求していれば
    /// エンコードして渡し、オルタネート画面 (自前のスクロールバックを持たない
    /// ページャ) なら 1 ノッチを lines 回の矢印に変換し、それ以外は false を返す。
    ///
    /// col / row は PTY グリッド内の 1 始まり座標で、マウスレポートの符号化にだけ使う。
    pub fn forward_scroll_to_session(
        &self,
        idx: usize,
        lines: usize,
        up: bool,
        col: u16,
        row: u16,
    ) -> bool {
        let Some(session) = self.sessions.get(idx) else {
            return false;
        };
        let (is_alt, app_cursor, mouse_mode, mouse_encoding) = {
            let parser = lock(&session.io.screen);
            let screen = parser.screen();
            (
                screen.alternate_screen(),
                screen.application_cursor(),
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            )
        };

        if mouse_mode != vt100::MouseProtocolMode::None {
            let seq = encode_mouse_wheel(up, col, row, mouse_encoding);
            if let Err(e) = self.write_to_session(idx, &seq) {
                log::warn!("failed to forward wheel event to PTY session: {e}");
            }
            return true;
        }
        if !is_alt {
            return false;
        }

        let arrow = scroll_arrow_sequence(up, app_cursor);
        let buf: Vec<u8> = arrow.repeat(lines);
        if let Err(e) = self.write_to_session(idx, &buf) {
            log::warn!("failed to inject scroll arrows to PTY session: {e}");
        }
        true
    }
}

/// チャンクは UTF-8 の文字境界で分ける。途中で切れたチャンクは不完全なマルチバイト
/// 列としてフラッシュされ、受け手が誤ってデコードする。
fn write_chunks(writer: &mut Box<dyn Write + Send>, text: &str) -> Result<()> {
    for chunk in utf8_chunks(text, CHUNK_SIZE) {
        writer
            .write_all(chunk.as_bytes())
            .context("failed to write chunk to PTY")?;
        writer.flush().context("failed to flush PTY writer")?;
        if chunk.len() == CHUNK_SIZE {
            thread::sleep(CHUNK_DELAY);
        }
    }
    Ok(())
}

/// クリップボードのテキストを PTY へ流す前に無害化する。
///
/// 色付き端末やウェブページからのコピーには ANSI エスケープが混じる。そのまま送ると
/// カーソルやモードが動き、最悪は \x1b[201~ が bracketed paste を途中で終わらせて、
/// 続きがタイプされたコマンドとして実行される。エスケープ列は丸ごと落とし、制御文字は
/// タブと改行だけ残す (CR は LF に正規化)。
pub fn sanitize_pasted_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape_sequence(&mut chars),
            '\t' | '\n' => out.push(c),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// 導入の ESC は読み取り済みの前提。CSI は最終バイト 0x40..=0x7E まで、OSC/DCS/SOS/PM/APC
/// は BEL か ESC \ まで、SS2/SS3 は続く 1 バイトだけ、それ以外は消費済み。
fn skip_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
        Some(']' | 'P' | 'X' | '^' | '_') => {
            while let Some(c) = chars.next() {
                if c == '\u{07}' {
                    break;
                }
                if c == '\u{1b}' {
                    if chars.peek() == Some(&'\\') {
                        chars.next();
                    }
                    break;
                }
            }
        }
        Some('N' | 'O') => {
            chars.next();
        }
        _ => {}
    }
}

/// ホイール 1 ノッチを、マウスレポートを有効にした子へ端末が送るバイト列にする。
///
/// SGR (1006) は CSI < b ; col ; row M。それ以外はレガシーな X10 形式 CSI M Cb Cx Cy で、
/// 各値を 32 ずらして 1 バイトに収める — 223 を超える座標は表現できないのでクランプする。
pub fn encode_mouse_wheel(
    up: bool,
    col: u16,
    row: u16,
    encoding: vt100::MouseProtocolEncoding,
) -> Vec<u8> {
    let button: u16 = if up { 64 } else { 65 };
    let col = col.max(1);
    let row = row.max(1);
    match encoding {
        vt100::MouseProtocolEncoding::Sgr => format!("\x1b[<{button};{col};{row}M").into_bytes(),
        _ => vec![
            0x1b,
            b'[',
            b'M',
            (32 + button).min(255) as u8,
            (32 + col).min(255) as u8,
            (32 + row).min(255) as u8,
        ],
    }
}

/// オルタネート画面のページャをスクロールさせる Up/Down キーのエスケープ列。
///
/// less などはアプリケーションカーソルキーモード (DECCKM) を有効にして SS3 形式に
/// バインドするので、尊重しないと矢印が効かない。
pub fn scroll_arrow_sequence(up: bool, app_cursor: bool) -> &'static [u8] {
    match (up, app_cursor) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    }
}
