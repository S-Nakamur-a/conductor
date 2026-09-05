//! テストで使う一時 git リポジトリのビルダー。

use std::fs;
use std::path::{Path, PathBuf};

use git2::{Oid, Repository};

use crate::git_engine::GitEngine;

pub(crate) fn signature() -> git2::Signature<'static> {
    git2::Signature::now("Test", "test@test.com").unwrap()
}

/// 1 つの worktree。main worktree にも linked worktree にも同じ操作をする。
pub(crate) struct Tree {
    pub(crate) path: PathBuf,
    pub(crate) repo: Repository,
}

impl Tree {
    pub(crate) fn open(path: PathBuf) -> Self {
        let repo = Repository::open(&path).unwrap();
        Self { path, repo }
    }

    pub(crate) fn file(&self, rel: &str, content: impl AsRef<[u8]>) -> &Self {
        let full = self.path.join(rel);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
        self
    }

    pub(crate) fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path.join(rel)).unwrap()
    }

    pub(crate) fn add(&self, rel: &str) -> &Self {
        let mut index = self.repo.index().unwrap();
        index.add_path(Path::new(rel)).unwrap();
        index.write().unwrap();
        self
    }

    /// index の内容を HEAD にコミットする。
    pub(crate) fn commit(&self, message: &str) -> Oid {
        let sig = signature();
        let mut index = self.repo.index().unwrap();
        let tree = self.repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parent = self.repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        self.repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .unwrap()
    }

    /// parent のツリーに files を重ねたコミットを作る。ref も index も動かさないので、
    /// どの ref が存在するかはテストが決める。
    pub(crate) fn commit_tree(&self, parent: Option<Oid>, files: &[(&str, &[u8])]) -> Oid {
        let parent = parent.map(|oid| self.repo.find_commit(oid).unwrap());
        let base_tree = parent.as_ref().map(|c| c.tree().unwrap());
        let mut builder = self.repo.treebuilder(base_tree.as_ref()).unwrap();
        for (path, content) in files {
            let blob = self.repo.blob(content).unwrap();
            builder.insert(*path, blob, 0o100644).unwrap();
        }
        let tree = self.repo.find_tree(builder.write().unwrap()).unwrap();
        let sig = signature();
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        self.repo
            .commit(None, &sig, &sig, "test commit", &tree, &parents)
            .unwrap()
    }

    /// HEAD にブランチを作ってチェックアウトする。
    pub(crate) fn branch(&self, name: &str) -> &Self {
        let head = self.repo.head().unwrap().peel_to_commit().unwrap();
        self.repo.branch(name, &head, false).unwrap();
        self.checkout(name)
    }

    /// name ブランチを oid に作る (あれば動かす)。チェックアウトはしない。
    pub(crate) fn branch_at(&self, name: &str, oid: Oid) -> &Self {
        let commit = self.repo.find_commit(oid).unwrap();
        self.repo.branch(name, &commit, true).unwrap();
        self
    }

    pub(crate) fn checkout(&self, name: &str) -> &Self {
        self.repo.set_head(&format!("refs/heads/{name}")).unwrap();
        self.repo
            .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        self
    }

    pub(crate) fn checkout_at(&self, name: &str, oid: Oid) -> &Self {
        self.branch_at(name, oid).checkout(name)
    }

    pub(crate) fn tip(&self, branch: &str) -> Oid {
        self.repo
            .find_branch(branch, git2::BranchType::Local)
            .unwrap()
            .get()
            .target()
            .unwrap()
    }

    pub(crate) fn head_branch(&self) -> String {
        self.repo.head().unwrap().shorthand().unwrap().to_string()
    }

    /// リモートを登録せずに refs/remotes/<name> だけを作る。
    pub(crate) fn remote_ref(&self, name: &str, oid: Oid) -> &Self {
        self.repo
            .reference(&format!("refs/remotes/{name}"), oid, true, "test")
            .unwrap();
        self
    }

    pub(crate) fn tag(&self, name: &str, oid: Oid) -> &Self {
        let target = self.repo.find_object(oid, None).unwrap();
        self.repo.tag_lightweight(name, &target, false).unwrap();
        self
    }

    pub(crate) fn annotated_tag(&self, name: &str, oid: Oid) -> &Self {
        let target = self.repo.find_object(oid, None).unwrap();
        self.repo
            .tag(name, &target, &signature(), "annotated tag", false)
            .unwrap();
        self
    }

    pub(crate) fn engine(&self) -> GitEngine {
        GitEngine::open(&self.path).unwrap()
    }
}

/// 一時ディレクトリの `main/` にリポジトリを持ち、linked worktree はその隣に作る。
pub(crate) struct TestRepo {
    root: tempfile::TempDir,
    main: Tree,
}

impl std::ops::Deref for TestRepo {
    type Target = Tree;
    fn deref(&self) -> &Tree {
        &self.main
    }
}

impl TestRepo {
    pub(crate) fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("main");
        let mut opts = git2::RepositoryInitOptions::new();
        opts.initial_head("main");
        Repository::init_opts(&path, &opts).unwrap();
        Self {
            root,
            main: Tree::open(path),
        }
    }

    /// a.txt を 1 つコミットした状態。
    pub(crate) fn with_base_commit() -> Self {
        let repo = Self::new();
        repo.file("a.txt", "base\n").add("a.txt").commit("base");
        repo
    }

    /// HEAD から name ブランチを切り、`<root>/<name>` に linked worktree を作る。
    pub(crate) fn linked_worktree(&self, name: &str) -> Tree {
        let head = self.repo.head().unwrap().peel_to_commit().unwrap();
        self.repo.branch(name, &head, false).unwrap();
        let reference = self
            .repo
            .find_reference(&format!("refs/heads/{name}"))
            .unwrap();
        let path = self.root.path().join(name);
        self.repo
            .worktree(
                name,
                &path,
                Some(git2::WorktreeAddOptions::new().reference(Some(&reference))),
            )
            .unwrap();
        Tree::open(path)
    }

    pub(crate) fn worktrees_dir(&self) -> PathBuf {
        self.root.path().join("worktrees")
    }
}

/// 大文字小文字を区別しないファイルシステムでしか再現しない事象のテストが、
/// 走らない環境で黙って緑にならないよう cfg と組で使う。
#[cfg(target_os = "macos")]
pub(crate) fn fs_ignores_case(dir: &Path) -> bool {
    let probe = dir.join("CaseProbe");
    fs::write(&probe, b"").unwrap();
    let ignores = dir.join("caseprobe").is_file();
    fs::remove_file(&probe).unwrap();
    ignores
}
