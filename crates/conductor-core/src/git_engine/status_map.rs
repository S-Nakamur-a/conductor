//! パスから git status を引く一発スナップショット。Explorer のリフレッシュごとに
//! 1 回だけ読み、ファイルツリーの薄暗い表示と Changed files の色分けで共有する。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use git2::{Repository, Status, StatusOptions, StatusShow};

/// ファイルツリーの薄暗い表示のための大まかな分類。staged / unstaged の区別は
/// [GitStatusMap::status] 側。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeGitState {
    Tracked,
    Untracked,
    Ignored,
}

/// libgit2 が報告する worktree ルートからの相対パスをキーにした status。
#[derive(Debug, Clone, Default)]
pub struct GitStatusMap {
    by_path: HashMap<String, Status>,
}

impl GitStatusMap {
    pub fn load(worktree_path: &Path) -> Result<Self> {
        let repo = Repository::discover(worktree_path)
            .with_context(|| format!("failed to discover repo from {}", worktree_path.display()))?;

        // recurse_ignored_dirs は付けない。付けると ignored ディレクトリの中身を全部
        // 走査する。21GB の target/ を持つ親リポジトリで実測 2,771ms / 122,407 件、
        // 付けなければ約 3.1ms。これは 3 秒ポーリングから UI スレッドで走る。
        let mut opts = StatusOptions::new();
        opts.show(StatusShow::IndexAndWorkdir)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(true);
        let statuses = repo
            .statuses(Some(&mut opts))
            .context("failed to compute git status")?;

        let workdir = repo.workdir();
        let mut by_path = HashMap::with_capacity(statuses.len());
        for entry in statuses.iter() {
            let Some(path) = entry.path() else {
                continue;
            };
            let status = drop_deletion_still_on_disk(entry.status(), workdir, path);
            if !status.is_empty() {
                by_path.insert(path.to_string(), status);
            }
        }
        Ok(Self { by_path })
    }

    /// git status が報告した生のビット。変更なしの tracked ファイルと、折りたたまれた
    /// ignored ディレクトリの配下は None (祖先を遡らない。それは [classify](Self::classify))。
    pub fn status(&self, path: &str) -> Option<Status> {
        self.by_path.get(path).copied()
    }

    /// path (ファイルでもディレクトリでも、末尾スラッシュ無し) を分類する。
    ///
    /// libgit2 は ignored ディレクトリを `target/` のような末尾スラッシュ付きの 1 エントリ
    /// に折りたたむので、配下のパスは祖先から Ignored を継承する。untracked ディレクトリは
    /// recurse_untracked_dirs でファイル単位に展開されるため、ディレクトリ自身には
    /// エントリが無く、配下が全部 untracked かで判定する。
    pub fn classify(&self, path: &str) -> TreeGitState {
        if let Some(status) = self
            .by_path
            .get(path)
            .or_else(|| self.by_path.get(&format!("{path}/")))
        {
            return classify_status(*status);
        }
        let ignored_ancestor = ancestor_dirs(path)
            .any(|ancestor| self.by_path.get(&ancestor).is_some_and(Status::is_ignored));
        if ignored_ancestor {
            return TreeGitState::Ignored;
        }
        let mut descendants = self.descendants(path).peekable();
        if descendants.peek().is_some() && descendants.all(|s| s.is_wt_new()) {
            return TreeGitState::Untracked;
        }
        TreeGitState::Tracked
    }

    fn descendants<'a>(&'a self, path: &str) -> impl Iterator<Item = Status> + 'a {
        let prefix = format!("{path}/");
        self.by_path
            .iter()
            .filter(move |(k, _)| k.starts_with(&prefix))
            .map(|(_, s)| *s)
    }
}

/// 大文字小文字を区別しない FS ではケース違いの 2 エントリに実ファイルが 1 つしか無く、
/// libgit2 は余った方を削除として報告する (git 本体は clean)。
fn drop_deletion_still_on_disk(status: Status, workdir: Option<&Path>, path: &str) -> Status {
    let still_there =
        status.is_wt_deleted() && workdir.is_some_and(|workdir| workdir.join(path).is_file());
    if still_there {
        status.difference(Status::WT_DELETED)
    } else {
        status
    }
}

fn classify_status(status: Status) -> TreeGitState {
    if status.is_ignored() {
        TreeGitState::Ignored
    } else if status.is_wt_new() {
        TreeGitState::Untracked
    } else {
        TreeGitState::Tracked
    }
}

/// 末尾スラッシュ付き・近い順。"a/b/c.rs" -> ["a/b/", "a/"]。
fn ancestor_dirs(path: &str) -> impl Iterator<Item = String> + '_ {
    let mut current = path;
    std::iter::from_fn(move || {
        let slash = current.rfind('/')?;
        current = &current[..slash];
        Some(format!("{current}/"))
    })
}
