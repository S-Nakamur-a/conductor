//! 端末への問い合わせによるケイパビリティ検出。
//!
//! 背景色 (OSC 11) の問い合わせと、ファイルアイコンの文字セット判定の 2 つがある。
//! 前者は端末に実際に問い合わせ、後者は `TERM_PROGRAM` だけを見る
//! ([detect_icon_set] にその理由がある)。

use std::io::Write;

use crate::icons::IconSet;

#[cfg(test)]
mod tests;

/// OSC 11 で端末に背景色を問い合わせ、相対輝度を返す
/// (0.0 = 黒、1.0 = 白、線形スケール)。
///
/// raw mode に入ったあと、かつ crossterm のイベントループが stdin を読み始める前に
/// 呼ばないと、問い合わせへの応答が入力イベントとして飲み込まれる。
///
/// 応答の読み取りは libc::poll で 150ms の期限つきにしてある。OSC 11 に対応しない
/// 端末を待ち続けないため。tmux の中ではパススルーが通常無効で応答が来ないので、
/// 問い合わせ自体を送らずに None を返す。
pub fn query_background_luminance() -> Option<f64> {
    let term = std::env::var("TERM").unwrap_or_default();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    if term.starts_with("tmux") || term_program.eq_ignore_ascii_case("tmux") {
        return None;
    }

    {
        let mut stdout = std::io::stdout().lock();
        if stdout.write_all(b"\x1b]11;?\x1b\\").is_err() {
            return None;
        }
        if stdout.flush().is_err() {
            return None;
        }
    }

    read_osc11_response().and_then(|r| parse_osc11_luminance(&r))
}

const OSC11_TIMEOUT_MS: i32 = 150;

/// stdin から OSC 11 の応答本体を読み切る。1 バイトずつ読むのは、終端 (ST か BEL) を
/// 正確に検出して、後続の問い合わせ (グラフィックスプロトコル等) へバイトを漏らさないため。
fn read_osc11_response() -> Option<String> {
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(OSC11_TIMEOUT_MS as u64);
    let mut buf: Vec<u8> = Vec::with_capacity(64);
    loop {
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .map(|d| d.as_millis().min(OSC11_TIMEOUT_MS as u128) as i32)
            .unwrap_or(0);
        if remaining <= 0 {
            return None;
        }

        // SAFETY: poll は単純なシステムコールで、pollfd も素のデータ構造。
        let ready = unsafe {
            let mut pfd = libc::pollfd {
                fd: libc::STDIN_FILENO,
                events: libc::POLLIN,
                revents: 0,
            };
            libc::poll(&mut pfd, 1, remaining)
        };
        if ready <= 0 {
            return None;
        }

        let mut byte = [0u8; 1];
        // SAFETY: 読み取り可能と分かっている fd 0 (stdin) から 1 バイト読むだけ。
        let n = unsafe {
            libc::read(
                libc::STDIN_FILENO,
                byte.as_mut_ptr().cast::<libc::c_void>(),
                1,
            )
        };
        if n <= 0 {
            return None;
        }
        buf.push(byte[0]);

        if buf.ends_with(b"\x1b\\") || buf.ends_with(b"\x07") {
            return String::from_utf8(buf).ok();
        }
        if buf.len() > 256 {
            return None;
        }
    }
}

/// 端末の背景輝度からテーマ名を選ぶ。
///
/// configured が None (テーマ未設定) かつ lum > 0.5 (明るい背景) のときだけ
/// Some を返す。それ以外は今のテーマをそのまま使わせる。
pub fn auto_theme_for_background(lum: f64, configured: Option<&str>) -> Option<&'static str> {
    if configured.is_some() || lum <= 0.5 {
        return None;
    }
    Some("catppuccin-latte")
}

/// 端末が Nerd Font のシンボルを同梱しているなら [IconSet::Nerd] を返す。
///
/// 判定できないときは None を返す。ユーザのフォントに何が入っているかは端末に
/// 問い合わせられないので、同梱が公表されている端末だけを Some にしている。
/// tmux の中では TERM_PROGRAM が tmux に置き換わって内側の端末が見えないため、
/// 同じく None になる。
pub fn detect_icon_set() -> Option<IconSet> {
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    icon_set_for_term_program(&term_program)
}

/// Ghostty (1.2.0 以降) と WezTerm は symbols-only の Nerd Font をフォールバックに
/// 同梱していて、ユーザがどのフォントを設定していてもグリフが出る。kitty・
/// Alacritty・iTerm2 は同梱しないのでユーザのフォント次第となり、判定しない。
pub fn icon_set_for_term_program(term_program: &str) -> Option<IconSet> {
    if term_program.eq_ignore_ascii_case("ghostty") || term_program.eq_ignore_ascii_case("WezTerm")
    {
        return Some(IconSet::Nerd);
    }
    None
}

/// 各チャネルは 16 進 4 桁 (0000-FFFF) が基本だが、8bit (2 桁) で返す端末もある。
/// どちらも上位バイトだけを輝度の計算に使う。
fn parse_osc11_luminance(response: &str) -> Option<f64> {
    let inner = response
        .trim_start_matches('\x1b')
        .trim_start_matches(']')
        .trim_end_matches('\x1b')
        .trim_end_matches('\\')
        .trim_end_matches('\x07');
    let rgb_part = inner.strip_prefix("11;rgb:")?;
    let parts: Vec<&str> = rgb_part.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let r = u8::from_str_radix(parts[0].get(..2)?, 16).ok()? as f64 / 255.0;
    let g = u8::from_str_radix(parts[1].get(..2)?, 16).ok()? as f64 / 255.0;
    let b = u8::from_str_radix(parts[2].get(..2)?, 16).ok()? as f64 / 255.0;
    Some(0.299 * r + 0.587 * g + 0.114 * b)
}
