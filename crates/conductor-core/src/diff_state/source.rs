//! diff の出どころ。

use std::path::Path;

use anyhow::{Context, Result};
use git2::{Oid, Repository};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSource {
    /// merge-base(base, HEAD) から作業ツリー (index 込み) まで。
    WorkingTree { base: String },
    /// 最初の親からそのコミットまで。ルートコミットは空の tree から数え、マージ
    /// コミットは 2 つ目以降の親を見ない (git diff <c>^ <c> と同じ)。
    Commit { oid: String },
}

impl DiffSource {
    pub fn working_tree(base: &str) -> Self {
        Self::WorkingTree {
            base: base.to_string(),
        }
    }

    pub fn commit(oid: &str) -> Self {
        Self::Commit {
            oid: oid.to_string(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::WorkingTree { .. } => "working tree".to_string(),
            Self::Commit { oid } => short_oid(oid).to_string(),
        }
    }

    /// diff の新しい側の本文。
    pub fn read_new_side(&self, worktree: &Path, path: &str) -> Result<Vec<u8>> {
        match self {
            Self::WorkingTree { .. } => {
                std::fs::read(worktree.join(path)).map_err(|e| anyhow::anyhow!("{e}"))
            }
            Self::Commit { oid } => {
                let repo = Repository::open(worktree)
                    .with_context(|| format!("cannot open repo at {}", worktree.display()))?;
                let commit = repo.find_commit(Oid::from_str(oid)?)?;
                let entry = commit
                    .tree()?
                    .get_path(Path::new(path))
                    .with_context(|| format!("{path} is not in {}", short_oid(oid)))?;
                let blob = entry.to_object(&repo)?.peel_to_blob()?;
                Ok(blob.content().to_vec())
            }
        }
    }
}

pub fn short_oid(oid: &str) -> &str {
    &oid[..8.min(oid.len())]
}
