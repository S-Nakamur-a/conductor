//! worktree とブランチの列挙、worktree ごとのステータス、main worktree の解決。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use git2::Repository;

use super::{ChangeCounts, GitEngine, GitStatusMap, Head, WorktreeHead, WorktreeInfo};

fn head_of(repo: &Repository) -> Head {
    match repo.head() {
        Ok(head) if head.is_branch() => Head::Branch(head.shorthand().unwrap_or_default().into()),
        Ok(head) => Head::Detached(
            head.target()
                .map(|oid| oid.to_string()[..7].to_string())
                .unwrap_or_default(),
        ),
        Err(_) => Head::Unborn(
            repo.find_reference("HEAD")
                .ok()
                .and_then(|r| r.symbolic_target().map(str::to_string))
                .map(|target| target.trim_start_matches("refs/heads/").to_string())
                .unwrap_or_default(),
        ),
    }
}

impl GitEngine {
    /// 開いた dir の worktree の HEAD。
    pub fn head(&self) -> Head {
        head_of(&self.repo)
    }

    /// main と linked のすべての worktree を、ブランチつきで返す (件数は数えない)。
    /// 1 つでも調べられればそれを返し、調べられなかったものはログに残して飛ばす。
    ///
    /// 全滅したときだけ Err にするのは、空の一覧が「worktree が無いリポジトリ」と
    /// 区別できないため。
    pub fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        let mut infos = Vec::new();
        let mut first_error = None;

        let main_path = self.main_worktree_path()?;
        match self.worktree_info_at(&main_path, true) {
            Ok(info) => infos.push(info),
            Err(e) => {
                log::warn!(
                    "failed to inspect main worktree at {}: {e}",
                    main_path.display()
                );
                first_error = Some(e.context(format!("main worktree at {}", main_path.display())));
            }
        }

        if let Ok(names) = self.repo.worktrees() {
            for name in names.iter().flatten() {
                match self.linked_worktree_info(name) {
                    Ok(info) => infos.push(info),
                    Err(e) => {
                        log::warn!("failed to inspect linked worktree '{name}': {e}");
                        first_error =
                            first_error.or_else(|| Some(e.context(format!("worktree '{name}'"))));
                    }
                }
            }
        }

        match first_error {
            Some(e) if infos.is_empty() => Err(e),
            _ => Ok(infos),
        }
    }

    /// main を先頭に、すべての worktree のパスと HEAD。dir の消えた worktree は飛ばす。
    pub fn worktree_heads(&self) -> Result<Vec<WorktreeHead>> {
        let main = self.main_worktree_path()?;
        let mut heads = vec![WorktreeHead {
            head: head_of(&Repository::open(&main)?),
            path: main,
        }];
        if let Ok(names) = self.repo.worktrees() {
            for name in names.iter().flatten() {
                let Ok(path) = self
                    .repo
                    .find_worktree(name)
                    .map(|wt| wt.path().to_path_buf())
                else {
                    continue;
                };
                if let Ok(repo) = Repository::open(&path) {
                    heads.push(WorktreeHead {
                        head: head_of(&repo),
                        path,
                    });
                }
            }
        }
        Ok(heads)
    }

    /// ローカルブランチ名をソート済みで返す。
    pub fn list_local_branches(&self) -> Result<Vec<String>> {
        let branches = self.repo.branches(Some(git2::BranchType::Local))?;
        let mut names: Vec<String> = branches
            .filter_map(|b| {
                let (branch, _) = b.ok()?;
                branch.name().ok()?.map(String::from)
            })
            .collect();
        names.sort();
        Ok(names)
    }

    /// worktree のディレクトリ名を短くするため、feature/ や fix/ などの prefix を落とす。
    pub fn strip_branch_prefix(branch: &str) -> &str {
        [
            "feature/", "fix/", "bugfix/", "hotfix/", "release/", "chore/",
        ]
        .iter()
        .find_map(|prefix| branch.strip_prefix(prefix))
        .unwrap_or(branch)
    }

    /// worktree を置くディレクトリ。無ければ作る。
    ///
    /// 環境変数 CONDUCTOR_WORKTREE_DIR、override_dir (config の general.worktree_dir)、
    /// 既定の `<main の親>/<repo 名>-worktrees/` の順で決まる。
    pub fn worktrees_base_dir(&self, override_dir: Option<&Path>) -> Result<PathBuf> {
        let base = if let Ok(env_dir) = std::env::var("CONDUCTOR_WORKTREE_DIR") {
            PathBuf::from(env_dir)
        } else if let Some(dir) = override_dir {
            dir.to_path_buf()
        } else {
            let main_path = self.main_worktree_path()?;
            let repo_name = main_path
                .file_name()
                .ok_or_else(|| anyhow!("cannot determine repo name"))?
                .to_string_lossy();
            let parent = main_path
                .parent()
                .ok_or_else(|| anyhow!("cannot determine parent directory"))?;
            parent.join(format!("{repo_name}-worktrees"))
        };
        if !base.exists() {
            std::fs::create_dir_all(&base).with_context(|| {
                format!("failed to create worktrees base dir: {}", base.display())
            })?;
        }
        Ok(base)
    }

    /// main worktree の絶対パス。
    ///
    /// linked worktree から開くと repo.workdir() はその worktree を返すので、
    /// git dir が `<main>/.git/worktrees/<name>/` の形かどうかで見分ける。
    pub fn main_worktree_path(&self) -> Result<PathBuf> {
        let git_dir = self.repo.path();

        if let Some(worktrees_dir) = git_dir.parent()
            && worktrees_dir.file_name() == Some("worktrees".as_ref())
            && let Some(dot_git) = worktrees_dir.parent()
            && let Some(main_repo) = dot_git.parent()
        {
            return Ok(main_repo.to_path_buf());
        }

        // libgit2 は workdir を末尾スラッシュ付きで返すことがある。linked worktree の
        // パスや $PWD と比較できるよう components() で正規化する。
        if let Some(workdir) = self.repo.workdir() {
            return Ok(workdir.components().collect());
        }

        git_dir
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow!("cannot determine main worktree path"))
    }

    fn linked_worktree_info(&self, name: &str) -> Result<WorktreeInfo> {
        let wt = self.repo.find_worktree(name)?;
        self.worktree_info_at(wt.path(), false)
    }

    /// 変更件数は数えない (counts は None)。理由は WorktreeInfo::counts。
    fn worktree_info_at(&self, path: &Path, is_main: bool) -> Result<WorktreeInfo> {
        let repo = Repository::open(path)
            .with_context(|| format!("cannot open repo at {}", path.display()))?;

        let (ahead, behind) = Self::ahead_behind_upstream(&repo);
        let head_oid = repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .map(|oid| oid.to_string());

        Ok(WorktreeInfo {
            path: path.to_path_buf(),
            branch: Self::current_branch_name(&repo),
            is_main,
            counts: None,
            ahead,
            behind,
            head_oid,
        })
    }

    fn ahead_behind_upstream(repo: &Repository) -> (Option<usize>, Option<usize>) {
        let counts = (|| {
            let head = repo.head().ok().filter(|h| h.is_branch())?;
            let local_oid = head.target()?;
            let branch = repo
                .find_branch(head.shorthand()?, git2::BranchType::Local)
                .ok()?;
            let upstream_oid = branch.upstream().ok()?.get().target()?;
            repo.graph_ahead_behind(local_oid, upstream_oid).ok()
        })();
        match counts {
            Some((ahead, behind)) => (Some(ahead), Some(behind)),
            None => (None, None),
        }
    }

    fn current_branch_name(repo: &Repository) -> String {
        if let Ok(head) = repo.head()
            && head.is_branch()
            && let Some(name) = head.shorthand()
        {
            return name.to_string();
        }
        "HEAD (detached)".to_string()
    }

    /// 1 つの worktree の変更件数。
    pub fn change_counts_at(path: &Path) -> Result<ChangeCounts> {
        Ok(GitStatusMap::load(path)?.change_counts())
    }
}
