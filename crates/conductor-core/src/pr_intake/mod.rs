//! PR 番号や GitHub の PR URL を、レビューできるローカルの worktree に変える。
//!
//! gh のサブコマンドと JSON フィールド名、refspec の綴りをここ 1 か所に集める。
//! 散らばると gh の出力が変わったときに直す場所を取りこぼす。

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::git_engine::GitEngine;
use crate::review_store::PrReviewMeta;

/// 取得した PR head を指すローカルブランチ名。
///
/// pr/<N> ではなくハイフンなのは、リポジトリが自分の作業で既に使っている
/// pr/ 名前空間のブランチと衝突するため。
pub fn local_branch_name(pr_number: u64) -> String {
    format!("pr-{pr_number}")
}

/// gh pr view --json が返す形。
#[derive(Debug, Clone, serde::Deserialize)]
struct PrMeta {
    title: String,
    #[serde(rename = "headRefName")]
    head_ref: String,
    #[serde(rename = "baseRefName")]
    base_ref: String,
    #[serde(rename = "headRepositoryOwner")]
    head_owner: Option<GhOwner>,
    url: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct GhOwner {
    login: String,
}

/// PR 取り込みが失敗した原因。バリアントごとに次の手が決まるので、生のエラー
/// 文字列ではなくこの分類を呼び出し側へ渡す。
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
            PrIntakeError::GhNotAuthenticated => write!(
                f,
                "Not logged in to GitHub CLI. Run `gh auth login` and retry."
            ),
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

/// PR 番号、または GitHub の PR URL (末尾にパスやクエリが付いていてもよい) を
/// 素の PR 番号にする。
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

/// 先頭の - がある ref 名は、git に渡すと ref ではなくオプションとして読まれ得る。
fn is_suspicious_ref(ref_name: &str) -> bool {
    ref_name.starts_with('-')
}

/// worktree の .git は gitdir を指すファイルだが、ここではファイルでもディレクトリ
/// でも通す。「明らかに worktree ではない」を弾ければ十分。
fn is_valid_worktree_dir(dir: &Path) -> bool {
    dir.join(".git").exists()
}

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

/// 新規に取得・作成したときの PR のメタデータ。
pub struct FetchedPr {
    pub branch: String,
    pub title: String,
    pub base_ref: String,
    pub head_ref: String,
    pub url: String,
    pub head_owner_login: Option<String>,
}

impl FetchedPr {
    /// gh の JSON をレビュー DB の形へ移す。author に入るのは PR の作者ではなく
    /// head リポジトリの所有者 (headRepositoryOwner.login)。
    pub fn review_meta(&self, pr_number: u64) -> PrReviewMeta {
        PrReviewMeta {
            pr_number: Some(pr_number as i64),
            pr_url: Some(self.url.clone()),
            pr_title: Some(self.title.clone()),
            base_ref: Some(self.base_ref.clone()),
            head_ref: Some(self.head_ref.clone()),
            author: self.head_owner_login.clone(),
        }
    }
}

pub enum PrIntakeOutcome {
    /// worktree が使える状態になった。meta が None なら過去の取り込みの再利用で、
    /// 永続化は既に済んでいる。
    ///
    /// DB への書き込みを呼び出し側に残してあるのは、ReviewStore がスレッドを
    /// 跨いで共有される想定ではないため。
    Ready {
        pr_number: u64,
        worktree_path: PathBuf,
        meta: Option<FetchedPr>,
    },
    Failed {
        error: PrIntakeError,
    },
}

/// input を PR 番号へ解決し、その worktree を用意する。ネットワークと git の I/O を
/// 伴うのでバックグラウンドスレッドから呼ぶ。
///
/// 同じ PR 番号の worktree が既にディスクにあれば gh も fetch も走らせずに再利用し、
/// 既存のチェックアウトを自動で fast-forward もしない。黙ってブランチを進めない。
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

    // ベースの取得はベストエフォート。ここが失敗しても差分はローカルに既にある
    // ベース ref で出せるので、取り込み全体は失敗させない。
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
mod tests;
