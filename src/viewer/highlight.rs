//! シンタックスハイライト — content.file_content に syntect をかけ、
//! 結果の ratatui スタイル付きスパンをキャッシュする。

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use syntect::easy::HighlightLines;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::state::ViewerState;

/// syntect が拡張子を持たない一部の言語。ここに無いと plain text 扱いになり、
/// ほとんど色が付かなくなる。値は syntect 側が知っている拡張子。
const EXTENSION_ALIASES: &[(&str, &str)] = &[
    // JSX / ESM・CJS のモジュール拡張子は syntect の JavaScript 定義には
    // 登録されていない。
    ("jsx", "js"),
    ("mjs", "js"),
    ("cjs", "js"),
];

/// ファイル名と先頭行から syntect のシンタックス定義を決める。解決順は「ファイル名まるごと
/// → 拡張子 → 拡張子のエイリアス → shebang → plain text」。
///
/// ファイル名を拡張子より先に見るのが要点。拡張子だけだと Dockerfile・Makefile・
/// .gitignore が plain text になり、CMakeLists.txt は .txt に引っかかって積極的に間違う。
pub(crate) fn find_syntax<'a>(
    syntax_set: &'a SyntaxSet,
    path: Option<&str>,
    first_line: Option<&str>,
) -> &'a syntect::parsing::SyntaxReference {
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

    let by_first_line = || syntax_set.find_syntax_by_first_line(first_line?);

    by_name
        .or_else(by_ext)
        .or_else(by_first_line)
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

impl ViewerState {
    /// file_content に syntect のハイライトをかけ、結果をキャッシュする。
    ///
    /// キーは (theme_generation, current_file, file_content) のハッシュ。theme_generation を
    /// 含めないと、テーマだけが変わったときにキャッシュを素通りして古い配色の span が残る。
    pub fn highlight_content(
        &mut self,
        syntax_set: &SyntaxSet,
        theme: &SyntectTheme,
        theme_generation: u64,
    ) {
        if self.content.file_content.is_empty() {
            self.content.highlighted_lines.clear();
            self.content.highlighted_cache_key = None;
            return;
        }

        let hash = {
            let mut hasher = DefaultHasher::new();
            theme_generation.hash(&mut hasher);
            self.content.current_file.hash(&mut hasher);
            self.content.file_content.hash(&mut hasher);
            hasher.finish()
        };

        if self.content.highlighted_cache_key == Some(hash) {
            return; // 内容が変わっていないので無駄なハイライトをスキップする。
        }

        self.content.highlighted_lines.clear();

        let syntax = find_syntax(
            syntax_set,
            self.content.current_file.as_deref(),
            self.content.file_content.first().map(|s| s.as_str()),
        );

        let mut h = HighlightLines::new(syntax, theme);

        let full_text: String = self
            .content
            .file_content
            .iter()
            .map(|l| format!("{l}\n"))
            .collect();

        for line in LinesWithEndings::from(&full_text) {
            let ranges = match h.highlight_line(line, syntax_set) {
                Ok(r) => r,
                Err(_) => {
                    self.content.highlighted_lines.push(vec![(
                        ratatui::style::Style::default().fg(ratatui::style::Color::White),
                        line.trim_end_matches('\n').to_string(),
                    )]);
                    continue;
                }
            };

            let spans: Vec<(ratatui::style::Style, String)> = ranges
                .into_iter()
                .map(|(style, text)| {
                    let ratatui_style = syntect_tui::translate_style(style)
                        .unwrap_or_default()
                        .bg(ratatui::style::Color::Reset);
                    let text = text.trim_end_matches('\n').to_string();
                    (ratatui_style, text)
                })
                .collect();

            self.content.highlighted_lines.push(spans);
        }

        self.content.highlighted_cache_key = Some(hash);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syntax_name(path: &str, first_line: Option<&str>) -> String {
        let ss = two_face::syntax::extra_newlines();
        find_syntax(&ss, Some(path), first_line).name.clone()
    }

    /// 拡張子を持たないファイルがファイル名で正しく判定されること (拡張子だけだと Plain Text)。
    #[test]
    fn 拡張子の無いファイルは名前で引く() {
        for (path, expected) in [
            ("Dockerfile", "Dockerfile"),
            ("deep/dir/Dockerfile", "Dockerfile"),
            ("Makefile", "Makefile"),
            (".gitignore", "Git Ignore"),
            (".env", "DotENV"),
            ("Gemfile", "Ruby"),
            ("go.mod", "Gomod"),
        ] {
            assert_eq!(syntax_name(path, None), expected, "for {path}");
        }
    }

    /// ファイル名は拡張子より優先されること (CMakeLists.txt が .txt に当たらない)。
    #[test]
    fn ファイル名は拡張子より優先される() {
        assert_eq!(syntax_name("CMakeLists.txt", None), "CMake");
    }

    /// 通常の拡張子は今までどおり引けること。
    #[test]
    fn 普通の拡張子は引ける() {
        for (path, expected) in [
            ("src/main.rs", "Rust"),
            ("a.tsx", "TypeScriptReact"),
            ("b.py", "Python"),
            ("c.go", "Go"),
        ] {
            assert_eq!(syntax_name(path, None), expected, "for {path}");
        }
    }

    /// syntect に登録の無い拡張子がエイリアス経由で解決されること。
    #[test]
    fn 別名の拡張子も引ける() {
        for path in ["a.jsx", "b.mjs", "c.cjs"] {
            let name = syntax_name(path, None);
            assert_ne!(
                name, "Plain Text",
                "{path} must not fall back to plain text"
            );
            assert!(name.contains("JavaScript"), "{path} resolved to {name}");
        }
    }

    /// 拡張子もファイル名も手がかりにならない場合、shebang で判定すること。
    #[test]
    fn 手がかりが無ければshebangに落ちる() {
        assert_eq!(
            syntax_name("bin/deploy", Some("#!/usr/bin/env python3")),
            "Python"
        );
        assert!(syntax_name("bin/run", Some("#!/bin/bash")).contains("bash"));
    }

    /// 手がかりが何も無ければ plain text に落ちること（パニックしない）。
    #[test]
    fn 最後はplain_textに落ちる() {
        assert_eq!(
            syntax_name("LICENSE", Some("Copyright (c) 2026")),
            "Plain Text"
        );
    }

    /// テーマ世代が変わるとハイライトのキャッシュが無効化されること。効かないと、テーマを
    /// 切り替えても内容が同じ限り古い配色の span が残る。
    #[test]
    fn テーマ世代が変わるとキャッシュが無効になる() {
        let ss = two_face::syntax::extra_newlines();
        let themes = two_face::theme::extra();
        let dark = themes.get(two_face::theme::EmbeddedThemeName::CatppuccinMocha);
        let light = themes.get(two_face::theme::EmbeddedThemeName::InspiredGithub);

        let mut vs = ViewerState::default();
        vs.content.current_file = Some(String::from("main.rs"));
        vs.content.file_content = vec![String::from("fn main() { let x = 1; }")];

        vs.highlight_content(&ss, dark, 0);
        let first: Vec<_> = vs.content.highlighted_lines[0]
            .iter()
            .map(|(s, _)| s.fg)
            .collect();

        // 同じ世代なら再ハイライトされない（キャッシュが効く）。
        vs.highlight_content(&ss, light, 0);
        let cached: Vec<_> = vs.content.highlighted_lines[0]
            .iter()
            .map(|(s, _)| s.fg)
            .collect();
        assert_eq!(cached, first, "same generation must reuse the cache");

        // 世代が進めばテーマが実際に反映される。
        vs.highlight_content(&ss, light, 1);
        let recolored: Vec<_> = vs.content.highlighted_lines[0]
            .iter()
            .map(|(s, _)| s.fg)
            .collect();
        assert_ne!(
            recolored, first,
            "bumping the generation must re-highlight with the new theme"
        );
    }
}
