//! モデルに返すテキストの組み立て — 成功/エラー応答、位置の表記、パスの検証、
//! コメントスレッドの描画。

use std::path::{Component, Path};

use rmcp::ErrorData;
use rmcp::model::{CallToolResult, ContentBlock};

use conductor_core::review_store::{ReviewComment, ReviewReply};

pub(crate) fn ok_text(text: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// ツールレベルの失敗。isError を持つ *成功* した呼び出しとして返し、モデルが
/// メッセージを読んで自分で訂正できるようにする。
pub(crate) fn err_text(text: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![ContentBlock::text(text)]))
}

/// 応答全体で使う位置の表記。
pub(crate) fn line_range(file_path: &str, line_start: u32, line_end: Option<u32>) -> String {
    match line_end {
        Some(end) => format!("{file_path}:{line_start}-{end}"),
        None => format!("{file_path}:{line_start}"),
    }
}

/// id の先頭 8 文字。読み上げられるほど短く、プレフィックスとして再入力できる
/// ほど長い。
pub(crate) fn short_id(id: &str) -> &str {
    let end = id.char_indices().nth(8).map_or(id.len(), |(i, _)| i);
    &id[..end]
}

/// 空文字を拒む。スキーマは "string" としか言えず、空の本文は分かりやすい誤り
/// ではなく TUI 上の見えない行として現れる。
pub(crate) fn ensure_not_blank(value: &str, what: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{what} must not be empty."));
    }
    Ok(())
}

/// 呼び出し側のリポジトリ相対パスを、DB に入る綴りへ正規化する。
///
/// `./` を剥がすのが検証より先なのがこの関数の要点。`.//etc/passwd` は絶対でも
/// `..` を含むわけでもないので生のままだと検証を通り、後から剥がすと
/// `/etc/passwd` になって `Path::join` が worktree の外へ辿ってしまう。
///
/// 返るのは剥がしただけの形ではなく [conductor_core::repo_path::normalize] の
/// 正規形。保存後に FileDiff::path と文字列の完全一致で照合されるので、git の
/// 綴りで入っている必要がある。
///
/// エラー文が引用するのは呼び出し側が実際に送った綴り。
pub(crate) fn normalize_repo_relative(file_path: &str, what: &str) -> Result<String, String> {
    ensure_not_blank(file_path, what)?;
    let stripped = file_path.strip_prefix("./").unwrap_or(file_path);
    ensure_repo_relative(stripped, what).map_err(|_| {
        format!("{what} must be repo-relative and must not escape the repo root: {file_path}")
    })?;
    let normalized = conductor_core::repo_path::normalize(stripped);
    // 正規化が落とすのは ./ と空セグメントだけなので脱出は生まれないが、パスが
    // 空になることはある ("./" など)。何にも紐付かないコメントは保存させない。
    ensure_not_blank(&normalized, what)?;
    Ok(normalized)
}

/// リポジトリルートから脱出するパスを拒む。読み戻す際に worktree のルートと
/// 結合されるが、`Path::join` は絶対パスを渡されると左側を捨てるので、ここを
/// 通さない値は worktree の外のファイルをまるごと読ませてしまう。
fn ensure_repo_relative(file_path: &str, what: &str) -> Result<(), String> {
    if Path::new(file_path).is_absolute() {
        return Err(format!(
            "{what} must be repo-relative (e.g. src/foo.rs), got absolute path: {file_path}"
        ));
    }
    if Path::new(file_path)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(format!(
            "{what} must not escape the repo root (contains \"..\"): {file_path}"
        ));
    }
    Ok(())
}

/// コメントとその返信を、モデルが読む Markdown 風のブロックとして描画する。
pub(crate) fn render_thread(comment: &ReviewComment, replies: &[ReviewReply]) -> String {
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
    use conductor_core::review_store::{Author, CommentKind, CommentStatus};

    #[test]
    fn 位置とidの表記() {
        assert_eq!(line_range("src/a.rs", 3, None), "src/a.rs:3");
        assert_eq!(line_range("src/a.rs", 3, Some(9)), "src/a.rs:3-9");
        assert_eq!(short_id("0123456789abcdef"), "01234567");
        assert_eq!(short_id("abc"), "abc");
    }

    /// `.//etc/passwd` と `././../../etc/shadow` はリグレッション対策。検証の前に
    /// `./` を剥がさないと、どちらも通ったうえで worktree の外を指す。
    #[test]
    fn パスは同じ形に正規化され脱出は拒まれる() {
        let ok = [
            "src/foo.rs",
            "./src/foo.rs",
            "src//foo.rs",
            "src/./foo.rs",
            "  src/foo.rs  ",
            "src/foo.rs/",
        ];
        for spelling in ok {
            assert_eq!(
                normalize_repo_relative(spelling, "file_path"),
                Ok("src/foo.rs".to_string()),
                "{spelling}"
            );
        }
        let rejected = [
            ".//etc/passwd",
            "././../../etc/shadow",
            "/etc/passwd",
            "../secret",
            "a/../../b",
            "",
            "./",
            ".",
        ];
        for spelling in rejected {
            assert!(
                normalize_repo_relative(spelling, "file_path").is_err(),
                "{spelling}"
            );
        }
    }

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
            author: Author::User,
            branch: branch.map(str::to_owned),
            created_at: "2026-07-30 00:00:00".into(),
        }
    }

    fn sample_reply() -> ReviewReply {
        ReviewReply {
            id: "reply-1".into(),
            body: "Sounds good.".into(),
            author: Author::Claude,
            created_at: "2026-07-30 00:01:00".into(),
        }
    }

    /// branch と replies は独立した任意セクション。片方の有無がもう片方の出方や
    /// 常に出る骨組みを変えてはいけない。
    #[test]
    fn スレッドの描画() {
        for branch in [Some("feature-x"), None] {
            for replies in [vec![sample_reply()], vec![]] {
                let case = format!("branch={branch:?} replies={}", replies.len());
                let text = render_thread(&sample_comment(branch), &replies);

                assert!(
                    text.starts_with("## SUGGEST — src/foo.rs:10-12\n"),
                    "{case}"
                );
                assert!(
                    text.contains("ID: abcdef01-2345-6789-abcd-ef0123456789\n"),
                    "{case}"
                );
                assert!(text.contains("Status: pending | Author: user\n"), "{case}");

                let worktree_line = match branch {
                    Some(b) => format!("Worktree: feature-x | Branch: {b}\nCreated:"),
                    None => "Worktree: feature-x\nCreated:".to_string(),
                };
                assert!(text.contains(&worktree_line), "{case}");
                assert_eq!(
                    text.contains("\n**claude** (2026-07-30 00:01:00):\nSounds good.\n"),
                    !replies.is_empty(),
                    "{case}"
                );
                assert_eq!(
                    text.contains("\n### Replies (1)\n"),
                    !replies.is_empty(),
                    "{case}"
                );
            }
        }
    }
}
