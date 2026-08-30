//! PR 取り込みの段取り。PR 番号や GitHub の PR URL を、レビューモードで使える
//! ローカルの worktree に変える。
//!
//! PR のメタデータには gh を呼び出し (ユーザーの既存の gh auth セッションに
//! 依存する)、fetch と worktree の作成には [crate::git_engine::GitEngine] を使う。
//! gh や git のコマンドの正確な綴り (JSON のフィールド、refspec の形式、
//! ブランチの命名) を 1 か所にまとめるため、worktree/mod.rs から分けてある。

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::git_engine::GitEngine;

/// 取得した PR の head に対応するローカルブランチ名: pr-<N> (ハイフン。
/// pr/<N> にしないのは、リポジトリが自分の作業で既に使っている pr/ 名前空間の
/// ブランチと衝突するため)。
pub fn local_branch_name(pr_number: u64) -> String {
    format!("pr-{pr_number}")
}

/// gh pr view --json ... が返す PR のメタデータ。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PrMeta {
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

/// PR 取り込みが失敗した原因。gh や git の出力から分類する。各バリアントは
/// それぞれ具体的で行動につながるメッセージに対応しており、入力オーバーレイが
/// 生のエラー文字列ではなく次に何をすべきかを伝えられるようにしている。
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
                write!(
                    f,
                    "Not logged in to GitHub CLI. Run `gh auth login` and retry."
                )
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

/// gh や git の失敗テキストを、行動につながる [PrIntakeError] に分類する。
/// 「見つからない」に該当したときに PR を名指しできるよう pr_number を持ち回る。
fn classify_failure_text(pr_number: u64, text: &str) -> PrIntakeError {
    let lower = text.to_lowercase();
    if lower.contains("gh auth login")
        || lower.contains("not logged in")
        || lower.contains("authentication")
    {
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

/// PR 番号または GitHub の PR URL (例:
/// https://github.com/owner/repo/pull/123。末尾にパスやクエリが付いていても
/// よい) を、素の PR 番号へパースする。
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

/// (gh の JSON 出力が返す) ref 名を、そのまま git の引数として渡すのが危険か
/// どうか。先頭の - があると、ref 名ではなくオプションとして解釈され得る。
fn is_suspicious_ref(ref_name: &str) -> bool {
    ref_name.starts_with('-')
}

/// dir が、はぐれた・壊れたディレクトリ (中断された取り込みの残骸、ユーザーが
/// 手で作った空のディレクトリなど) ではなく本物の git worktree に見えるかどうか。
/// worktree では .git はディレクトリではなくファイル (gitdir へのポインタ) だが、
/// ここではどちらも受け入れる。この検査は「明らかに worktree ではない」を弾ければ
/// 十分で、git の内部まで完全に検証する必要は無い。
fn is_valid_worktree_dir(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// gh pr view <N> --json ... で PR のメタデータを取得する。
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

/// 新規の取得・作成が成功したときに永続化する PR のメタデータ。既存 worktree への
/// 再入場では None になる。前回の取り込みで既に永続化済みで、今回は gh を
/// 呼んでいないため。
pub struct FetchedPr {
    pub branch: String,
    pub title: String,
    pub base_ref: String,
    pub head_ref: String,
    pub url: String,
    pub head_owner_login: Option<String>,
}

/// PR 取り込みの試行が完了した結果。
pub enum PrIntakeOutcome {
    /// worktree が使える状態になった。新規に取得・作成したか、同じ PR 番号の
    /// 過去の取り込みから再利用したかのどちらか。呼び出し側は worktree_path へ
    /// 切り替え、(あれば) meta をレビューストアへ永続化し、レビューモードへ入る。
    ///
    /// DB への書き込みをここで行わず意図的に呼び出し側 (メインスレッド) に任せて
    /// いるのは、[crate::review_store::ReviewStore] が App にあり、処理の途中で
    /// スレッド間を共有する想定ではないため。
    Ready {
        pr_number: u64,
        worktree_path: PathBuf,
        meta: Option<FetchedPr>,
    },
    Failed {
        error: PrIntakeError,
    },
}

/// PR 取り込みの一連の流れを同期的に実行する。input を PR 番号へ解決し、
/// その worktree を取得する (または再利用する)。
///
/// バックグラウンドスレッドでの実行を想定している (ネットワークと git の I/O)。
/// 呼び出し側は返された結果をポーリングし、(DB への書き込みを含めて) メイン
/// スレッドで適用する。
///
/// 再入場: この PR 番号の worktree が既にディスクにあれば、そのまま再利用する。
/// gh や git fetch の往復もしないし、既存のチェックアウトを自動で
/// fast-forward することもしない (「プルリクエストを開く」ときと同じく、
/// 黙ってブランチを更新してユーザーを驚かせないという先例に合わせている)。
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

    // ベストエフォート。ここが失敗しても、レビューモードの差分は既にある
    // ローカルのベース ref で動くので、取り込み全体を失敗させはしない。
    // '-' で始まる base_ref が git に届く前に弾く。この値は gh の JSON 出力
    // 由来で、先頭のダッシュがあると ref 名ではなく git のオプションとして
    // 誤読され得る。
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
            classify_failure_text(
                1,
                "To get started with GitHub CLI, please run:  gh auth login"
            ),
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
            classify_failure_text(
                1,
                "fatal: unable to access: Could not resolve host: github.com"
            ),
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

    /// 再入場: intake_pr は gh も git も一切触らずに既存の worktree ディレクトリを
    /// 再利用しなければならない (gh が入っていなくても動くように)。
    #[test]
    fn intake_pr_reenters_existing_worktree_without_gh_or_network() {
        let parent = tempfile::tempdir().unwrap();
        // OS の一時ディレクトリ自体が symlink になっているプラットフォーム
        // (macOS の /tmp -> /private/tmp など) でも等しく比較できるよう
        // 正規化しておく。
        let parent_path = parent.path().canonicalize().unwrap();
        let repo_path = parent_path.join("repo");
        git2::Repository::init(&repo_path).unwrap();

        // 過去の取り込みが PR 42 のために作った worktree を模す (本物の worktree の
        // .git はディレクトリではなく gitdir を指すファイル)。
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

    /// worktree のパスに残った古い・壊れたディレクトリ (.git が無い) は、黙って
    /// Ready を返して空のレビュー画面を見せるのではなく、行動につながる
    /// メッセージで失敗しなければならない。
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
                assert!(
                    error
                        .to_string()
                        .contains(&broken_dir.display().to_string())
                );
            }
            PrIntakeOutcome::Ready { .. } => panic!("expected Failed, got Ready"),
        }
    }

    /// このリポジトリの実際の GitHub リモートと、マージ済みの PR
    /// (refs/pull/<N>/head はマージ後も解決できる) に対する end-to-end の検査。
    /// ネットワークと認証済みの gh が要るので既定では #[ignore] してある。
    /// 実行するときは cargo test -- --ignored intake_pr_against_real_pr。
    /// このリポジトリ自身の worktree に触れないよう、一時ディレクトリへ clone する。
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
