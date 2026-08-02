//! モデルに返すテキストの組み立て: 成功/エラーの応答、共通の file:line と
//! short-id のフォーマット、パス/空文字のバリデーション、コメントスレッドの
//! レンダリング。

use std::path::{Component, Path};

use rmcp::ErrorData;
use rmcp::model::{CallToolResult, ContentBlock};

use crate::review_store::{ReviewComment, ReviewReply};

pub(super) fn ok_text(text: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// ツールレベルの失敗。isError を持つ *成功* した呼び出しとして報告する。
/// Node サーバがそうしていたやり方であり、モデルがメッセージを読んで自分で
/// 訂正できるようにするための形でもある。
///
/// これは単純な「入力が悪い vs サーバが壊れている」の区別ではない。*書き込み*
/// でのデータベース失敗も意図的にこの形で返す — save_walkthrough や
/// create_comment が失敗したときは、バリデーションエラーと同様にモデルが
/// 理由を見て再試行する必要がある。一方 *読み込み* でのデータベース失敗は
/// 代わりに ErrorData として送出される（tools.rs の db_error 経由）。
/// モデル側に訂正すべき誤りは無く、違うやり方で再試行しても意味が無いため。
pub(super) fn err_text(text: impl Into<String>) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![ContentBlock::text(text)]))
}

/// file:line または file:start-end を描画する。応答全体を通じて使われる
/// 位置表記の形式。
pub(super) fn line_range(file_path: &str, line_start: u32, line_end: Option<u32>) -> String {
    match line_end {
        Some(end) => format!("{file_path}:{line_start}-{end}"),
        None => format!("{file_path}:{line_start}"),
    }
}

/// id の先頭8文字。応答の中でコメントを参照する際の形式で、読み上げられる
/// ほど短く、プレフィックスとして再入力できるほど長い。
pub(super) fn short_id(id: &str) -> &str {
    let end = id.char_indices().nth(8).map_or(id.len(), |(i, _)| i);
    &id[..end]
}

/// 必須の文字列が空だったら拒否する。
///
/// スキーマは「string」としか言えない。これが置き換えた Node サーバはこれら
/// すべてに最小長を強制していたし、空のコメント本文やステップタイトルは、
/// 分かりやすい誤りとしてではなく TUI 上の見えない行として現れてしまう。
pub(super) fn ensure_not_blank(value: &str, what: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{what} must not be empty."));
    }
    Ok(())
}

/// 呼び出し側から渡されたリポジトリ相対パスを正規化する。使えない場合は
/// その理由を説明する。
///
/// ./ プレフィックスはバリデーションの *前に* 剥がす。この関数が存在する
/// 理由そのものが、この順序にある。.//etc/passwd は絶対パスでも .. を含む
/// わけでもないので、生の値のままバリデーションすると通ってしまう —
/// その後で剥がすと /etc/passwd になり、Path::join はこれをそのまま
/// worktree の外へたどってしまう。剥がした後の形をバリデーションすること
/// でこれを塞いでいる。
///
/// 返るのは単に剥がしただけの形ではなく、[crate::repo_path::normalize] の
/// 正規形である。この値は保存された後、文字列の完全一致で FileDiff::path
/// と照合されるので、./src/a.rs や src//a.rs は git の綴り方ですでに
/// データベースに入っている必要がある。
///
/// エラーメッセージは剥がした後の形ではなく呼び出し側が実際に渡した綴りを
/// そのまま引用する。実際に送った内容とメッセージが一致するように。
pub(super) fn normalize_repo_relative(file_path: &str, what: &str) -> Result<String, String> {
    ensure_not_blank(file_path, what)?;
    let stripped = file_path.strip_prefix("./").unwrap_or(file_path);
    ensure_repo_relative(stripped, what).map_err(|_| {
        format!("{what} must be repo-relative and must not escape the repo root: {file_path}")
    })?;
    let normalized = crate::repo_path::normalize(stripped);
    // 正規化が取り除けるのは ./ や空のセグメントだけなので、脱出を新たに
    // 生み出すことは無い — ただしパスを完全に空にしてしまうことはあり得る
    // （"./" など）。それを許すと、何にも紐付かないステップが保存されてしまう。
    ensure_not_blank(&normalized, what)?;
    Ok(normalized)
}

/// リポジトリルートから脱出してしまうパスを拒否する。
///
/// コメントとウォークスルーのステップはリポジトリ相対パスをキーにしており、
/// 読み戻す際に worktree のルートと結合される（viewer::content）。
/// Path::join は絶対パスを渡されると左側を捨ててしまうので、ここで
/// チェックしない値は worktree の外にあるファイルをまるごと読んでしまう。
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

/// コメントとその返信群を、モデルが読む Markdown 風のブロックとして描画する。
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

    /// リグレッション対策: バリデーション後に ./ を剥がすと、.//etc/passwd が
    /// /etc/passwd として通ってしまい、Path::join が worktree の外へたどって
    /// しまっていた。剥がすのは先にやらないといけない。
    #[test]
    fn normalize_repo_relative_rejects_paths_that_strip_into_absolute() {
        assert!(normalize_repo_relative(".//etc/passwd", "file_path").is_err());
        assert!(normalize_repo_relative("././../../etc/shadow", "file_path").is_err());
        assert!(normalize_repo_relative("/etc/passwd", "file_path").is_err());
        assert!(normalize_repo_relative("../secret", "file_path").is_err());
        assert!(normalize_repo_relative("", "file_path").is_err());
    }

    /// 上の対策を入れても通常のケースは生き残らないといけない: 素の相対
    /// パスはそのまま、先頭の ./ 1つだけは剥がされる。
    #[test]
    fn normalize_repo_relative_keeps_ordinary_paths() {
        assert_eq!(
            normalize_repo_relative("src/foo.rs", "file_path"),
            Ok("src/foo.rs".to_string())
        );
        assert_eq!(
            normalize_repo_relative("./src/foo.rs", "file_path"),
            Ok("src/foo.rs".to_string())
        );
    }

    /// src/foo.rs を意味するどんな綴りも、保存される前に src/foo.rs に
    /// *ならなければ* いけない。差分リストはこれらを文字列の完全一致で
    /// 照合するので、./src/foo.rs のまま保存されたステップは決してジャンプ
    /// できない。
    #[test]
    fn normalize_repo_relative_canonicalises_every_spelling() {
        for spelling in [
            "src/foo.rs",
            "./src/foo.rs",
            "src//foo.rs",
            "src/./foo.rs",
            "  src/foo.rs  ",
            "src/foo.rs/",
        ] {
            assert_eq!(
                normalize_repo_relative(spelling, "file_path"),
                Ok("src/foo.rs".to_string()),
                "spelling: {spelling}"
            );
        }
    }

    /// 正規化すると何も残らなくなるパスは、何にも紐付かないアンカーとして
    /// 保存されるのではなく拒否される。
    #[test]
    fn normalize_repo_relative_rejects_a_path_that_normalises_to_empty() {
        assert!(normalize_repo_relative("./", "file_path").is_err());
        assert!(normalize_repo_relative(".", "file_path").is_err());
    }

    /// Node サーバは絶対パスだけを拒否していたが、.. も同じ join してから
    /// 読み込むという経路に到達するので、ここでも拒否する。
    #[test]
    fn ensure_repo_relative_catches_absolute_and_parent_dir() {
        assert!(ensure_repo_relative("/etc/passwd", "file_path").is_err());
        assert!(ensure_repo_relative("../../secret", "file_path").is_err());
        assert!(ensure_repo_relative("a/../../b", "file_path").is_err());
        assert!(ensure_repo_relative("src/foo.rs", "file_path").is_ok());
        assert!(ensure_repo_relative("./src/foo.rs", "file_path").is_ok());
    }

    // render_thread

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

    /// branch あり、replies あり: 全てのオプションのセクションが描画される。
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

    /// branch なし、replies なし: 両方のオプションセクションが消え、worktree の
    /// 行は | Branch: を挟まずそのまま Created: に続く。
    #[test]
    fn render_thread_without_branch_or_replies() {
        let text = render_thread(&sample_comment(None), &[]);

        assert!(text.contains("Worktree: feature-x\nCreated:"));
        assert!(!text.contains("Branch:"));
        assert!(!text.contains("Replies"));
    }

    /// branch あり、replies なし: | Branch: の接尾部は描画されるが
    /// ### Replies セクションは付いてこない。
    #[test]
    fn render_thread_with_branch_but_no_replies() {
        let text = render_thread(&sample_comment(Some("feature-x")), &[]);

        assert!(text.contains("Worktree: feature-x | Branch: feature-x\nCreated:"));
        assert!(!text.contains("Replies"));
    }

    /// branch なし、replies あり: 上とは逆の組み合わせ。
    #[test]
    fn render_thread_without_branch_but_with_replies() {
        let text = render_thread(&sample_comment(None), &[sample_reply()]);

        assert!(!text.contains("Branch:"));
        assert!(text.contains("\n### Replies (1)\n"));
    }
}
