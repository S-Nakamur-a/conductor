//! File content loading — opening a file into `content.file_content` and the
//! small helpers that support it (media detection, tab expansion).

use std::fs;
use std::path::Path;

use crate::media_state;

use super::state::ViewerState;

impl ViewerState {
    /// Invalidate the cached diff annotations (call when diff data changes).
    pub fn invalidate_diff_annotations(&mut self) {
        self.content.cached_diff_annotations = None;
        self.content.cached_diff_annotations_file = None;
    }

    /// Open (read) a file and store its lines in `file_content`.
    pub fn open_file(&mut self, worktree_path: &Path, relative_path: &str, tab_width: usize) {
        self.exit_diff_mode();
        self.content.highlighted_lines.clear();
        self.content.highlighted_cache_key = None;
        self.content.grep_highlight_line = None;
        self.content.test_runs.clear();
        let full = worktree_path.join(relative_path);

        // Handle media files (images/videos) via aa-media.
        if media_state::is_media_file(relative_path) {
            self.content.file_content.clear();
            self.content.current_file = Some(relative_path.to_string());
            self.content.file_scroll = 0;
            self.content.h_scroll = 0;
            // Actual rendering is triggered lazily during render (when panel
            // size is known). Clear the cache so it re-renders for the new file.
            self.media_state.clear();
            return;
        }

        // Clear media state when opening a non-media file.
        self.media_state.clear();

        match fs::read_to_string(&full) {
            Ok(text) => {
                self.content.file_content = text
                    .lines()
                    .map(|l| Self::expand_tabs(l, tab_width))
                    .collect();
                // If file is empty but not zero-length, show one empty line.
                if self.content.file_content.is_empty() && !text.is_empty() {
                    self.content.file_content.push(String::new());
                }
                self.content.current_file = Some(relative_path.to_string());
                self.content.file_scroll = 0;
                self.content.h_scroll = 0;
                // Detect runnable tests for the ▶ run buttons, dispatching by
                // language: Go's `*_test.go` files and Rust's `*.rs` files.
                self.content.test_runs = if relative_path.ends_with(".rs") {
                    crate::rust_test::scan_rust_test_runs(&self.content.file_content, relative_path)
                } else {
                    crate::go_test::scan_go_test_runs(&self.content.file_content, relative_path)
                };
            }
            Err(e) => {
                // Show error as file content so the user sees feedback.
                self.content.file_content = vec![format!("Error reading file: {e}")];
                self.content.current_file = Some(relative_path.to_string());
                self.content.file_scroll = 0;
                self.content.h_scroll = 0;
            }
        }
    }

    /// Returns true if the current file is a media file.
    pub fn is_current_file_media(&self) -> bool {
        self.content
            .current_file
            .as_deref()
            .is_some_and(media_state::is_media_file)
    }

    /// Expand tab characters to spaces, respecting tab stop positions.
    fn expand_tabs(line: &str, tab_width: usize) -> String {
        if !line.contains('\t') {
            return line.to_string();
        }
        let mut result = String::with_capacity(line.len());
        let mut col = 0;
        for ch in line.chars() {
            if ch == '\t' {
                let spaces = tab_width - (col % tab_width);
                for _ in 0..spaces {
                    result.push(' ');
                }
                col += spaces;
            } else {
                result.push(ch);
                col += 1;
            }
        }
        result
    }
}
