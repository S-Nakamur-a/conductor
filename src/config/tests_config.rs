//! Tests for `Config` and its section structs: TOML round-tripping and defaults.

use std::path::PathBuf;

use super::*;

#[test]
fn default_config_round_trips_through_toml() {
    let cfg = Config::default();
    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    let cfg2: Config = toml::from_str(&toml_str).expect("deserialize");

    assert_eq!(cfg2.general.main_branch, "main");
    assert_eq!(cfg2.general.decoration, "aquarium");
    assert_eq!(cfg2.terminal.inactive_scrollback, 1000);
    assert_eq!(cfg2.terminal.active_scrollback, 10000);
    assert_eq!(cfg2.viewer.theme, "catppuccin-mocha");
    assert_eq!(cfg2.viewer.tab_width, 2);
    assert!(!cfg2.viewer.word_wrap);
    assert_eq!(cfg2.diff.default_view, DiffView::Unified);
    assert!(cfg2.diff.word_diff);
    assert!(!cfg2.ccusage.enabled);
    assert_eq!(cfg2.ccusage.poll_interval_secs, 120);
    assert!(cfg2.updates.check_on_startup);
    assert_eq!(cfg2.updates.check_interval_secs, 3600);
}

#[test]
fn empty_toml_gives_defaults() {
    let cfg: Config = toml::from_str("").expect("empty toml");
    assert_eq!(cfg.general.main_branch, "main");
    assert_eq!(cfg.diff.default_view, DiffView::Unified);
}

#[test]
fn diff_view_serde() {
    let cfg: DiffConfig = toml::from_str(r#"default_view = "side-by-side""#).expect("parse");
    assert_eq!(cfg.default_view, DiffView::SideBySide);
}

/// A config file still carrying the removed review-prompt keys must load, not
/// fail: they were written into every generated `config.toml` and dropping
/// them cannot break an existing install.
#[test]
fn removed_review_prompt_keys_are_ignored() {
    let cfg: ReviewConfig = toml::from_str(
        "prompt_template = \"…{comments}\"\nprompt_action = \"send_to_session\"\n",
    )
    .expect("stale keys should be ignored, not rejected");
    assert!(cfg.walkthrough_language.is_none());
}

#[test]
fn tilde_expansion() {
    let p = PathBuf::from("~/dev/project");
    let expanded = super::persist::expand_tilde(&p);
    assert!(!expanded.to_string_lossy().starts_with('~'));
}

#[test]
fn ccusage_config_parse() {
    let cfg: CcusageConfig = toml::from_str(
        r#"enabled = true
poll_interval_secs = 60"#,
    )
    .expect("parse");
    assert!(cfg.enabled);
    assert_eq!(cfg.poll_interval_secs, 60);
}

#[test]
fn updates_config_parse() {
    let cfg: UpdatesConfig = toml::from_str(
        r#"check_on_startup = false
check_interval_secs = 3600"#,
    )
    .expect("parse");
    assert!(!cfg.check_on_startup);
    assert_eq!(cfg.check_interval_secs, 3600);
}

#[test]
fn keybinds_parse() {
    // The [keybinds] section is captured as a raw table (key→action schema)
    // and handed to keymap::KeyMap, which owns parsing.
    let toml_str = r#"
[keybinds.keys]
"ctrl+q" = "quit"

[keybinds.layers.worktree]
"j" = "navigate_down"
"w" = "create_worktree"
"#;
    let cfg: Config = toml::from_str(toml_str).expect("parse config");
    let keys = cfg.keybinds.get("keys").and_then(|v| v.as_table()).unwrap();
    assert_eq!(keys.get("ctrl+q").and_then(|v| v.as_str()), Some("quit"));

    let worktree = cfg
        .keybinds
        .get("layers")
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("worktree"))
        .and_then(|v| v.as_table())
        .unwrap();
    assert_eq!(
        worktree.get("j").and_then(|v| v.as_str()),
        Some("navigate_down")
    );
}

#[test]
fn generated_default_config_is_valid_toml() {
    let content = generate_default_config();
    let cfg: Config = toml::from_str(&content).expect("generated config must be valid TOML");
    // All values should match defaults since everything is commented out.
    assert_eq!(cfg.general.main_branch, "main");
    assert_eq!(cfg.terminal.inactive_scrollback, 1000);
    assert_eq!(cfg.viewer.tab_width, 2);
    assert!(cfg.updates.check_on_startup);
}

#[test]
fn ui_config_default_has_no_theme() {
    let cfg = Config::default();
    assert!(cfg.ui.theme.is_none());
}

#[test]
fn ui_config_high_contrast_defaults_off_and_round_trips() {
    let cfg = Config::default();
    assert!(!cfg.ui.high_contrast, "high_contrast must default to false");

    let toml_str = "[ui]\nhigh_contrast = true\n";
    let cfg: Config = toml::from_str(toml_str).expect("parse");
    assert!(cfg.ui.high_contrast);

    // high_contrast is a live appearance field, so flipping it must register
    // in the snapshot (and never as a restart change).
    let base = Config::default();
    assert_ne!(cfg.appearance_snapshot(), base.appearance_snapshot());
    assert!(!has_restart_changes(&base, &cfg));
}

#[test]
fn ui_config_round_trips_through_toml() {
    let toml_str = r#"[ui]
theme = "catppuccin-latte"
"#;
    let cfg: Config = toml::from_str(toml_str).expect("parse");
    assert_eq!(cfg.ui.theme.as_deref(), Some("catppuccin-latte"));

    // Serialize and deserialize again.
    let serialized = toml::to_string_pretty(&cfg).expect("serialize");
    let cfg2: Config = toml::from_str(&serialized).expect("round-trip");
    assert_eq!(cfg2.ui.theme.as_deref(), Some("catppuccin-latte"));
}

#[test]
fn layout_config_defaults() {
    let cfg = LayoutConfig::default();
    assert_eq!(cfg.explorer_width_pct, 24);
    assert_eq!(cfg.viewer_width_pct, 38);
    assert_eq!(cfg.terminal_split_pct, 80);
}

#[test]
fn layout_config_round_trips_through_toml() {
    let toml_str = r#"[layout]
explorer_width_pct = 30
viewer_width_pct = 40
terminal_split_pct = 75
"#;
    let cfg: Config = toml::from_str(toml_str).expect("parse");
    assert_eq!(cfg.layout.explorer_width_pct, 30);
    assert_eq!(cfg.layout.viewer_width_pct, 40);
    assert_eq!(cfg.layout.terminal_split_pct, 75);

    let serialized = toml::to_string_pretty(&cfg).expect("serialize");
    let cfg2: Config = toml::from_str(&serialized).expect("round-trip");
    assert_eq!(cfg2.layout.explorer_width_pct, 30);
    assert_eq!(cfg2.layout.viewer_width_pct, 40);
    assert_eq!(cfg2.layout.terminal_split_pct, 75);
}

#[test]
fn layout_config_empty_toml_gives_defaults() {
    let cfg: Config = toml::from_str("").expect("empty toml");
    assert_eq!(cfg.layout.explorer_width_pct, 24);
    assert_eq!(cfg.layout.viewer_width_pct, 38);
    assert_eq!(cfg.layout.terminal_split_pct, 80);
}
