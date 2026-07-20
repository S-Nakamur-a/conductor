//! Code navigation: symbol lookup under the cursor, jump-to-definition,
//! jump history, background symbol-index builds, and on-screen symbol hints.

use super::focus::Focus;
use super::App;

impl App {
    // ── Code navigation helpers ────────────────────────────────────

    /// Extract the symbol under the cursor from the current viewer line.
    pub fn get_symbol_at_cursor(&self) -> Option<String> {
        let scroll = self.viewer_state.content.file_scroll;
        let line = self.viewer_state.content.file_content.get(scroll)?;
        extract_symbol_from_line(line)
    }

    /// Check if the cursor is currently at (or very near) a definition site
    /// for the given symbol. Returns `true` when the current file + line
    /// matches one of the symbol's definition locations.
    pub fn is_cursor_at_definition(&self, symbol: &str) -> bool {
        let cur_file = match &self.viewer_state.content.current_file {
            Some(f) => f,
            None => return false,
        };
        // Cursor line is 1-indexed (file_scroll is 0-indexed).
        let cursor_line = self.viewer_state.content.file_scroll + 1;
        let defs = self.symbol_index.find_definitions(symbol);
        defs.iter().any(|d| {
            d.file_path == *cur_file && (d.line as isize - cursor_line as isize).unsigned_abs() <= 2
        })
    }

    /// Jump to a file location, pushing the current position onto the history.
    ///
    /// `source_screen_row` is the screen row (0-indexed) where the source
    /// symbol was displayed. The target line will be placed at the same row
    /// so the user's eye position is preserved.
    pub fn jump_to_location(&mut self, file_path: &str, line: usize, source_screen_row: usize) {
        // Skip self-referencing jumps (destination == current position).
        let target_line_0 = line.saturating_sub(1);
        if let Some(ref cur_file) = self.viewer_state.content.current_file {
            let current_line_0 = self.viewer_state.content.file_scroll + source_screen_row;
            if cur_file == file_path && current_line_0 == target_line_0 {
                return;
            }
        }

        // Save current location to history.
        if let Some(ref cur_file) = self.viewer_state.content.current_file.clone() {
            let loc = crate::jump_history::Location {
                file_path: cur_file.clone(),
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            };
            self.jump_history.push(loc);
        }

        // Open the target file.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            let wt_path = wt.path.clone();
            let tab_width = self.config.viewer.tab_width;
            self.viewer_state.open_file(&wt_path, file_path, tab_width);
            self.rehighlight_viewer();
            self.viewer_state.reveal_file_in_tree(file_path, &wt_path);
        }

        // Scroll so the target line appears at the same screen row as the source symbol.
        let target_0 = line.saturating_sub(1);
        let total = self.viewer_state.content.file_content.len();
        let scroll = target_0
            .saturating_sub(source_screen_row)
            .min(total.saturating_sub(1));
        self.viewer_state.content.file_scroll = scroll;
        self.viewer_state.content.h_scroll = 0;
        self.set_focus(Focus::Viewer);
    }

    /// Navigate back in the jump history.
    pub fn jump_back(&mut self) {
        let current = match self.viewer_state.content.current_file.clone() {
            Some(f) => crate::jump_history::Location {
                file_path: f,
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            },
            None => return,
        };

        if let Some(loc) = self.jump_history.go_back(current) {
            if let Some(wt) = self.worktrees.get(self.selected_worktree) {
                let wt_path = wt.path.clone();
                let tab_width = self.config.viewer.tab_width;
                self.viewer_state
                    .open_file(&wt_path, &loc.file_path, tab_width);
                self.rehighlight_viewer();
                self.viewer_state
                    .reveal_file_in_tree(&loc.file_path, &wt_path);
            }
            let total = self.viewer_state.content.file_content.len();
            self.viewer_state.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer_state.content.h_scroll = loc.h_scroll;
        }
    }

    /// Navigate forward in the jump history.
    pub fn jump_forward(&mut self) {
        let current = match self.viewer_state.content.current_file.clone() {
            Some(f) => crate::jump_history::Location {
                file_path: f,
                line: self.viewer_state.content.file_scroll,
                h_scroll: self.viewer_state.content.h_scroll,
            },
            None => return,
        };

        if let Some(loc) = self.jump_history.go_forward(current) {
            if let Some(wt) = self.worktrees.get(self.selected_worktree) {
                let wt_path = wt.path.clone();
                let tab_width = self.config.viewer.tab_width;
                self.viewer_state
                    .open_file(&wt_path, &loc.file_path, tab_width);
                self.rehighlight_viewer();
                self.viewer_state
                    .reveal_file_in_tree(&loc.file_path, &wt_path);
            }
            let total = self.viewer_state.content.file_content.len();
            self.viewer_state.content.file_scroll = loc.line.min(total.saturating_sub(1));
            self.viewer_state.content.h_scroll = loc.h_scroll;
        }
    }

    /// Start building the symbol index in the background.
    pub fn start_symbol_index_build(&mut self) {
        let index = self.symbol_index.clone();
        self.bg.symbol_index.start(move |tx| {
            let result = match index.build() {
                Ok(count) => Ok(count),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(result);
        });
    }

    /// Check whether a symbol has definitions in the symbol index.
    pub fn can_jump_to_symbol(&self, name: &str) -> bool {
        if !self.symbol_index.is_available() {
            return false;
        }
        !self.symbol_index.find_definitions(name).is_empty()
    }

    /// Build symbol hints for visible lines in the viewer.
    /// Returns hints with 2-character labels for jumpable symbols on screen.
    pub fn build_symbol_hints(&self, inner_height: usize) -> Vec<crate::overlay::SymbolHint> {
        let scroll = self.viewer_state.content.file_scroll;
        let total = self.viewer_state.content.file_content.len();
        let end = (scroll + inner_height).min(total);

        let re = match regex::Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\b") {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut seen = std::collections::HashSet::new();
        let mut candidates = Vec::new();

        for line_idx in scroll..end {
            let line = &self.viewer_state.content.file_content[line_idx];
            let line_1 = line_idx + 1;
            for cap in re.captures_iter(line) {
                if let Some(m) = cap.get(1) {
                    let word = m.as_str();
                    if word.len() <= 1 || is_rust_keyword(word) {
                        continue;
                    }
                    if !seen.insert(word.to_string()) {
                        continue;
                    }
                    if !self.can_jump_to_symbol(word) {
                        continue;
                    }
                    candidates.push((word.to_string(), line_1, m.start(), m.end()));
                }
            }
        }

        // Assign 2-character labels: aa, ab, ..., az, ba, bb, ...
        candidates
            .into_iter()
            .enumerate()
            .map(|(i, (name, line, start, end))| {
                let first = (b'a' + (i / 26) as u8) as char;
                let second = (b'a' + (i % 26) as u8) as char;
                crate::overlay::SymbolHint {
                    label: format!("{first}{second}"),
                    symbol_name: name,
                    line,
                    start_col: start,
                    end_col: end,
                }
            })
            .collect()
    }
}

// ── Free functions for symbol extraction ──────────────────────────────

/// Extract a symbol name from a source code line at the cursor position.
/// Returns the first Rust-like identifier found on the line that is not a keyword.
pub fn extract_symbol_from_line(line: &str) -> Option<String> {
    let re = regex::Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\b").ok()?;
    for cap in re.captures_iter(line) {
        let word = cap.get(1)?.as_str();
        if !is_rust_keyword(word) && word.len() > 1 {
            return Some(word.to_string());
        }
    }
    None
}

/// Check if a word is a Rust keyword (should not be treated as a symbol).
pub fn is_rust_keyword(word: &str) -> bool {
    matches!(
        word,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
    )
}

/// Extract the symbol (identifier) at a specific column in a line.
/// Returns `(symbol_text, start_col, end_col)` where cols are 0-indexed character offsets.
pub fn extract_symbol_at_column(line: &str, col: usize) -> Option<(String, usize, usize)> {
    if col >= line.len() {
        return None;
    }
    // Check that the character at `col` is part of an identifier.
    let ch = line.as_bytes().get(col).copied()?;
    if !(ch.is_ascii_alphanumeric() || ch == b'_') {
        return None;
    }
    // Walk backwards to find start of identifier.
    let start = line[..col]
        .bytes()
        .rev()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let start_col = col - start;
    // Walk forwards to find end of identifier.
    let end = line[col..]
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let end_col = col + end;
    let word = &line[start_col..end_col];
    if word.len() <= 1 || is_rust_keyword(word) {
        return None;
    }
    // Must start with letter or underscore.
    if !word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    Some((word.to_string(), start_col, end_col))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_symbol_at_column_basic() {
        let line = "    let foo = AppState::new();";
        // Click on 'A' of AppState at col 14
        let result = extract_symbol_at_column(line, 14);
        assert_eq!(result, Some(("AppState".to_string(), 14, 22)));
    }

    #[test]
    fn test_extract_symbol_at_column_middle() {
        let line = "    let foo = AppState::new();";
        // Click on 'S' of AppState at col 17
        let result = extract_symbol_at_column(line, 17);
        assert_eq!(result, Some(("AppState".to_string(), 14, 22)));
    }

    #[test]
    fn test_extract_symbol_at_column_on_keyword() {
        let line = "    let foo = bar;";
        // Click on 'l' of let at col 4
        let result = extract_symbol_at_column(line, 4);
        assert_eq!(result, None); // "let" is a keyword
    }

    #[test]
    fn test_extract_symbol_at_column_on_space() {
        let line = "fn main() {}";
        let result = extract_symbol_at_column(line, 2);
        assert_eq!(result, None); // space
    }

    #[test]
    fn test_extract_symbol_at_column_out_of_bounds() {
        let line = "short";
        let result = extract_symbol_at_column(line, 100);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_symbol_at_column_single_char() {
        let line = "x + y";
        // Single char identifiers are filtered out
        let result = extract_symbol_at_column(line, 0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_symbol_at_column_underscore_prefix() {
        let line = "    _handler.call();";
        let result = extract_symbol_at_column(line, 5);
        assert_eq!(result, Some(("_handler".to_string(), 4, 12)));
    }
}
