//! 未公開のレビューコメントを gh api 経由で GitHub の PR へ投稿する。
//!
//! gh のサブコマンドと API のパス、ペイロードのフィールド名をここ 1 か所に集める。
//! 取り消せない外部アクションなので、実行前の確認は呼び出し側が通す。

use std::io::Write;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::diff_state::DiffState;

/// 公開できる状態のコメント 1 件。GitHub のレビューコメントにはスレッド返信が
/// 無いので、返信は呼び出し側が body へ平坦化してから渡す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishComment {
    pub id: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: Option<u32>,
    pub body: String,
}

/// 確認済みで実行できる公開リクエスト。owner と repo は
/// [owner_repo_from_pr_url]、comments は [filter_publishable] を通したもの。
pub struct PublishRequest {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub comments: Vec<PublishComment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    /// リクエスト内の全コメントを投稿した (投稿するものが無かった場合も含む)。
    Succeeded {
        published_ids: Vec<String>,
    },
    /// 一括投稿が失敗し、コメント単位のフォールバックが一部だけ通った。
    PartialFailure {
        published_ids: Vec<String>,
        failed: Vec<(String, String)>,
    },
    Failed {
        error: String,
    },
}

/// comments を、公開して安全なものと飛ばした件数に分ける。
///
/// GitHub は現在の差分のハンクに含まれる行にしかレビューコメントを付けられず、
/// 差分外が 1 件でも混ざると一括投稿が丸ごと 422 になる。失敗してから捨てるのでは
/// 遅いので、[publish] へ届く前にここで除く。
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

fn line_range_in_diff(file_path: &str, start: u32, end: u32, diff: &DiffState) -> bool {
    diff.files
        .iter()
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

/// https://github.com/{owner}/{repo}/pull/{n} から owner と repo を取り出す。
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

/// POST /repos/{owner}/{repo}/pulls/{N}/reviews のボディ。
#[derive(Serialize)]
struct BatchReviewPayload<'a> {
    commit_id: &'a str,
    event: &'static str,
    body: &'static str,
    comments: Vec<ReviewCommentPayload<'a>>,
}

/// POST /repos/{owner}/{repo}/pulls/{N}/comments のボディ。
#[derive(Serialize)]
struct SingleCommentPayload<'a> {
    commit_id: &'a str,
    #[serde(flatten)]
    comment: ReviewCommentPayload<'a>,
}

/// レビュー API の既定 (投稿時点の HEAD) に任せない。同時に走る push と競合する。
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

struct GhApiError {
    /// gh は 422 を stderr に "gh: Validation Failed (HTTP 422): ..." の形で報告する。
    /// gh 自身のエラー整形をパースせずに済む中では、部分一致がいちばん壊れにくい。
    is_422: bool,
    message: String,
}

/// JSON のボディは引数ではなく stdin へ流す。コメント本文は長く複数行になる。
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

/// コミット ID を解決して一括レビューを POST し、422 で拒まれたらコメント単位の
/// 投稿へフォールバックする。一括エンドポイントが line / side を受け付けるかは
/// gh api で確かめきれなかった唯一の点で、コメント単位の方は必ず受け付ける。
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
mod tests;
