//! Incremental grep search for [`App`]'s grep-search overlay.
//!
//! Short queries (≤3 chars) run a fast 2-phase search — recently modified
//! files first, then a full search in parallel that replaces the phase-1
//! results — while longer queries go straight to a single full search.
//! Input is debounced by 200ms so rapid typing doesn't spawn a search per
//! keystroke.

use super::*;
use crate::grep_search::GrepProgress;

impl App {
    /// Schedule an incremental grep search with debounce (200ms).
    ///
    /// Called on every keystroke that modifies the query. Sets a deadline;
    /// `check_grep_debounce()` fires the actual search when the deadline passes.
    pub fn schedule_grep_search(&mut self) {
        let query = self.overlays.grep_search.query.text().to_string();
        if query.is_empty() {
            // Clear everything immediately.
            self.overlays.grep_search.result_tree = Default::default();
            self.overlays.grep_search.pending_matches.clear();
            self.overlays.grep_search.selected = 0;
            self.overlays.grep_search.scroll = 0;
            self.overlays.grep_search.running = false;
            self.overlays.grep_search.bg_op.clear();
            self.overlays.grep_search.bg_op_phase2.clear();
            self.overlays.grep_search.debounce_deadline = None;
            self.overlays.grep_search.phase1_active = false;
            return;
        }
        self.overlays.grep_search.debounce_deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
    }

    /// Check if the debounce deadline has passed; if so, start the search.
    /// Returns `true` if a search was started (caller should trigger redraw).
    pub fn check_grep_debounce(&mut self) -> bool {
        if let Some(deadline) = self.overlays.grep_search.debounce_deadline
            && std::time::Instant::now() >= deadline
        {
            self.overlays.grep_search.debounce_deadline = None;
            self.start_incremental_grep_search();
            return true;
        }
        false
    }

    /// Start an incremental grep search.
    ///
    /// For short queries (≤3 chars), uses 2-phase search:
    ///   phase1 — search only recently modified files (fast)
    ///   phase2 — full search (runs in parallel, replaces phase1 results)
    /// For longer queries, runs only a full search.
    fn start_incremental_grep_search(&mut self) {
        let query = self.overlays.grep_search.query.text().to_string();
        if query.is_empty() {
            return;
        }

        let wt_path = match self.worktrees.selected() {
            Some(wt) => wt.path.clone(),
            None => return,
        };

        // Cancel any previous search.
        self.overlays.grep_search.bg_op.clear();
        self.overlays.grep_search.bg_op_phase2.clear();

        // Reset results.
        self.overlays.grep_search.result_tree = Default::default();
        self.overlays.grep_search.pending_matches.clear();
        self.overlays.grep_search.selected = 0;
        self.overlays.grep_search.scroll = 0;
        self.overlays.grep_search.running = true;

        let regex_mode = self.overlays.grep_search.regex_mode;
        let case_sensitive = self.overlays.grep_search.case_sensitive;

        if query.chars().count() <= 3 {
            // 2-phase search for short queries.
            self.overlays.grep_search.phase1_active = true;

            // Get recently modified files (synchronous, fast).
            let recent_files =
                crate::git_engine::recently_modified_files(&wt_path, 200).unwrap_or_default();

            // Phase1: search only recent files.
            if !recent_files.is_empty() {
                let wt1 = wt_path.clone();
                let q1 = query.clone();
                let files1 = recent_files;
                self.overlays.grep_search.bg_op.start(move |tx| {
                    crate::grep_search::run_search_files(
                        &wt1,
                        &q1,
                        regex_mode,
                        case_sensitive,
                        files1,
                        tx,
                    );
                });
            }

            // Phase2: full search (runs in parallel).
            let wt2 = wt_path.clone();
            let q2 = query.clone();
            self.overlays.grep_search.bg_op_phase2.start(move |tx| {
                crate::grep_search::run_search(&wt2, &q2, regex_mode, case_sensitive, tx);
            });
        } else {
            // Single-phase full search for longer queries.
            self.overlays.grep_search.phase1_active = false;
            let wt2 = wt_path.clone();
            let q2 = query.clone();
            self.overlays.grep_search.bg_op.start(move |tx| {
                crate::grep_search::run_search(&wt2, &q2, regex_mode, case_sensitive, tx);
            });
        }
    }

    /// Poll for background grep search results.
    pub fn poll_grep_search(&mut self) {
        let mut tree_dirty = false;

        // Poll phase1 / single-phase bg_op.
        let messages = self.overlays.grep_search.bg_op.poll_all();
        for msg in messages {
            match msg {
                GrepProgress::Results(batch) => {
                    self.overlays.grep_search.pending_matches.extend(batch);
                    tree_dirty = true;
                }
                GrepProgress::Done(total) => {
                    // If phase1 completed but phase2 is still running, keep running = true.
                    if !self.overlays.grep_search.phase1_active
                        || !self.overlays.grep_search.bg_op_phase2.is_running()
                    {
                        self.overlays.grep_search.running = false;
                        self.overlays.grep_search.bg_op.clear();
                        if total >= 5000 {
                            self.set_status(
                                format!("Search truncated at {total} results."),
                                StatusLevel::Warning,
                            );
                        }
                    } else {
                        self.overlays.grep_search.bg_op.clear();
                    }
                }
                GrepProgress::Error(msg) => {
                    self.overlays.grep_search.running = false;
                    self.overlays.grep_search.bg_op.clear();
                    self.set_status(format!("Search error: {msg}"), StatusLevel::Error);
                    return;
                }
            }
        }

        // Poll phase2 bg_op.
        if self.overlays.grep_search.phase1_active {
            let messages2 = self.overlays.grep_search.bg_op_phase2.poll_all();
            let mut got_phase2_results = false;
            for msg in messages2 {
                match msg {
                    GrepProgress::Results(batch) => {
                        if !got_phase2_results {
                            // Replace phase1 results with phase2 results.
                            self.overlays.grep_search.pending_matches.clear();
                            self.overlays.grep_search.selected = 0;
                            self.overlays.grep_search.scroll = 0;
                            self.overlays.grep_search.phase1_active = false;
                            got_phase2_results = true;
                        }
                        self.overlays.grep_search.pending_matches.extend(batch);
                        tree_dirty = true;
                    }
                    GrepProgress::Done(total) => {
                        if !got_phase2_results {
                            self.overlays.grep_search.phase1_active = false;
                        }
                        self.overlays.grep_search.running = false;
                        self.overlays.grep_search.bg_op_phase2.clear();
                        if total >= 5000 {
                            self.set_status(
                                format!("Search truncated at {total} results."),
                                StatusLevel::Warning,
                            );
                        }
                    }
                    GrepProgress::Error(msg) => {
                        self.overlays.grep_search.phase1_active = false;
                        self.overlays.grep_search.running = false;
                        self.overlays.grep_search.bg_op_phase2.clear();
                        self.set_status(format!("Search error: {msg}"), StatusLevel::Error);
                        return;
                    }
                }
            }
        }

        // Rebuild the tree when new results arrived.
        if tree_dirty {
            self.overlays.grep_search.result_tree =
                crate::search_result_tree::SearchResultTree::build(
                    &self.overlays.grep_search.pending_matches,
                );
        }
    }
}
