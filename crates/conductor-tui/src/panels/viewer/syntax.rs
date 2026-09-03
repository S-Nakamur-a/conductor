//! syntect による構文ハイライトと、そのテーマ解決。
//!
//! two-face (bat 由来) のテーマを使うのは色を付けるスコープ数が段違いなため。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use conductor_core::config::Config;
use ratatui::style::{Color, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

/// syntect が知らない拡張子。値は syntect 側が知っている綴り。
const EXTENSION_ALIASES: &[(&str, &str)] = &[("jsx", "js"), ("mjs", "js"), ("cjs", "js")];

/// 対応表は [conductor_core::theme::Theme::all_names] を網羅する。
fn embedded_theme_for(theme: &str) -> EmbeddedThemeName {
    match theme {
        "catppuccin-mocha" => EmbeddedThemeName::CatppuccinMocha,
        "dracula" => EmbeddedThemeName::Dracula,
        "nord" => EmbeddedThemeName::Nord,
        "solarized-dark" => EmbeddedThemeName::SolarizedDark,
        "gruvbox" => EmbeddedThemeName::GruvboxDark,
        // 同名の組み込みが無いダークテーマは、前景アクセントの傾向が最も近いもので
        // 代用する。背景色は使わない (ハイライト結果の背景は Reset に潰している)。
        "tokyo-night" => EmbeddedThemeName::CatppuccinMacchiato,
        "rose-pine" => EmbeddedThemeName::CatppuccinFrappe,
        "kanagawa" => EmbeddedThemeName::GruvboxDark,
        "catppuccin-latte" => EmbeddedThemeName::CatppuccinLatte,
        "solarized-light" => EmbeddedThemeName::SolarizedLight,
        "github-light" => EmbeddedThemeName::InspiredGithub,
        // theme::Theme::from_name も既定テーマに落ちるので合わせる。
        _ => EmbeddedThemeName::CatppuccinMocha,
    }
}

/// 設定から syntect のテーマを解決する。
///
/// 参照するテーマ名を [Config::theme_name] に一本化しているのが要点。viewer.theme
/// だけを見ると、テーマピッカーが書き込む ui.theme とずれて、UI の配色だけが変わって
/// コードの配色が取り残される。
pub fn syntect_theme_for(cfg: &Config, themes: &EmbeddedLazyThemeSet) -> SyntectTheme {
    let builtin = || themes.get(embedded_theme_for(cfg.theme_name())).clone();
    let Some(path) = cfg.viewer.syntax_theme_file.as_ref() else {
        return builtin();
    };
    syntect::highlighting::ThemeSet::get_theme(path).unwrap_or_else(|e| {
        log::warn!("failed to load syntax theme file {path}: {e}; falling back to the built-in");
        builtin()
    })
}

/// テーマの解決結果を一意に決める入力の指紋。ハイライトのキャッシュを、テーマが
/// 実際に変わったときだけ捨てるために使う。
pub fn syntax_theme_id(cfg: &Config) -> String {
    match cfg.viewer.syntax_theme_file {
        Some(ref path) => format!("file:{path}"),
        None => format!("builtin:{}", cfg.theme_name()),
    }
}

/// 拡張子だけだと Dockerfile・Makefile・.gitignore が plain text になり、
/// CMakeLists.txt は .txt に引っかかる。ファイル名を先に見るのはそのため。
pub fn find_syntax<'a>(
    syntax_set: &'a SyntaxSet,
    path: Option<&str>,
    first_line: Option<&str>,
) -> &'a SyntaxReference {
    let path = path.map(Path::new);
    let by_name = path
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .and_then(|n| syntax_set.find_syntax_by_extension(n));
    let by_ext = || {
        let ext = path.and_then(|p| p.extension()).and_then(|e| e.to_str())?;
        syntax_set.find_syntax_by_extension(ext).or_else(|| {
            let alias = EXTENSION_ALIASES
                .iter()
                .find(|(from, _)| *from == ext)
                .map(|(_, to)| *to)?;
            syntax_set.find_syntax_by_extension(alias)
        })
    };
    by_name
        .or_else(by_ext)
        .or_else(|| syntax_set.find_syntax_by_first_line(first_line?))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

/// 構文定義とテーマを 1 組だけ持ち回る。どちらも構築が重い。
pub struct Highlighter {
    syntax_set: SyntaxSet,
    themes: EmbeddedLazyThemeSet,
    theme: SyntectTheme,
    id: String,
}

impl std::fmt::Debug for Highlighter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Highlighter").field("id", &self.id).finish()
    }
}

impl Highlighter {
    pub fn new(cfg: &Config) -> Self {
        let themes = two_face::theme::extra();
        Self {
            syntax_set: two_face::syntax::extra_newlines(),
            theme: syntect_theme_for(cfg, &themes),
            id: syntax_theme_id(cfg),
            themes,
        }
    }

    /// 設定が変わったらテーマを引き直す。指紋が同じならキャッシュを持ち越してよい。
    pub fn adopt(&mut self, cfg: &Config) -> bool {
        let id = syntax_theme_id(cfg);
        if id == self.id {
            return false;
        }
        self.theme = syntect_theme_for(cfg, &self.themes);
        self.id = id;
        true
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn syntax_set(&self) -> &SyntaxSet {
        &self.syntax_set
    }

    pub fn theme(&self) -> &SyntectTheme {
        &self.theme
    }

    /// 1 行 = スタイル付きの断片列。行数は入力と同じ。
    pub fn highlight(&self, path: Option<&str>, lines: &[String]) -> Vec<Vec<(Style, String)>> {
        let syntax = find_syntax(&self.syntax_set, path, lines.first().map(String::as_str));
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();

        LinesWithEndings::from(&text)
            .map(
                |line| match highlighter.highlight_line(line, &self.syntax_set) {
                    Ok(ranges) => ranges
                        .into_iter()
                        .map(|(style, text)| {
                            let style = syntect_tui::translate_style(style)
                                .unwrap_or_default()
                                .bg(Color::Reset);
                            (style, text.trim_end_matches('\n').to_string())
                        })
                        .collect(),
                    Err(_) => vec![(Style::default(), line.trim_end_matches('\n').to_string())],
                },
            )
            .collect()
    }
}

/// (テーマ, パス, 本文) の指紋。同じなら描き直す理由が無い。
pub fn cache_key(theme_id: &str, path: Option<&str>, lines: &[String]) -> u64 {
    let mut hasher = DefaultHasher::new();
    theme_id.hash(&mut hasher);
    path.hash(&mut hasher);
    lines.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::config::{UiConfig, ViewerConfig};
    use conductor_core::theme::Theme;

    fn cfg_with(theme: &str) -> Config {
        Config {
            ui: UiConfig {
                theme: Some(theme.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// 背景輝度の推定値 (0〜255)。
    fn bg_luma(theme: &SyntectTheme) -> f32 {
        theme
            .settings
            .background
            .map(|c| 0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32)
            .unwrap_or(0.0)
    }

    /// [Theme::all_names] を回すので、新しいテーマを足せば自動でここに乗る。
    #[test]
    fn 全テーマが同じ明暗のsyntectテーマに対応する() {
        let ts = two_face::theme::extra();
        for name in Theme::all_names() {
            let luma = bg_luma(&syntect_theme_for(&cfg_with(name), &ts));
            assert_eq!(
                luma >= 128.0,
                Theme::from_name(name).light,
                "'{name}' の明暗が syntect 側と食い違う (luma={luma:.0})"
            );
        }
    }

    /// 対応表に漏れがあるとそのテーマだけ既定へフォールバックし、「テーマを切り替えても
    /// コードの色が変わらない」ように見える。
    #[test]
    fn 組み込みテーマは全部が自前のsyntectテーマを持つ() {
        let ts = two_face::theme::extra();
        let names = Theme::all_names();
        let resolved: Vec<String> = names
            .iter()
            .map(|n| {
                syntect_theme_for(&cfg_with(n), &ts)
                    .name
                    .clone()
                    .unwrap_or_default()
            })
            .collect();

        let distinct: std::collections::HashSet<&String> = resolved.iter().collect();
        assert!(
            distinct.len() >= 8,
            "builtin themes collapse onto too few syntect themes: {resolved:?}"
        );

        let mocha: Vec<&str> = names
            .iter()
            .zip(&resolved)
            .filter(|(_, r)| r.as_str() == "Catppuccin Mocha")
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(
            mocha,
            vec!["catppuccin-mocha"],
            "themes other than catppuccin-mocha are silently falling back to the default"
        );
    }

    /// base16 系はスコープ規則が 47〜49 件しかなく、着色率は約 51% だった。
    #[test]
    fn 主要言語で十分な割合のトークンに色が付く() {
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

        let ss = two_face::syntax::extra_newlines();
        let ts = two_face::theme::extra();
        for theme_name in ["catppuccin-mocha", "dracula", "nord", "catppuccin-latte"] {
            let theme = syntect_theme_for(&cfg_with(theme_name), &ts);
            let default_fg = theme.settings.foreground.expect("theme has a foreground");

            for (ext, src) in samples {
                let syntax = ss
                    .find_syntax_by_extension(ext)
                    .unwrap_or_else(|| panic!("syntax for .{ext} must be registered"));
                let mut h = HighlightLines::new(syntax, &theme);
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

    #[test]
    fn テーマ名はui_themeを優先しviewer_themeへ落ちる() {
        let ts = two_face::theme::extra();
        let cases: [(Option<&str>, &str, bool); 2] = [
            (Some("github-light"), "catppuccin-mocha", true),
            (None, "github-light", true),
        ];
        for (ui, viewer, light) in cases {
            let cfg = Config {
                ui: UiConfig {
                    theme: ui.map(str::to_string),
                    ..Default::default()
                },
                viewer: ViewerConfig {
                    theme: viewer.to_string(),
                    ..Default::default()
                },
                ..Default::default()
            };
            assert_eq!(bg_luma(&syntect_theme_for(&cfg, &ts)) >= 128.0, light);
        }
    }

    #[test]
    fn 構文テーマのidはテーマ名とファイルを追う() {
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
        assert_ne!(
            syntax_theme_id(&cfg_with("dracula")),
            syntax_theme_id(&cfg_with("nord"))
        );
        assert_eq!(
            syntax_theme_id(&cfg_with("dracula")),
            syntax_theme_id(&cfg_with("dracula"))
        );
        // ファイル指定はテーマ名に関わらずパスで決まる。
        assert_eq!(
            syntax_theme_id(&with_file("dracula")),
            syntax_theme_id(&with_file("nord"))
        );
        assert_ne!(
            syntax_theme_id(&with_file("dracula")),
            syntax_theme_id(&cfg_with("dracula"))
        );
    }

    #[test]
    fn 解決できない指定でも落ちずに既定へ落ちる() {
        let ts = two_face::theme::extra();
        let missing_file = Config {
            viewer: ViewerConfig {
                syntax_theme_file: Some(String::from("/nonexistent/path/theme.tmTheme")),
                ..Default::default()
            },
            ..Default::default()
        };
        for cfg in [cfg_with("nonexistent-theme-xyz"), missing_file] {
            let theme = syntect_theme_for(&cfg, &ts);
            assert!(theme.settings.foreground.is_some());
        }
    }

    #[test]
    fn 構文定義はファイル名を拡張子より先に見る() {
        let ss = two_face::syntax::extra_newlines();
        let name_of =
            |path: &str, first: Option<&str>| find_syntax(&ss, Some(path), first).name.clone();
        let cases: [(&str, Option<&str>, &str); 15] = [
            ("Dockerfile", None, "Dockerfile"),
            ("deep/dir/Dockerfile", None, "Dockerfile"),
            ("Makefile", None, "Makefile"),
            (".gitignore", None, "Git Ignore"),
            (".env", None, "DotENV"),
            ("Gemfile", None, "Ruby"),
            ("go.mod", None, "Gomod"),
            ("CMakeLists.txt", None, "CMake"),
            ("src/main.rs", None, "Rust"),
            ("a.tsx", None, "TypeScriptReact"),
            ("b.py", None, "Python"),
            ("c.go", None, "Go"),
            ("d.jsx", None, "JavaScript"),
            ("noext", Some("#!/usr/bin/env python3"), "Python"),
            ("noext", None, "Plain Text"),
        ];
        for (path, first, expected) in cases {
            assert_eq!(name_of(path, first), expected, "for {path}");
        }
    }

    #[test]
    fn ハイライトは行数を保ちテーマでキャッシュ鍵が変わる() {
        let h = Highlighter::new(&cfg_with("dracula"));
        let lines: Vec<String> = ["fn main() {", "    let x = 1;", "}"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(h.highlight(Some("a.rs"), &lines).len(), 3);

        let key = cache_key(h.id(), Some("a.rs"), &lines);
        assert_eq!(key, cache_key(h.id(), Some("a.rs"), &lines));
        assert_ne!(key, cache_key("builtin:nord", Some("a.rs"), &lines));
        assert_ne!(key, cache_key(h.id(), Some("b.rs"), &lines));
    }
}
