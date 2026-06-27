//! Terminal capability detection for rich mode.
//!
//! Rich mode has two tiers resolved at startup:
//!
//! - **Tier A** (truecolor): gradient breathing borders and title-bar
//!   gradients, rendered purely with per-cell RGB. Works in any truecolor
//!   terminal (Alacritty included).
//! - **Tier B** (graphics protocol): everything in Tier A, plus pixel-quality
//!   image previews via the kitty/iTerm2 graphics protocols (Ghostty, kitty,
//!   WezTerm, iTerm2).
//!
//! Detection is two-staged on purpose: cheap environment-variable hints decide
//! *whether* to probe, and the actual escape-sequence probe (delegated to
//! `ratatui_image::picker::Picker::from_query_stdio`, which needs up to a
//! second on unresponsive terminals) only runs when the hints say a graphics
//! protocol is likely. The probe must run after entering raw mode but before
//! the crossterm event loop starts reading stdin, or the query response would
//! be swallowed as input events.

use std::io::Write;

use ratatui_image::picker::ProtocolType;

/// Resolved rich-mode tier for this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RichTier {
    /// No rich effects (non-truecolor terminal or `mode = "off"`).
    Off,
    /// Truecolor cell effects only.
    TierA,
    /// Truecolor cell effects + graphics-protocol image rendering.
    TierB,
}

impl RichTier {
    /// Whether any rich effect (Tier A or above) is active.
    pub fn is_rich(self) -> bool {
        !matches!(self, RichTier::Off)
    }

    /// Whether graphics-protocol features (Tier B) are active.
    pub fn has_graphics(self) -> bool {
        matches!(self, RichTier::TierB)
    }
}

/// Capabilities inferred from environment variables (no terminal I/O).
#[derive(Debug, Clone, Default)]
pub struct TermCaps {
    /// Terminal advertises 24-bit color (`COLORTERM`), or is a terminal known
    /// to support it.
    pub truecolor: bool,
    /// Environment hints that a graphics protocol (kitty/iTerm2) is likely
    /// supported, making an escape-sequence probe worthwhile.
    pub graphics_likely: bool,
    /// Friendly terminal name for user-facing messages (e.g. "Ghostty").
    pub terminal_name: Option<String>,
}

impl TermCaps {
    /// Detect capabilities from environment variables only.
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

    /// Pure detection logic, separated from `std::env` for testability.
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

/// Resolve the rich tier from the config mode, env capabilities, and the
/// (optional) result of the graphics-protocol probe.
///
/// `probed_protocol` is `Some` only when the probe ran and succeeded;
/// `Halfblocks` means the terminal answered but supports no real protocol.
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
        // "auto" and anything unrecognized.
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

/// Query the terminal for its background color via OSC 11 and return the
/// relative luminance (0.0 = black, 1.0 = white, linear scale).
///
/// **How it works:** sends `ESC ] 11 ; ? ST` to stdout while in raw mode, then
/// polls fd 0 (stdin) on the **main thread** with `libc::poll` and a 150 ms
/// deadline. Reading happens only when the fd is reported readable, so the call
/// never blocks longer than the deadline. The response is drained completely
/// before returning so no bytes leak into the subsequent graphics-protocol probe
/// (`Picker::from_query_stdio`).
///
/// **tmux:** when `TERM` starts with `"tmux"` or `TERM_PROGRAM == "tmux"`,
/// the passthrough is typically disabled and no response arrives; the function
/// returns `None` immediately without sending the query.
///
/// Returns `None` when the terminal does not respond within the timeout or the
/// response cannot be parsed.
pub fn query_background_luminance() -> Option<f64> {
    // Skip when running inside tmux (passthrough usually disabled).
    let term = std::env::var("TERM").unwrap_or_default();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    if term.starts_with("tmux") || term_program.eq_ignore_ascii_case("tmux") {
        return None;
    }

    // Send the OSC 11 query to stdout. Raw mode is already active and the
    // crossterm event loop has not started yet, so there is no concurrent reader.
    {
        let mut stdout = std::io::stdout().lock();
        // OSC 11 query: ESC ] 11 ; ? ST  (ST = ESC \)
        if stdout.write_all(b"\x1b]11;?\x1b\\").is_err() {
            return None;
        }
        if stdout.flush().is_err() {
            return None;
        }
    }

    // Read the response on the main thread using libc::poll so we never block
    // longer than the deadline even on terminals that don't support OSC 11.
    const TIMEOUT_MS: i32 = 150;
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_millis(TIMEOUT_MS as u64);

    let mut buf: Vec<u8> = Vec::with_capacity(64);
    loop {
        // Compute remaining time for this poll call.
        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .map(|d| d.as_millis().min(TIMEOUT_MS as u128) as i32)
            .unwrap_or(0);
        if remaining <= 0 {
            return None; // Timed out.
        }

        // SAFETY: poll is a simple syscall with a plain-old-data pollfd struct.
        let ready = unsafe {
            let mut pfd = libc::pollfd {
                fd: libc::STDIN_FILENO,
                events: libc::POLLIN,
                revents: 0,
            };
            libc::poll(&mut pfd, 1, remaining)
        };

        if ready <= 0 {
            return None; // Timeout or error.
        }

        // Read one byte at a time to detect the OSC terminator precisely.
        let mut byte = [0u8; 1];
        // SAFETY: reading one byte from fd 0 (stdin) which we know is readable.
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

        // OSC terminators: ST (ESC \) or BEL (0x07).
        let ended = buf.ends_with(b"\x1b\\") || buf.ends_with(b"\x07");
        if ended {
            return std::str::from_utf8(&buf)
                .ok()
                .and_then(parse_osc11_luminance);
        }

        // Bail if the response grows unreasonably large.
        if buf.len() > 256 {
            return None;
        }
    }
}

/// Choose a theme name based on the terminal background luminance.
///
/// Returns `Some(name)` only when the caller should switch themes automatically:
/// - `configured` is `None` (user has not pinned a theme in the config), and
/// - `lum > 0.5` (light background detected).
///
/// Returns `None` when a theme is already configured, or the background is dark
/// (keep whatever theme is already active).
pub fn auto_theme_for_background(lum: f64, configured: Option<&str>) -> Option<&'static str> {
    if configured.is_some() {
        return None; // Explicit config always wins.
    }
    if lum > 0.5 {
        Some("catppuccin-latte")
    } else {
        None // Dark background — keep the default dark theme.
    }
}

/// Parse `ESC ] 11 ; rgb:RRRR/GGGG/BBBB ST` and return the relative luminance.
///
/// Each channel is a 4-hex-digit value (0000–FFFF); we use the high byte (first
/// two hex digits) for the luminance calculation, matching common practice.
fn parse_osc11_luminance(response: &str) -> Option<f64> {
    // Strip the OSC prefix and suffix, then extract the rgb:…/…/… part.
    let inner = response
        .trim_start_matches('\x1b')
        .trim_start_matches(']')
        .trim_end_matches('\x1b')
        .trim_end_matches('\\')
        .trim_end_matches('\x07');
    // Expected: "11;rgb:RRRR/GGGG/BBBB"
    let rgb_part = inner.strip_prefix("11;rgb:")?;
    let parts: Vec<&str> = rgb_part.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    // Each part is a 4-digit hex value; take the first two digits (high byte).
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
        // Graphics-capable terminals are always truecolor even without COLORTERM.
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
        // Probe answered but found no graphics protocol: stay on Tier A.
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

    // auto_theme_for_background pure-function tests.

    #[test]
    fn auto_theme_configured_always_returns_none() {
        // User has pinned a theme — auto-detection must not override it.
        assert!(super::auto_theme_for_background(0.9, Some("dracula")).is_none());
        assert!(super::auto_theme_for_background(0.9, Some("catppuccin-mocha")).is_none());
        assert!(super::auto_theme_for_background(0.1, Some("github-light")).is_none());
    }

    #[test]
    fn auto_theme_light_background_selects_latte() {
        // Light background (luminance > 0.5) with no configured theme.
        let t = super::auto_theme_for_background(0.9, None);
        assert_eq!(t, Some("catppuccin-latte"));
        // Exactly on the threshold (0.5) counts as dark.
        assert!(super::auto_theme_for_background(0.5, None).is_none());
        // Just above threshold.
        let t2 = super::auto_theme_for_background(0.501, None);
        assert_eq!(t2, Some("catppuccin-latte"));
    }

    #[test]
    fn auto_theme_dark_background_returns_none() {
        assert!(super::auto_theme_for_background(0.1, None).is_none());
        assert!(super::auto_theme_for_background(0.0, None).is_none());
        assert!(super::auto_theme_for_background(0.499, None).is_none());
    }

    // OSC11 parser tests (no terminal I/O needed).

    #[test]
    fn parse_osc11_black_background() {
        // Black background: all channels 0x00.
        let lum = super::parse_osc11_luminance("\x1b]11;rgb:0000/0000/0000\x1b\\");
        assert!(lum.is_some());
        assert!((lum.unwrap() - 0.0).abs() < 0.01);
    }

    #[test]
    fn parse_osc11_white_background() {
        // White background: all channels 0xFF.
        let lum = super::parse_osc11_luminance("\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
        assert!(lum.is_some());
        assert!((lum.unwrap() - 1.0).abs() < 0.01);
    }

    #[test]
    fn parse_osc11_catppuccin_mocha_bg() {
        // Catppuccin Mocha base: #1e1e2e → 0x1e1e ≈ 30, 0x1e1e ≈ 30, 0x2e2e ≈ 46
        let lum = super::parse_osc11_luminance("\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\");
        assert!(lum.is_some());
        let v = lum.unwrap();
        // Expect a dark background — luminance well below 0.5.
        assert!(v < 0.2, "expected dark bg, got {v}");
    }

    #[test]
    fn parse_osc11_malformed_returns_none() {
        assert!(super::parse_osc11_luminance("garbage").is_none());
        assert!(super::parse_osc11_luminance("\x1b]11;rgb:ZZ/GG/HH\x1b\\").is_none());
        assert!(super::parse_osc11_luminance("\x1b]11;rgb:ffff/ffff\x1b\\").is_none());
    }

    // additional parser coverage.

    #[test]
    fn parse_osc11_bel_terminator() {
        // Some terminals (e.g. old xterm) use BEL (0x07) as the OSC terminator.
        let lum = super::parse_osc11_luminance("\x1b]11;rgb:ffff/ffff/ffff\x07");
        assert!(lum.is_some());
        assert!((lum.unwrap() - 1.0).abs() < 0.01, "white bg via BEL terminator");
    }

    #[test]
    fn parse_osc11_8bit_channels() {
        // Some terminals reply with 8-bit (2-digit) channel values: `rgb:RR/GG/BB`.
        // The parser reads the first two hex digits, so this is handled naturally.
        let lum = super::parse_osc11_luminance("\x1b]11;rgb:ff/ff/ff\x1b\\");
        assert!(lum.is_some());
        assert!((lum.unwrap() - 1.0).abs() < 0.01, "white bg via 8-bit channels");

        let dark = super::parse_osc11_luminance("\x1b]11;rgb:1e/1e/2e\x1b\\");
        assert!(dark.is_some());
        assert!(dark.unwrap() < 0.2, "dark bg via 8-bit channels");
    }
}
