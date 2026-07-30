//! Assembling the text sent back to the model: success/error replies, the
//! shared `file:line` and short-id formatting, path/blankness validation, and
//! rendering a comment thread.

use std::path::{Component, Path};

use rmcp::ErrorData;
use rmcp::model::{CallToolResult, ContentBlock};

use crate::review_store::{ReviewComment, ReviewReply};

pub(super) fn ok_text(text: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// A tool-level failure: reported as a *successful* call carrying `isError`,
/// which is what the Node server did and what lets the model read the message
/// and correct itself.
///
/// This is not simply "bad input vs. broken server": a database failure on a
/// *write* also comes back this way, on purpose — a `save_walkthrough` or
/// `create_comment` that fails needs the model to see why and retry, the same
/// as a validation error. A database failure on a *read* goes out as
/// `ErrorData` instead (via `db_error` in `tools.rs`): there is nothing the
/// model did wrong to correct, and nothing useful it can retry differently.
pub(super) fn err_text(text: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![ContentBlock::text(text)]))
}

/// Render `file:line` or `file:start-end`, the location form used throughout
/// the replies.
pub(super) fn line_range(file_path: &str, line_start: u32, line_end: Option<u32>) -> String {
    match line_end {
        Some(end) => format!("{file_path}:{line_start}-{end}"),
        None => format!("{file_path}:{line_start}"),
    }
}

/// First 8 characters of an id — how comments are referred to in replies, short
/// enough to read aloud and long enough to feed back as a prefix.
pub(super) fn short_id(id: &str) -> &str {
    let end = id.char_indices().nth(8).map_or(id.len(), |(i, _)| i);
    &id[..end]
}

/// Reject a blank required string.
///
/// The schema can only say "string"; the Node server it replaces enforced a
/// minimum length on every one of these, and an empty comment body or step
/// title renders as an invisible row in the TUI rather than an obvious mistake.
pub(super) fn ensure_not_blank(value: &str, what: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{what} must not be empty."));
    }
    Ok(())
}

/// Normalise a caller-supplied repo-relative path, or explain why it is unusable.
///
/// The `./` prefix is stripped **before** validating, and that order is the whole
/// point of this function existing: `.//etc/passwd` is neither absolute nor
/// contains `..`, so validating the raw value passes it — and stripping then
/// turns it into `/etc/passwd`, which `Path::join` would follow straight out of
/// the worktree. Validating the stripped form is what closes that.
///
/// Errors quote the caller's own spelling rather than the stripped form, so the
/// message matches what it actually sent.
pub(super) fn normalize_repo_relative<'a>(
    file_path: &'a str,
    what: &str,
) -> Result<&'a str, String> {
    ensure_not_blank(file_path, what)?;
    let stripped = file_path.strip_prefix("./").unwrap_or(file_path);
    ensure_repo_relative(stripped, what).map_err(|_| {
        format!("{what} must be repo-relative and must not escape the repo root: {file_path}")
    })?;
    Ok(stripped)
}

/// Reject a path that would escape the repository root.
///
/// Comments and walkthrough steps are keyed by repo-relative path and joined
/// onto a worktree root to be read back (`viewer::content`); `Path::join`
/// discards its left side when handed an absolute path, so an unchecked value
/// here would read files outside the worktree entirely.
pub(super) fn ensure_repo_relative(file_path: &str, what: &str) -> Result<(), String> {
    if Path::new(file_path).is_absolute() {
        return Err(format!(
            "{what} must be repo-relative (e.g. src/foo.rs), got absolute path: {file_path}"
        ));
    }
    let escapes = Path::new(file_path)
        .components()
        .any(|c| matches!(c, Component::ParentDir));
    if escapes {
        return Err(format!(
            "{what} must not escape the repo root (contains \"..\"): {file_path}"
        ));
    }
    Ok(())
}

/// Render a comment and its replies as the markdown-ish block the model reads.
pub(super) fn render_thread(comment: &ReviewComment, replies: &[ReviewReply]) -> String {
    let mut text = format!(
        "## {} — {}\n",
        comment.kind.as_str().to_uppercase(),
        line_range(&comment.file_path, comment.line_start, comment.line_end)
    );
    text.push_str(&format!("ID: {}\n", comment.id));
    text.push_str(&format!(
        "Status: {} | Author: {}\n",
        comment.status.as_str(),
        comment.author.as_str()
    ));
    text.push_str(&format!("Worktree: {}", comment.worktree));
    if let Some(branch) = &comment.branch {
        text.push_str(&format!(" | Branch: {branch}"));
    }
    text.push_str(&format!("\nCreated: {}\n", comment.created_at));
    text.push_str(&format!("\n{}\n", comment.body));

    if !replies.is_empty() {
        text.push_str(&format!("\n### Replies ({})\n", replies.len()));
        for r in replies {
            text.push_str(&format!(
                "\n**{}** ({}):\n{}\n",
                r.author.as_str(),
                r.created_at,
                r.body
            ));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_store::{Author, CommentKind, CommentStatus};

    #[test]
    fn line_range_renders_single_and_range() {
        assert_eq!(line_range("src/a.rs", 3, None), "src/a.rs:3");
        assert_eq!(line_range("src/a.rs", 3, Some(9)), "src/a.rs:3-9");
    }

    #[test]
    fn short_id_truncates_to_eight_chars() {
        assert_eq!(short_id("0123456789abcdef"), "01234567");
        assert_eq!(short_id("abc"), "abc");
    }

    /// Regression: stripping `./` after validating let `.//etc/passwd` through
    /// as `/etc/passwd`, which `Path::join` follows out of the worktree. The
    /// strip has to happen first.
    #[test]
    fn normalize_repo_relative_rejects_paths_that_strip_into_absolute() {
        assert!(normalize_repo_relative(".//etc/passwd", "file_path").is_err());
        assert!(normalize_repo_relative("././../../etc/shadow", "file_path").is_err());
        assert!(normalize_repo_relative("/etc/passwd", "file_path").is_err());
        assert!(normalize_repo_relative("../secret", "file_path").is_err());
        assert!(normalize_repo_relative("", "file_path").is_err());
    }

    /// The ordinary cases must survive the above: a plain relative path is
    /// untouched, and a single `./` prefix is stripped.
    #[test]
    fn normalize_repo_relative_keeps_ordinary_paths() {
        assert_eq!(
            normalize_repo_relative("src/foo.rs", "file_path"),
            Ok("src/foo.rs")
        );
        assert_eq!(
            normalize_repo_relative("./src/foo.rs", "file_path"),
            Ok("src/foo.rs")
        );
    }

    /// The Node server only rejected absolute paths; `..` reaches the same
    /// `join`-then-read sink, so it is rejected here too.
    #[test]
    fn ensure_repo_relative_catches_absolute_and_parent_dir() {
        assert!(ensure_repo_relative("/etc/passwd", "file_path").is_err());
        assert!(ensure_repo_relative("../../secret", "file_path").is_err());
        assert!(ensure_repo_relative("a/../../b", "file_path").is_err());
        assert!(ensure_repo_relative("src/foo.rs", "file_path").is_ok());
        assert!(ensure_repo_relative("./src/foo.rs", "file_path").is_ok());
    }

    // ── render_thread ───────────────────────────────────────────────

    fn sample_comment(branch: Option<&str>) -> ReviewComment {
        ReviewComment {
            id: "abcdef01-2345-6789-abcd-ef0123456789".into(),
            worktree: "feature-x".into(),
            file_path: "src/foo.rs".into(),
            line_start: 10,
            line_end: Some(12),
            kind: CommentKind::Suggest,
            body: "Consider extracting this.".into(),
            status: CommentStatus::Pending,
            commit_ref: "HEAD".into(),
            author: Author::User,
            branch: branch.map(str::to_owned),
            created_at: "2026-07-30 00:00:00".into(),
            updated_at: "2026-07-30 00:00:00".into(),
        }
    }

    fn sample_reply() -> ReviewReply {
        ReviewReply {
            id: "reply-1".into(),
            review_id: "abcdef01-2345-6789-abcd-ef0123456789".into(),
            body: "Sounds good.".into(),
            author: Author::Claude,
            created_at: "2026-07-30 00:01:00".into(),
        }
    }

    /// Branch present, replies present: every optional section renders.
    #[test]
    fn render_thread_with_branch_and_replies() {
        let text = render_thread(&sample_comment(Some("feature-x")), &[sample_reply()]);

        assert!(text.starts_with("## SUGGEST — src/foo.rs:10-12\n"));
        assert!(text.contains("ID: abcdef01-2345-6789-abcd-ef0123456789\n"));
        assert!(text.contains("Status: pending | Author: user\n"));
        assert!(text.contains("Worktree: feature-x | Branch: feature-x\nCreated:"));
        assert!(text.contains("\n### Replies (1)\n"));
        assert!(text.contains("\n**claude** (2026-07-30 00:01:00):\nSounds good.\n"));
    }

    /// Branch absent, replies absent: both optional sections are gone, and the
    /// worktree line runs straight into `Created:` with no `| Branch:` in
    /// between.
    #[test]
    fn render_thread_without_branch_or_replies() {
        let text = render_thread(&sample_comment(None), &[]);

        assert!(text.contains("Worktree: feature-x\nCreated:"));
        assert!(!text.contains("Branch:"));
        assert!(!text.contains("Replies"));
    }

    /// Branch present, replies absent: the `| Branch:` suffix renders without
    /// pulling in a `### Replies` section.
    #[test]
    fn render_thread_with_branch_but_no_replies() {
        let text = render_thread(&sample_comment(Some("feature-x")), &[]);

        assert!(text.contains("Worktree: feature-x | Branch: feature-x\nCreated:"));
        assert!(!text.contains("Replies"));
    }

    /// Branch absent, replies present: the reverse combination from above.
    #[test]
    fn render_thread_without_branch_but_with_replies() {
        let text = render_thread(&sample_comment(None), &[sample_reply()]);

        assert!(!text.contains("Branch:"));
        assert!(text.contains("\n### Replies (1)\n"));
    }
}
