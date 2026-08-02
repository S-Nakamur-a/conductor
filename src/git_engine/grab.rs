//! wt grab/wt ungrab によるブランチ入れ替えワークフロー: ある worktree で
//! チェックアウト中のブランチを別の worktree へ移し、また元に戻す。クラッシュ
//! からの復旧と zsh の wt との互換性のため状態を永続化する。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use git2::Repository;

use super::GitEngine;

impl GitEngine {
    // Grab / Ungrab

    /// worktree に tracked ファイルの未コミット変更があるか確認する。
    ///
    /// wt grab zsh ヘルパーの挙動と正確に一致させるため git diff --quiet
    /// HEAD(シェルアウト)を使う。libgit2 の status API は git diff HEAD
    /// が報告しない余分なエントリ(rename、type-change、ignored ファイルの
    /// エッジケース)を報告することがあり、誤検知の原因になる。
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

        // exit 0 = clean, exit 1 = dirty
        Ok(!status.success())
    }

    /// git の共有ディレクトリを返す(main worktree なら .git/、linked
    /// worktree なら commondir 経由で解決したパス)。ここが ref、object、
    /// そして自前の wt-grab 状態ファイルが置かれる共有ディレクトリになる。
    pub fn git_common_dir(&self) -> Result<PathBuf> {
        let git_dir = self.repo.path(); // .git/ か .git/worktrees/<name>/
        // .git/worktrees/<name>/commondir があれば、共有 .git/ を指しているか確認する。
        let commondir_file = git_dir.join("commondir");
        if commondir_file.exists() {
            let content =
                std::fs::read_to_string(&commondir_file).context("failed to read commondir")?;
            let relative = content.trim();
            let resolved = git_dir.join(relative);
            return Ok(resolved.canonicalize().unwrap_or(resolved));
        }
        // すでに main リポジトリの .git/ ディレクトリである。
        Ok(git_dir.to_path_buf())
    }

    /// grab の状態を $git_common_dir/wt-grab に永続化する。
    /// フォーマット: 必須の3行(branch, worktree path, stash branch)に加え、
    /// 任意の4行目(resume 用の Claude Code セッション ID)。
    pub fn save_grab_state(
        &self,
        branch: &str,
        source_worktree_path: &Path,
        stash_branch: &str,
        claude_session_id: Option<&str>,
    ) -> Result<()> {
        let grab_file = self.git_common_dir()?.join("wt-grab");
        let mut content = format!(
            "{}\n{}\n{}\n",
            branch,
            source_worktree_path.display(),
            stash_branch,
        );
        if let Some(session_id) = claude_session_id {
            content.push_str(session_id);
            content.push('\n');
        }
        std::fs::write(&grab_file, content)
            .with_context(|| format!("failed to write {}", grab_file.display()))
    }

    /// grab の状態を $git_common_dir/wt-grab から読み込む。
    /// ファイルが存在しない場合は (branch, source_worktree_path,
    /// stash_branch, claude_session_id) の代わりに None を返す。4番目の
    /// フィールド(session ID)は任意。
    #[allow(clippy::type_complexity)]
    pub fn load_grab_state(&self) -> Result<Option<(String, PathBuf, String, Option<String>)>> {
        let grab_file = self.git_common_dir()?.join("wt-grab");
        if !grab_file.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&grab_file)
            .with_context(|| format!("failed to read {}", grab_file.display()))?;
        let mut lines = content.lines();
        let branch = lines
            .next()
            .ok_or_else(|| anyhow!("wt-grab: missing branch line"))?
            .to_string();
        let wt_path = PathBuf::from(
            lines
                .next()
                .ok_or_else(|| anyhow!("wt-grab: missing worktree path line"))?,
        );
        let stash_branch = lines
            .next()
            .ok_or_else(|| anyhow!("wt-grab: missing stash branch line"))?
            .to_string();
        let claude_session_id = lines
            .next()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        Ok(Some((branch, wt_path, stash_branch, claude_session_id)))
    }

    /// $git_common_dir/wt-grab の状態ファイルを削除する。
    pub fn remove_grab_state(&self) -> Result<()> {
        let grab_file = self.git_common_dir()?.join("wt-grab");
        if grab_file.exists() {
            std::fs::remove_file(&grab_file)
                .with_context(|| format!("failed to remove {}", grab_file.display()))?;
        }
        Ok(())
    }

    /// ブランチを grab する: source worktree を一時的な __grab ブランチへ
    /// 移し、main を元のブランチへチェックアウトする。
    ///
    /// 両方の worktree に未コミットの tracked 変更がないことが前提。
    /// 状態は $git_common_dir/wt-grab に永続化する。
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

        let grab_branch_name = format!("{branch_name}__grab");

        // source worktree に __grab ブランチを作成してチェックアウトする。
        let source_repo = Repository::open(source_worktree_path).with_context(|| {
            format!("cannot open worktree at {}", source_worktree_path.display())
        })?;
        let head_commit = source_repo.head()?.peel_to_commit()?;
        source_repo
            .branch(&grab_branch_name, &head_commit, false)
            .with_context(|| format!("failed to create branch '{grab_branch_name}'"))?;
        source_repo
            .set_head(&format!("refs/heads/{grab_branch_name}"))
            .with_context(|| format!("failed to set HEAD to '{grab_branch_name}'"))?;
        source_repo
            .checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .context("failed to checkout __grab branch")?;

        // main worktree を元のブランチへチェックアウトする。
        let main_repo = Repository::open(main_path)
            .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;
        main_repo
            .set_head(&format!("refs/heads/{branch_name}"))
            .with_context(|| format!("failed to set main HEAD to '{branch_name}'"))?;
        main_repo
            .checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .with_context(|| format!("failed to checkout '{branch_name}' in main worktree"))?;

        // クラッシュ復旧と zsh の wt との互換性のため grab 状態を永続化する。
        self.save_grab_state(
            branch_name,
            source_worktree_path,
            &grab_branch_name,
            claude_session_id,
        )?;

        Ok(())
    }

    /// ungrab: main を main ブランチへ戻し、source worktree を元のブランチへ
    /// 復元し、一時的な __grab ブランチを削除する。
    ///
    /// 両方の worktree に未コミットの tracked 変更がないことが前提。grab の後
    /// にコミットが追加されていても index と working tree が確実に更新される
    /// よう set_head + hard reset を使う。
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

        // main worktree を main ブランチへ戻す。
        // source worktree の repo を開く前に main_repo を drop(ファイル
        // ハンドルを解放)させるためスコープで囲む。
        {
            let main_repo = Repository::open(main_path)
                .with_context(|| format!("cannot open main worktree at {}", main_path.display()))?;
            main_repo
                .set_head(&format!("refs/heads/{main_branch}"))
                .with_context(|| format!("failed to set main HEAD to '{main_branch}'"))?;
            let head_commit = main_repo.head()?.peel_to_commit()?;
            main_repo
                .reset(head_commit.as_object(), git2::ResetType::Hard, None)
                .with_context(|| format!("failed to reset main worktree to '{main_branch}'"))?;
        }

        // source worktree を元のブランチへ戻す。
        let source_repo = Repository::open(source_worktree_path).with_context(|| {
            format!("cannot open worktree at {}", source_worktree_path.display())
        })?;
        source_repo
            .set_head(&format!("refs/heads/{branch_name}"))
            .with_context(|| format!("failed to set HEAD to '{branch_name}'"))?;
        let head_commit = source_repo.head()?.peel_to_commit()?;
        source_repo
            .reset(head_commit.as_object(), git2::ResetType::Hard, None)
            .with_context(|| format!("failed to reset worktree to '{branch_name}'"))?;

        // 一時的な __grab ブランチを削除する。
        let grab_branch_name = format!("{branch_name}__grab");
        if let Ok(mut grab_branch) =
            source_repo.find_branch(&grab_branch_name, git2::BranchType::Local)
        {
            let _ = grab_branch.delete();
        }

        // 永続化していた grab 状態ファイルを削除する。
        self.remove_grab_state()?;

        Ok(())
    }
}
