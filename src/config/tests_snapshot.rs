//! AppearanceSnapshot / adopt_appearance / has_restart_changes の不変条件の
//! テスト: すべてのフィールドは live で再読込可能か restart が必要かの
//! どちらかに必ず属し、どちらでもないことは許されない。

use std::path::PathBuf;

use super::*;

#[test]
fn appearance_snapshot_includes_layout() {
    let mut cfg = Config::default();
    cfg.layout.explorer_width_pct = 30;
    let snap = cfg.appearance_snapshot();
    assert_eq!(snap.layout_explorer_width_pct, 30);
    assert_eq!(snap.layout_viewer_width_pct, 38);
    assert_eq!(snap.layout_terminal_split_pct, 80);
}

// adopt_appearance / appearance_snapshot の不変条件 / has_restart_changes

/// 往復不変条件: adopt_appearance 後に snapshot が new と一致すること。
/// AppearanceSnapshot にフィールドを足したのに adopt_appearance のコピーに
/// 足し忘れた場合を検出する。
#[test]
fn adopt_appearance_round_trip_invariant() {
    let mut cur = Config::default();
    let mut new = Config::default();
    // すべての live フィールドをデフォルトでない値に変更する。
    new.ui.theme = Some(String::from("dracula"));
    new.viewer.theme = String::from("dracula");
    new.viewer.syntax_theme_file = Some(String::from("/tmp/custom.tmTheme"));
    new.viewer.tab_width = 4; // デフォルトは2
    new.viewer.word_wrap = true; // デフォルトは false
    new.diff.word_diff = false; // デフォルトは true
    new.diff.default_view = DiffView::SideBySide; // デフォルトは Unified
    new.general.decoration = String::from("space");
    new.layout.explorer_width_pct = 30;
    new.layout.viewer_width_pct = 42;
    new.layout.terminal_split_pct = 70;

    cur.adopt_appearance(&new);

    assert_eq!(
        cur.appearance_snapshot(),
        new.appearance_snapshot(),
        "adopt_appearance must copy all snapshot-tracked live fields"
    );
}

/// snapshot 等価: 同一 config は等価。
#[test]
fn appearance_snapshot_equal_for_identical_configs() {
    let cfg = Config::default();
    assert_eq!(cfg.appearance_snapshot(), cfg.appearance_snapshot());
}

/// snapshot 不等価: 各 live フィールドを 1 つ変えると != になること。
#[test]
fn appearance_snapshot_detects_each_live_field_change() {
    let base = Config::default();

    let mut c = base.clone();
    c.ui.theme = Some(String::from("dracula"));
    assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "ui.theme");

    let mut c = base.clone();
    c.viewer.theme = String::from("nord");
    assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "viewer.theme");

    let mut c = base.clone();
    c.viewer.syntax_theme_file = Some(String::from("/custom.tmTheme"));
    assert_ne!(
        c.appearance_snapshot(),
        base.appearance_snapshot(),
        "viewer.syntax_theme_file"
    );

    let mut c = base.clone();
    c.viewer.tab_width = 4; // デフォルトは2
    assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "viewer.tab_width");

    let mut c = base.clone();
    c.diff.word_diff = false; // デフォルトは true
    assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "diff.word_diff");

    let mut c = base.clone();
    c.diff.default_view = DiffView::SideBySide; // デフォルトは Unified
    assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "diff.default_view");

    let mut c = base.clone();
    c.general.decoration = String::from("space");
    assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "general.decoration");

    let mut c = base.clone();
    c.layout.explorer_width_pct = 30;
    assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "layout.explorer_width_pct");

    let mut c = base.clone();
    c.layout.viewer_width_pct = 42;
    assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "layout.viewer_width_pct");

    let mut c = base.clone();
    c.layout.terminal_split_pct = 70;
    assert_ne!(c.appearance_snapshot(), base.appearance_snapshot(), "layout.terminal_split_pct");
}

/// has_restart_changes: live フィールドのみ変えたら false。
#[test]
fn has_restart_changes_false_for_live_only_diff() {
    let old = Config::default();
    let mut new = Config::default();
    new.ui.theme = Some(String::from("dracula"));
    new.viewer.theme = String::from("nord");
    new.viewer.tab_width = 4; // デフォルトは2
    new.diff.word_diff = false; // デフォルトは true
    new.general.decoration = String::from("space");
    new.layout.explorer_width_pct = 30;
    assert!(!has_restart_changes(&old, &new));
}

/// has_restart_changes: 各 restart フィールドを 1 つ変えたら true。
#[test]
fn has_restart_changes_true_for_each_restart_field() {
    let base = Config::default();

    let mut c = base.clone();
    c.general.shell = String::from("/bin/fish");
    assert!(has_restart_changes(&base, &c), "general.shell");

    let mut c = base.clone();
    c.general.main_branch = String::from("master");
    assert!(has_restart_changes(&base, &c), "general.main_branch");

    let mut c = base.clone();
    c.general.repo = Some(PathBuf::from("/other/repo"));
    assert!(has_restart_changes(&base, &c), "general.repo");

    let mut c = base.clone();
    c.general.auto_resume = false; // デフォルトは true
    assert!(has_restart_changes(&base, &c), "general.auto_resume");

    let mut c = base.clone();
    c.general.auto_resume_main = true;
    assert!(has_restart_changes(&base, &c), "general.auto_resume_main");

    let mut c = base.clone();
    c.terminal.active_scrollback = 99999;
    assert!(has_restart_changes(&base, &c), "terminal.active_scrollback");

    let mut c = base.clone();
    c.api.provider = String::from("claude"); // デフォルトは "gemini"
    assert!(has_restart_changes(&base, &c), "api.provider");

    let mut c = base.clone();
    c.ccusage.enabled = true;
    assert!(has_restart_changes(&base, &c), "ccusage.enabled");
}

/// 分割テスト: すべてのフィールドが live か restart のどちらかに必ず属する。
/// フィールドを 1 つ変えた new で snapshot != か has_restart_changes が必ず true になること。
#[test]
fn every_field_is_either_live_or_restart() {
    let base = Config::default();

    // general
    {
        let mut c = base.clone();
        c.general.repo = Some(PathBuf::from("/p"));
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.repo");
    }
    {
        let mut c = base.clone();
        c.general.main_branch = String::from("master");
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.main_branch");
    }
    {
        let mut c = base.clone();
        c.general.shell = String::from("/bin/fish");
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.shell");
    }
    {
        let mut c = base.clone();
        c.general.repos = vec![PathBuf::from("/p")];
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.repos");
    }
    {
        let mut c = base.clone();
        c.general.worktree_dir = Some(PathBuf::from("/wt"));
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.worktree_dir");
    }
    {
        let mut c = base.clone();
        c.general.decoration = String::from("space");
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.decoration");
    }
    {
        let mut c = base.clone();
        c.general.auto_resume = false; // デフォルトは true
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.auto_resume");
    }
    {
        let mut c = base.clone();
        c.general.auto_resume_main = true;
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "general.auto_resume_main");
    }
    // terminal
    {
        let mut c = base.clone();
        c.terminal.active_scrollback = 9999;
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "terminal.active_scrollback");
    }
    // viewer (live)
    {
        let mut c = base.clone();
        c.viewer.theme = String::from("nord");
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "viewer.theme");
    }
    {
        let mut c = base.clone();
        c.viewer.tab_width = 4; // デフォルトは2
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "viewer.tab_width");
    }
    // diff (live)
    {
        let mut c = base.clone();
        c.diff.word_diff = false; // デフォルトは true
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "diff.word_diff");
    }
    // api
    {
        let mut c = base.clone();
        c.api.provider = String::from("claude"); // デフォルトは "gemini"
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "api.provider");
    }
    // ccusage
    {
        let mut c = base.clone();
        c.ccusage.enabled = true;
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "ccusage.enabled");
    }
    // layout (live)
    {
        let mut c = base.clone();
        c.layout.explorer_width_pct = 30;
        assert!(c.appearance_snapshot() != base.appearance_snapshot() || has_restart_changes(&base, &c), "layout.explorer_width_pct");
    }
}
