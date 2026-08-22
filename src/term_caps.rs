//! 端末への問い合わせによるケイパビリティ検出。
//!
//! 用途は 2 つある。1 つは背景色 (OSC 11) の問い合わせで、明るい端末を検出
//! したらライトテーマへ自動で切り替えるために使う。この問い合わせは raw mode に
//! 入ったあと、かつ crossterm のイベントループが stdin を読み始める前に実行
//! しなければならない。そうしないと問い合わせへの応答が入力イベントとして
//! 飲み込まれてしまう。
//!
//! もう 1 つはファイルアイコンの文字セットの判定で、こちらは端末に問い合わせず
//! TERM_PROGRAM だけを見る ([detect_icon_set] にその理由がある)。

use std::io::Write;

use crate::config::IconSet;

/// OSC 11 で端末に背景色を問い合わせ、相対輝度を返す
/// (0.0 = 黒、1.0 = 白、線形スケール)。
///
/// 仕組み: raw mode のまま ESC ] 11 ; ? ST を stdout へ送り、メインスレッドで
/// libc::poll を使って 150ms の期限つきで fd 0 (stdin) をポーリングする。
/// 読み取るのは fd が読み取り可能と報告されたときだけなので、この呼び出しが
/// 期限より長くブロックすることはない。応答は返る前に完全に読み切るので、
/// 後続のグラフィックスプロトコルの問い合わせ (Picker::from_query_stdio) へ
/// バイトが漏れることはない。
///
/// tmux: TERM が "tmux" で始まるか TERM_PROGRAM == "tmux" のとき、
/// パススルーは通常無効で応答が来ない。その場合は問い合わせを送らずに
/// 即座に None を返す。
///
/// タイムアウト内に端末が応答しない場合や、応答をパースできない場合は
/// None を返す。
pub fn query_background_luminance() -> Option<f64> {
    // tmux の中で動いているときは飛ばす (パススルーは通常無効)。
    let term = std::env::var("TERM").unwrap_or_default();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    if term.starts_with("tmux") || term_program.eq_ignore_ascii_case("tmux") {
        return None;
    }

    // OSC 11 の問い合わせを stdout へ送る。raw mode は既に有効で、crossterm の
    // イベントループはまだ始まっていないので、同時に読む相手はいない。
    {
        let mut stdout = std::io::stdout().lock();
        // OSC 11 の問い合わせ: ESC ] 11 ; ? ST  (ST = ESC \)
        if stdout.write_all(b"\x1b]11;?\x1b\\").is_err() {
            return None;
        }
        if stdout.flush().is_err() {
            return None;
        }
    }

    // 応答はメインスレッドで libc::poll を使って読む。OSC 11 に対応しない端末でも
    // 期限より長くブロックしないようにするため。
    const TIMEOUT_MS: i32 = 150;
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_millis(TIMEOUT_MS as u64);

    let mut buf: Vec<u8> = Vec::with_capacity(64);
    loop {
        // この poll 呼び出しに残された時間を求める。
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .map(|d| d.as_millis().min(TIMEOUT_MS as u128) as i32)
            .unwrap_or(0);
        if remaining <= 0 {
            return None; // タイムアウト。
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
            return None; // タイムアウトまたはエラー。
        }

        // OSC の終端を正確に検出するため 1 バイトずつ読む。
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

        // OSC の終端: ST (ESC \) または BEL (0x07)。
        let ended = buf.ends_with(b"\x1b\\") || buf.ends_with(b"\x07");
        if ended {
            return std::str::from_utf8(&buf)
                .ok()
                .and_then(parse_osc11_luminance);
        }

        // 応答が不自然に大きくなったら諦める。
        if buf.len() > 256 {
            return None;
        }
    }
}

/// 端末の背景輝度からテーマ名を選ぶ。
///
/// 呼び出し側が自動でテーマを切り替えるべきときだけ Some(name) を返す:
/// - configured が None (設定でテーマを固定していない)、かつ
/// - lum > 0.5 (明るい背景を検出)。
///
/// 既にテーマが設定されている場合や背景が暗い場合は None を返す
/// (今のテーマをそのまま使う)。
pub fn auto_theme_for_background(lum: f64, configured: Option<&str>) -> Option<&'static str> {
    if configured.is_some() {
        return None; // 明示的な設定が常に優先。
    }
    if lum > 0.5 {
        Some("catppuccin-latte")
    } else {
        None // 暗い背景。デフォルトのダークテーマのままにする。
    }
}

/// 端末が Nerd Font のシンボルを同梱しているなら [IconSet::Nerd] を返す。
///
/// 判定できないときは None を返す — 呼び出し側はフォールバックのまま進み、
/// 設定ファイルにも何も書かない。ユーザのフォントに何が入っているかは端末に
/// 問い合わせられないので、同梱が公表されている端末だけを Some にしている。
///
/// tmux の中では TERM_PROGRAM が tmux に置き換わって内側の端末が見えないため、
/// 同じく None を返す (背景色の問い合わせが tmux を諦めるのと同じ理由)。
pub fn detect_icon_set() -> Option<IconSet> {
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    icon_set_for_term_program(&term_program)
}

/// TERM_PROGRAM の値から文字セットを選ぶ純粋関数。
///
/// Ghostty は 1.2.0 以降 symbols-only の Nerd Font を同梱してフォールバックに
/// 使い、WezTerm は Symbols Nerd Font Mono をビルトインのフォールバックとして
/// 持つ。どちらもユーザがどのフォントを設定していてもグリフが出る。
/// kitty・Alacritty・iTerm2 は同梱しないのでユーザのフォント次第となり、
/// ここでは判定しない。
pub fn icon_set_for_term_program(term_program: &str) -> Option<IconSet> {
    if term_program.eq_ignore_ascii_case("ghostty") || term_program.eq_ignore_ascii_case("WezTerm")
    {
        return Some(IconSet::Nerd);
    }
    None
}

/// ESC ] 11 ; rgb:RRRR/GGGG/BBBB ST をパースして相対輝度を返す。
///
/// 各チャネルは 16 進 4 桁の値 (0000-FFFF)。輝度の計算には一般的な慣習に合わせて
/// 上位バイト (先頭 2 桁) を使う。
fn parse_osc11_luminance(response: &str) -> Option<f64> {
    // OSC の前後を剥がしてから rgb:…/…/… の部分を取り出す。
    let inner = response
        .trim_start_matches('\x1b')
        .trim_start_matches(']')
        .trim_end_matches('\x1b')
        .trim_end_matches('\\')
        .trim_end_matches('\x07');
    // 期待する形: "11;rgb:RRRR/GGGG/BBBB"
    let rgb_part = inner.strip_prefix("11;rgb:")?;
    let parts: Vec<&str> = rgb_part.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    // 各要素は 16 進 4 桁。先頭 2 桁 (上位バイト) を取る。
    let r = u8::from_str_radix(parts[0].get(..2)?, 16).ok()? as f64 / 255.0;
    let g = u8::from_str_radix(parts[1].get(..2)?, 16).ok()? as f64 / 255.0;
    let b = u8::from_str_radix(parts[2].get(..2)?, 16).ok()? as f64 / 255.0;
    Some(0.299 * r + 0.587 * g + 0.114 * b)
}

#[cfg(test)]
mod tests {
    // auto_theme_for_background の純粋関数としてのテスト。

    #[test]
    fn auto_theme_configured_always_returns_none() {
        // ユーザーがテーマを固定している。自動検出がそれを上書きしてはいけない。
        assert!(super::auto_theme_for_background(0.9, Some("dracula")).is_none());
        assert!(super::auto_theme_for_background(0.9, Some("catppuccin-mocha")).is_none());
        assert!(super::auto_theme_for_background(0.1, Some("github-light")).is_none());
    }

    #[test]
    fn auto_theme_light_background_selects_latte() {
        // 明るい背景 (輝度 > 0.5) かつテーマ未設定。
        let t = super::auto_theme_for_background(0.9, None);
        assert_eq!(t, Some("catppuccin-latte"));
        // しきい値ちょうど (0.5) は暗い扱い。
        assert!(super::auto_theme_for_background(0.5, None).is_none());
        // しきい値のすぐ上。
        let t2 = super::auto_theme_for_background(0.501, None);
        assert_eq!(t2, Some("catppuccin-latte"));
    }

    #[test]
    fn auto_theme_dark_background_returns_none() {
        assert!(super::auto_theme_for_background(0.1, None).is_none());
        assert!(super::auto_theme_for_background(0.0, None).is_none());
        assert!(super::auto_theme_for_background(0.499, None).is_none());
    }

    // OSC11 のパーサのテスト (端末との I/O は不要)。

    #[test]
    fn parse_osc11_black_background() {
        // 黒い背景: 全チャネル 0x00。
        let lum = super::parse_osc11_luminance("\x1b]11;rgb:0000/0000/0000\x1b\\");
        assert!(lum.is_some());
        assert!((lum.unwrap() - 0.0).abs() < 0.01);
    }

    #[test]
    fn parse_osc11_white_background() {
        // 白い背景: 全チャネル 0xFF。
        let lum = super::parse_osc11_luminance("\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
        assert!(lum.is_some());
        assert!((lum.unwrap() - 1.0).abs() < 0.01);
    }

    #[test]
    fn parse_osc11_catppuccin_mocha_bg() {
        // Catppuccin Mocha の base: #1e1e2e → 0x1e1e ≈ 30, 0x1e1e ≈ 30, 0x2e2e ≈ 46
        let lum = super::parse_osc11_luminance("\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\");
        assert!(lum.is_some());
        let v = lum.unwrap();
        // 暗い背景を期待する。輝度は 0.5 を大きく下回るはず。
        assert!(v < 0.2, "expected dark bg, got {v}");
    }

    #[test]
    fn parse_osc11_malformed_returns_none() {
        assert!(super::parse_osc11_luminance("garbage").is_none());
        assert!(super::parse_osc11_luminance("\x1b]11;rgb:ZZ/GG/HH\x1b\\").is_none());
        assert!(super::parse_osc11_luminance("\x1b]11;rgb:ffff/ffff\x1b\\").is_none());
    }

    // パーサの追加カバレッジ。

    #[test]
    fn parse_osc11_bel_terminator() {
        // 一部の端末 (古い xterm など) は OSC の終端に BEL (0x07) を使う。
        let lum = super::parse_osc11_luminance("\x1b]11;rgb:ffff/ffff/ffff\x07");
        assert!(lum.is_some());
        assert!((lum.unwrap() - 1.0).abs() < 0.01, "white bg via BEL terminator");
    }

    #[test]
    fn parse_osc11_8bit_channels() {
        // 一部の端末は 8bit (2 桁) のチャネル値 rgb:RR/GG/BB で応答する。
        // パーサは先頭 2 桁を読むので、これは自然に扱える。
        let lum = super::parse_osc11_luminance("\x1b]11;rgb:ff/ff/ff\x1b\\");
        assert!(lum.is_some());
        assert!((lum.unwrap() - 1.0).abs() < 0.01, "white bg via 8-bit channels");

        let dark = super::parse_osc11_luminance("\x1b]11;rgb:1e/1e/2e\x1b\\");
        assert!(dark.is_some());
        assert!(dark.unwrap() < 0.2, "dark bg via 8-bit channels");
    }

    /// Nerd Font のシンボルを同梱している端末だけを Nerd と判定すること。
    #[test]
    fn icon_set_only_for_terminals_bundling_the_symbols() {
        use crate::config::IconSet;
        for name in ["ghostty", "Ghostty", "WezTerm", "wezterm"] {
            assert_eq!(
                super::icon_set_for_term_program(name),
                Some(IconSet::Nerd),
                "{name} は Nerd Font のシンボルを同梱している"
            );
        }
    }

    /// フォントを同梱しない端末と、tmux 越しで内側が見えない場合は判定しないこと。
    /// ここで推測すると、Nerd Font を入れていないユーザの画面が tofu で埋まる。
    #[test]
    fn icon_set_declines_to_guess() {
        for name in [
            "kitty",
            "Alacritty",
            "iTerm.app",
            "Apple_Terminal",
            "tmux",
            "",
        ] {
            assert_eq!(
                super::icon_set_for_term_program(name),
                None,
                "{name} からはフォントの有無が判らない"
            );
        }
    }
}
