//! PR 番号や GitHub の PR URL を、レビューできるローカルの worktree に変える。
//!
//! gh のサブコマンドと JSON フィールド名、refspec の綴りをここ 1 か所に集める。
//! 散らばると gh の出力が変わったときに直す場所を取りこぼす。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::git_engine::GitEngine;
use crate::review_store::PrReviewMeta;

/// head を origin から取れない PR を、pull/<N>/head から取ったローカルブランチ名。
///
/// pr/<N> ではなくハイフンなのは、リポジトリが自分の作業で既に使っている
/// pr/ 名前空間のブランチと衝突するため。
pub fn local_branch_name(pr_number: u64) -> String {
    format!("pr-{pr_number}")
}

/// PR の head。どこから取ってどのローカルブランチへ乗せるかはこれだけで決まる。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PrHead {
    pub number: u64,
    #[serde(rename = "headRefName")]
    head_ref: String,
    #[serde(rename = "isCrossRepository")]
    from_fork: bool,
    state: String,
}

impl PrHead {
    /// fork の head は origin に無く、閉じた PR の head は消されていることがある。
    /// open の PR は head ブランチを消すと GitHub が閉じるので、origin にあると言える。
    fn on_origin(&self) -> bool {
        !self.from_fork && self.state == "OPEN"
    }

    pub fn local_branch(&self) -> String {
        if self.on_origin() {
            self.head_ref.clone()
        } else {
            local_branch_name(self.number)
        }
    }
}

/// gh pr view --json が返す形。
#[derive(Debug, Clone, serde::Deserialize)]
struct PrMeta {
    #[serde(flatten)]
    head: PrHead,
    title: String,
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

fn other(e: impl std::fmt::Display) -> PrIntakeError {
    PrIntakeError::Other(e.to_string())
}

/// gh はカレントディレクトリの remote から問い合わせ先を決めるので、repo_path で走らせる。
fn gh<T: serde::de::DeserializeOwned>(
    repo_path: &Path,
    args: &[&str],
    failed: impl FnOnce(&str) -> PrIntakeError,
) -> Result<T, PrIntakeError> {
    let output = Command::new("gh")
        .current_dir(repo_path)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(PrIntakeError::GhNotInstalled);
        }
        Err(e) => return Err(other(e)),
    };

    if !output.status.success() {
        return Err(failed(&String::from_utf8_lossy(&output.stderr)));
    }

    serde_json::from_slice(&output.stdout).map_err(|e| {
        other(format!(
            "failed to parse `gh {}` output: {e}",
            args[..2].join(" ")
        ))
    })
}

fn fetch_pr_meta(repo_path: &Path, pr_number: u64) -> Result<PrMeta, PrIntakeError> {
    gh(
        repo_path,
        &[
            "pr",
            "view",
            &pr_number.to_string(),
            "--json",
            "number,title,headRefName,baseRefName,headRepositoryOwner,url,isCrossRepository,state",
        ],
        |stderr| classify_failure_text(pr_number, stderr),
    )
}

/// 自分に直接レビューが依頼されている open な PR。チーム宛ての依頼は含めない。
/// 含めると、チームに来た PR を全部取り込むことになる。
/// --limit は、既定の件数で打ち切ると外れたと見なして tried を忘れるので広げてある。
pub fn review_requests(repo_path: &Path) -> Result<Vec<PrHead>, PrIntakeError> {
    gh(
        repo_path,
        &[
            "pr",
            "list",
            "--search",
            "user-review-requested:@me",
            "--state",
            "open",
            "--limit",
            "200",
            "--json",
            "number,headRefName,isCrossRepository,state",
        ],
        |stderr| other(stderr.trim()),
    )
}

/// repo_worktrees の結果。
pub struct RepoWorktrees {
    pub main: PathBuf,
    pub branches: Vec<String>,
}

/// repo_path の main worktree と、各 worktree が checkout しているブランチ。detached は含めない。
pub fn repo_worktrees(repo_path: &Path) -> anyhow::Result<RepoWorktrees> {
    let engine = GitEngine::open(repo_path)?;
    let heads = engine.worktree_heads()?;
    let branches = heads
        .iter()
        .filter_map(|w| match &w.head {
            crate::git_engine::Head::Branch(name) => Some(name.clone()),
            _ => None,
        })
        .collect();
    Ok(RepoWorktrees {
        main: engine.main_worktree_path()?,
        branches,
    })
}

/// 1 周の結果。取り込みを試した 1 件、依頼一覧を読めた repo、問い合わせられなかった repo。
#[derive(Debug, Default)]
pub struct ReviewRound {
    pub intaken: Option<RequestedIntake>,
    /// 取り込んだ repo より後ろは問い合わせないので入らない。
    pub queried: Vec<QueriedRepo>,
    /// 問い合わせられなかったリポジトリと理由。
    pub failures: Vec<String>,
}

/// 依頼一覧を読めた repo。一覧から外れた PR は、依頼し直されたら取り込み直してよい。
#[derive(Debug)]
pub struct QueriedRepo {
    pub main: PathBuf,
    pub listed: Vec<u64>,
    /// 既に worktree のある PR。消されても作り直さない。
    pub with_worktree: Vec<u64>,
}

/// 取り込みを試した PR とその結果。
#[derive(Debug)]
pub struct RequestedIntake {
    /// main worktree のパス。同じ PR を試し直さないための鍵にもなる。
    pub repo: PathBuf,
    pub number: u64,
    pub outcome: Result<IntakenPr, String>,
}

/// repos を順に見て、worktree がまだ無く tried にも無い依頼 PR を 1 件だけ取り込む。
/// 1 周に 1 件なのは、大きなリポジトリの checkout を同時にいくつも走らせないため。
pub fn review_round(
    repos: &[PathBuf],
    tried: &HashSet<(PathBuf, u64)>,
    open: impl Fn(&Path) -> anyhow::Result<RepoWorktrees>,
    requested: impl Fn(&Path) -> Result<Vec<PrHead>, PrIntakeError>,
    intake: impl FnOnce(&Path, u64) -> Result<IntakenPr, String>,
) -> ReviewRound {
    let mut round = ReviewRound::default();
    let mut seen = HashSet::new();
    for repo in repos {
        let opened = match open(repo) {
            Ok(opened) => opened,
            Err(e) => {
                round.failures.push(format!("{}: {e}", repo.display()));
                continue;
            }
        };
        if !seen.insert(opened.main.clone()) {
            continue;
        }
        let heads = match requested(&opened.main) {
            Ok(heads) => heads,
            Err(e) => {
                round.failures.push(format!("{}: {e}", repo.display()));
                continue;
            }
        };
        let (with_worktree, without): (Vec<PrHead>, Vec<PrHead>) = heads
            .into_iter()
            .partition(|pr| opened.branches.contains(&pr.local_branch()));
        let listed = with_worktree
            .iter()
            .chain(&without)
            .map(|pr| pr.number)
            .collect();
        round.queried.push(QueriedRepo {
            main: opened.main.clone(),
            listed,
            with_worktree: with_worktree.iter().map(|pr| pr.number).collect(),
        });
        let fresh = without
            .into_iter()
            .find(|pr| !tried.contains(&(opened.main.clone(), pr.number)));
        if let Some(pr) = fresh {
            round.intaken = Some(RequestedIntake {
                outcome: intake(&opened.main, pr.number),
                repo: opened.main,
                number: pr.number,
            });
            return round;
        }
    }
    round
}

/// gh から取った PR のメタデータ。
#[derive(Debug)]
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

/// worktree が使える状態になった PR。
///
/// DB への書き込みを呼び出し側に残してあるのは、ReviewStore がスレッドを
/// 跨いで共有される想定ではないため。
#[derive(Debug)]
pub struct IntakenPr {
    pub pr_number: u64,
    pub worktree_path: PathBuf,
    pub meta: FetchedPr,
    /// 既にあった worktree を origin へ追いつかせたときの git の答え。早送りできずに
    /// 古いまま残ったことも、ここでしか分からない。
    pub pulled: Option<String>,
}

/// input を PR 番号へ解決し、その worktree を用意する。ネットワークと git の I/O を
/// 伴うのでバックグラウンドスレッドから呼ぶ。
pub fn intake_pr(
    repo_path: &Path,
    worktree_dir_override: Option<&Path>,
    input: &str,
) -> Result<IntakenPr, PrIntakeError> {
    let pr_number = parse_pr_input(input)?;
    let engine = GitEngine::open(repo_path).map_err(other)?;
    let pr_meta = fetch_pr_meta(repo_path, pr_number)?;
    let head = &pr_meta.head;
    if let Some(name) = [&head.head_ref, &pr_meta.base_ref]
        .into_iter()
        .find(|name| is_suspicious_ref(name))
    {
        return Err(other(format!(
            "refusing a ref that starts with '-': {name}"
        )));
    }

    let (worktree_path, pulled) = if head.on_origin() {
        checkout_head_branch(
            &engine,
            &head.head_ref,
            &pr_meta.base_ref,
            worktree_dir_override,
        )?
    } else {
        let path =
            checkout_pull_head(&engine, pr_number, &pr_meta.base_ref, worktree_dir_override)?;
        (path, None)
    };

    Ok(IntakenPr {
        pr_number,
        worktree_path,
        pulled,
        meta: FetchedPr {
            branch: head.local_branch(),
            title: pr_meta.title,
            base_ref: pr_meta.base_ref,
            head_ref: pr_meta.head.head_ref,
            url: pr_meta.url,
            head_owner_login: pr_meta.head_owner.map(|o| o.login),
        },
    })
}

/// origin の head ブランチを Switch Branch (Remote) と同じ形で worktree にする。
/// 既にどこかの worktree で checkout されていれば、作らずにそこを origin へ早送りする。
/// 早送りの相手は upstream ではなく origin/<head>。upstream は利用者が何に向けていても
/// よく、origin/main を向いていれば main の先端までブランチを動かしてしまう。
fn checkout_head_branch(
    engine: &GitEngine,
    head: &str,
    base: &str,
    worktree_dir_override: Option<&Path>,
) -> Result<(PathBuf, Option<String>), PrIntakeError> {
    engine
        .fetch_branches("origin", &[head, base])
        .map_err(other)?;
    let remote_branch = format!("origin/{head}");
    let existing = engine
        .list_worktrees()
        .map_err(other)?
        .into_iter()
        .find(|w| w.branch == head);
    if let Some(worktree) = existing {
        let pulled = engine
            .fast_forward_worktree(&worktree.path, head, &remote_branch)
            .map_err(other)?;
        return Ok((worktree.path, Some(pulled)));
    }
    let path = engine
        .create_worktree_from_remote(&remote_branch, worktree_dir_override)
        .map_err(other)?;
    Ok((path, None))
}

/// head を origin から取れない PR は、GitHub が base 側に置く pull/<N>/head を取る。
/// 既にあれば取り直さずに使う。checkout 中のブランチへの fetch は git が拒む。
fn checkout_pull_head(
    engine: &GitEngine,
    pr_number: u64,
    base: &str,
    worktree_dir_override: Option<&Path>,
) -> Result<PathBuf, PrIntakeError> {
    let branch = local_branch_name(pr_number);
    let wt_dir = engine
        .worktrees_base_dir(worktree_dir_override)
        .map_err(other)?
        .join(&branch);
    if wt_dir.exists() {
        if !is_valid_worktree_dir(&wt_dir) {
            return Err(other(format!(
                "Existing directory is not a valid worktree: {}. Delete it and re-run intake.",
                wt_dir.display()
            )));
        }
        return Ok(wt_dir);
    }

    engine
        .fetch_refspec(&format!("pull/{pr_number}/head:{branch}"))
        .map_err(|e| classify_failure_text(pr_number, &e.to_string()))?;
    engine
        .create_worktree_for_existing_branch(&branch, &wt_dir)
        .map_err(other)?;

    // ベースの取得はベストエフォート。ここが失敗しても差分はローカルに既にある
    // ベース ref で出せるので、取り込み全体は失敗させない。
    if let Err(e) = engine.ensure_base_ref_available(base) {
        log::warn!("pr_intake: failed to ensure base ref '{base}' is available: {e}");
    }
    Ok(wt_dir)
}

#[cfg(test)]
mod tests;
