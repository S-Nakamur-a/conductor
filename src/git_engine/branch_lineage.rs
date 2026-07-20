//! Parent/derived branch detection (via reflog and merge-base heuristics)
//! and building a GitHub/GitLab pull-request URL for a branch.

use super::GitEngine;

impl GitEngine {
    // ── PR URL ───────────────────────────────────────────────────

    /// Build a GitHub/GitLab pull-request URL for the given branch.
    ///
    /// Reads the `origin` remote URL, converts it to an HTTPS base, and
    /// appends the platform-specific path for creating a new pull request.
    /// Returns `None` if the remote URL cannot be parsed.
    /// Detect the parent branch using reflog, falling back to merge-base heuristic.
    ///
    /// 1. Read the reflog for `branch` and find the creation commit (`id_new` of the oldest entry).
    /// 2. For each candidate, check if the creation commit is an ancestor and pick the closest one.
    /// 3. If reflog is unavailable, fall back to merge-base distance from `main_branch`.
    pub fn detect_parent_branch(
        &self,
        branch: &str,
        main_branch: &str,
        candidates: &[String],
    ) -> Option<String> {
        if branch == main_branch {
            return None;
        }

        let branch_oid = self
            .repo
            .find_branch(branch, git2::BranchType::Local)
            .ok()?
            .get()
            .target()?;

        // Try reflog-based detection first.
        if let Some(parent) =
            self.detect_parent_via_reflog(branch, branch_oid, main_branch, candidates)
        {
            return Some(parent);
        }

        // Fallback: find the candidate whose merge-base is closest to branch HEAD.
        self.detect_parent_via_merge_base(branch, branch_oid, main_branch, candidates)
    }

    /// Reflog-based parent detection: find which candidate contains the branch creation point.
    fn detect_parent_via_reflog(
        &self,
        branch: &str,
        _branch_oid: git2::Oid,
        main_branch: &str,
        candidates: &[String],
    ) -> Option<String> {
        let reflog = self.repo.reflog(&format!("refs/heads/{branch}")).ok()?;

        if reflog.is_empty() {
            return None;
        }

        // The oldest entry (last index) records when the branch was created.
        let oldest = reflog.get(reflog.len() - 1)?;
        let creation_oid = oldest.id_new();

        // Build candidate list: provided candidates + main branch.
        let all_candidates: Vec<&str> = candidates
            .iter()
            .map(|s| s.as_str())
            .chain(std::iter::once(main_branch))
            .filter(|&c| c != branch)
            .collect();

        let mut best: Option<(String, usize)> = None;

        for &candidate_name in &all_candidates {
            let candidate_oid = self.resolve_branch_oid(candidate_name)?;

            // Check if the creation commit is an ancestor of the candidate.
            let is_ancestor = self
                .repo
                .graph_descendant_of(candidate_oid, creation_oid)
                .unwrap_or(false);
            let is_same = candidate_oid == creation_oid;

            if !is_ancestor && !is_same {
                continue;
            }

            // Compute distance from creation_oid to candidate HEAD.
            let (ahead, _) = self
                .repo
                .graph_ahead_behind(candidate_oid, creation_oid)
                .unwrap_or((usize::MAX, 0));

            if best.as_ref().is_none_or(|(_, d)| ahead < *d) {
                best = Some((candidate_name.to_string(), ahead));
            }
        }

        // Also check: is the creation commit actually on the branch itself only?
        // If creation_oid == branch_oid and best is main with distance 0, that's fine.
        best.map(|(name, _)| name)
    }

    /// Merge-base fallback: find closest candidate by merge-base distance.
    fn detect_parent_via_merge_base(
        &self,
        branch: &str,
        branch_oid: git2::Oid,
        main_branch: &str,
        candidates: &[String],
    ) -> Option<String> {
        let all_candidates: Vec<&str> = candidates
            .iter()
            .map(|s| s.as_str())
            .chain(std::iter::once(main_branch))
            .filter(|&c| c != branch)
            .collect();

        let mut best: Option<(String, usize)> = None;

        for &candidate_name in &all_candidates {
            let candidate_oid = match self.resolve_branch_oid(candidate_name) {
                Some(oid) => oid,
                None => continue,
            };

            let merge_base = match self.repo.merge_base(branch_oid, candidate_oid) {
                Ok(mb) => mb,
                Err(_) => continue,
            };

            // Distance from merge-base to branch HEAD (how many commits since branching).
            let (ahead, _) = self
                .repo
                .graph_ahead_behind(branch_oid, merge_base)
                .unwrap_or((usize::MAX, 0));

            if best.as_ref().is_none_or(|(_, d)| ahead < *d) {
                best = Some((candidate_name.to_string(), ahead));
            }
        }

        best.map(|(name, _)| name)
    }

    /// Resolve a branch name to its OID, trying local first then origin remote.
    fn resolve_branch_oid(&self, name: &str) -> Option<git2::Oid> {
        // Try local branch first.
        if let Ok(branch) = self.repo.find_branch(name, git2::BranchType::Local)
            && let Some(oid) = branch.get().target()
        {
            return Some(oid);
        }
        // Try origin/<name>.
        let remote_ref = format!("refs/remotes/origin/{name}");
        self.repo.refname_to_id(&remote_ref).ok()
    }

    /// Find branches (from `candidates`) that were forked from the given branch.
    ///
    /// For each candidate, calls `detect_parent_branch` and checks if the result
    /// is the current branch.
    pub fn find_derived_branches(
        &self,
        branch: &str,
        main_branch: &str,
        candidates: &[String],
    ) -> anyhow::Result<Vec<String>> {
        let mut derived = Vec::new();

        for candidate in candidates {
            if candidate == branch {
                continue;
            }
            // Build candidates for the inner detect_parent_branch call:
            // include the current branch + other candidates (excluding the candidate itself).
            let inner_candidates: Vec<String> = candidates
                .iter()
                .filter(|c| c.as_str() != candidate)
                .cloned()
                .collect();

            if let Some(parent) =
                self.detect_parent_branch(candidate, main_branch, &inner_candidates)
                && parent == branch
            {
                derived.push(candidate.clone());
            }
        }

        Ok(derived)
    }

    pub fn pr_url_for_branch(&self, branch: &str) -> Option<String> {
        let remote = self.repo.find_remote("origin").ok()?;
        let raw_url = remote.url()?;
        let base = Self::remote_url_to_https_base(raw_url)?;

        // GitHub: /compare/<branch>  (shows existing PR or create form)
        // GitLab: /-/merge_requests/new?merge_request[source_branch]=<branch>
        if base.contains("gitlab") {
            Some(format!(
                "{base}/-/merge_requests/new?merge_request[source_branch]={branch}",
            ))
        } else {
            // Default to GitHub-style.
            Some(format!("{base}/pull/{branch}"))
        }
    }

    /// Convert a git remote URL to an HTTPS base URL (no trailing slash).
    ///
    /// Handles SSH (`git@host:owner/repo.git`) and HTTPS
    /// (`https://host/owner/repo.git`) formats.
    ///
    /// `pub(crate)` (rather than private) because the unit tests in
    /// `tests.rs` exercise it directly; that submodule sits outside this
    /// one's privacy boundary.
    pub(crate) fn remote_url_to_https_base(url: &str) -> Option<String> {
        let url = url.trim();
        if url.starts_with("git@") || url.starts_with("ssh://") {
            // git@github.com:owner/repo.git  →  https://github.com/owner/repo
            // ssh://git@github.com/owner/repo.git
            let without_prefix = url
                .strip_prefix("ssh://")
                .unwrap_or(url)
                .strip_prefix("git@")
                .unwrap_or(url);
            // "github.com:owner/repo.git" or "github.com/owner/repo.git"
            let normalised = without_prefix.replace(':', "/");
            let trimmed = normalised.trim_end_matches(".git");
            Some(format!("https://{trimmed}"))
        } else if url.starts_with("https://") || url.starts_with("http://") {
            let trimmed = url.trim_end_matches(".git");
            Some(trimmed.to_string())
        } else {
            None
        }
    }
}
