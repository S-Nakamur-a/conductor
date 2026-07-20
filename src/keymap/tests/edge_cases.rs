//! Tests for chord-normalization edge cases (SHIFT folding, multi-byte
//! macOS Option glyphs, canonical string casing) and other miscellaneous
//! guards not tied to a specific context or config-override behavior.

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use keymap_suite::ActionName;

#[test]
fn keys_for_action_lists_canonical_strings() {
    let km = default_keymap();
    let keys = km.keys_for_action(KeyContext::Worktree, Action::NavigateDown);
    assert!(keys.contains(&"j".to_string()), "{keys:?}");
    assert!(keys.contains(&"down".to_string()), "{keys:?}");
}

#[test]
fn action_name_roundtrips_for_every_variant() {
    // The macro-generated ActionName impl must be a bijection over ALL —
    // this covers every variant, not a hand-picked sample, because the
    // names and the match arms now come from the same declaration.
    for &action in Action::ALL {
        assert_eq!(Action::from_name(action.name()), Some(action));
    }
}

#[test]
fn lowercase_char_with_shift_is_not_recased() {
    // Behavior divergence from the old hand-rolled normalizer, locked in:
    // keymap-core trusts the glyph and only drops a redundant sole SHIFT, so
    // 'g'+SHIFT stays Char('g') and hits the bare 'g' binding (GoToTop) — it
    // is NOT re-cased to 'G' (GoToBottom). A terminal that delivers the
    // resolved glyph 'G' (the common case) still hits GoToBottom; see
    // `shift_g_resolves_uppercase_binding`.
    let km = default_keymap();
    let key = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&key, KeyContext::Worktree),
        Some(Action::GoToTop)
    );
}

#[test]
fn macos_unicode_fallback_chords_resolve() {
    // These glyphs are otherwise undetectable-by-eye in the TOML; this proves
    // the file→keymap-config→keymap-core→crossterm path survives multi-byte
    // chars for both the plain-Option and Shift-Option families.
    let km = default_keymap();
    let cases = [
        ('˙', Action::CycleFocusBackward),
        ('¬', Action::CycleFocusForward),
        ('¡', Action::FocusWorktree),
        ('§', Action::FocusTerminalShell),
        ('÷', Action::TogglePanelOverlay),
    ];
    for (glyph, action) in cases {
        let key = KeyEvent::new(KeyCode::Char(glyph), KeyModifiers::empty());
        assert_eq!(km.resolve(&key, KeyContext::Global), Some(action), "glyph {glyph:?}");
    }
}

#[test]
fn alt_shift_digit_does_not_fold_into_alt_digit() {
    // The "keep SHIFT when another modifier is held" rule: alt+1 focuses the
    // worktree, but alt+shift+1 must NOT drop the SHIFT and collapse onto it.
    // alt+shift+digit is now unbound (focus+expand was removed), so a correct
    // resolver returns None rather than folding to FocusWorktree.
    let km = default_keymap();
    let alt_1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT);
    let alt_shift_1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&alt_1, KeyContext::Global),
        Some(Action::FocusWorktree)
    );
    assert_eq!(km.resolve(&alt_shift_1, KeyContext::Global), None);
}

#[test]
fn enter_and_shift_enter_distinct_in_diff_mode() {
    // SHIFT discrimination on a named key, in one layer.
    let km = default_keymap();
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
    let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&enter, KeyContext::ViewerDiffMode),
        Some(Action::ExpandContext)
    );
    assert_eq!(
        km.resolve(&shift_enter, KeyContext::ViewerDiffMode),
        Some(Action::ExpandAllContext)
    );
}

#[test]
fn keys_for_action_uses_lowercase_canonical_form() {
    // The help screen renders these verbatim; pin the casing that changed
    // from the old "Ctrl+d" to keymap-core's canonical "ctrl+d".
    let km = default_keymap();
    let keys = km.keys_for_action(KeyContext::Viewer, Action::ScrollHalfPageDown);
    assert_eq!(keys, vec!["ctrl+d".to_string()]);
}

#[test]
fn unmappable_key_event_passes_through() {
    // A key with no neutral representation (CapsLock) fails KeyInput::try_from
    // and must resolve to None ("pass through"), never panic.
    let km = default_keymap();
    let key = KeyEvent::new(KeyCode::CapsLock, KeyModifiers::empty());
    assert_eq!(km.resolve(&key, KeyContext::Terminal), None);
}

#[test]
fn removed_lsp_actions_no_longer_parse() {
    // Unwired actions were dropped from the vocabulary so binding them
    // warns (UnknownAction) instead of silently doing nothing.
    assert_eq!(Action::from_name("go_to_definition"), None);
    assert_eq!(Action::from_name("go_to_implementation"), None);
    assert_eq!(Action::from_name("find_references"), None);
}

#[test]
fn f_keys_are_unbound_after_cleanup() {
    let km = default_keymap();
    for n in 2..=7 {
        let key = KeyEvent::new(KeyCode::F(n), KeyModifiers::empty());
        assert_eq!(km.resolve(&key, KeyContext::Global), None, "F{n}");
    }
}
