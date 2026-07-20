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

    /// Explicitly show the hover-info popup for the symbol under the viewer
    /// cursor (the first identifier on the top line — same lookup as `gd`).
    ///
    /// Bound to `K` as an instant, no-wait trigger. Because it's a deliberate
    /// press it gives feedback when it can't produce a popup (status flash),
    /// unlike the passive auto-hover which stays silent.
    pub fn show_hover_info(&mut self) {
        use crate::app::StatusLevel;

        let symbol = match self.get_symbol_at_cursor() {
            Some(s) => s,
            None => {
                self.set_status("No symbol under cursor".to_string(), StatusLevel::Warning);
                return;
            }
        };
        if !self.symbol_index.is_available() {
            self.set_status(
                "Symbol index not ready yet".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        let current_file = self.viewer_state.content.current_file.clone();
        match crate::hover_info::build_hover_info(
            &self.symbol_index,
            &symbol,
            current_file.as_deref(),
        ) {
            Some(info) => {
                self.hover_info_overlay.shown_file = current_file.clone();
                self.hover_info_overlay.info = Some(info);
            }
            None => {
                self.set_status(
                    format!("No definition indexed for '{symbol}'"),
                    StatusLevel::Info,
                );
            }
        }
    }

    /// Whether the passive auto-hover popup is allowed to appear right now:
    /// a file (plain or diff) open in the focused Viewer, with no blocking
    /// overlay or the summary pseudo-view stealing the surface.
    fn hover_auto_allowed(&self) -> bool {
        self.focus == Focus::Viewer
            && !self.viewer_state.is_summary()
            && self.overlays.active == crate::overlay::ActiveOverlay::None
            && !self.references_overlay.active
            && !self.symbol_action_overlay.active
            && !self.symbol_hint_overlay.active
            && self.viewer_state.content.current_file.is_some()
    }

    /// Clear the whole hover modal stack (popup, pending candidate, refs list,
    /// preview, pin). Returns whether anything was actually showing.
    pub fn clear_hover(&mut self) -> bool {
        let had = self.hover_info_overlay.info.is_some()
            || self.hover_info_overlay.pending.is_some()
            || self.hover_info_overlay.pinned;
        self.hover_info_overlay.reset();
        had
    }

    /// Record the symbol the mouse is currently resting on (from a mouse-move
    /// event). `cand` is `(symbol, 1-indexed line, anchor_row, anchor_col)` in
    /// absolute screen coords, or `None` when the mouse is over blank space / a
    /// non-identifier. A *new* symbol restarts the idle countdown and drops any
    /// popup shown for the previous one.
    pub fn set_mouse_hover_candidate(&mut self, cand: Option<(String, usize, u16, u16)>) {
        match cand {
            Some((symbol, line, anchor_row, anchor_col)) => {
                let same = self
                    .hover_info_overlay
                    .pending
                    .as_ref()
                    .is_some_and(|c| c.symbol == symbol && c.line == line);
                if same {
                    return;
                }
                let file = self.viewer_state.content.current_file.clone();
                self.hover_info_overlay.pending = Some(crate::overlay::HoverCandidate {
                    symbol,
                    line,
                    file,
                    anchor_row,
                    anchor_col,
                    since: std::time::Instant::now(),
                    resolved: false,
                });
                self.hover_info_overlay.leave_at = None;
                if self.hover_info_overlay.info.take().is_some() {
                    self.dirty.mark_all();
                }
            }
            None => {
                // Mouse moved off the symbol onto blank space. If a popup is
                // showing, don't drop it instantly — start a short grace window
                // (see `tick_hover`) so the cursor can travel onto the popup to
                // click it. If nothing is shown yet, just drop the candidate.
                if self.hover_info_overlay.info.is_some() {
                    self.hover_info_overlay.pending = None;
                    if self.hover_info_overlay.leave_at.is_none() {
                        self.hover_info_overlay.leave_at = Some(std::time::Instant::now());
                    }
                } else if self.hover_info_overlay.pending.take().is_some() {
                    self.dirty.mark_all();
                }
            }
        }
    }

    /// True when the given absolute screen point lies within any part of the
    /// hover modal stack (base popup, refs list, or preview) — used to keep the
    /// popup alive while the mouse is over it and to route clicks.
    pub fn hover_point_hit(&self, col: u16, row: u16) -> bool {
        let hv = &self.hover_info_overlay;
        let in_rect = |r: ratatui::layout::Rect| {
            r.width > 0
                && r.height > 0
                && col >= r.x
                && col < r.x + r.width
                && row >= r.y
                && row < r.y + r.height
        };
        if in_rect(hv.info_rect) {
            return true;
        }
        if let Some(refs) = &hv.refs {
            if in_rect(refs.rect) {
                return true;
            }
            if let Some(p) = &refs.preview {
                if in_rect(p.rect) {
                    return true;
                }
            }
        }
        false
    }

    /// Per-frame auto-hover driver. When the mouse has rested on a symbol past
    /// the debounce, resolves its hover popup; stays silent when nothing is
    /// found. Also manages the grace window and stale-file/focus invalidation.
    pub fn tick_hover(&mut self) {
        /// How long the mouse must rest on a symbol before the popup appears.
        const HOVER_IDLE: std::time::Duration = std::time::Duration::from_millis(350);
        /// Grace window keeping a transient popup alive after the mouse leaves
        /// the symbol, so the cursor can reach the popup to click it.
        const HOVER_GRACE: std::time::Duration = std::time::Duration::from_millis(700);

        // A pinned modal is user-driven: it survives focus/idle loss and is only
        // dismissed by Esc or a click outside (handled in the event layer).
        if self.hover_info_overlay.pinned {
            return;
        }

        // Stale-file guard: if the viewer switched files (via a jump, the file
        // tree, or an external reload) while a popup was up, the popup now
        // describes a symbol from a file no longer on screen. Drop it — even
        // within the grace window — so it can never linger over unrelated code.
        if self.hover_info_overlay.info.is_some()
            && self.hover_info_overlay.shown_file != self.viewer_state.content.current_file
        {
            if self.clear_hover() {
                self.dirty.mark_all();
            }
            return;
        }

        // Grace window: a popup whose symbol the mouse left stays up briefly, but
        // only while the mouse is actually over it or the timer hasn't expired.
        if let Some(left) = self.hover_info_overlay.leave_at {
            if left.elapsed() >= HOVER_GRACE {
                if self.clear_hover() {
                    self.dirty.mark_all();
                }
                return;
            }
        }

        if !self.hover_auto_allowed() {
            // Don't kill a popup that's within its grace window — the user may be
            // moving the mouse toward it (which briefly leaves the content area).
            if self.hover_info_overlay.leave_at.is_some() {
                return;
            }
            if self.clear_hover() {
                self.dirty.mark_all();
            }
            return;
        }

        // Auto-hover is driven purely by the mouse resting on a symbol (set by the
        // mouse-move handler). A top-line/keyboard heuristic was tried and dropped:
        // with no per-line text cursor, "the cursor line" is always the top visible
        // line, so it fired for code the user wasn't pointing at. Mouse position is
        // exact — on a symbol shows the popup, on whitespace shows nothing.

        // Resolve a candidate that has rested long enough.
        let ready = self
            .hover_info_overlay
            .pending
            .as_ref()
            .is_some_and(|c| !c.resolved && c.since.elapsed() >= HOVER_IDLE);
        if ready {
            let (symbol, file, anchor_row, anchor_col) = {
                let c = self.hover_info_overlay.pending.as_ref().unwrap();
                (c.symbol.clone(), c.file.clone(), c.anchor_row, c.anchor_col)
            };
            let info = crate::hover_info::build_hover_info(
                &self.symbol_index,
                &symbol,
                file.as_deref(),
            );
            if let Some(c) = self.hover_info_overlay.pending.as_mut() {
                c.resolved = true;
            }
            self.hover_info_overlay.anchor_row = anchor_row;
            self.hover_info_overlay.anchor_col = anchor_col;
            // Remember which viewed file this popup describes, so the stale-file
            // guard can drop it the moment the viewer moves to another file.
            self.hover_info_overlay.shown_file = if info.is_some() { file } else { None };
            self.hover_info_overlay.info = info;
            self.dirty.mark_all();
        }
    }

    /// Cancel the grace window because the mouse is now over the popup itself.
    pub fn hover_keep_alive(&mut self) {
        self.hover_info_overlay.leave_at = None;
    }

    /// Open the references list (level 1) for the currently-shown symbol and pin
    /// the popup. No-op when nothing is shown or the symbol has no references.
    pub fn open_hover_refs(&mut self) {
        let symbol = match self.hover_info_overlay.info.as_ref() {
            Some(info) if info.ref_count > 0 => info.symbol_name.clone(),
            _ => return,
        };
        let root = self.symbol_index.root();
        let results = self.symbol_index.find_references(&symbol, &root);
        if results.is_empty() {
            return;
        }
        self.hover_info_overlay.pinned = true;
        self.hover_info_overlay.leave_at = None;
        self.hover_info_overlay.refs = Some(crate::overlay::HoverRefs {
            symbol,
            results,
            selected: 0,
            scroll: 0,
            rect: ratatui::layout::Rect::default(),
            row_hits: Vec::new(),
            preview: None,
        });
        self.dirty.mark_all();
    }

    /// Open the code preview (level 2) for reference row `idx` in the list.
    pub fn open_hover_preview(&mut self, idx: usize) {
        let (file, line) = match self.hover_info_overlay.refs.as_mut() {
            Some(refs) => match refs.results.get(idx) {
                Some(r) => {
                    refs.selected = idx;
                    (r.file_path.clone(), r.line)
                }
                None => return,
            },
            None => return,
        };
        let root = self.symbol_index.root();
        let preview = build_hover_preview(&root, &file, line);
        if let Some(refs) = self.hover_info_overlay.refs.as_mut() {
            refs.preview = preview;
        }
        self.dirty.mark_all();
    }

    /// Jump to the open preview's location and dismiss the whole hover stack.
    pub fn hover_jump_to_preview(&mut self) {
        let target = self
            .hover_info_overlay
            .refs
            .as_ref()
            .and_then(|r| r.preview.as_ref())
            .map(|p| (p.file.clone(), p.center_line));
        if let Some((file, line)) = target {
            self.clear_hover();
            self.jump_to_location(&file, line, 0);
        }
    }

    /// Move the references-list selection by `delta` (keyboard nav), clamping.
    pub fn hover_refs_move(&mut self, delta: isize) {
        if let Some(refs) = self.hover_info_overlay.refs.as_mut() {
            let n = refs.results.len();
            if n == 0 {
                return;
            }
            let cur = refs.selected as isize;
            refs.selected = (cur + delta).clamp(0, n as isize - 1) as usize;
            self.dirty.mark_all();
        }
    }

    /// Esc from the hover stack: close the deepest open level (preview → list →
    /// the whole popup). Returns whether a level was closed.
    pub fn hover_pop_level(&mut self) -> bool {
        if let Some(refs) = self.hover_info_overlay.refs.as_mut() {
            if refs.preview.take().is_some() {
                self.dirty.mark_all();
                return true;
            }
            self.hover_info_overlay.refs = None;
            self.hover_info_overlay.pinned = false;
            self.dirty.mark_all();
            return true;
        }
        if self.clear_hover() {
            self.dirty.mark_all();
            return true;
        }
        false
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

/// Build a code preview window around `line_1` (1-indexed) in `rel_path`,
/// reading a few lines of context on each side. Returns `None` if the file
/// can't be read or the line is out of range.
fn build_hover_preview(
    root: &std::path::Path,
    rel_path: &str,
    line_1: usize,
) -> Option<crate::overlay::HoverPreview> {
    /// Lines of context shown on each side of the reference line.
    const CONTEXT: usize = 3;

    let source = std::fs::read_to_string(root.join(rel_path)).ok()?;
    let all: Vec<&str> = source.lines().collect();
    if line_1 == 0 || line_1 > all.len() {
        return None;
    }
    let idx = line_1 - 1;
    let start = idx.saturating_sub(CONTEXT);
    let end = (idx + CONTEXT + 1).min(all.len());
    let lines = (start..end)
        .map(|i| (i + 1, all[i].to_string()))
        .collect::<Vec<_>>();
    Some(crate::overlay::HoverPreview {
        file: rel_path.to_string(),
        center_line: line_1,
        lines,
        rect: ratatui::layout::Rect::default(),
    })
}

// ── Free functions for symbol extraction ──────────────────────────────

/// Extract a symbol name from a source code line at the cursor position.
/// Returns the first Rust-like identifier found on the line that is not a
/// keyword. Comment and attribute lines yield nothing — an English word in a
/// doc comment (e.g. `//! Building …`) must not resolve to a same-named symbol.
pub fn extract_symbol_from_line(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    // Skip lines that are comments (`//`, `///`, `//!`, `/* … */`, block-comment
    // continuation `* …`) or attributes (`#[…]`) — none carry a code symbol.
    if trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with('#')
    {
        return None;
    }
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

    #[test]
    fn build_hover_preview_windows_around_line() {
        let dir = std::env::temp_dir().join(format!("hover_prev_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = (1..=10)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(dir.join("f.rs"), src).unwrap();

        // Center line 5 → 3 lines of context each side (2..=8).
        let p = build_hover_preview(&dir, "f.rs", 5).expect("preview");
        assert_eq!(p.center_line, 5);
        assert_eq!(p.file, "f.rs");
        assert_eq!(
            p.lines,
            vec![
                (2, "line2".to_string()),
                (3, "line3".to_string()),
                (4, "line4".to_string()),
                (5, "line5".to_string()),
                (6, "line6".to_string()),
                (7, "line7".to_string()),
                (8, "line8".to_string()),
            ]
        );

        // Near the top the window clamps to the file start.
        let p = build_hover_preview(&dir, "f.rs", 1).expect("preview");
        assert_eq!(p.lines.first().unwrap().0, 1);

        // Out-of-range / missing file → None.
        assert!(build_hover_preview(&dir, "f.rs", 999).is_none());
        assert!(build_hover_preview(&dir, "nope.rs", 1).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_symbol_from_line_skips_comments_and_attributes() {
        // Doc/line/block comments must not yield an English word that happens
        // to collide with a real type name (the "Building" bug).
        assert_eq!(extract_symbol_from_line("//! Building and navigating"), None);
        assert_eq!(extract_symbol_from_line("/// Create a new state"), None);
        assert_eq!(extract_symbol_from_line("    // Building the list"), None);
        assert_eq!(extract_symbol_from_line("/* Building */"), None);
        assert_eq!(extract_symbol_from_line("     * Building (block cont.)"), None);
        assert_eq!(extract_symbol_from_line("#[derive(Debug)]"), None);
        // Real code lines still resolve to their first identifier.
        assert_eq!(
            extract_symbol_from_line("    let state = DiffState::new();"),
            Some("state".to_string())
        );
        assert_eq!(
            extract_symbol_from_line("pub struct Building {"),
            Some("Building".to_string())
        );
    }
}
