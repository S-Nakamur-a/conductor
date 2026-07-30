//! Path -> git status lookup, computed once per Explorer refresh and shared
//! by the file tree (untracked/ignored dimming, S4) and the Changed files
//! list (stage-state coloring, S5) — see D5 in the plan doc.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use git2::{Repository, Status, StatusOptions, StatusShow};

/// Coarse git classification used for the Explorer file tree's dimming.
/// Doesn't distinguish staged/unstaged — that finer view lives in
/// [`GitStatusMap::status`], used by the Changed files list instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeGitState {
    Tracked,
    Untracked,
    Ignored,
}

/// A one-shot snapshot of `git status`, indexed by path (relative to the
/// worktree root, as reported by libgit2). Built once per Explorer refresh
/// (file-watcher tick / 3s worktree poll), not per frame.
#[derive(Debug, Clone, Default)]
pub struct GitStatusMap {
    by_path: HashMap<String, Status>,
}

impl GitStatusMap {
    /// Load a fresh snapshot for the given worktree.
    pub fn load(worktree_path: &Path) -> Result<Self> {
        let repo = Repository::discover(worktree_path)
            .with_context(|| format!("failed to discover repo from {}", worktree_path.display()))?;

        let mut opts = StatusOptions::new();
        opts.show(StatusShow::IndexAndWorkdir)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(true);
        // Deliberately NOT recurse_ignored_dirs(true). With it on, libgit2
        // walks every file inside an ignored directory instead of reporting
        // the directory as a single collapsed entry. Measured (D5): on the
        // parent repo, whose `target/` is 21GB, that's 2,771ms / 122,407
        // entries versus ~3.1ms with it off. This runs synchronously on the
        // UI thread from the 3s worktree-poll timer, so leaving it on would
        // stall the UI on almost every poll. Do not turn this on without
        // re-measuring on a repo with a large build-output directory.
        let statuses = repo
            .statuses(Some(&mut opts))
            .context("failed to compute git status")?;

        let mut by_path = HashMap::with_capacity(statuses.len());
        for entry in statuses.iter() {
            if let Some(path) = entry.path() {
                by_path.insert(path.to_string(), entry.status());
            }
        }
        Ok(Self { by_path })
    }

    /// Raw git2 status bits for `path` (relative to the worktree root), if
    /// git status reported anything for it. `None` covers both
    /// tracked-and-unchanged files (git status omits those entirely) and
    /// paths nested under a collapsed ignored directory — this accessor
    /// does not walk parent prefixes; use [`classify`](Self::classify) for
    /// that.
    pub fn status(&self, path: &str) -> Option<Status> {
        self.by_path.get(path).copied()
    }

    /// Classify a path as tracked/untracked/ignored, for the file tree's
    /// dimming. `path` may be a file or a directory (no trailing slash
    /// either way — pass tree entries' own `path` field as-is).
    ///
    /// Unlike `status()`, this also walks up parent-directory prefixes:
    /// libgit2 reports an ignored directory as a single collapsed entry
    /// with a trailing slash (e.g. `"target/"`) rather than one entry per
    /// file inside it, so a nested path like `target/debug/foo` has no
    /// entry of its own and must inherit `Ignored` from its ancestor.
    /// Untracked directories don't need this — `recurse_untracked_dirs(true)`
    /// already expands them file by file.
    pub fn classify(&self, path: &str) -> TreeGitState {
        if let Some(status) = self.by_path.get(path) {
            return Self::classify_status(*status);
        }
        // `path` itself may be the collapsed ignored directory (its own
        // tree-entry path has no trailing slash, but libgit2's key does).
        if let Some(status) = self.by_path.get(&format!("{path}/")) {
            return Self::classify_status(*status);
        }
        for ancestor in Self::ancestor_dirs(path) {
            if let Some(status) = self.by_path.get(&ancestor) {
                // Only an ignored ancestor propagates down (a collapsed
                // directory entry). A tracked ancestor tells us nothing
                // about this specific path, since directories themselves
                // are never tracked/untracked in git — keep looking further
                // up in case a grandparent is the collapsed ignored entry.
                if status.is_ignored() {
                    return TreeGitState::Ignored;
                }
            }
        }
        // A directory that git has never seen has no entry of its own:
        // `recurse_untracked_dirs(true)` expands it into its files rather than
        // collapsing it the way ignored directories are collapsed. Reaching
        // here without that check left a brand-new directory drawn in the
        // normal tracked colour while everything inside it was dimmed — the
        // parent looking "known" and the children "new" is backwards.
        if self.has_descendants(path) && self.all_descendants_untracked(path) {
            return TreeGitState::Untracked;
        }
        TreeGitState::Tracked
    }

    /// Whether any status entry lives under `path/`.
    fn has_descendants(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        self.by_path.keys().any(|k| k.starts_with(&prefix))
    }

    /// Whether every status entry under `path/` is untracked. Only meaningful
    /// together with [`has_descendants`] — vacuously true otherwise.
    fn all_descendants_untracked(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        self.by_path
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .all(|(_, s)| s.is_wt_new())
    }

    fn classify_status(status: Status) -> TreeGitState {
        if status.is_ignored() {
            TreeGitState::Ignored
        } else if status.is_wt_new() {
            TreeGitState::Untracked
        } else {
            TreeGitState::Tracked
        }
    }

    /// Yield `path`'s ancestor directory paths with a trailing slash,
    /// closest first, e.g. `"a/b/c.rs"` -> `["a/b/", "a/"]`. The trailing
    /// slash matches how libgit2 reports a collapsed ignored directory
    /// (confirmed empirically — see `status_map_classify_tests`).
    fn ancestor_dirs(path: &str) -> impl Iterator<Item = String> + '_ {
        let mut current = path;
        std::iter::from_fn(move || {
            let slash = current.rfind('/')?;
            current = &current[..slash];
            Some(format!("{current}/"))
        })
    }
}

#[cfg(test)]
mod tests;
