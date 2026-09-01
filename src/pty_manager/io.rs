//! PTY セッションへの入力書き込み: 生バイト、大きなペイロードのチャンク分割、
//! サニタイズ済みクリップボードペースト、マウスホイールのスクロール転送。

use std::io::Write;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use super::PtyManager;
use super::locale::utf8_chunks;

impl PtyManager {
    /// 指定セッションインデックスの PTY へ入力データを送る。
    pub fn write_to_session(&mut self, idx: usize, data: &[u8]) -> Result<()> {
        let session = self
            .sessions
            .get_mut(idx)
            .context("Session index out of bounds")?;
        let mut writer = session.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.write_all(data).context("Failed to write to PTY")?;
        writer.flush().context("Failed to flush PTY writer")?;
        Ok(())
    }

    /// マウスホイールのスクロールを、画面を所有する PTY セッションへ転送する。処理したら
    /// true を返し、呼び出し側はローカルのスクロールバックオフセットを調整して**はいけない**。
    ///
    /// tmux / iTerm2 の挙動に合わせた 3 つのケースがある。
    ///
    /// 1. **子がマウスレポートを要求している** (vim、less --mouse、fzf): SGR 1006 または
    ///    レガシー X10 でエンコードして転送し、アプリ自身にスクロールさせる。通常画面・
    ///    オルタネート画面のどちらでも。
    /// 2. **オルタネート画面かつマウスレポート無し** (ページャ): 自前のスクロールバックを
    ///    持たないので、1 ノッチを lines 回の Up/Down 矢印に変換する (alternate-scroll)。
    /// 3. **通常画面かつマウスレポート無し**: false を返し、呼び出し側がパネルのローカル
    ///    スクロールバックをスクロールする。
    ///
    /// col / row は PTY グリッド内の 1-based 座標で、ケース 1 のエンコードにのみ使う。
    pub fn forward_scroll_to_session(
        &mut self,
        idx: usize,
        lines: usize,
        up: bool,
        col: u16,
        row: u16,
    ) -> bool {
        // 端末モードを読んでから、書き込み前に session/parser の借用を解放する。
        let (is_alt, app_cursor, mouse_mode, mouse_encoding) = {
            let Some(session) = self.sessions.get(idx) else {
                return false;
            };
            let parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut buf = Vec::with_capacity(arrow.len() * lines);
        for _ in 0..lines {
            buf.extend_from_slice(arrow);
        }
        if let Err(e) = self.write_to_session(idx, &buf) {
            log::warn!("failed to inject scroll arrows to PTY session: {e}");
        }
        true
    }

    /// 大きなテキストペイロードを、通常のタイプ入力として (bracketed paste は使わずに)
    /// チャンク書き込みで PTY へ送る。カーネルの PTY 入力バッファ上限 (通常 4096 バイト) に
    /// 触れないようにするため。プロンプト注入で、受け手のアプリケーションに全文表示させたい
    /// 場合に使う。
    pub fn write_chunked_to_session(&mut self, idx: usize, text: &str) -> Result<()> {
        const CHUNK_SIZE: usize = 1024;
        const CHUNK_DELAY: Duration = Duration::from_millis(5);

        let session = self
            .sessions
            .get_mut(idx)
            .context("Session index out of bounds")?;
        let mut writer = session.writer.lock().unwrap_or_else(|e| e.into_inner());

        // チャンクは UTF-8 の文字境界で分ける。文字の途中で終わるチャンクは不完全なマルチバイト
        // シーケンスとしてフラッシュされ、受け手が誤ってデコードする (全角文字が壊れる)。
        for chunk in utf8_chunks(text, CHUNK_SIZE) {
            writer
                .write_all(chunk.as_bytes())
                .context("Failed to write chunk to PTY")?;
            writer.flush().context("Failed to flush PTY writer")?;
            if chunk.len() == CHUNK_SIZE {
                thread::sleep(CHUNK_DELAY);
            }
        }

        Ok(())
    }

    /// クリップボードのペーストペイロードをサニタイズしたうえで、チャンク書き込み
    /// で PTY へ送る。カーネルの PTY 入力バッファ上限(macOS/Linux では通常 4096
    /// バイト)に触れないようにするため。
    ///
    /// 挙動の良い端末がペースト時に行う2つの安全策をここでも踏襲する。
    ///
    /// 1. **サニタイズ** (sanitize_pasted_text): クリップボードの内容には
    ///    ANSI エスケープシーケンスやその他の非表示制御バイトが混じることが
    ///    ある(色付きの端末や TUI、スタイル付きのウェブページからのコピーなど)。
    ///    これらをそのまま転送すると、カーソル移動やモード変更、最悪の場合は
    ///    \x1b[201~ を紛れ込ませて bracketed paste を早期終了させ、残りが
    ///    タイプされたコマンドとして実行されてしまう。エスケープシーケンスは
    ///    丸ごと取り除き、制御文字のうちタブと改行だけを残す(CR は LF に正規化)。
    /// 2. **条件付きブラケット化**: \x1b[200~ / \x1b[201~ マーカーは、
    ///    フォアグラウンドのアプリケーションが実際に bracketed paste
    ///    (DECSET 2004) を有効にしている場合にのみ出力する。本物の端末が
    ///    ゲートしているのと同じである。無条件でラップすると、それを要求
    ///    していないアプリ(素のプロンプトや cat など)にリテラルな
    ///    [200~ / [201~ テキストが流れ込んでしまう。
    pub fn write_paste_to_session(&mut self, idx: usize, text: &str) -> Result<()> {
        const CHUNK_SIZE: usize = 1024;
        const CHUNK_DELAY: Duration = Duration::from_millis(5);

        let cleaned = sanitize_pasted_text(text);

        let session = self
            .sessions
            .get_mut(idx)
            .context("Session index out of bounds")?;

        // bracketed paste モードのフラグを screen ロック下で読み取ってから、
        // writer ロックを取る前に解放する。
        let bracketed = {
            let parser = session.screen.lock().unwrap_or_else(|e| e.into_inner());
            parser.screen().bracketed_paste()
        };

        let mut writer = session.writer.lock().unwrap_or_else(|e| e.into_inner());

        // bracketed paste モードを開始する(アプリが対応している場合のみ)。
        if bracketed {
            writer
                .write_all(b"\x1b[200~")
                .context("Failed to write paste-start to PTY")?;
            writer.flush().context("Failed to flush PTY writer")?;
        }

        // ペイロードを小さなチャンクに分けて書き込む。UTF-8 の文字境界で
        // 分割し、フラッシュされたチャンクが不完全なマルチバイトシーケンスで
        // 終わらないようにする(utf8_chunks を参照) — さもないと 1 KiB
        // 境界をまたぐ全角文字やマルチバイトテキストが受け手側で誤デコード
        // される可能性がある。
        for chunk in utf8_chunks(&cleaned, CHUNK_SIZE) {
            writer
                .write_all(chunk.as_bytes())
                .context("Failed to write chunk to PTY")?;
            writer.flush().context("Failed to flush PTY writer")?;
            if chunk.len() == CHUNK_SIZE {
                thread::sleep(CHUNK_DELAY);
            }
        }

        // bracketed paste モードを終了する。
        if bracketed {
            writer
                .write_all(b"\x1b[201~")
                .context("Failed to write paste-end to PTY")?;
            writer.flush().context("Failed to flush PTY writer")?;
        }

        Ok(())
    }
}

// 自由関数のヘルパー

/// クリップボードのテキストを、ペーストとして PTY へ書き込む前にサニタイズする。
///
/// クリップボードの内容には、端末の入力ストリームへそのまま転送するのが
/// 危険なバイトがしばしば混じっている。
/// * **ANSI エスケープシーケンス**(色付きの端末や TUI、スタイル付きの
///   ウェブページからのコピー): これらはカーソルを移動させたり、モードを
///   切り替えたり、最も危険なケースでは bracketed paste を途中で*終了*
///   させる \x1b[201~ を含んでいて、その後のクリップボード内容がタイプ
///   されたコマンドとして解釈されてしまう。エスケープシーケンスは丸ごと
///   取り除く。
/// * **その他の C0/C1 制御文字と DEL**: そのまま転送するとベルを鳴らしたり、
///   (line discipline 経由で)シグナルを送ったり、入力を壊したりする。
///
/// 残すもの: 通常の表示可能テキスト、**タブ** (\t)、**改行** (\n)。
/// キャリッジリターンは正規化する — \r\n と単独の \r はどちらも単一の
/// \n になる — ので、複数行のペーストは裸の CR を紛れ込ませずに行構造を保つ。
pub(super) fn sanitize_pasted_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // ESC はエスケープシーケンスの開始 — 丸ごと読み飛ばす。
            '\u{1b}' => skip_escape_sequence(&mut chars),
            '\t' | '\n' => out.push(c),
            // CR / CRLF を単一の LF に正規化する。
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            // それ以外の制御文字はすべて破棄する(残りの C0、DEL、C1)。
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
        Some(']') | Some('P') | Some('X') | Some('^') | Some('_') => {
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
        Some('N') | Some('O') => {
            // シングルシフト: それが選択する1文字を読み飛ばす。
            chars.next();
        }
        _ => {}
    }
}

/// マウスホイールの1ノッチを、マウスレポートを有効にした子プログラムへ端末が
/// 送るバイトシーケンスとしてエンコードする。
///
/// up はホイールアップ(xterm ボタン 64)かホイールダウン(65)かを選ぶ。
/// col / row は 1-based のセル座標。encoding は子プロセスが要求した
/// モードに従う: SGR (1006、223列制限のない現代的なデフォルト)は
/// CSI < b ; col ; row M を出力し、それ以外はレガシーな X10 形式
/// CSI M Cb Cx Cy を使う。各値は 32 だけオフセットして1バイトにクランプする。
pub(super) fn encode_mouse_wheel(
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
        // デフォルト(X10)と Utf8: CSI M Cb Cx Cy、各バイトは 32 だけオフセットする。
        // 223 を超える値はレガシー形式では表現できないため、座標が
        // ラップしないようクランプする。
        _ => {
            let cb = (32 + button).min(255) as u8;
            let cx = (32 + col).min(255) as u8;
            let cy = (32 + row).min(255) as u8;
            vec![0x1b, b'[', b'M', cb, cx, cy]
        }
    }
}

/// オルタネート画面上でページャをスクロールするのに使う、Up/Down 矢印キー
/// 押下のエスケープシーケンスを返す。
///
/// up は Up (true) か Down (false) かを選ぶ。app_cursor は DECCKM
/// (アプリケーションカーソルキーモード)に従う: 有効な場合、端末は SS3
/// (ESC O) シーケンスを送る。そうでなければ CSI (ESC [)。less などの
/// ページャはアプリケーションカーソルモードを有効にして SS3 形式にバインド
/// しているため、これを尊重しないと矢印キーが確実に効かなくなる。
pub(super) fn scroll_arrow_sequence(up: bool, app_cursor: bool) -> &'static [u8] {
    match (up, app_cursor) {
        (true, true) => b"\x1bOA",   // Up   (SS3)
        (true, false) => b"\x1b[A",  // Up   (CSI)
        (false, true) => b"\x1bOB",  // Down (SS3)
        (false, false) => b"\x1b[B", // Down (CSI)
    }
}
