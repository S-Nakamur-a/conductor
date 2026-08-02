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

impl ViewerState {
    /// file_content に syntect のハイライトをかけ、結果をキャッシュする。
    ///
    /// (current_file, file_content) のハッシュを計算し、前回呼び出し以降
    /// 内容が変わっていなければ再ハイライトをスキップする。
    pub fn highlight_content(&mut self, syntax_set: &SyntaxSet, theme: &SyntectTheme) {
        if self.content.file_content.is_empty() {
            self.content.highlighted_lines.clear();
            self.content.highlighted_cache_key = None;
            return;
        }

        // ファイルパスと内容からキャッシュキーを計算する。
        let hash = {
            let mut hasher = DefaultHasher::new();
            self.content.current_file.hash(&mut hasher);
            self.content.file_content.hash(&mut hasher);
            hasher.finish()
        };

        if self.content.highlighted_cache_key == Some(hash) {
            return; // 内容が変わっていないので無駄なハイライトをスキップする。
        }

        self.content.highlighted_lines.clear();

        // ファイル拡張子からシンタックスを決定する。
        let ext = self
            .content
            .current_file
            .as_ref()
            .and_then(|p| Path::new(p).extension())
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let syntax = syntax_set
            .find_syntax_by_extension(ext)
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

        let mut h = HighlightLines::new(syntax, theme);

        // syntect は改行付きのテキストを期待するので、改行を補いながら全文を組み立てる。
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
                    // フォールバック: 白一色。
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
                    // 最後のトークンの末尾改行を取り除く。
                    let text = text.trim_end_matches('\n').to_string();
                    (ratatui_style, text)
                })
                .collect();

            self.content.highlighted_lines.push(spans);
        }

        self.content.highlighted_cache_key = Some(hash);
    }
}
