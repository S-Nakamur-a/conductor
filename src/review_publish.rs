//! gh api 経由でレビューコメントを GitHub へ公開する。
//!
//! app/review_publish.rs (確認オーバーレイの状態、バックグラウンドスレッドの起動、
//! DB への書き込みといった App 側の段取り) とは分けてある。pr_intake.rs を
//! worktree/mod.rs から分けているのと同じ理由で、gh の CLI と JSON の正確な
//! 綴りを 1 か所にまとめるため。ここにあるのは素のデータと gh のサブプロセス
//! 呼び出しだけで App に依存しないので、アプリを起動せずに単体テストできる。

use std::io::Write;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::diff_state::DiffState;

/// 公開できる状態のレビューコメント 1 件。親コメントの本文に返信を連結したもの。
/// ここでの GitHub のレビューコメントにはスレッド返信の概念が無いので、返信を持つ
/// コメントは 1 つの GitHub コメント本文へ平坦化する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishComment {
    pub id: String,
    pub file_path: String,
    pub line_start: u32,
    pub line_end: Option<u32>,
    pub body: String,
}

/// 確認済みで実行できる公開リクエスト。[publish] が必要とするものが、呼び出し側で
/// 既に解決されている (owner と repo は pr_review_meta.pr_url 由来、コメントは
/// 実際に差分に含まれる行だけに絞り込み済み)。
pub struct PublishRequest {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub comments: Vec<PublishComment>,
}

/// App::publish_confirm の y/n オーバーレイを支える状態。確認後の公開が送る
/// 絞り込み済みのコメントと、現在の差分が覆っていない行にあったために飛ばした件数。
/// ユーザーが確認したあと [PublishRequest] を組み立てる元にもなる。
pub struct PublishConfirm {
    pub owner: String,
    pub repo: String,
    pub pr_number: u64,
    pub comments: Vec<PublishComment>,
    pub skipped: usize,
}

/// 公開を試みた結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    /// リクエスト内の全コメントを投稿した (または投稿するものが無かった)。
    Succeeded { published_ids: Vec<String> },
    /// 一括レビューの呼び出しが失敗し、コメント単位のフォールバックが一部だけを
    /// 投稿できた。
    PartialFailure {
        published_ids: Vec<String>,
        failed: Vec<(String, String)>,
    },
    /// 何も投稿できなかった (コミット ID を解決できなかったか、一括とフォールバックの
    /// どちらの試行も失敗した)。
    Failed { error: String },
}

/// comments を、公開して安全なものと、飛ばした件数に分ける。
///
/// GitHub のレビューコメントは現在の差分のハンクに含まれる行に紐づいていなければ
/// ならない。差分の外の行にコメントが 1 件でもあると一括全体が 422 で失敗するので、
/// 差分外のコメントは失敗してから捨てるのではなく、[publish] に届く前に
/// 除いておかねばならない。
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

/// [start, end] (新側の行番号) の両方が、file_path の同一の差分ハンクに
/// 収まるかどうか。
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

/// GitHub の PR URL (https://github.com/{owner}/{repo}/pull/{n}) から
/// owner と repo を取り出す。
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

// ペイロードの形 (フィールド名は実際の gh api の応答で確認済み)

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

/// POST /repos/{owner}/{repo}/pulls/{N}/comments のボディ (コメント 1 件ずつの
/// フォールバック)。一括エンドポイントが line / side を受け付けるかは未検証だが、
/// こちらは必ず受け付ける。
#[derive(Serialize)]
struct SingleCommentPayload<'a> {
    commit_id: &'a str,
    #[serde(flatten)]
    comment: ReviewCommentPayload<'a>,
}

// gh CLI の配管

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

/// gh api <path> --input -。JSON のボディは CLI の引数ではなく stdin へ流す。
/// コメントの本文は長かったり複数行だったりするため。
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
    /// 失敗が GitHub の 422 Unprocessable Entity に見えるかどうか。コメント 1 件ずつの
    /// 投稿へフォールバックする合図になる。gh api はこれを stderr に
    /// gh: Validation Failed (HTTP 422): ... のような形で報告する。gh 自身の
    /// エラー整形をパースせずに済む中では、部分一致がいちばん壊れにくい判定。
    is_422: bool,
    message: String,
}

/// 公開リクエストを実行する。コミット ID を解決し、一括レビューを POST し、
/// 一括が 422 で拒否されたらコメントを 1 件ずつ投稿する方式へフォールバックする
/// (実装前に gh api で確かめたなかで、一括エンドポイントが line / side を
/// 受け付けるかどうかだけが未検証のまま残っていた)。
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

/// 各コメントを /pulls/{N}/comments へ個別に投稿する。こちらは line / side を
/// 無条件で受け付ける。GitHub API が一括レビューを拒否したときのフォールバック。
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
        ds.files = vec![FileDiff {
            path: path.to_string(),
            added_lines: new_lines.len(),
            deleted_lines: 0,
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
    fn diff上の単一行のコメントは残す() {
        let diff = diff_with_hunk("src/a.rs", &[10, 11, 12]);
        let (kept, skipped) = filter_publishable(vec![comment("src/a.rs", 11, None)], &diff);
        assert_eq!(kept.len(), 1);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn 両端がハンクに入る範囲は残す() {
        let diff = diff_with_hunk("src/a.rs", &[10, 11, 12]);
        let (kept, skipped) = filter_publishable(vec![comment("src/a.rs", 10, Some(12))], &diff);
        assert_eq!(kept.len(), 1);
        assert_eq!(skipped, 0);
    }

    #[test]
    fn diffの外の行は落とす() {
        let diff = diff_with_hunk("src/a.rs", &[10, 11, 12]);
        let (kept, skipped) = filter_publishable(vec![comment("src/a.rs", 99, None)], &diff);
        assert!(kept.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn diffに無いファイルは落とす() {
        let diff = diff_with_hunk("src/a.rs", &[10]);
        let (kept, skipped) = filter_publishable(vec![comment("src/missing.rs", 10, None)], &diff);
        assert!(kept.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn 標準のprのurlからownerとrepoを取る() {
        assert_eq!(
            owner_repo_from_pr_url("https://github.com/S-Nakamur-a/conductor/pull/279"),
            Some(("S-Nakamur-a".to_string(), "conductor".to_string()))
        );
    }

    #[test]
    fn github以外のurlは拒む() {
        assert_eq!(
            owner_repo_from_pr_url("https://example.com/o/r/pull/1"),
            None
        );
    }

    #[test]
    fn 単一行のコメントはstart_lineを付けない() {
        let c = comment("src/a.rs", 10, None);
        let payload = ReviewCommentPayload::from_comment(&c);
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["line"], 10);
        assert_eq!(json["side"], "RIGHT");
        assert!(json.get("start_line").is_none());
        assert!(json.get("start_side").is_none());
    }

    #[test]
    fn 範囲のコメントはstart_lineとend_lineを付ける() {
        let c = comment("src/a.rs", 10, Some(15));
        let payload = ReviewCommentPayload::from_comment(&c);
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["line"], 15);
        assert_eq!(json["start_line"], 10);
        assert_eq!(json["start_side"], "RIGHT");
    }

    #[test]
    fn コメントが無ければghを呼ばずに成功する() {
        // コメントが空ならコミット ID の問い合わせは起きないはず。本物の gh も
        // ネットワークも無いサンドボックスのテスト環境ではそれがハングまたは
        // 失敗するので、ここで動かせる publish() の経路はこれだけ。
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
