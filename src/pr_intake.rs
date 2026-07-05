//! PR intake orchestration: turn a PR number or GitHub PR URL into a local
//! worktree ready for review mode.
//!
//! Shells out to `gh` for PR metadata (relies on the user's existing `gh
//! auth` session) and to [`crate::git_engine::GitEngine`] for the fetch and
//! worktree creation. Kept separate from `app/worktree.rs` so the exact
//! `gh`/`git` command spelling (JSON fields, refspec format, branch naming)
//! lives in one place.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::git_engine::GitEngine;

/// Local branch name for a fetched PR head: `pr-<N>` (hyphen, not `pr/<N>` —
/// the latter would collide with any existing `pr/`-namespaced branches a
/// repo already uses for its own work).
pub fn local_branch_name(pr_number: u64) -> String {
    format!("pr-{pr_number}")
}

/// PR metadata as reported by `gh pr view --json ...`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PrMeta {
    /// Echo of the requested PR number; deserialized for completeness but
    /// callers thread the number they asked for instead.
    #[allow(dead_code)]
    pub number: u64,
    pub title: String,
    #[serde(rename = "headRefName")]
    pub head_ref: String,
    #[serde(rename = "baseRefName")]
    pub base_ref: String,
    #[serde(rename = "headRepositoryOwner")]
    pub head_owner: Option<GhOwner>,
    pub url: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GhOwner {
    pub login: String,
}

/// Cause of a PR-intake failure, classified from `gh`/`git` output — each
/// variant maps to a distinct, actionable message so the input overlay can
/// tell the user what to do next instead of showing a raw error string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrIntakeError {
    GhNotInstalled,
    GhNotAuthenticated,
    PrNotFound(u64),
    NetworkError,
    InvalidInput(String),
    Other(String),
}

impl std::fmt::Display for PrIntakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrIntakeError::GhNotInstalled => write!(
                f,
                "`gh` (GitHub CLI) is not installed. Install it from https://cli.github.com/ and retry."
            ),
            PrIntakeError::GhNotAuthenticated => {
                write!(f, "Not logged in to GitHub CLI. Run `gh auth login` and retry.")
            }
            PrIntakeError::PrNotFound(n) => write!(f, "Pull request #{n} not found."),
            PrIntakeError::NetworkError => write!(
                f,
                "Network error while contacting GitHub. Check your connection and retry."
            ),
            PrIntakeError::InvalidInput(s) => {
                write!(f, "Not a valid PR number or GitHub PR URL: {s}")
            }
            PrIntakeError::Other(s) => write!(f, "{s}"),
        }
    }
}

/// Classify `gh`/`git` failure text into an actionable [`PrIntakeError`].
/// `pr_number` is threaded through so a "not found" match can name the PR.
fn classify_failure_text(pr_number: u64, text: &str) -> PrIntakeError {
    let lower = text.to_lowercase();
    if lower.contains("gh auth login") || lower.contains("not logged in") || lower.contains("authentication") {
        PrIntakeError::GhNotAuthenticated
    } else if lower.contains("could not resolve host")
        || lower.contains("connection timed out")
        || lower.contains("could not read from remote repository")
        || lower.contains("network")
        || lower.contains("dial tcp")
        || lower.contains("timeout")
    {
        PrIntakeError::NetworkError
    } else if lower.contains("no pull requests found")
        || lower.contains("could not find")
        || lower.contains("couldn't find remote ref")
        || lower.contains("not found")
    {
        PrIntakeError::PrNotFound(pr_number)
    } else {
        PrIntakeError::Other(text.trim().to_string())
    }
}

/// Parse a PR number or a GitHub PR URL (e.g.
/// `https://github.com/owner/repo/pull/123`, with or without a trailing
/// path/query) into a bare PR number.
pub fn parse_pr_input(input: &str) -> Result<u64, PrIntakeError> {
    let trimmed = input.trim();
    if let Ok(n) = trimmed.parse::<u64>() {
        return Ok(n);
    }
    if let Some(idx) = trimmed.find("/pull/") {
        let rest = &trimmed[idx + "/pull/".len()..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty()
            && let Ok(n) = digits.parse::<u64>()
        {
            return Ok(n);
        }
    }
    Err(PrIntakeError::InvalidInput(trimmed.to_string()))
}

/// Whether a ref name (as reported by `gh`'s JSON output) is unsafe to hand
/// to git as a bare argument — a leading `-` would let it be parsed as an
/// option rather than a ref name.
fn is_suspicious_ref(ref_name: &str) -> bool {
    ref_name.starts_with('-')
}

/// Whether `dir` looks like a real git worktree rather than a stray/broken
/// directory (e.g. left over from an interrupted intake, or an empty dir a
/// user created by hand). In a worktree, `.git` is a file (a gitdir pointer),
/// not a directory, but either is accepted here — the check only needs to
/// rule out "definitely not a worktree", not fully validate git's internals.
fn is_valid_worktree_dir(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Fetch PR metadata via `gh pr view <N> --json ...`.
fn fetch_pr_meta(pr_number: u64) -> Result<PrMeta, PrIntakeError> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--json",
            "number,title,headRefName,baseRefName,headRepositoryOwner,url",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(PrIntakeError::GhNotInstalled);
        }
        Err(e) => return Err(PrIntakeError::Other(e.to_string())),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(classify_failure_text(pr_number, &stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str::<PrMeta>(&stdout)
        .map_err(|e| PrIntakeError::Other(format!("failed to parse `gh pr view` output: {e}")))
}

/// PR metadata to persist once a fresh fetch/create succeeds. `None` on a
/// worktree re-entry hit, since the prior intake already persisted it and no
/// `gh` call was made this time.
pub struct FetchedPr {
    pub branch: String,
    pub title: String,
    pub base_ref: String,
    pub head_ref: String,
    pub url: String,
    pub head_owner_login: Option<String>,
}

/// Outcome of a completed PR intake attempt.
pub enum PrIntakeOutcome {
    /// A worktree is ready — either freshly fetched/created, or reused from
    /// a prior intake of the same PR number. The caller should switch to
    /// `worktree_path`, persist `meta` (if present) to the review store, and
    /// enter review mode.
    ///
    /// DB writes are deliberately left to the caller (main thread) rather
    /// than done here, since [`crate::review_store::ReviewStore`] lives on
    /// `App` and isn't meant to be shared across threads mid-flight.
    Ready {
        pr_number: u64,
        worktree_path: PathBuf,
        meta: Option<FetchedPr>,
    },
    Failed { error: PrIntakeError },
}

/// Run the full PR intake flow synchronously: resolve `input` to a PR
/// number, and fetch (or reuse) its worktree.
///
/// Intended to run on a background thread (network + git I/O); the caller
/// polls for the returned outcome and applies it (including any DB writes)
/// on the main thread.
///
/// Re-entry: if a worktree for this PR number already exists on disk, it is
/// reused as-is — no `gh`/`git fetch` round-trip, and no auto-fast-forward
/// of the existing checkout (matches the "open pull request" precedent of
/// never surprising the user with a silent branch update).
pub fn intake_pr(
    repo_path: &Path,
    worktree_dir_override: Option<&Path>,
    input: &str,
) -> PrIntakeOutcome {
    let pr_number = match parse_pr_input(input) {
        Ok(n) => n,
        Err(error) => return PrIntakeOutcome::Failed { error },
    };

    let engine = match GitEngine::open(repo_path) {
        Ok(e) => e,
        Err(e) => {
            return PrIntakeOutcome::Failed {
                error: PrIntakeError::Other(e.to_string()),
            };
        }
    };

    let branch = local_branch_name(pr_number);
    let wt_dir = match engine.worktrees_base_dir(worktree_dir_override) {
        Ok(base) => base.join(&branch),
        Err(e) => {
            return PrIntakeOutcome::Failed {
                error: PrIntakeError::Other(e.to_string()),
            };
        }
    };

    if wt_dir.exists() {
        if !is_valid_worktree_dir(&wt_dir) {
            return PrIntakeOutcome::Failed {
                error: PrIntakeError::Other(format!(
                    "Existing directory is not a valid worktree: {}. Delete it and re-run intake.",
                    wt_dir.display()
                )),
            };
        }
        return PrIntakeOutcome::Ready {
            pr_number,
            worktree_path: wt_dir,
            meta: None,
        };
    }

    let pr_meta = match fetch_pr_meta(pr_number) {
        Ok(m) => m,
        Err(error) => return PrIntakeOutcome::Failed { error },
    };

    let refspec = format!("pull/{pr_number}/head:{branch}");
    if let Err(e) = engine.fetch_refspec(&refspec) {
        return PrIntakeOutcome::Failed {
            error: classify_failure_text(pr_number, &e.to_string()),
        };
    }

    if let Err(e) = engine.create_worktree_for_existing_branch(&branch, &wt_dir) {
        return PrIntakeOutcome::Failed {
            error: PrIntakeError::Other(e.to_string()),
        };
    }

    // Best-effort: review mode's diff still works off whatever local base
    // ref already exists if this fails, so don't fail the whole intake over it.
    // Guard against a base_ref starting with '-' before it reaches git: it
    // comes from `gh`'s JSON output, and a leading dash would let it be
    // misread as a git option rather than a ref name.
    if is_suspicious_ref(&pr_meta.base_ref) {
        log::warn!(
            "pr_intake: refusing to fetch suspicious base ref '{}' (starts with '-')",
            pr_meta.base_ref
        );
    } else if let Err(e) = engine.ensure_base_ref_available(&pr_meta.base_ref) {
        log::warn!(
            "pr_intake: failed to ensure base ref '{}' is available: {e}",
            pr_meta.base_ref
        );
    }

    PrIntakeOutcome::Ready {
        pr_number,
        worktree_path: wt_dir,
        meta: Some(FetchedPr {
            branch,
            title: pr_meta.title,
            base_ref: pr_meta.base_ref,
            head_ref: pr_meta.head_ref,
            url: pr_meta.url,
            head_owner_login: pr_meta.head_owner.map(|o| o.login),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_suspicious_ref_rejects_leading_dash() {
        assert!(is_suspicious_ref("--upload-pack=evil"));
        assert!(!is_suspicious_ref("main"));
        assert!(!is_suspicious_ref("release/1.0"));
    }

    #[test]
    fn parse_pr_input_accepts_bare_number() {
        assert_eq!(parse_pr_input("279"), Ok(279));
        assert_eq!(parse_pr_input("  42  "), Ok(42));
    }

    #[test]
    fn parse_pr_input_accepts_github_url() {
        assert_eq!(
            parse_pr_input("https://github.com/S-Nakamur-a/conductor/pull/279"),
            Ok(279)
        );
        assert_eq!(
            parse_pr_input("https://github.com/o/r/pull/12/files"),
            Ok(12)
        );
    }

    #[test]
    fn parse_pr_input_rejects_garbage() {
        assert_eq!(
            parse_pr_input("not-a-pr"),
            Err(PrIntakeError::InvalidInput("not-a-pr".to_string()))
        );
    }

    #[test]
    fn classify_failure_text_detects_auth() {
        assert_eq!(
            classify_failure_text(1, "To get started with GitHub CLI, please run:  gh auth login"),
            PrIntakeError::GhNotAuthenticated
        );
    }

    #[test]
    fn classify_failure_text_detects_not_found() {
        assert_eq!(
            classify_failure_text(404, "no pull requests found for branch \"x\""),
            PrIntakeError::PrNotFound(404)
        );
        assert_eq!(
            classify_failure_text(5, "fatal: couldn't find remote ref pull/5/head"),
            PrIntakeError::PrNotFound(5)
        );
    }

    #[test]
    fn classify_failure_text_detects_network_error() {
        assert_eq!(
            classify_failure_text(1, "fatal: unable to access: Could not resolve host: github.com"),
            PrIntakeError::NetworkError
        );
    }

    #[test]
    fn classify_failure_text_falls_back_to_other() {
        assert_eq!(
            classify_failure_text(1, "something unexpected happened"),
            PrIntakeError::Other("something unexpected happened".to_string())
        );
    }

    #[test]
    fn display_messages_are_actionable() {
        assert!(
            PrIntakeError::GhNotAuthenticated
                .to_string()
                .contains("gh auth login")
        );
        assert!(PrIntakeError::PrNotFound(9).to_string().contains('9'));
    }

    /// Re-entry: intake_pr must reuse an existing worktree directory without
    /// touching gh/git at all (so it works even without `gh` installed).
    #[test]
    fn intake_pr_reenters_existing_worktree_without_gh_or_network() {
        let parent = tempfile::tempdir().unwrap();
        // Canonicalize so this compares equal on platforms (e.g. macOS) where
        // the OS temp dir is itself a symlink (`/tmp` -> `/private/tmp`).
        let parent_path = parent.path().canonicalize().unwrap();
        let repo_path = parent_path.join("repo");
        git2::Repository::init(&repo_path).unwrap();

        // Simulate a worktree that a prior intake already created for PR 42
        // (a real worktree's `.git` is a gitdir-pointer file, not a directory).
        let base_dir = parent_path.join("repo-worktrees");
        std::fs::create_dir_all(base_dir.join("pr-42")).unwrap();
        std::fs::write(base_dir.join("pr-42").join(".git"), "gitdir: /tmp/fake").unwrap();

        let outcome = intake_pr(&repo_path, None, "42");
        match outcome {
            PrIntakeOutcome::Ready {
                pr_number,
                worktree_path,
                ..
            } => {
                assert_eq!(pr_number, 42);
                assert_eq!(worktree_path, base_dir.join("pr-42"));
            }
            PrIntakeOutcome::Failed { error } => panic!("expected Ready, got Failed: {error}"),
        }
    }

    /// A stale/broken directory left at the worktree path (no `.git`) must
    /// fail with an actionable message rather than silently returning `Ready`
    /// and showing a blank review screen.
    #[test]
    fn intake_pr_reenters_broken_directory_fails_with_actionable_message() {
        let parent = tempfile::tempdir().unwrap();
        let parent_path = parent.path().canonicalize().unwrap();
        let repo_path = parent_path.join("repo");
        git2::Repository::init(&repo_path).unwrap();

        let base_dir = parent_path.join("repo-worktrees");
        let broken_dir = base_dir.join("pr-42");
        std::fs::create_dir_all(&broken_dir).unwrap();

        let outcome = intake_pr(&repo_path, None, "42");
        match outcome {
            PrIntakeOutcome::Failed { error } => {
                assert!(error.to_string().contains(&broken_dir.display().to_string()));
            }
            PrIntakeOutcome::Ready { .. } => panic!("expected Failed, got Ready"),
        }
    }

    /// End-to-end check against this repository's real GitHub remote and a
    /// merged PR (`refs/pull/<N>/head` stays resolvable after merge). Needs
    /// network + an authenticated `gh`, so it's `#[ignore]`d by default; run
    /// explicitly with `cargo test --  --ignored intake_pr_against_real_pr`.
    /// Clones into a tempdir so it never touches this repo's own worktrees.
    #[test]
    #[ignore]
    fn intake_pr_against_real_pr() {
        let parent = tempfile::tempdir().unwrap();
        let repo_path = parent.path().join("repo");
        let mut fetch_opts = git2::FetchOptions::new();
        fetch_opts.depth(50);
        git2::build::RepoBuilder::new()
            .fetch_options(fetch_opts)
            .clone("https://github.com/S-Nakamur-a/conductor.git", &repo_path)
            .expect("clone should succeed");

        let outcome = intake_pr(&repo_path, None, "279");
        match outcome {
            PrIntakeOutcome::Ready {
                pr_number,
                worktree_path,
                meta,
            } => {
                assert_eq!(pr_number, 279);
                assert!(worktree_path.exists());
                let meta = meta.expect("fresh intake should carry fetched metadata");
                assert_eq!(meta.base_ref, "main");
                assert_eq!(meta.branch, "pr-279");
            }
            PrIntakeOutcome::Failed { error } => panic!("expected Ready, got Failed: {error}"),
        }
    }
}
