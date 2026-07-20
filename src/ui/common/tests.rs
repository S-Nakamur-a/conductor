//! Unit tests for `ui::common`: the status bar's keybinding hint and the
//! badge color math's WCAG contrast guarantees.

mod status_bar_tests {
    use crate::app::Focus;
    use crate::keymap::{Action, KeyContext, KeyMap};
    use crate::ui::common::representative_chord;
    use crate::ui::common::status_bar::status_bar_hint;

    fn keymap() -> KeyMap {
        KeyMap::new(&toml::Table::new())
    }

    #[test]
    fn representative_chord_prefers_short_ascii_over_unicode() {
        let km = keymap();
        // cycle_focus_backward is bound to alt+h AND the macOS glyph '˙'; the
        // glyph must never be shown.
        let chord = representative_chord(&km, KeyContext::Global, Action::CycleFocusBackward);
        assert_eq!(chord, Some("alt+h".to_string()));

        // nav prefers the bare 'j' over the 'down' alias.
        let nav = representative_chord(&km, KeyContext::Worktree, Action::NavigateDown);
        assert_eq!(nav, Some("j".to_string()));
    }

    #[test]
    fn worktree_footer_is_truthful_and_has_no_unicode() {
        let hint = status_bar_hint(Focus::Worktree, &keymap());
        assert!(hint.contains("j/k: nav"), "{hint}");
        assert!(hint.contains("tab: panel"), "{hint}");
        assert!(hint.contains("w: new"), "{hint}");
        // The old hardcoded lie must be gone, and no fallback glyphs leak in.
        assert!(!hint.contains("Cmd+1-5"), "{hint}");
        assert!(hint.is_ascii(), "footer must be ASCII-only: {hint}");
    }

    #[test]
    fn terminal_footer_notes_passthrough_and_leave_key() {
        let hint = status_bar_hint(Focus::TerminalClaude, &keymap());
        assert!(hint.contains("keys → terminal"), "{hint}");
        // leave_terminal is ctrl+esc, not a bare Esc.
        assert!(hint.contains("ctrl+esc: leave"), "{hint}");
    }
}

mod color_tests {
    use ratatui::style::Color;

    use crate::ui::common::color::{hsl_to_rgb, readable_fg_on, relative_luminance};

    /// Contrast ratio between two relative-luminance values per WCAG 2.1.
    fn contrast_ratio(l1: f64, l2: f64) -> f64 {
        (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
    }

    #[test]
    fn relative_luminance_endpoints() {
        assert!(relative_luminance(0, 0, 0).abs() < 1e-9);
        assert!((relative_luminance(255, 255, 255) - 1.0).abs() < 1e-9);
        // Green contributes far more luminance than blue at full intensity.
        assert!(relative_luminance(0, 255, 0) > relative_luminance(0, 0, 255));
    }

    #[test]
    fn readable_fg_matches_higher_contrast_choice() {
        // Bright background → black text wins.
        assert_eq!(readable_fg_on(255, 255, 0), Color::Rgb(0, 0, 0));
        // Dark background → white text wins.
        assert_eq!(readable_fg_on(20, 20, 120), Color::Rgb(255, 255, 255));
    }

    /// Across every hue the badge can take, the chosen text color must beat the
    /// rejected one — guaranteeing the project name never collides with its
    /// background, whether the badge lands light or dark.
    #[test]
    fn badge_fg_is_always_the_more_readable_choice() {
        for hue in 0..360 {
            let (r, g, b) = hsl_to_rgb(hue as f64, 0.6, 0.45);
            let bg = relative_luminance(r, g, b);
            let fg = readable_fg_on(r, g, b);
            let (chosen, rejected) = match fg {
                Color::Rgb(0, 0, 0) => (0.0, 1.0),
                Color::Rgb(255, 255, 255) => (1.0, 0.0),
                other => panic!("unexpected fg {other:?} at hue {hue}"),
            };
            assert!(
                contrast_ratio(bg, chosen) >= contrast_ratio(bg, rejected),
                "hue {hue}: chosen fg has worse contrast than the alternative",
            );
            // Sanity: the badge stays comfortably above the 3:1 large-text /
            // UI-component floor for every hue.
            assert!(
                contrast_ratio(bg, chosen) >= 3.0,
                "hue {hue}: contrast {:.2} fell below 3:1",
                contrast_ratio(bg, chosen),
            );
        }
    }
}
