//! worktree とブランチの列挙、worktree ごとのステータス、main worktree の解決。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use git2::{Repository, Status, StatusOptions, StatusShow};

use super::{GitEngine, WorktreeInfo};

struct ChangeCounts {
    added: usize,
    modified: usize,
    deleted: usize,
    staged: usize,
}

impl GitEngine {
    /// main と linked のすべての worktree を、ブランチとステータス件数つきで返す。
    /// 調べられなかった worktree はログに残して飛ばす。
    pub fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        let mut infos = Vec::new();

        let main_path = self.main_worktree_path()?;
        match self.worktree_info_at(&main_path, true) {
            Ok(info) => infos.push(info),
            Err(e) => log::warn!(
                "failed to inspect main worktree at {}: {e}",
                main_path.display()
            ),
        }

        if let Ok(names) = self.repo.worktrees() {
            for name in names.iter().flatten() {
                match self.linked_worktree_info(name) {
                    Ok(info) => infos.push(info),
                    Err(e) => log::warn!("failed to inspect linked worktree '{name}': {e}"),
                }
            }
        }

        Ok(infos)
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

    fn worktree_info_at(&self, path: &Path, is_main: bool) -> Result<WorktreeInfo> {
        let repo = Repository::open(path)
            .with_context(|| format!("cannot open repo at {}", path.display()))?;

        let counts = Self::change_counts(&repo).unwrap_or(ChangeCounts {
            added: 0,
            modified: 0,
            deleted: 0,
            staged: 0,
        });
        let (ahead, behind) = Self::ahead_behind_upstream(&repo);
        let head = repo.head().ok();
        let head_oid = head
            .as_ref()
            .and_then(|h| h.target())
            .map(|oid| oid.to_string());
        let head_time = head
            .and_then(|h| h.peel_to_commit().ok())
            .map(|c| c.time().seconds());

        Ok(WorktreeInfo {
            path: path.to_path_buf(),
            branch: Self::current_branch_name(&repo),
            is_main,
            added: counts.added,
            modified: counts.modified,
            deleted: counts.deleted,
            staged: counts.staged,
            is_clean: counts.added == 0 && counts.modified == 0 && counts.deleted == 0,
            ahead,
            behind,
            head_oid,
            head_time,
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

    /// added / modified / deleted はファイルごとに 1 バケットだが、staged だけは
    /// それらと重複して数える (理由は WorktreeInfo::staged)。
    fn change_counts(repo: &Repository) -> Result<ChangeCounts> {
        const STAGED: Status = Status::INDEX_NEW
            .union(Status::INDEX_MODIFIED)
            .union(Status::INDEX_DELETED)
            .union(Status::INDEX_RENAMED)
            .union(Status::INDEX_TYPECHANGE);
        const INDEX_MODIFIED: Status = Status::INDEX_MODIFIED
            .union(Status::INDEX_RENAMED)
            .union(Status::INDEX_TYPECHANGE);
        const WT_MODIFIED: Status = Status::WT_MODIFIED
            .union(Status::WT_RENAMED)
            .union(Status::WT_TYPECHANGE);

        let mut opts = StatusOptions::new();
        opts.show(StatusShow::IndexAndWorkdir)
            .include_untracked(true)
            .renames_head_to_index(true);
        let statuses = repo.statuses(Some(&mut opts))?;

        let mut counts = ChangeCounts {
            added: 0,
            modified: 0,
            deleted: 0,
            staged: 0,
        };
        for entry in statuses.iter() {
            let s = entry.status();
            if s.intersects(STAGED) {
                counts.staged += 1;
            }
            if s.intersects(Status::INDEX_NEW) {
                counts.added += 1;
            } else if s.intersects(INDEX_MODIFIED) {
                counts.modified += 1;
            } else if s.intersects(Status::INDEX_DELETED) {
                counts.deleted += 1;
            } else if s.intersects(Status::WT_NEW) {
                counts.added += 1;
            } else if s.intersects(WT_MODIFIED) {
                counts.modified += 1;
            } else if s.intersects(Status::WT_DELETED) {
                counts.deleted += 1;
            }
        }
        Ok(counts)
    }
}
