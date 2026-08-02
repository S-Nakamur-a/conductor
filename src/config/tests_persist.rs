//! コメントを保持したまま編集する upsert_section_kv/upsert_ui_theme と
//! is_section_header のテスト。

use super::*;
use super::persist::{is_section_header, upsert_section_kv, upsert_ui_theme};

#[test]
fn upsert_high_contrast_inserts_into_ui_section() {
    let contents = "[ui]\ntheme = \"nord\"\n";
    let result = upsert_section_kv(contents, "ui", "high_contrast", "true");
    assert!(result.contains("high_contrast = true"));
    assert!(result.contains("theme = \"nord\""));
    let cfg: Config = toml::from_str(&result).expect("valid toml");
    assert!(cfg.ui.high_contrast);
}

#[test]
fn upsert_ui_theme_appends_when_no_ui_section() {
    let contents = "[general]\nmain_branch = \"main\"\n";
    let result = upsert_ui_theme(contents, "nord");
    assert!(result.contains("[ui]"));
    assert!(result.contains("theme = \"nord\""));
    // 元の内容が保持されていること。
    assert!(result.contains("[general]"));
}

#[test]
fn upsert_section_kv_inserts_layout_value_over_commented_default() {
    // 生成される config は layout のキーをコメントアウトした状態で出荷される。
    // リサイズ時はヘッダの後にコメントを残したまま実値を挿入しなければならない。
    let contents = "[layout]\n# explorer_width_pct = 24    # default\n";
    let result = upsert_section_kv(contents, "layout", "explorer_width_pct", "30");
    assert!(result.contains("explorer_width_pct = 30"));
    assert!(result.contains("# explorer_width_pct = 24"));
}

#[test]
fn upsert_section_kv_replaces_existing_layout_value() {
    let contents = "[layout]\nexplorer_width_pct = 24\nviewer_width_pct = 38\n";
    let result = upsert_section_kv(contents, "layout", "viewer_width_pct", "42");
    assert_eq!(result, "[layout]\nexplorer_width_pct = 24\nviewer_width_pct = 42\n");
}

#[test]
fn upsert_section_kv_chains_for_all_three_layout_keys() {
    // persist_layout_proportions を模している: 3回連続の upsert が互いを
    // 壊さずに同じ [layout] テーブルへ収まること。
    let contents = "[layout]\n# explorer_width_pct = 24\n# viewer_width_pct = 38\n# terminal_split_pct = 80\n\n[ui]\ntheme = \"nord\"\n";
    let r = upsert_section_kv(contents, "layout", "explorer_width_pct", "30");
    let r = upsert_section_kv(&r, "layout", "viewer_width_pct", "40");
    let r = upsert_section_kv(&r, "layout", "terminal_split_pct", "65");
    assert!(r.contains("explorer_width_pct = 30"));
    assert!(r.contains("viewer_width_pct = 40"));
    assert!(r.contains("terminal_split_pct = 65"));
    // 隣接するセクションは変更されない。
    assert!(r.contains("[ui]"));
    assert!(r.contains("theme = \"nord\""));
    // 新しい値を反映した状態で有効な TOML として往復すること。
    let cfg: Config = toml::from_str(&r).expect("layout edits stay valid TOML");
    assert_eq!(cfg.layout.explorer_width_pct, 30);
    assert_eq!(cfg.layout.viewer_width_pct, 40);
    assert_eq!(cfg.layout.terminal_split_pct, 65);
}

#[test]
fn upsert_ui_theme_replaces_existing_theme_line() {
    let contents = "[ui]\ntheme = \"dracula\"\n";
    let result = upsert_ui_theme(contents, "github-light");
    assert_eq!(
        result,
        "[ui]\ntheme = \"github-light\"\n",
        "existing theme line must be replaced in place"
    );
}

#[test]
fn upsert_ui_theme_inserts_after_ui_header_when_only_comments() {
    let contents = "[ui]\n# theme = \"catppuccin-mocha\"\n";
    let result = upsert_ui_theme(contents, "catppuccin-latte");
    // 新しい行は [ui] の後、コメントの前に挿入されるはず。
    assert!(result.contains("theme = \"catppuccin-latte\""));
    // コメントは保持されなければならない。
    assert!(result.contains("# theme = \"catppuccin-mocha\""));
}

#[test]
fn upsert_ui_theme_preserves_other_sections_after_ui() {
    let contents = "[viewer]\ntheme = \"dracula\"\n\n[ui]\n# theme placeholder\n\n[general]\n";
    let result = upsert_ui_theme(contents, "nord");
    assert!(result.contains("theme = \"nord\""));
    assert!(result.contains("[viewer]"));
    assert!(result.contains("[general]"));
}

#[test]
fn upsert_ui_theme_trailing_newline_preserved() {
    let with_newline = "[ui]\ntheme = \"dracula\"\n";
    let without_newline = "[ui]\ntheme = \"dracula\"";
    assert!(upsert_ui_theme(with_newline, "nord").ends_with('\n'));
    assert!(!upsert_ui_theme(without_newline, "nord").ends_with('\n'));
}

// [ui] ヘッダのインラインコメント検出。

#[test]
fn upsert_ui_theme_handles_inline_comment_on_ui_header() {
    // [ui]  # color settings も [ui] セクションとして認識されなければならない。
    let contents = "[general]\n\n[ui]  # color settings\ntheme = \"dracula\"\n";
    let result = upsert_ui_theme(contents, "nord");
    assert_eq!(
        result.matches("[ui]").count(),
        1,
        "must not append a duplicate [ui] section"
    );
    assert!(result.contains("theme = \"nord\""));
}

#[test]
fn upsert_ui_theme_does_not_match_ui_subsection() {
    // [ui.colors] は [ui] セクションではない。新しい [ui] ブロックが追記されるはず。
    let contents = "[ui.colors]\nfoo = \"bar\"\n";
    let result = upsert_ui_theme(contents, "nord");
    assert!(
        result.contains("[ui]\n"),
        "a new [ui] section should be appended, not matched"
    );
    // サブセクションは引き続き存在すること。
    assert!(result.contains("[ui.colors]"));
}

#[test]
fn is_section_header_cases() {
    assert!(is_section_header("[ui]", "ui"));
    assert!(is_section_header("[ui]  ", "ui"));
    assert!(is_section_header("[ui]  # comment", "ui"));
    assert!(is_section_header("  [ui]", "ui"));
    assert!(!is_section_header("[ui.sub]", "ui"));
    assert!(!is_section_header("[ui.colors]", "ui"));
    assert!(!is_section_header("[viewer]", "ui"));
    // セクション名に対して汎用であること。
    assert!(is_section_header("[layout]", "layout"));
    assert!(!is_section_header("[layout]", "ui"));
}
