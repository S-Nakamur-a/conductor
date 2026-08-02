//! rich モードのための端末ケイパビリティ検出。
//!
//! rich モードには起動時に決まる 2 つのティアがある:
//!
//! - Tier A (truecolor): 呼吸するグラデーションの枠とタイトルバーの
//!   グラデーション。セルごとの RGB だけで描く。truecolor の端末なら
//!   どれでも動く (Alacritty も含む)。
//! - Tier B (グラフィックスプロトコル): Tier A のすべてに加え、kitty / iTerm2 の
//!   グラフィックスプロトコルによるピクセル品質の画像プレビュー
//!   (Ghostty, kitty, WezTerm, iTerm2)。
//!
//! 検出を 2 段階にしてあるのは意図的で、安価な環境変数のヒントで「問い合わせるか
//! どうか」を決め、実際のエスケープシーケンスによる問い合わせ
//! (ratatui_image::picker::Picker::from_query_stdio に委譲。応答しない端末では
//! 最大 1 秒かかる) は、ヒントがグラフィックスプロトコルの存在を示唆したときだけ
//! 走らせる。この問い合わせは raw mode に入ったあと、かつ crossterm の
//! イベントループが stdin を読み始める前に実行しなければならない。そうしないと
//! 問い合わせへの応答が入力イベントとして飲み込まれてしまう。

use std::io::Write;

use ratatui_image::picker::ProtocolType;

/// このセッションで確定した rich モードのティア。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RichTier {
    /// rich な効果は無し (truecolor でない端末、または mode = "off")。
    Off,
    /// truecolor のセル効果のみ。
    TierA,
    /// truecolor のセル効果に加え、グラフィックスプロトコルによる画像描画。
    TierB,
}

impl RichTier {
    /// rich な効果 (Tier A 以上) が有効かどうか。
    pub fn is_rich(self) -> bool {
        !matches!(self, RichTier::Off)
    }

    /// グラフィックスプロトコルの機能 (Tier B) が有効かどうか。
    pub fn has_graphics(self) -> bool {
        matches!(self, RichTier::TierB)
    }
}

/// 環境変数から推測したケイパビリティ (端末との I/O は行わない)。
#[derive(Debug, Clone, Default)]
pub struct TermCaps {
    /// 端末が 24bit カラーを表明している (COLORTERM)、または対応が既知の端末。
    pub truecolor: bool,
    /// グラフィックスプロトコル (kitty / iTerm2) に対応していそうだと環境が
    /// 示唆しており、エスケープシーケンスでの問い合わせに見合うかどうか。
    pub graphics_likely: bool,
    /// ユーザー向けメッセージに使う端末の呼び名 (例: "Ghostty")。
    pub terminal_name: Option<String>,
}

impl TermCaps {
    /// 環境変数だけからケイパビリティを検出する。
    pub fn detect_from_env() -> Self {
        Self::from_vars(
            &std::env::var("COLORTERM").unwrap_or_default(),
            &std::env::var("TERM").unwrap_or_default(),
            &std::env::var("TERM_PROGRAM").unwrap_or_default(),
            std::env::var("KITTY_WINDOW_ID").is_ok(),
            std::env::var("GHOSTTY_RESOURCES_DIR").is_ok(),
            std::env::var("WEZTERM_EXECUTABLE").is_ok(),
            std::env::var("ITERM_SESSION_ID").is_ok(),
        )
    }

    /// 検出ロジックそのもの。テストしやすいよう std::env から切り離してある。
    #[allow(clippy::too_many_arguments)]
    fn from_vars(
        colorterm: &str,
        term: &str,
        term_program: &str,
        has_kitty_window: bool,
        has_ghostty_dir: bool,
        has_wezterm_exe: bool,
        has_iterm_session: bool,
    ) -> Self {
        let term_lc = term.to_lowercase();
        let program_lc = term_program.to_lowercase();

        let terminal_name = if program_lc == "ghostty" || has_ghostty_dir || term_lc.contains("ghostty") {
            Some("Ghostty")
        } else if has_kitty_window || term_lc.contains("kitty") {
            Some("kitty")
        } else if program_lc == "wezterm" || has_wezterm_exe {
            Some("WezTerm")
        } else if program_lc == "iterm.app" || has_iterm_session {
            Some("iTerm2")
        } else if program_lc == "alacritty" || term_lc.contains("alacritty") {
            Some("Alacritty")
        } else {
            None
        };

        let graphics_likely = matches!(
            terminal_name,
            Some("Ghostty") | Some("kitty") | Some("WezTerm") | Some("iTerm2")
        );

        let colorterm_lc = colorterm.to_lowercase();
        let truecolor =
            colorterm_lc == "truecolor" || colorterm_lc == "24bit" || graphics_likely;

        Self {
            truecolor,
            graphics_likely,
            terminal_name: terminal_name.map(String::from),
        }
    }
}

/// 設定のモード、環境から得たケイパビリティ、そして (あれば) グラフィックス
/// プロトコルの問い合わせ結果から rich のティアを決める。
///
/// probed_protocol が Some になるのは問い合わせが走って成功したときだけ。
/// Halfblocks は「端末は応答したが実際のプロトコルには対応していない」を意味する。
pub fn resolve_rich_tier(
    mode: &str,
    caps: &TermCaps,
    probed_protocol: Option<ProtocolType>,
) -> RichTier {
    let graphics_confirmed = matches!(
        probed_protocol,
        Some(ProtocolType::Kitty) | Some(ProtocolType::Iterm2) | Some(ProtocolType::Sixel)
    );
    match mode {
        "off" => RichTier::Off,
        "force" => {
            if graphics_confirmed {
                RichTier::TierB
            } else {
                RichTier::TierA
            }
        }
        // "auto" と、認識できない値すべて。
        _ => {
            if graphics_confirmed {
                RichTier::TierB
            } else if caps.truecolor {
                RichTier::TierA
            } else {
                RichTier::Off
            }
        }
    }
}

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
    use super::*;

    fn caps(colorterm: &str, term: &str, program: &str) -> TermCaps {
        TermCaps::from_vars(colorterm, term, program, false, false, false, false)
    }

    #[test]
    fn alacritty_is_truecolor_without_graphics() {
        let c = caps("truecolor", "xterm-256color", "");
        assert!(c.truecolor);
        assert!(!c.graphics_likely);
        assert_eq!(c.terminal_name, None);
    }

    #[test]
    fn alacritty_named_via_term_program() {
        let c = caps("truecolor", "xterm-256color", "Alacritty");
        assert!(c.truecolor);
        assert!(!c.graphics_likely);
        assert_eq!(c.terminal_name.as_deref(), Some("Alacritty"));
    }

    #[test]
    fn ghostty_is_graphics_likely() {
        let c = caps("truecolor", "xterm-ghostty", "ghostty");
        assert!(c.truecolor);
        assert!(c.graphics_likely);
        assert_eq!(c.terminal_name.as_deref(), Some("Ghostty"));
    }

    #[test]
    fn kitty_detected_via_window_id() {
        let c = TermCaps::from_vars("", "xterm-kitty", "", true, false, false, false);
        assert!(c.graphics_likely);
        // グラフィックス対応の端末は COLORTERM が無くても必ず truecolor。
        assert!(c.truecolor);
        assert_eq!(c.terminal_name.as_deref(), Some("kitty"));
    }

    #[test]
    fn dumb_terminal_has_no_caps() {
        let c = caps("", "vt100", "");
        assert!(!c.truecolor);
        assert!(!c.graphics_likely);
    }

    #[test]
    fn resolve_off_overrides_everything() {
        let c = caps("truecolor", "xterm-ghostty", "ghostty");
        assert_eq!(
            resolve_rich_tier("off", &c, Some(ProtocolType::Kitty)),
            RichTier::Off
        );
    }

    #[test]
    fn resolve_auto_tiers() {
        let ghostty = caps("truecolor", "xterm-ghostty", "ghostty");
        assert_eq!(
            resolve_rich_tier("auto", &ghostty, Some(ProtocolType::Kitty)),
            RichTier::TierB
        );

        let alacritty = caps("truecolor", "xterm-256color", "");
        assert_eq!(resolve_rich_tier("auto", &alacritty, None), RichTier::TierA);

        let dumb = caps("", "vt100", "");
        assert_eq!(resolve_rich_tier("auto", &dumb, None), RichTier::Off);
    }

    #[test]
    fn resolve_probe_halfblocks_falls_back_to_tier_a() {
        // 問い合わせに応答はあったがグラフィックスプロトコルは見つからず。Tier A に留まる。
        let c = caps("truecolor", "xterm-256color", "WezTerm");
        assert_eq!(
            resolve_rich_tier("auto", &c, Some(ProtocolType::Halfblocks)),
            RichTier::TierA
        );
    }

    #[test]
    fn resolve_force_gives_at_least_tier_a() {
        let dumb = caps("", "vt100", "");
        assert_eq!(resolve_rich_tier("force", &dumb, None), RichTier::TierA);
        assert_eq!(
            resolve_rich_tier("force", &dumb, Some(ProtocolType::Kitty)),
            RichTier::TierB
        );
    }

    #[test]
    fn tier_predicates() {
        assert!(!RichTier::Off.is_rich());
        assert!(RichTier::TierA.is_rich());
        assert!(RichTier::TierB.is_rich());
        assert!(!RichTier::TierA.has_graphics());
        assert!(RichTier::TierB.has_graphics());
    }

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
}
