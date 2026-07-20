//! Tests for the user `[keybinds]` overlay: additive overrides, chord
//! tombstones, unknown actions/layers, legacy config format detection, and
//! in-layer chord conflicts.

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn user_override_adds_a_chord() {
    // Bind "n" -> navigate_down in the worktree layer.
    let mut layer = toml::Table::new();
    layer.insert(
        "n".to_string(),
        toml::Value::String("navigate_down".to_string()),
    );
    let mut layers = toml::Table::new();
    layers.insert("worktree".to_string(), toml::Value::Table(layer));
    let mut user = toml::Table::new();
    user.insert("layers".to_string(), toml::Value::Table(layers));

    let (km, warnings) = KeyMap::with_warnings(&user);
    assert!(warnings.is_empty(), "{warnings:?}");

    // 'n' now navigates down …
    let key_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_n, KeyContext::Worktree),
        Some(Action::NavigateDown)
    );
    // … and the default 'j' still works (layering, not replacement).
    let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_j, KeyContext::Worktree),
        Some(Action::NavigateDown)
    );
}

#[test]
fn user_override_shadows_a_default_chord() {
    // Rebind "g" -> go_to_top in worktree (default is grab_branch).
    let mut layer = toml::Table::new();
    layer.insert("g".to_string(), toml::Value::String("go_to_top".to_string()));
    let mut layers = toml::Table::new();
    layers.insert("worktree".to_string(), toml::Value::Table(layer));
    let mut user = toml::Table::new();
    user.insert("layers".to_string(), toml::Value::Table(layers));

    let km = KeyMap::new(&user);
    let key_g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_g, KeyContext::Worktree),
        Some(Action::GoToTop)
    );
}

#[test]
fn user_tombstone_unbinds_a_default_chord() {
    // `"ctrl+q" = false` removes the default Quit binding outright (the
    // keymap-suite `merge` tombstone), so the chord passes through instead of
    // being shadowed by another action. This is a no-op warning-wise.
    let mut keys = toml::Table::new();
    keys.insert("ctrl+q".to_string(), toml::Value::Boolean(false));
    let mut user = toml::Table::new();
    user.insert("keys".to_string(), toml::Value::Table(keys));

    let (km, warnings) = KeyMap::with_warnings(&user);
    assert!(warnings.is_empty(), "{warnings:?}");

    let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(km.resolve(&ctrl_q, KeyContext::Global), None);
    // A default the tombstone did not touch still resolves.
    let ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&ctrl_n, KeyContext::Global),
        Some(Action::NewClaudeCode)
    );
}

#[test]
fn user_tombstone_in_panel_layer_unbinds() {
    // Tombstones work in a named layer too: drop worktree 'c' (cherry-pick).
    let mut layer = toml::Table::new();
    layer.insert("c".to_string(), toml::Value::Boolean(false));
    let mut layers = toml::Table::new();
    layers.insert("worktree".to_string(), toml::Value::Table(layer));
    let mut user = toml::Table::new();
    user.insert("layers".to_string(), toml::Value::Table(layers));

    let (km, warnings) = KeyMap::with_warnings(&user);
    assert!(warnings.is_empty(), "{warnings:?}");
    let key_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty());
    assert_eq!(km.resolve(&key_c, KeyContext::Worktree), None);
}

#[test]
fn user_unknown_action_is_warned() {
    let mut keys = toml::Table::new();
    keys.insert(
        "ctrl+z".to_string(),
        toml::Value::String("frobnicate".to_string()),
    );
    let mut user = toml::Table::new();
    user.insert("keys".to_string(), toml::Value::Table(keys));

    let (_km, warnings) = KeyMap::with_warnings(&user);
    assert!(
        warnings.iter().any(|w| matches!(
            w,
            KeybindWarning::UnknownAction { action, .. } if action == "frobnicate"
        )),
        "expected UnknownAction, got {warnings:?}"
    );
}

#[test]
fn legacy_format_is_reported_not_silent() {
    // Old schema: [keybinds.worktree] navigate_down = "j" — a top-level
    // table named "worktree" rather than "keys"/"layers".
    let mut wt = toml::Table::new();
    wt.insert(
        "navigate_down".to_string(),
        toml::Value::String("j".to_string()),
    );
    let mut user = toml::Table::new();
    user.insert("worktree".to_string(), toml::Value::Table(wt));

    let (_km, warnings) = KeyMap::with_warnings(&user);
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, KeybindWarning::InvalidConfig { .. })),
        "expected InvalidConfig, got {warnings:?}"
    );
}

#[test]
fn in_layer_conflict_is_warned() {
    // Two spellings of the same chord in one layer: keymap-config reports a
    // Conflict and the last binding wins.
    let mut keys = toml::Table::new();
    keys.insert("ctrl+x".to_string(), toml::Value::String("quit".to_string()));
    keys.insert(
        "control+x".to_string(),
        toml::Value::String("show_help".to_string()),
    );
    let mut user = toml::Table::new();
    user.insert("keys".to_string(), toml::Value::Table(keys));

    let (km, warnings) = KeyMap::with_warnings(&user);
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, KeybindWarning::Conflict { .. })),
        "expected Conflict, got {warnings:?}"
    );
    // Whichever won, ctrl+x must resolve to one of the two contenders.
    let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
    let resolved = km.resolve(&key, KeyContext::Global);
    assert!(
        matches!(resolved, Some(Action::Quit) | Some(Action::ShowHelp)),
        "got {resolved:?}"
    );
}

#[test]
fn unknown_layer_with_bindings_is_warned() {
    // Guards the empty-GLOBAL_LAYER suppression: a non-empty unrecognized
    // layer name must warn (an empty one, always injected, must not).
    let mut layer = toml::Table::new();
    layer.insert(
        "j".to_string(),
        toml::Value::String("navigate_down".to_string()),
    );
    let mut layers = toml::Table::new();
    layers.insert("bogus".to_string(), toml::Value::Table(layer));
    let mut user = toml::Table::new();
    user.insert("layers".to_string(), toml::Value::Table(layers));

    let (_km, warnings) = KeyMap::with_warnings(&user);
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, KeybindWarning::UnknownLayer { layer } if layer == "bogus")),
        "expected UnknownLayer, got {warnings:?}"
    );
}
