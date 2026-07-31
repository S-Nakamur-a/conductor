//! Worktree switching core for [`App`].
//!
//! Handles selecting a worktree (by index or path), the full
//! `on_worktree_changed` refresh flow (view/session bookkeeping plus
//! dispatching the background file-tree, diff, and branch-details work),
//! polling those background results, and the small helpers (PR url lookup,
//! `gh` availability, worktree-op channel) shared by the other `worktree_*`
//! submodules.

use std::sync::mpsc;

use crate::git_engine::status_map::GitStatusMap;

use super::*;

impl App {
    // ── Worktree create / delete helpers ──────────────────────────

    /// Select a worktree by its path and trigger UI updates.
    ///
    /// `pub(super)` — shared with [`super::worktree_grab`] and [`super::worktree_pr`].
    pub(super) fn select_worktree_by_path(&mut self, path: &std::path::Path) {
        if let Some(idx) = self.worktrees.iter().position(|w| w.path == path) {
            self.selected_worktree = idx;
            self.on_worktree_changed();
        }
    }

    /// Called when the selected worktree changes — refreshes viewer, diff, sessions.
    ///
    /// Heavy operations (file tree walk, diff computation, branch details) are
    /// dispatched to background threads so the UI stays responsive. Results are
    /// applied in `poll_worktree_switch_ops()`.
    /// Switch the selection to the next worktree, wrapping around. Mirrors a
    /// click on the strip (updates views + active sessions via
    /// `on_worktree_changed`, which also makes the strip follow), but keeps the
    /// current panel focus rather than jumping to the terminal. No-op with ≤1
    /// worktree.
    pub fn select_next_worktree(&mut self) {
        let n = self.worktrees.len();
        if n <= 1 {
            return;
        }
        self.selected_worktree = (self.selected_worktree + 1) % n;
        self.on_worktree_changed();
    }

    /// Switch the selection to the previous worktree, wrapping around. See
    /// [`Self::select_next_worktree`].
    pub fn select_prev_worktree(&mut self) {
        let n = self.worktrees.len();
        if n <= 1 {
            return;
        }
        self.selected_worktree = (self.selected_worktree + n - 1) % n;
        self.on_worktree_changed();
    }

    pub fn on_worktree_changed(&mut self) {
        // A reflow transcript belongs to the previous worktree's session;
        // switching worktrees must reset it before new session state loads.
        if self.reflow.active {
            self.close_reflow();
        }

        // An embedded editor belongs to the worktree it was opened on; leaving
        // that worktree would strand it editing the wrong tree, so close it
        // first. The view reload below covers the new worktree.
        self.discard_editor_on_worktree_change();

        // Reveal the newly selected worktree's chip in the bar on the next
        // render (width-dependent panning happens there, where the area is known).
        // This is only safe to set on *user-initiated* selection changes: if a
        // background event ever drives selection while the user is free-scrolling
        // the strip to peek elsewhere, setting this would yank the bar back.
        self.wtbar_reveal_selected = true;

        // Persist the outgoing worktree's view before we wipe it.
        if let Some(outgoing) = self.current_view_branch.clone() {
            self.save_view_for(&outgoing);
        }

        self.viewer_state = ViewerState::default();

        // Rebuild the symbol index over the tree the user is now looking at.
        // Worktrees are siblings of the repository root, so an index built over
        // one of them cannot see any of the others: without this, navigation
        // keeps answering from the previous worktree and lands on the right
        // file at a line number taken from a different branch. The worse the
        // branches have diverged, the further off it is — which puts the error
        // exactly where the diff is most worth reading.
        //
        // Deliberately hung on this method rather than on assignments to
        // `selected_worktree`: several of those are not worktree switches at
        // all (a temporary hop while spawning a session, moving the highlight
        // to open a delete prompt), and two more run on a 3-second poll and on
        // every mouse wheel tick, where a rebuild would pile up.
        self.start_symbol_index_build();

        // The file lists deliberately survive until the background diff lands
        // (swapping them for an empty pane would flicker), but the error must
        // not: it belongs to the worktree we just left, and leaving a red
        // banner up would attribute the outgoing worktree's failure to the
        // incoming one.
        self.diff_state.error = None;

        // Track the worktree now being loaded and seed its saved file/scroll
        // so it gets re-opened once the file tree arrives.
        let new_branch = self.selected_worktree_branch();
        self.pending_view_restore = None;
        self.current_view_branch = if new_branch.is_empty() {
            None
        } else {
            Some(new_branch.clone())
        };
        if let Some(store) = &self.review_store {
            if !new_branch.is_empty() {
                let _ = store.set_selected_worktree(&new_branch);
            }
            if let Ok(Some((Some(file), line))) = store.get_view_state(&new_branch) {
                self.pending_view_restore = Some(crate::app::PendingViewRestore {
                    file,
                    scroll: line.max(0) as usize,
                });
            }
        }

        // Clear "new" badge for the worktree the user just selected.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            self.new_worktree_paths.remove(&wt.path);
        }

        // Reviews are fast (SQLite) — keep synchronous.
        self.refresh_reviews();

        // Snapshot baseline so the next poll cycle doesn't trigger a redundant refresh.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            self.last_poll_head_oid = self.worktree_heads.get(&wt.branch).cloned();
            self.last_poll_status = Some((wt.added, wt.modified, wt.deleted, wt.staged));
        }

        // Update active sessions to match the new worktree.
        let wt_name = self.selected_worktree_branch();
        let claude_sessions = self.current_worktree_claude_sessions();
        self.terminal.active_claude_session = claude_sessions.first().map(|(idx, _)| *idx);
        let shell_sessions = self.current_worktree_shell_sessions();
        self.terminal.active_shell_session = shell_sessions.first().map(|(idx, _)| *idx);

        // Activate the PTY sessions.
        if let Some(idx) = self.terminal.active_claude_session {
            self.terminal.pty_manager.activate_session(idx);
        }
        if let Some(idx) = self.terminal.active_shell_session {
            self.terminal.pty_manager.activate_session(idx);
        }

        self.terminal.scroll_claude = 0;
        self.terminal.scroll_shell = 0;
        self.terminal.cache_claude = Default::default();
        self.terminal.cache_shell = Default::default();

        // Dispatch heavy operations to background threads.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            let wt_path = wt.path.clone();
            let wt_branch = wt.branch.clone();

            // Background file tree walk.
            {
                let path = wt_path.clone();
                self.bg.file_tree.start(move |tx| {
                    // Computed alongside the walk (not on the main thread)
                    // so switching worktrees doesn't add a second, separate
                    // git-status pause — see D5.
                    // Same fallback-and-log rationale as the synchronous path
                    // in `ViewerState::load_file_tree`: an empty map makes the
                    // UI claim everything is tracked and committed, so a
                    // failure here must not pass silently.
                    let git_status = GitStatusMap::load(&path).unwrap_or_else(|e| {
                        log::warn!(
                            "git status unavailable for {} during worktree switch — tree and Changed files will render as if everything is tracked and committed: {e}",
                            path.display()
                        );
                        GitStatusMap::default()
                    });
                    let mut entries = Vec::new();
                    ViewerState::walk_dir(&path, &path, 0, &mut entries, &git_status);
                    let _ = tx.send((entries, git_status));
                });
            }

            // Background diff computation.
            {
                let path = wt_path.clone();
                // Same base as `refresh_diff`: using a different one here would
                // make the file list change out from under the user moments
                // after the switch. `diff_base_for` is the single decision point.
                let base_branch = self.diff_base_for(&wt_branch);
                let word_diff = self.config.diff.word_diff;
                let tab_width = self.config.viewer.tab_width;
                self.bg.diff.start(move |tx| {
                    let _ = tx.send(compute_bg_diff(&path, &base_branch, word_diff, tab_width));
                });
            }

            // Background branch details computation.
            self.start_bg_branch_details();
        }

        self.set_status(
            format!("Switched to worktree: {wt_name}"),
            StatusLevel::Success,
        );
    }

    /// Spawn background branch details computation.
    fn start_bg_branch_details(&mut self) {
        let Some(wt) = self.worktrees.get(self.selected_worktree) else {
            self.branch_details = Default::default();
            return;
        };
        let branch = wt.branch.clone();
        let is_main = wt.is_main;
        let repo_path = self.repo_path.clone();
        let main_branch = self.config.general.main_branch.clone();
        let worktree_branches: Vec<String> = self
            .worktrees
            .iter()
            .filter(|w| !w.is_main && w.branch != branch)
            .map(|w| w.branch.clone())
            .collect();

        // Check DB for cached parent/children before spawning the thread.
        let db_initial_branch = if !is_main {
            self.review_store
                .as_ref()
                .and_then(|store| store.get_worktree_base_branch(&branch).ok().flatten())
        } else {
            None
        };

        let active_branches: std::collections::HashSet<String> =
            self.worktrees.iter().map(|w| w.branch.clone()).collect();
        let db_children: Vec<String> = self
            .review_store
            .as_ref()
            .and_then(|store| store.get_worktree_children(&branch).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|c| active_branches.contains(c))
            .collect();

        // Reset branch_details and start PR lookup (already async).
        self.branch_details = Default::default();
        if !is_main && self.gh_available {
            self.branch_details.pr_loading = true;
            self.start_pr_url_lookup(&branch);
        }

        self.bg.branch_details.start(move |tx| {
            let mut details = git_engine::BranchDetails::default();

            if !is_main {
                details.initial_branch = db_initial_branch.or_else(|| {
                    git_engine::GitEngine::open(&repo_path)
                        .ok()
                        .and_then(|engine| {
                            engine.detect_parent_branch(&branch, &main_branch, &worktree_branches)
                        })
                });
            }

            if !db_children.is_empty() {
                details.derived_branches = db_children;
            } else if let Ok(engine) = git_engine::GitEngine::open(&repo_path)
                && let Ok(derived) =
                    engine.find_derived_branches(&branch, &main_branch, &worktree_branches)
            {
                details.derived_branches = derived;
            }

            let _ = tx.send(details);
        });
    }

    /// Poll background worktree-switch operations (file tree, diff, branch details).
    pub fn poll_worktree_switch_ops(&mut self) {
        // File tree result.
        if let Some((entries, git_status)) = self.bg.file_tree.poll() {
            self.viewer_state.tree.file_tree = entries;
            self.viewer_state.tree.git_status = git_status;
            self.viewer_state.invalidate_visible_cache();
            // Restore the previously viewed file + scroll for this worktree now
            // that its file tree is available (one-shot).
            self.consume_pending_view_restore();
            self.rehighlight_viewer();
        }

        // Diff result.
        if let Some(result) = self.bg.diff.poll() {
            apply_bg_diff_result(&mut self.diff_state, result);
        }

        // Branch details result.
        if let Some(details) = self.bg.branch_details.poll() {
            // Preserve pr_url and pr_loading from the already-running PR lookup.
            let pr_url = self.branch_details.pr_url.take();
            let pr_loading = self.branch_details.pr_loading;
            self.branch_details = details;
            self.branch_details.pr_url = pr_url;
            self.branch_details.pr_loading = pr_loading;
        }
    }

    // ── Branch details (worktree detail panel) ───────────────────

    /// Check whether the `gh` CLI is available on this system.
    pub(super) fn check_gh_available() -> bool {
        std::process::Command::new("gh")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Get (or lazily create) a sender for worktree operation results.
    ///
    /// `pub(super)` — shared with [`super::worktree_crud`] and [`super::worktree_smart`].
    pub(super) fn worktree_op_sender(&mut self) -> mpsc::Sender<WorktreeOpResult> {
        if self.worktree_mgr.bg_worktree_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.worktree_mgr.bg_worktree_tx = Some(tx);
            self.worktree_mgr.bg_worktree_rx = Some(rx);
        }
        self.worktree_mgr.bg_worktree_tx.as_ref().unwrap().clone()
    }

    /// Spawn a background thread to look up the PR URL via `gh pr view`.
    fn start_pr_url_lookup(&mut self, branch: &str) {
        let branch = branch.to_string();
        let repo_path = self.repo_path.clone();

        self.bg.pr_url.start(move |tx| {
            let result = std::process::Command::new("gh")
                .args([
                    "pr", "view", "--head", &branch, "--json", "url", "-q", ".url",
                ])
                .current_dir(&repo_path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .and_then(|output| {
                    if output.status.success() {
                        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if url.is_empty() { None } else { Some(url) }
                    } else {
                        None
                    }
                });
            let _ = tx.send(result);
        });
    }

    /// Poll the background PR URL lookup for a result.
    pub fn poll_pr_url(&mut self) {
        if let Some(result) = self.bg.pr_url.poll() {
            self.branch_details.pr_url = result;
            self.branch_details.pr_loading = false;
        }
    }
}

/// Compute both diff ranges for the background worktree-switch worker.
///
/// Lifted out of the worker closure so it can be exercised directly: the rule it
/// encodes — a committed failure records the error but must not stop the
/// uncommitted diff, which doesn't depend on the base ref — is the one this
/// module got wrong, and inside a `bg.diff.start` closure nothing could reach it
/// to check. Mirrors [`DiffState::load_diff`]'s handling of the same two ranges.
fn compute_bg_diff(
    path: &std::path::Path,
    base_branch: &str,
    word_diff: bool,
    tab_width: usize,
) -> BgDiffResult {
    let mut result = BgDiffResult {
        committed: Vec::new(),
        uncommitted: Vec::new(),
        error: None,
    };
    match DiffState::compute_diff_range_static(path, base_branch, true, word_diff, tab_width) {
        Ok(mut files) => {
            files.sort_by(|a, b| a.path.cmp(&b.path));
            result.committed = files;
        }
        Err(e) => result.error = Some(format!("{e:#}")),
    }
    match DiffState::compute_diff_range_static(path, base_branch, false, word_diff, tab_width) {
        Ok(mut files) => {
            files.sort_by(|a, b| a.path.cmp(&b.path));
            result.uncommitted = files;
        }
        // Non-fatal on its own: the committed half may still be worth showing.
        Err(e) => log::warn!("failed to compute uncommitted diff: {e:#}"),
    }
    result
}

/// Copy a finished background diff into the [`DiffState`].
///
/// A free function rather than a `DiffState` method so it can be unit-tested
/// without building a whole `App`, and so `diff_state` never has to depend on
/// `app::types::BgDiffResult` — that would invert the module dependency.
///
/// Both file lists are applied unconditionally, including when `error` is set:
/// the error only ever comes from resolving the base ref, which the uncommitted
/// list (HEAD vs workdir+index) does not depend on. Clearing it alongside the
/// committed list is what made a bad base ref look identical to a clean tree.
fn apply_bg_diff_result(diff_state: &mut DiffState, result: BgDiffResult) {
    diff_state.committed_files = result.committed;
    diff_state.uncommitted_files = result.uncommitted;
    diff_state.error = result.error;
    diff_state.rebuild_display_list();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_state::{DiffViewMode, FileDiff};

    fn file(path: &str) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            added_lines: 1,
            deleted_lines: 0,
            hunks: Vec::new(),
        }
    }

    /// The reported bug: a base ref that won't resolve used to wipe the
    /// uncommitted list too, so 17 modified files rendered as `(0)`.
    #[test]
    fn bg_diff_result_with_error_keeps_uncommitted() {
        let mut ds = DiffState::new("origin/main", DiffViewMode::Unified);
        apply_bg_diff_result(
            &mut ds,
            BgDiffResult {
                committed: Vec::new(),
                uncommitted: vec![file("CLAUDE.md"), file("src/config.rs")],
                error: Some("base ref 'origin/main' not found".to_string()),
            },
        );

        assert!(ds.committed_files.is_empty());
        assert_eq!(
            ds.uncommitted_files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["CLAUDE.md", "src/config.rs"],
        );
        assert!(ds.error.is_some(), "the failure must stay visible");

        // Resolve every File entry back through the display list rather than
        // just checking it's non-empty. `diff_list.rs` indexes
        // `committed_files`/`uncommitted_files` by the entry's `file_index`, so
        // a display list left un-rebuilt after swapping the file vectors is an
        // out-of-bounds panic waiting for the next render — this pins the
        // "always rebuild" invariant, not merely "something got listed".
        let listed: Vec<&str> = (0..ds.display_list.len())
            .filter_map(|idx| ds.resolve_file(idx))
            .map(|(f, _section)| f.path.as_str())
            .collect();
        // Directory-grouped order, not input order: files under a directory
        // node come before top-level ones.
        assert_eq!(listed, vec!["src/config.rs", "CLAUDE.md"]);
    }

    /// Build a repo with one commit on `main`, HEAD on `feature`, and an
    /// uncommitted file in the worktree. Returns the tempdir (kept alive by the
    /// caller) — every path below needs a real repo, not a hand-built struct.
    fn repo_with_uncommitted_change() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        let blob = repo.blob(b"a").unwrap();
        let mut tb = repo.treebuilder(None).unwrap();
        tb.insert("a.txt", blob, 0o100644).unwrap();
        let tree = repo.find_tree(tb.write().unwrap()).unwrap();
        let oid = repo
            .commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        let commit = repo.find_commit(oid).unwrap();
        repo.branch("feature", &commit, true).unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        std::fs::write(dir.path().join("dirty.txt"), b"uncommitted").unwrap();
        dir
    }

    /// The bg worker's half of the fix. `apply_bg_diff_result` only proves the
    /// *reporting* side; this proves the worker still computes the uncommitted
    /// diff after the committed one fails. Put the `return` back in the Err arm
    /// and this is the test that catches it.
    #[test]
    fn compute_bg_diff_keeps_uncommitted_when_base_is_unresolvable() {
        let dir = repo_with_uncommitted_change();

        let result = compute_bg_diff(dir.path(), "no-such-base", false, 4);

        let err = result.error.as_deref().expect("base failure must be recorded");
        assert!(err.contains("no-such-base"), "error was: {err}");
        assert!(result.committed.is_empty());
        assert_eq!(
            result
                .uncommitted
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["dirty.txt"],
        );
    }

    /// A resolvable base leaves no error behind, so the panel shows no banner.
    #[test]
    fn compute_bg_diff_reports_no_error_for_a_resolvable_base() {
        let dir = repo_with_uncommitted_change();

        let result = compute_bg_diff(dir.path(), "main", false, 4);

        assert_eq!(result.error, None);
        assert!(result.committed.is_empty(), "feature == main, so nothing committed");
        assert_eq!(
            result
                .uncommitted
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            vec!["dirty.txt"],
        );
    }

    /// A later successful result must clear a stale error, or the panel would
    /// keep the error marker forever.
    #[test]
    fn bg_diff_result_without_error_clears_a_stale_one() {
        let mut ds = DiffState::new("main", DiffViewMode::Unified);
        ds.error = Some("previous failure".to_string());
        apply_bg_diff_result(
            &mut ds,
            BgDiffResult {
                committed: vec![file("src/main.rs")],
                uncommitted: Vec::new(),
                error: None,
            },
        );

        assert!(ds.error.is_none());
        assert_eq!(ds.committed_files.len(), 1);
    }
}
