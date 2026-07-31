//! File content loading — opening a file into `content.file_content` and the
//! small helpers that support it (media/markdown detection, tab expansion).

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
        // `md_rendered` deliberately survives (the mode is sticky across files);
        // the scroll offset does not, since it indexes into the old document.
        self.md_scroll = 0;
        self.content.highlighted_lines.clear();
        self.content.highlighted_cache_key = None;
        self.content.grep_highlight_line = None;
        self.content.test_runs.clear();
        // Dropped up front rather than only on the success path: the media and
        // read-error branches below also replace `file_content`, and a mask
        // left over from the previous file would describe the wrong text.
        self.content.code_mask = crate::symbol_index::CodeMask::default();
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
                // Record which identifiers are code before anything can be
                // navigated. Built from `text` rather than `file_content`
                // because tree-sitter needs the file as written, tabs and all.
                self.content.code_mask =
                    crate::symbol_index::CodeMask::compute(&text, relative_path);
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

    /// Whether the Raw/Rendered toggle applies to what the Viewer is showing.
    ///
    /// Only the plain-file view of a markdown file offers it: unified-diff mode
    /// shows a diff (where rendering prose would destroy the +/- structure) and
    /// the SUMMARY pseudo-file is already rendered markdown by definition. This
    /// single predicate decides both whether the header toggle is drawn and
    /// whether its click target is live, so the two can't drift apart.
    pub fn markdown_toggle_available(&self) -> bool {
        !self.show_summary
            && !self.diff_view.diff_mode
            && self
                .content
                .current_file
                .as_deref()
                .is_some_and(is_markdown_path)
    }

    /// Whether the Viewer is currently drawing rendered markdown instead of raw
    /// source. **Every line-oriented feature must be gated on this**: the
    /// rendered view has no line numbers, so line selection, hover highlight,
    /// comment creation/threads, and line-anchored jumps have nothing to anchor
    /// to (see `ui::viewer_panel::markdown_view`).
    pub fn is_showing_rendered_markdown(&self) -> bool {
        self.md_rendered && self.markdown_toggle_available()
    }

    /// Toggle between raw source and rendered markdown, resetting the rendered
    /// view's scroll so a toggle always lands at the top of the document.
    ///
    /// Also tears down the line-anchored interactions the rendered view can't
    /// draw. A selection would silently reappear on the way back, and — worse —
    /// an inline reply left open would keep swallowing every keystroke into a
    /// compose box that is no longer on screen (the toggle is clickable while
    /// one is open).
    pub fn toggle_markdown_rendered(&mut self) {
        self.md_rendered = !self.md_rendered;
        self.md_scroll = 0;
        self.clear_selection();
        // An in-flight gutter drag would otherwise pop the comment composer on
        // mouse-up, over a view with no gutter to have dragged in.
        self.click.gutter_drag_anchor = None;
        self.explorer.inline_reply_line = None;
        self.explorer.inline_reply_comment_id = None;
        self.explorer.inline_reply_buffer.clear();
    }

    /// Leave rendered markdown so a line-anchored destination is actually
    /// visible.
    ///
    /// Called by every path that positions the Viewer *by line* (jump to
    /// definition, jump history, grep hit, `file:line` from a terminal). Without
    /// it those jumps would land on rendered prose, which has no line numbers —
    /// the requested line would be silently ignored and the reader dropped at
    /// the top of the document instead.
    pub fn show_raw_for_line_target(&mut self) {
        self.md_rendered = false;
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

/// Whether `path` names a markdown file, i.e. one the Viewer can offer the
/// Raw/Rendered toggle for.
///
/// Extension-only and case-insensitive (`README.MD` counts). Deliberately does
/// *not* match `.mdx`, `.mdown`, or extensionless `README`: the renderer is the
/// small CommonMark subset used for change summaries, so widening the net would
/// silently reformat files it can't represent.
pub fn is_markdown_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening a file must leave behind a mask that agrees with the tab-expanded
    /// lines the viewer will render, because every navigation query indexes into
    /// that expansion. Driven through `open_file` rather than `CodeMask::compute`
    /// so the expansion and the mask are exercised against each other; the
    /// fixture is tab-indented for exactly that reason.
    #[test]
    fn opening_a_file_masks_its_comments_and_strings() {
        let dir = std::env::temp_dir().join(format!("mask_open_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("sample.go"),
            "package main\n// Server handles things\nfunc Serve() {\n\tname := \"Server\"\n}\n",
        )
        .unwrap();

        let mut vs = ViewerState::default();
        vs.open_file(&dir, "sample.go", 4);

        // Walk each line the way `build_symbol_hints` does and collect the
        // words it would offer as jumpable.
        let mut jumpable: Vec<(usize, String)> = Vec::new();
        for (i, line) in vs.content.file_content.iter().enumerate() {
            let line_1 = i + 1;
            for (k, (_, _, w)) in crate::symbol_index::identifier_occurrences(line).enumerate() {
                if vs.content.code_mask.is_code(line_1, k) {
                    jumpable.push((line_1, w.to_string()));
                }
            }
        }

        // "Server" appears three times: in the comment, as the function's
        // neighbour in code, and inside a string. Only the code one survives.
        let servers: Vec<usize> = jumpable
            .iter()
            .filter(|(_, w)| w == "Server")
            .map(|(line, _)| *line)
            .collect();
        assert!(
            servers.is_empty(),
            "comment and string occurrences of `Server` must not be jumpable, got lines {servers:?}"
        );

        assert!(jumpable.contains(&(3, "func".to_string())));
        assert!(jumpable.contains(&(3, "Serve".to_string())));
        // The tab-indented line still resolves, which is the point of keying
        // the mask by occurrence rather than column.
        assert!(jumpable.contains(&(4, "name".to_string())));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file we have no grammar for offers nothing rather than offering the
    /// previous file's answers or falling back to raw word matching.
    #[test]
    fn opening_an_unsupported_language_clears_the_mask() {
        let dir = std::env::temp_dir().join(format!("mask_unsup_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn keep() {}\n").unwrap();
        std::fs::write(dir.join("b.py"), "def keep():\n    pass\n").unwrap();

        let mut vs = ViewerState::default();
        vs.open_file(&dir, "a.rs", 4);
        assert!(vs.content.code_mask.is_code(1, 0), "Rust file is masked");

        vs.open_file(&dir, "b.py", 4);
        assert!(
            !vs.content.code_mask.is_code(1, 0),
            "Python must not inherit the Rust file's mask"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdown_paths_are_detected_case_insensitively() {
        assert!(is_markdown_path("README.md"));
        assert!(is_markdown_path("docs/plan.markdown"));
        assert!(is_markdown_path("README.MD"));
        assert!(is_markdown_path("a/b/c.Markdown"));
    }

    #[test]
    fn non_markdown_paths_are_rejected() {
        // Neighbours that must NOT get the toggle: a different renderer dialect
        // (.mdx), a no-extension file, and a name that merely contains "md".
        assert!(!is_markdown_path("src/main.rs"));
        assert!(!is_markdown_path("page.mdx"));
        assert!(!is_markdown_path("README"));
        assert!(!is_markdown_path("mdbook.toml"));
        assert!(!is_markdown_path(""));
    }

    /// The toggle is a plain-file-view affordance. Diff mode and the SUMMARY
    /// pseudo-file each own the whole panel with their own renderer, so the
    /// toggle must disappear there — and, critically, `is_showing_rendered_markdown`
    /// must go false with it even while `md_rendered` stays latched, or the
    /// diff view would be drawn with the line-oriented features switched off.
    #[test]
    fn rendered_markdown_is_confined_to_the_plain_file_view() {
        let mut vs = ViewerState::default();
        vs.content.current_file = Some("README.md".to_string());
        vs.md_rendered = true;
        assert!(vs.markdown_toggle_available());
        assert!(vs.is_showing_rendered_markdown());

        vs.diff_view.diff_mode = true;
        assert!(!vs.markdown_toggle_available());
        assert!(!vs.is_showing_rendered_markdown());
        vs.diff_view.diff_mode = false;

        vs.show_summary = true;
        assert!(!vs.markdown_toggle_available());
        assert!(!vs.is_showing_rendered_markdown());
        vs.show_summary = false;

        vs.content.current_file = Some("src/main.rs".to_string());
        assert!(!vs.markdown_toggle_available());
        assert!(!vs.is_showing_rendered_markdown());

        // The mode stayed latched through all of that, so coming back to a
        // markdown file in the plain view resumes rendering (session-sticky).
        vs.content.current_file = Some("CHANGELOG.md".to_string());
        assert!(vs.is_showing_rendered_markdown());
    }

    /// A selection or an open inline reply belongs to the raw view. Carrying
    /// either into the rendered view is silent: the reply box stops being drawn
    /// but keeps intercepting every keystroke, and the selection re-materialises
    /// on the way back.
    #[test]
    fn toggling_tears_down_line_anchored_interactions() {
        let mut vs = ViewerState {
            selection: crate::viewer::LineSelection::Selected { start: 3, end: 9 },
            ..Default::default()
        };
        vs.explorer.inline_reply_line = Some(7);
        vs.explorer.inline_reply_comment_id = Some("c1".to_string());

        vs.toggle_markdown_rendered();

        assert_eq!(vs.selection, crate::viewer::LineSelection::None);
        assert_eq!(vs.explorer.inline_reply_line, None);
        assert_eq!(vs.explorer.inline_reply_comment_id, None);
    }

    /// Jumping to `file:line` must show the line. Rendered prose has none, so
    /// every line-anchored entry point drops back to raw first.
    #[test]
    fn line_targets_drop_out_of_rendered_mode() {
        let mut vs = ViewerState {
            md_rendered: true,
            ..Default::default()
        };
        vs.content.current_file = Some("README.md".to_string());
        assert!(vs.is_showing_rendered_markdown());
        vs.show_raw_for_line_target();
        assert!(!vs.is_showing_rendered_markdown());
        assert!(!vs.md_rendered);
    }

    #[test]
    fn toggling_resets_the_rendered_scroll() {
        let mut vs = ViewerState {
            md_scroll: 42,
            ..Default::default()
        };
        vs.toggle_markdown_rendered();
        assert!(vs.md_rendered);
        assert_eq!(vs.md_scroll, 0);
    }
}
