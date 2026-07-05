//! Publishing review comments back to GitHub via `gh api`.
//!
//! Kept separate from `app/review_publish.rs` (the `App`-side orchestration:
//! confirm-overlay state, background-thread spawn, DB writes) the same way
//! `pr_intake.rs` is kept separate from `app/worktree.rs` — the exact `gh`
//! CLI/JSON spelling lives in one place, and everything here is plain data +
//! `gh` subprocess calls with no `App` dependency, so it can be unit tested
//! without a running application.

use std::io::Write;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::diff_state::DiffState;

/// One review comment ready to publish: the parent comment's body with its
/// replies appended. GitHub review comments don't have a v1 concept of
/// threaded replies here, so a comment with replies is flattened into one
/// GitHub comment body (see ADR-6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishComment {
    pub id: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: Option<u32>,
    pub body: String,
}

/// A confirmed, ready-to-run publish request: everything [`publish`] needs,
/// already resolved by the caller (owner/repo from `pr_review_meta.pr_url`,
/// comments already filtered to lines that are actually in the diff).
pub struct PublishRequest {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub comments: Vec<PublishComment>,
}

/// State backing `App::publish_confirm`'s y/n overlay: the already-filtered
/// comments a confirmed publish will send, plus how many were skipped for
/// not being on a line the current diff covers. Doubles as the source for
/// building the [`PublishRequest`] once the user confirms.
pub struct PublishConfirm {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub comments: Vec<PublishComment>,
    pub skipped: usize,
}

/// Result of a publish attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    /// Every comment in the request was posted (or there was nothing to post).
    Succeeded { published_ids: Vec<String> },
    /// The batch review call failed and the per-comment fallback posted some
    /// but not all of the comments.
    PartialFailure {
        published_ids: Vec<String>,
        failed: Vec<(String, String)>,
    },
    /// Nothing was posted (couldn't resolve the commit id, or every attempt
    /// — batch and fallback — failed).
    Failed { error: String },
}

/// Split `comments` into what's safe to publish and how many were skipped.
///
/// A GitHub review comment must anchor to a line that's part of the current
/// diff's hunks; posting even one comment on a line outside the diff fails
/// the *entire* batch with a 422, so out-of-diff comments must be filtered
/// out before ever reaching [`publish`], not just discarded on failure.
pub fn filter_publishable(
    comments: Vec<PublishComment>,
    diff: &DiffState,
) -> (Vec<PublishComment>, usize) {
    let mut publishable = Vec::new();
    let mut skipped = 0;
    for c in comments {
        let end = c.line_end.unwrap_or(c.line_start);
        if line_range_in_diff(&c.file_path, c.line_start, end, diff) {
            publishable.push(c);
        } else {
            skipped += 1;
        }
    }
    (publishable, skipped)
}

/// Whether `[start, end]` (new-side line numbers) both fall within the same
/// diff hunk for `file_path`, across either diff section (committed or
/// uncommitted — a review comment doesn't record which section it was made
/// against, so both are checked).
fn line_range_in_diff(file_path: &str, start: u32, end: u32, diff: &DiffState) -> bool {
    diff.committed_files
        .iter()
        .chain(diff.uncommitted_files.iter())
        .filter(|fd| fd.path == file_path)
        .any(|fd| {
            fd.hunks.iter().any(|hunk| {
                let mut has_start = false;
                let mut has_end = false;
                for line in &hunk.lines {
                    if let Some(n) = line.new_line_no {
                        has_start |= n as u32 == start;
                        has_end |= n as u32 == end;
                    }
                }
                has_start && has_end
            })
        })
}

/// Parse `owner`/`repo` out of a GitHub PR URL
/// (`https://github.com/{owner}/{repo}/pull/{n}`).
pub fn owner_repo_from_pr_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.splitn(3, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

// ---------------------------------------------------------------------------
// Payload shapes (see ADR-6's greta-grounded field spelling)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ReviewCommentPayload<'a> {
    path: &'a str,
    body: &'a str,
    line: u32,
    side: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_side: Option<&'static str>,
}

impl<'a> ReviewCommentPayload<'a> {
    fn from_comment(c: &'a PublishComment) -> Self {
        let end = c.line_end.unwrap_or(c.line_start);
        if end == c.line_start {
            ReviewCommentPayload {
                path: &c.file_path,
                body: &c.body,
                line: c.line_start,
                side: "RIGHT",
                start_line: None,
                start_side: None,
            }
        } else {
            ReviewCommentPayload {
                path: &c.file_path,
                body: &c.body,
                line: end,
                side: "RIGHT",
                start_line: Some(c.line_start),
                start_side: Some("RIGHT"),
            }
        }
    }
}

/// `POST /repos/{owner}/{repo}/pulls/{N}/reviews` body.
#[derive(Serialize)]
struct BatchReviewPayload<'a> {
    commit_id: &'a str,
    event: &'static str,
    body: &'static str,
    comments: Vec<ReviewCommentPayload<'a>>,
}

/// `POST /repos/{owner}/{repo}/pulls/{N}/comments` body (single-comment
/// fallback — always accepts `line`/`side`, unlike the batch endpoint's
/// unverified acceptance of them per ADR-6).
#[derive(Serialize)]
struct SingleCommentPayload<'a> {
    commit_id: &'a str,
    #[serde(flatten)]
    comment: ReviewCommentPayload<'a>,
}

// ---------------------------------------------------------------------------
// gh CLI plumbing
// ---------------------------------------------------------------------------

/// Fetch the PR's head commit sha via `gh pr view <N> --json headRefOid`,
/// explicitly rather than letting the review API default to whatever HEAD
/// happens to be at post time — the default is a race with a concurrent push.
fn fetch_head_commit_id(pr_number: u64) -> Result<String, String> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--json",
            "headRefOid",
            "-q",
            ".headRefOid",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let oid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if oid.is_empty() {
        return Err("`gh pr view` returned an empty headRefOid".to_string());
    }
    Ok(oid)
}

/// `gh api <path> --input -` — the JSON body is piped over stdin rather than
/// passed as a CLI argument, since a comment body can be long/multiline.
fn gh_api_post(path: &str, body: &str) -> Result<(), GhApiError> {
    let mut child = Command::new("gh")
        .args(["api", path, "--input", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GhApiError {
            is_422: false,
            message: e.to_string(),
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.as_bytes());
    }
    let output = child.wait_with_output().map_err(|e| GhApiError {
        is_422: false,
        message: e.to_string(),
    })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(GhApiError {
        is_422: stderr.contains("422"),
        message: stderr,
    })
}

struct GhApiError {
    /// Whether the failure looks like GitHub's 422 Unprocessable Entity —
    /// the signal to fall back to single-comment posting (ADR-6). `gh api`
    /// reports this as e.g. `gh: Validation Failed (HTTP 422): ...` on
    /// stderr; matching the substring is the least brittle check available
    /// without parsing `gh`'s own error formatting.
    is_422: bool,
    message: String,
}

/// Run a publish request: resolve the commit id, POST the batch review, and
/// fall back to posting comments one at a time if the batch is rejected as a
/// 422 (per ADR-6, the batch endpoint's acceptance of `line`/`side` was the
/// one thing left unverified by the pre-implementation `gh api` grounding).
pub fn publish(req: PublishRequest) -> PublishOutcome {
    if req.comments.is_empty() {
        return PublishOutcome::Succeeded {
            published_ids: Vec::new(),
        };
    }

    let commit_id = match fetch_head_commit_id(req.pr_number) {
        Ok(id) => id,
        Err(error) => return PublishOutcome::Failed { error },
    };

    let payload = BatchReviewPayload {
        commit_id: &commit_id,
        event: "COMMENT",
        body: "",
        comments: req
            .comments
            .iter()
            .map(ReviewCommentPayload::from_comment)
            .collect(),
    };
    let body = match serde_json::to_string(&payload) {
        Ok(b) => b,
        Err(e) => {
            return PublishOutcome::Failed {
                error: format!("failed to build request body: {e}"),
            };
        }
    };
    let reviews_path = format!(
        "repos/{}/{}/pulls/{}/reviews",
        req.owner, req.repo, req.pr_number
    );

    match gh_api_post(&reviews_path, &body) {
        Ok(()) => PublishOutcome::Succeeded {
            published_ids: req.comments.iter().map(|c| c.id.clone()).collect(),
        },
        Err(e) if e.is_422 => publish_fallback(&req, &commit_id),
        Err(e) => PublishOutcome::Failed { error: e.message },
    }
}

/// Post each comment individually to `/pulls/{N}/comments`, which accepts
/// `line`/`side` unconditionally — the fallback for a batch review the
/// GitHub API rejected.
fn publish_fallback(req: &PublishRequest, commit_id: &str) -> PublishOutcome {
    let comments_path = format!(
        "repos/{}/{}/pulls/{}/comments",
        req.owner, req.repo, req.pr_number
    );
    let mut published_ids = Vec::new();
    let mut failed = Vec::new();
    for c in &req.comments {
        let payload = SingleCommentPayload {
            commit_id,
            comment: ReviewCommentPayload::from_comment(c),
        };
        let body = match serde_json::to_string(&payload) {
            Ok(b) => b,
            Err(e) => {
                failed.push((c.id.clone(), e.to_string()));
                continue;
            }
        };
        match gh_api_post(&comments_path, &body) {
            Ok(()) => published_ids.push(c.id.clone()),
            Err(e) => failed.push((c.id.clone(), e.message)),
        }
    }
    if failed.is_empty() {
        PublishOutcome::Succeeded { published_ids }
    } else {
        PublishOutcome::PartialFailure {
            published_ids,
            failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_state::{DiffHunk, DiffLine, DiffLineTag, DiffViewMode, FileDiff};

    fn diff_with_hunk(path: &str, new_lines: &[usize]) -> DiffState {
        let mut ds = DiffState::new("main", DiffViewMode::Unified);
        let lines = new_lines
            .iter()
            .map(|&n| DiffLine {
                tag: DiffLineTag::Insert,
                old_line_no: None,
                new_line_no: Some(n),
                inline_segments: Vec::new(),
                content: String::new(),
            })
            .collect();
        ds.committed_files = vec![FileDiff {
            path: path.to_string(),
            added_lines: new_lines.len(),
            deleted_lines: 0,
            is_new: false,
            is_deleted: false,
            hunks: vec![DiffHunk {
                lines,
                func_header: None,
            }],
        }];
        ds
    }

    fn comment(file_path: &str, line_start: u32, line_end: Option<u32>) -> PublishComment {
        PublishComment {
            id: format!("{file_path}:{line_start}"),
            file_path: file_path.to_string(),
            line_start,
            line_end,
            body: "looks good".to_string(),
        }
    }

    #[test]
    fn filter_publishable_keeps_single_line_on_diff() {
        let diff = diff_with_hunk("src/a.rs", &[10, 11, 12]);
        let (kept, skipped) = filter_publishable(vec![comment("src/a.rs", 11, None)], &diff);
        assert_eq!(kept.len(), 1);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn filter_publishable_keeps_range_with_both_endpoints_in_hunk() {
        let diff = diff_with_hunk("src/a.rs", &[10, 11, 12]);
        let (kept, skipped) = filter_publishable(vec![comment("src/a.rs", 10, Some(12))], &diff);
        assert_eq!(kept.len(), 1);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn filter_publishable_skips_line_outside_diff() {
        let diff = diff_with_hunk("src/a.rs", &[10, 11, 12]);
        let (kept, skipped) = filter_publishable(vec![comment("src/a.rs", 99, None)], &diff);
        assert!(kept.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn filter_publishable_skips_file_not_in_diff() {
        let diff = diff_with_hunk("src/a.rs", &[10]);
        let (kept, skipped) = filter_publishable(vec![comment("src/missing.rs", 10, None)], &diff);
        assert!(kept.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn owner_repo_from_pr_url_parses_standard_url() {
        assert_eq!(
            owner_repo_from_pr_url("https://github.com/S-Nakamur-a/conductor/pull/279"),
            Some(("S-Nakamur-a".to_string(), "conductor".to_string()))
        );
    }

    #[test]
    fn owner_repo_from_pr_url_rejects_non_github_url() {
        assert_eq!(
            owner_repo_from_pr_url("https://example.com/o/r/pull/1"),
            None
        );
    }

    #[test]
    fn single_line_comment_payload_omits_start_line() {
        let c = comment("src/a.rs", 10, None);
        let payload = ReviewCommentPayload::from_comment(&c);
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["line"], 10);
        assert_eq!(json["side"], "RIGHT");
        assert!(json.get("start_line").is_none());
        assert!(json.get("start_side").is_none());
    }

    #[test]
    fn range_comment_payload_sets_start_line_and_end_line() {
        let c = comment("src/a.rs", 10, Some(15));
        let payload = ReviewCommentPayload::from_comment(&c);
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["line"], 15);
        assert_eq!(json["start_line"], 10);
        assert_eq!(json["start_side"], "RIGHT");
    }

    #[test]
    fn publish_with_no_comments_succeeds_without_calling_gh() {
        // No commit-id lookup should happen for an empty comment list — this
        // would hang/fail in a sandboxed test environment without a real
        // `gh`/network, so it's the one `publish()` path exercised here.
        let outcome = publish(PublishRequest {
            owner: "o".to_string(),
            repo: "r".to_string(),
            pr_number: 1,
            comments: Vec::new(),
        });
        assert_eq!(
            outcome,
            PublishOutcome::Succeeded {
                published_ids: Vec::new()
            }
        );
    }
}
