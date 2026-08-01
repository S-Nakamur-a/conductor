//! Publish-to-GitHub orchestration for [`App`]: computing what's publishable,
//! driving the y/n confirm overlay (`App::publish_confirm`), and running the
//! background `gh api` call. The actual `gh` CLI/JSON spelling lives in
//! `crate::review_publish`, mirroring how `pr_intake.rs` is kept separate
//! from its `app/worktree.rs` orchestration.

use chrono::Utc;

use crate::review_publish::{PublishComment, PublishConfirm, PublishOutcome, PublishRequest};

use super::*;

impl App {
    /// `Action::PublishReview`: compute the current branch's unpublished,
    /// in-diff comments and open the y/n confirm overlay (`Enter`/`y`/`n`
    /// handled in `event/mod.rs`'s `handle_publish_confirm_key`). A no-op
    /// with a status message instead of an overlay when there's nothing to
    /// confirm (no PR, no `gh`-publishable comments).
    pub fn cmd_publish_review(&mut self) {
        let Some(store) = self.review_store.as_ref() else {
            self.set_status(
                "Review database unavailable — cannot publish comments.".to_string(),
                StatusLevel::Error,
            );
            return;
        };
        let branch = self.selected_worktree_branch();
        if branch.is_empty() {
            self.set_status(
                "No worktree selected — open one to publish comments.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }

        let meta = store.get_pr_review_meta(&branch).ok().flatten();
        let (Some(pr_number), Some(pr_url)) = (
            meta.as_ref().and_then(|m| m.pr_number),
            meta.as_ref().and_then(|m| m.pr_url.clone()),
        ) else {
            self.set_status(
                "Comments can only be published on a branch opened via PR intake.".to_string(),
                StatusLevel::Warning,
            );
            return;
        };
        let Some((owner, repo)) = crate::review_publish::owner_repo_from_pr_url(&pr_url) else {
            self.set_status(
                format!("Could not parse owner/repo from PR URL: {pr_url}"),
                StatusLevel::Error,
            );
            return;
        };

        let unpublished = match store.unpublished_reviews(&branch) {
            Ok(comments) => comments,
            Err(e) => {
                self.set_status(
                    format!("Failed to load unpublished comments: {e}"),
                    StatusLevel::Error,
                );
                return;
            }
        };
        if unpublished.is_empty() {
            self.set_status(
                "No unpublished comments on this branch.".to_string(),
                StatusLevel::Info,
            );
            return;
        }

        let comments: Vec<PublishComment> = unpublished
            .into_iter()
            .map(|c| {
                let body = self.comment_body_with_replies(store, &c.id, &c.body);
                PublishComment {
                    id: c.id,
                    file_path: c.file_path,
                    line_start: c.line_start,
                    line_end: c.line_end,
                    body,
                }
            })
            .collect();
        let (comments, skipped) =
            crate::review_publish::filter_publishable(comments, &self.diff_state);
        if comments.is_empty() {
            self.set_status(
                format!("All {skipped} unpublished comment(s) are outside the current diff — nothing to publish."),
                StatusLevel::Warning,
            );
            return;
        }

        self.publish.confirm = Some(PublishConfirm {
            owner,
            repo,
            pr_number: pr_number as u64,
            comments,
            skipped,
        });
    }

    /// A comment's body with its replies appended (v1 has no GitHub-side
    /// reply thread — a comment with replies is flattened into one comment
    /// body, per ADR-6).
    fn comment_body_with_replies(
        &self,
        store: &ReviewStore,
        comment_id: &str,
        body: &str,
    ) -> String {
        let replies = store.get_replies(comment_id).unwrap_or_default();
        if replies.is_empty() {
            return body.to_string();
        }
        let mut out = body.to_string();
        out.push_str("\n\n---\n");
        for reply in replies {
            out.push_str(&format!("\n**{}:** {}\n", reply.author, reply.body));
        }
        out
    }

    /// Confirm the pending publish (`y`): hand the confirmed request off to a
    /// background thread. A no-op if there's nothing pending or a publish is
    /// already running.
    pub fn confirm_publish_review(&mut self) {
        let Some(confirm) = self.publish.confirm.take() else {
            return;
        };
        if !should_start_publish(self.publish.op.is_running()) {
            self.set_status(
                "A publish is already in progress.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        let count = confirm.comments.len();
        let request = PublishRequest {
            owner: confirm.owner,
            repo: confirm.repo,
            pr_number: confirm.pr_number,
            comments: confirm.comments,
        };
        self.publish.op.start(move |tx| {
            let outcome = crate::review_publish::publish(request);
            let _ = tx.send(outcome);
        });
        self.set_status(
            format!("Publishing {count} comment(s) to GitHub…"),
            StatusLevel::Info,
        );
    }

    /// Cancel the pending publish confirm (`n`/`Esc`).
    pub fn cancel_publish_review(&mut self) {
        self.publish.confirm = None;
        self.set_status("Publish cancelled.".to_string(), StatusLevel::Warning);
    }

    /// Poll the background publish operation (if any) and apply its result:
    /// mark whichever comments actually posted as published, then report
    /// what happened. Called from
    /// [`App::poll_all_background_ops`](Self::poll_all_background_ops).
    pub fn poll_publish_review(&mut self) {
        let Some(outcome) = self.publish.op.poll() else {
            return;
        };
        let now = Utc::now().to_rfc3339();
        match outcome {
            PublishOutcome::Succeeded { published_ids } => {
                let count = published_ids.len();
                self.mark_published(&published_ids, &now);
                self.set_status(
                    format!("Published {count} comment(s) to GitHub."),
                    StatusLevel::Success,
                );
            }
            PublishOutcome::PartialFailure {
                published_ids,
                failed,
            } => {
                let published_count = published_ids.len();
                self.mark_published(&published_ids, &now);
                for (id, error) in &failed {
                    log::warn!("failed to publish comment {id}: {error}");
                }
                self.set_status(
                    format!(
                        "Published {published_count} comment(s); {} failed — see logs.",
                        failed.len()
                    ),
                    StatusLevel::Warning,
                );
            }
            PublishOutcome::Failed { error } => {
                self.set_status(
                    format!("Failed to publish comments: {error}"),
                    StatusLevel::Error,
                );
            }
        }
    }

    /// Mark the given comment ids published and reload the comment list so
    /// their new `published_at` is reflected immediately.
    fn mark_published(&mut self, ids: &[String], timestamp: &str) {
        if ids.is_empty() {
            return;
        }
        if let Some(store) = &self.review_store
            && let Err(e) = store.mark_published(ids, timestamp)
        {
            log::warn!("failed to mark comments published: {e}");
        }
        self.refresh_reviews();
    }
}

/// Whether a confirmed publish should actually start a new background
/// request: never while one is already running, since a stray double
/// confirm (e.g. `Enter` pressed twice) before the first request completes
/// would otherwise submit two `gh api` calls for the same comments.
fn should_start_publish(is_running: bool) -> bool {
    !is_running
}

#[cfg(test)]
mod tests {
    use super::should_start_publish;

    #[test]
    fn confirming_again_while_a_publish_is_running_does_not_start_a_new_one() {
        assert!(!should_start_publish(true));
    }

    #[test]
    fn confirming_starts_when_nothing_is_running() {
        assert!(should_start_publish(false));
    }
}
