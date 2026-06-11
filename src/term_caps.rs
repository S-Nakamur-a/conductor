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
}
