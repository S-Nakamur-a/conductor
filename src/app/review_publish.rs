//! [App] における GitHub への publish のオーケストレーション: 何が publish
//! 可能かの算出、y/n 確認オーバーレイ(App::publish_confirm)の駆動、
//! バックグラウンドでの gh api 呼び出しの実行を担う。実際の gh CLI/JSON の
//! 綴りは crate::review_publish 側にある。pr_intake.rs がその
//! worktree/mod.rs のオーケストレーションと切り離されているのと同じ構成。

use chrono::Utc;

use crate::review_publish::{PublishComment, PublishConfirm, PublishOutcome, PublishRequest};

use super::*;

impl App {
    /// Action::PublishReview: 現在のブランチの、未公開かつ diff 内にある
    /// コメントを算出し、y/n 確認オーバーレイを開く(Enter/y/n は
    /// event/mod.rs の handle_publish_confirm_key で処理される)。確認する
    /// ものが何もない場合(PR がない、gh に publish できるコメントがない)
    /// はオーバーレイを出さずステータスメッセージのみを出す。
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

    /// コメント本文に返信を付け足したもの(現バージョンには GitHub 側の
    /// 返信スレッドがない — 返信付きのコメントは1つのコメント本文にまとめて
    /// フラット化する、という設計判断による)。
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

    /// 保留中の publish を確認する(y): 確認済みのリクエストをバックグラウンド
    /// スレッドへ渡す。保留中のものがない、または既に publish が実行中の
    /// 場合は何もしない。
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

    /// 保留中の publish 確認をキャンセルする(n/Esc)。
    pub fn cancel_publish_review(&mut self) {
        self.publish.confirm = None;
        self.set_status("Publish cancelled.".to_string(), StatusLevel::Warning);
    }

    /// バックグラウンドの publish 操作があればポーリングし、その結果を
    /// 反映する: 実際に投稿できたコメントを published としてマークし、
    /// 何が起きたかを報告する。
    /// [App::poll_all_background_ops](Self::poll_all_background_ops) から
    /// 呼ばれる。
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

    /// 指定したコメント id を published としてマークし、新しい published_at
    /// が即座に反映されるようコメント一覧を再読み込みする。
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

/// 確認済みの publish が実際に新しいバックグラウンドリクエストを開始すべき
/// かどうか: 既に実行中の間は決して開始しない。そうしないと、最初の
/// リクエストが完了する前に誤って二重確認(例えば Enter を2回押す)が
/// 起きた場合、同じコメントに対して gh api の呼び出しが2回送られてしまう。
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
