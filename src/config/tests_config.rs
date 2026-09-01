//! Config とその各セクション構造体のテスト: TOML の往復変換とデフォルト値。

use std::path::PathBuf;

use super::*;

#[test]
fn 既定のconfigはtomlを往復する() {
    let cfg = Config::default();
    let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
    let cfg2: Config = toml::from_str(&toml_str).expect("deserialize");

    assert_eq!(cfg2.general.main_branch, "main");
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
fn 空のtomlは既定値になる() {
    let cfg: Config = toml::from_str("").expect("empty toml");
    assert_eq!(cfg.general.main_branch, "main");
    assert_eq!(cfg.diff.default_view, DiffView::Unified);
}

#[test]
fn diff_viewのserde() {
    let cfg: DiffConfig = toml::from_str(r#"default_view = "side-by-side""#).expect("parse");
    assert_eq!(cfg.default_view, DiffView::SideBySide);
}

/// 生成される config.toml すべてに書き込まれていたキーなので、削除しても既存インストールを
/// 壊してはならない。
#[test]
fn 削除したreview_promptの鍵は無視される() {
    // walkthrough_language も、revidere へ移った今は「知らない鍵」の側にいる。
    let _cfg: ReviewConfig = toml::from_str(
        "prompt_template = \"…{comments}\"\nprompt_action = \"send_to_session\"\nwalkthrough_language = \"日本語\"\n",
    )
    .expect("stale keys should be ignored, not rejected");
}

/// 生成される config.toml すべてに書き込まれていたので、rich mode を廃止しても既存
/// インストールの起動を壊してはならない。
#[test]
fn 削除したrichセクションは無視される() {
    let cfg: Config =
        toml::from_str("[general]\nmain_branch = \"develop\"\n\n[rich]\nmode = \"force\"\n")
            .expect("stale [rich] section should be ignored, not rejected");
    // 同じファイル内の他のセクションは通常どおり読めていること。
    assert_eq!(cfg.general.main_branch, "develop");
}

#[test]
fn チルダの展開() {
    let p = PathBuf::from("~/dev/project");
    let expanded = super::persist::expand_tilde(&p);
    assert!(!expanded.to_string_lossy().starts_with('~'));
}

#[test]
fn ccusageの設定を読む() {
    let cfg: CcusageConfig = toml::from_str(
        r#"enabled = true
poll_interval_secs = 60"#,
    )
    .expect("parse");
    assert!(cfg.enabled);
    assert_eq!(cfg.poll_interval_secs, 60);
}

#[test]
fn updatesの設定を読む() {
    let cfg: UpdatesConfig = toml::from_str(
        r#"check_on_startup = false
check_interval_secs = 3600"#,
    )
    .expect("parse");
    assert!(!cfg.check_on_startup);
    assert_eq!(cfg.check_interval_secs, 3600);
}

#[test]
fn keybindsを読む() {
    // [keybinds] セクションは生のテーブル(key→action のスキーマ)として取り込み、
    // パースを担う keymap::KeyMap に渡す。
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
fn 生成した既定のconfigは妥当なtoml() {
    let content = generate_default_config();
    let cfg: Config = toml::from_str(&content).expect("generated config must be valid TOML");
    // 全部コメントアウトされているので、値はすべてデフォルトと一致するはず。
    assert_eq!(cfg.general.main_branch, "main");
    assert_eq!(cfg.terminal.inactive_scrollback, 1000);
    assert_eq!(cfg.viewer.tab_width, 2);
    assert!(cfg.updates.check_on_startup);
}

#[test]
fn high_contrastは既定offでtomlを往復する() {
    let cfg = Config::default();
    assert!(!cfg.ui.high_contrast, "high_contrast must default to false");

    let toml_str = "[ui]\nhigh_contrast = true\n";
    let cfg: Config = toml::from_str(toml_str).expect("parse");
    assert!(cfg.ui.high_contrast);

    // high_contrast は即時反映される見た目のフィールドなので、切り替えは
    // スナップショットに現れなければならない (再起動が要る変更として扱ってはいけない)。
    let base = Config::default();
    assert_ne!(cfg.appearance_snapshot(), base.appearance_snapshot());
    assert!(!has_restart_changes(&base, &cfg));
}

#[test]
fn uiセクションはtomlを往復する() {
    let toml_str = r#"[ui]
theme = "catppuccin-latte"
"#;
    let cfg: Config = toml::from_str(toml_str).expect("parse");
    assert_eq!(cfg.ui.theme.as_deref(), Some("catppuccin-latte"));

    // もう一度シリアライズしてデシリアライズする。
    let serialized = toml::to_string_pretty(&cfg).expect("serialize");
    let cfg2: Config = toml::from_str(&serialized).expect("round-trip");
    assert_eq!(cfg2.ui.theme.as_deref(), Some("catppuccin-latte"));
}

#[test]
fn layoutセクションはtomlを往復する() {
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
fn 空のtomlでもlayoutは既定値になる() {
    let cfg: Config = toml::from_str("").expect("empty toml");
    assert_eq!(cfg.layout.explorer_width_pct, 24);
    assert_eq!(cfg.layout.viewer_width_pct, 38);
    assert_eq!(cfg.layout.terminal_split_pct, 80);
}
