//! Syntax highlighting — runs syntect over `content.file_content` and caches
//! the resulting ratatui-styled spans.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use syntect::easy::HighlightLines;
use syntect::highlighting::Theme as SyntectTheme;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::state::ViewerState;

impl ViewerState {
    /// Run syntect highlighting on `file_content` and cache the result.
    ///
    /// Computes a hash of `(current_file, file_content)` and skips
    /// re-highlighting when the content has not changed since the last call.
    pub fn highlight_content(&mut self, syntax_set: &SyntaxSet, theme: &SyntectTheme) {
        if self.content.file_content.is_empty() {
            self.content.highlighted_lines.clear();
            self.content.highlighted_cache_key = None;
            return;
        }

        // Compute a cache key from the file path and content.
        let hash = {
            let mut hasher = DefaultHasher::new();
            self.content.current_file.hash(&mut hasher);
            self.content.file_content.hash(&mut hasher);
            hasher.finish()
        };

        if self.content.highlighted_cache_key == Some(hash) {
            return; // Content unchanged — skip redundant highlighting.
        }

        self.content.highlighted_lines.clear();

        // Determine syntax from file extension.
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

        // Reconstruct the full text with newlines for syntect (it expects them).
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
                    // Fallback: plain white.
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
                    // Strip trailing newline from the last token.
                    let text = text.trim_end_matches('\n').to_string();
                    (ratatui_style, text)
                })
                .collect();

            self.content.highlighted_lines.push(spans);
        }

        self.content.highlighted_cache_key = Some(hash);
    }
}
