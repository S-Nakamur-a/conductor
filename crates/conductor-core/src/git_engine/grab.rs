//! grab / ungrab: ある worktree でチェックアウト中のブランチを main worktree へ移し、
//! また元に戻す。状態は zsh の wt と同じ `$GIT_COMMON_DIR/wt-grab` に置く。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use git2::Repository;

use super::GitEngine;

/// wt-grab ファイルの中身。branch、元の worktree、退避先ブランチの 3 行に、
/// 任意で resume 用の Claude Code セッション ID が続く。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrabState {
    pub branch: String,
    pub source_worktree: PathBuf,
    pub stash_branch: String,
    pub claude_session_id: Option<String>,
}

impl GrabState {
    fn to_file_content(&self) -> String {
        let mut content = format!(
            "{}\n{}\n{}\n",
            self.branch,
            self.source_worktree.display(),
            self.stash_branch
        );
        if let Some(id) = &self.claude_session_id {
            content.push_str(id);
            content.push('\n');
        }
        content
    }

    fn parse(content: &str) -> Result<Self> {
        let mut lines = content.lines();
        let mut required = |what: &str| {
            lines
                .next()
                .map(String::from)
                .ok_or_else(|| anyhow!("wt-grab: missing {what} line"))
        };
        let branch = required("branch")?;
        let source_worktree = PathBuf::from(required("worktree path")?);
        let stash_branch = required("stash branch")?;
        let claude_session_id = lines.next().filter(|s| !s.is_empty()).map(String::from);
        Ok(Self {
            branch,
            source_worktree,
            stash_branch,
            claude_session_id,
        })
    }
}

impl GitEngine {
    /// tracked ファイルに未コミットの変更があるか。
    ///
    /// zsh の wt grab と判定を揃えるため `git diff --quiet HEAD` を使う。libgit2 の
    /// status は rename や type-change の縁で git diff HEAD より多く報告し、誤検知になる。
    pub fn has_tracked_changes(&self, worktree_path: &Path) -> Result<bool> {
        use std::process::{Command, Stdio};

        let status = Command::new("git")
            .args(["diff", "--quiet", "HEAD"])
            .current_dir(worktree_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("failed to run `git diff --quiet HEAD`")?;
        Ok(!status.success())
    }

    /// 全 worktree で共有される git ディレクトリ (main の .git/)。
    pub fn git_common_dir(&self) -> Result<PathBuf> {
        let dir = self.repo.commondir();
        Ok(dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()))
    }

    pub fn save_grab_state(&self, state: &GrabState) -> Result<()> {
        let grab_file = self.grab_file()?;
        std::fs::write(&grab_file, state.to_file_content())
            .with_context(|| format!("failed to write {}", grab_file.display()))
    }

    /// wt-grab ファイルが無ければ None。
    pub fn load_grab_state(&self) -> Result<Option<GrabState>> {
        let grab_file = self.grab_file()?;
        if !grab_file.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&grab_file)
            .with_context(|| format!("failed to read {}", grab_file.display()))?;
        GrabState::parse(&content).map(Some)
    }

    pub fn remove_grab_state(&self) -> Result<()> {
        let grab_file = self.grab_file()?;
        if grab_file.exists() {
            std::fs::remove_file(&grab_file)
                .with_context(|| format!("failed to remove {}", grab_file.display()))?;
        }
        Ok(())
    }

    /// source worktree を `<branch>__grab` へ逃がし、main worktree を branch_name に
    /// チェックアウトして、状態を wt-grab に残す。両 worktree が clean であること。
    pub fn grab_branch(
        &self,
        main_path: &Path,
        source_worktree_path: &Path,
        branch_name: &str,
        claude_session_id: Option<&str>,
    ) -> Result<()> {
        if self.has_tracked_changes(main_path)? {
            anyhow::bail!("Main worktree has uncommitted tracked changes. Commit or stash first.");
        }
        if self.has_tracked_changes(source_worktree_path)? {
            anyhow::bail!(
                "Worktree '{branch_name}' has uncommitted tracked changes. Commit or stash first."
            );
        }

        let stash_branch = format!("{branch_name}__grab");
        let source_repo = Repository::open(source_worktree_path).with_context(|| {
            format!("cannot open worktree at {}", source_worktree_path.display())
        })?;
        let head_commit = source_repo.head()?.peel_to_commit()?;
        source_repo
            .branch(&stash_branch, &head_commit, false)
            .with_context(|| format!("failed to create branch '{stash_branch}'"))?;
        Self::checkout_branch(&source_repo, &stash_branch)?;

        let main_repo = Repository::open(main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;
        Self::checkout_branch(&main_repo, branch_name)?;

        self.save_grab_state(&GrabState {
            branch: branch_name.to_string(),
            source_worktree: source_worktree_path.to_path_buf(),
            stash_branch,
            claude_session_id: claude_session_id.map(String::from),
        })
    }

    /// grab を巻き戻す: main を main_branch へ、source worktree を branch_name へ戻し、
    /// `<branch>__grab` と wt-grab を消す。両 worktree が clean であること。
    pub fn ungrab_branch(
        &self,
        main_path: &Path,
        source_worktree_path: &Path,
        branch_name: &str,
        main_branch: &str,
    ) -> Result<()> {
        if self.has_tracked_changes(main_path)? {
            anyhow::bail!("Main worktree has uncommitted tracked changes. Commit or stash first.");
        }
        if self.has_tracked_changes(source_worktree_path)? {
            anyhow::bail!(
                "Worktree (on __grab) has uncommitted tracked changes. Commit or stash first."
            );
        }

        // grab 中に積まれたコミットも反映されるよう、checkout ではなく hard reset で戻す。
        {
            let main_repo = Repository::open(main_path)
                .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;
            Self::reset_to_branch(&main_repo, main_branch)?;
        }
        let source_repo = Repository::open(source_worktree_path).with_context(|| {
            format!("cannot open worktree at {}", source_worktree_path.display())
        })?;
        Self::reset_to_branch(&source_repo, branch_name)?;

        if let Ok(mut stash) =
            source_repo.find_branch(&format!("{branch_name}__grab"), git2::BranchType::Local)
        {
            let _ = stash.delete();
        }
        self.remove_grab_state()
    }

    fn grab_file(&self) -> Result<PathBuf> {
        Ok(self.git_common_dir()?.join("wt-grab"))
    }

    fn checkout_branch(repo: &Repository, branch: &str) -> Result<()> {
        repo.set_head(&format!("refs/heads/{branch}"))
            .with_context(|| format!("failed to set HEAD to '{branch}'"))?;
        repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .with_context(|| format!("failed to checkout '{branch}'"))
    }

    fn reset_to_branch(repo: &Repository, branch: &str) -> Result<()> {
        repo.set_head(&format!("refs/heads/{branch}"))
            .with_context(|| format!("failed to set HEAD to '{branch}'"))?;
        let head_commit = repo.head()?.peel_to_commit()?;
        repo.reset(head_commit.as_object(), git2::ResetType::Hard, None)
            .with_context(|| format!("failed to reset to '{branch}'"))
    }
}
