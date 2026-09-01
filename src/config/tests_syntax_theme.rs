//! syntect_theme_for のダーク/ライトテーマ対応、全テーマの網羅、着色範囲、
//! および未知テーマ・theme ファイル不在時のフォールバック挙動のテスト。

use super::*;

/// syntect テーマの背景輝度の推定値 (0〜255)。
fn theme_bg_luma(theme: &syntect::highlighting::Theme) -> f32 {
    theme
        .settings
        .background
        .map(|c| 0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32)
        .unwrap_or(0.0)
}

fn cfg_with(theme: &str) -> Config {
    Config {
        ui: UiConfig {
            theme: Some(theme.to_string()),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// UI テーマの明暗と、割り当てた syntect テーマの明暗が一致すること。
///
/// [Theme::all_names] を回しているので、新しいテーマを足せば自動でここに乗る。
#[test]
fn 全テーマが同じ明暗のsyntectテーマに対応する() {
    let ts = two_face::theme::extra();
    for name in crate::theme::Theme::all_names() {
        let luma = theme_bg_luma(&syntect_theme_for(&cfg_with(name), &ts));
        let expected_light = crate::theme::Theme::from_name(name).light;
        assert_eq!(
            luma >= 128.0,
            expected_light,
            "'{name}' の明暗が syntect 側と食い違う (luma={luma:.0})"
        );
    }
}

/// syntect_theme_for: 組み込み UI テーマの全部が、他と重複しない自前の
/// syntect テーマを持つこと。
///
/// 対応表に漏れがあると、そのテーマだけ既定のシンタックス配色にフォールバック
/// して「テーマを切り替えてもコードの色が変わらない」ように見える。ここでは
/// 全テーマ分の解決結果を集め、重複が過度に無いこと(=別テーマがまとめて同じ
/// 配色に落ちていないこと)を確かめる。
#[test]
fn 組み込みテーマは全部が自前のsyntectテーマを持つ() {
    let ts = two_face::theme::extra();
    let names = crate::theme::Theme::all_names();

    let resolved: Vec<String> = names
        .iter()
        .map(|n| {
            syntect_theme_for(&cfg_with(n), &ts)
                .name
                .clone()
                .unwrap_or_default()
        })
        .collect();

    // 全 11 テーマが 8 種類以上の異なる syntect テーマに散ること。
    // (tokyo-night/rose-pine/kanagawa は同名の組み込みが無いため代用を共有
    // しうるが、それ以外がまとめて既定に落ちていれば必ずここで落ちる)
    let distinct: std::collections::HashSet<&String> = resolved.iter().collect();
    assert!(
        distinct.len() >= 8,
        "builtin themes collapse onto too few syntect themes: {resolved:?}"
    );

    // 既定へのフォールバック(Catppuccin Mocha)を共有してよいのは
    // catppuccin-mocha 自身だけ。
    let mocha_users: Vec<&str> = names
        .iter()
        .zip(&resolved)
        .filter(|(_, r)| r.as_str() == "Catppuccin Mocha")
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(
        mocha_users,
        vec!["catppuccin-mocha"],
        "themes other than catppuccin-mocha are silently falling back to the default"
    );
}

/// 以前使っていた syntect 同梱の base16-* 系はスコープ規則が 47〜49 件しか
/// なく、TypeScript ではトークンのおよそ半分が無着色のデフォルト前景のまま
/// 残っていた。two-face のテーマに寄せたことで着色範囲が広がっているのを
/// 固定する。
#[test]
fn 主要言語で十分な割合のトークンに色が付く() {
    use syntect::easy::HighlightLines;
    use syntect::util::LinesWithEndings;

    let ss = two_face::syntax::extra_newlines();
    let ts = two_face::theme::extra();

    let samples: &[(&str, &str)] = &[
        (
            "ts",
            "import {a} from 'b';\n\
             export interface P { id: number; name?: string }\n\
             // note\n\
             const f = async (x: P): Promise<void> => { await a(x.id); };\n",
        ),
        (
            "py",
            "import os\n\
             class A(Base):\n    \
             X = 3\n    \
             def f(self, n: int = 1) -> str:\n        \
             # note\n        \
             return f'{n}' if n > 0 else os.sep\n",
        ),
        (
            "go",
            "package main\n\
             import \"fmt\"\n\
             type T struct{ N int }\n\
             func (t *T) M() error { fmt.Println(t.N); return nil }\n",
        ),
    ];

    for theme_name in ["catppuccin-mocha", "dracula", "nord", "catppuccin-latte"] {
        let theme = syntect_theme_for(&cfg_with(theme_name), &ts);
        let default_fg = theme.settings.foreground.expect("theme has a foreground");

        for (ext, src) in samples {
            let syn = ss
                .find_syntax_by_extension(ext)
                .unwrap_or_else(|| panic!("syntax for .{ext} must be registered"));
            let mut h = HighlightLines::new(syn, &theme);
            let (mut total, mut colored) = (0usize, 0usize);

            for line in LinesWithEndings::from(src) {
                for (style, text) in h.highlight_line(line, &ss).expect("highlight succeeds") {
                    let n = text.trim().chars().count();
                    if n == 0 {
                        continue;
                    }
                    total += n;
                    let f = style.foreground;
                    if (f.r, f.g, f.b) != (default_fg.r, default_fg.g, default_fg.b) {
                        colored += n;
                    }
                }
            }

            let pct = colored as f64 * 100.0 / total.max(1) as f64;
            assert!(
                pct >= 65.0,
                "{theme_name} colors only {pct:.0}% of .{ext} tokens (base16 baseline was ~51%)"
            );
        }
    }
}

/// syntect_theme_for: ui.theme が viewer.theme より優先されること。テーマピッカーは
/// ui.theme に書き込むので、ここがずれると UI の配色だけが変わってコードの配色が残る。
#[test]
fn ui_themeはviewer_themeより優先される() {
    let ts = two_face::theme::extra();
    let cfg = Config {
        ui: UiConfig {
            theme: Some(String::from("github-light")),
            ..Default::default()
        },
        viewer: ViewerConfig {
            theme: String::from("catppuccin-mocha"),
            ..Default::default()
        },
        ..Default::default()
    };

    // ui.theme (light) が勝つので、明るい syntect テーマが返るはず。
    assert!(
        theme_bg_luma(&syntect_theme_for(&cfg, &ts)) >= 128.0,
        "ui.theme must win over viewer.theme"
    );
}

/// syntect_theme_for: ui.theme 未設定なら viewer.theme に後方互換で落ちること。
#[test]
fn ui_themeが無ければviewer_themeへ落ちる() {
    let ts = two_face::theme::extra();
    let cfg = Config {
        ui: UiConfig::default(),
        viewer: ViewerConfig {
            theme: String::from("github-light"),
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(theme_bg_luma(&syntect_theme_for(&cfg, &ts)) >= 128.0);
}

/// syntax_theme_id: テーマが変われば別の指紋になり、同じなら同じになること。
#[test]
fn 構文テーマのidはテーマ名とファイルを追う() {
    let a = cfg_with("dracula");
    let b = cfg_with("nord");
    assert_ne!(syntax_theme_id(&a), syntax_theme_id(&b));
    assert_eq!(syntax_theme_id(&a), syntax_theme_id(&cfg_with("dracula")));

    // ファイル指定はテーマ名に関わらずパスで決まる。
    let with_file = |theme: &str| Config {
        ui: UiConfig {
            theme: Some(theme.to_string()),
            ..Default::default()
        },
        viewer: ViewerConfig {
            syntax_theme_file: Some(String::from("/tmp/x.tmTheme")),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        syntax_theme_id(&with_file("dracula")),
        syntax_theme_id(&with_file("nord"))
    );
    assert_ne!(syntax_theme_id(&with_file("dracula")), syntax_theme_id(&a));
}

/// syntect_theme_for: 未知テーマ名はパニックせず既定へフォールバック。
#[test]
fn 知らないテーマ名でも落ちずに落ち着く() {
    let ts = two_face::theme::extra();
    let _ = syntect_theme_for(&cfg_with("nonexistent-theme-xyz"), &ts); // パニックしないこと
}

/// syntect_theme_for: 存在しないパスの syntax_theme_file はパニックしないこと。
#[test]
fn テーマファイルが無くても落ちずに落ち着く() {
    let ts = two_face::theme::extra();
    let cfg = Config {
        viewer: ViewerConfig {
            theme: String::from("catppuccin-mocha"),
            syntax_theme_file: Some(String::from("/nonexistent/path/theme.tmTheme")),
            ..Default::default()
        },
        ..Default::default()
    };
    let _ = syntect_theme_for(&cfg, &ts); // パニックしないこと
}
