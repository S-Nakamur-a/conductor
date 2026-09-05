use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use super::*;
use crate::test_support::TestRepo;

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap()
}

/// ロックファイルは残す。消しに行くと、別のプロセスが既に作って掴んだ実体を消してしまう。
#[test]
fn 先に取ったロックを離すまで次の取得は拒まれる() {
    let dir = tempfile::tempdir().unwrap();
    let first = acquire(dir.path()).unwrap().expect("first acquire");
    assert!(acquire(dir.path()).unwrap().is_none());

    drop(first);
    assert!(acquire(dir.path()).unwrap().is_some());
}

#[test]
fn 別のリポジトリ同士は邪魔し合わない() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let _lock_a = acquire(a.path()).unwrap().expect("repo a");
    assert!(acquire(b.path()).unwrap().is_some());
}

/// ロックはリポジトリにつき 1 つで、linked worktree もメインワークツリーと同じ 1 つを
/// 取り合う。どちらから先に取っても、もう一方は拒まれる。
#[test]
fn 全worktreeでロックは1つ() {
    let repo = TestRepo::with_base_commit();
    let linked = repo.linked_worktree("feature");

    assert_eq!(
        canonical(&locked_repo_root(&linked.path)),
        canonical(&repo.path)
    );

    let orderings = [(&repo.path, &linked.path), (&linked.path, &repo.path)];
    for (first, second) in orderings {
        let held = acquire(first).unwrap().expect("first acquire");
        assert!(
            acquire(second).unwrap().is_none(),
            "{} must be refused while {} holds the lock",
            second.display(),
            first.display()
        );
        drop(held);
    }
}

/// 更新後の再起動は exec でプロセスイメージを置き換える。ロックの fd がそこを生き延びると、
/// 新しいイメージは自分自身が握ったロックに弾かれて起動できなくなる。
#[test]
fn ロックはexecを越えて残らない() {
    let dir = tempfile::tempdir().unwrap();
    let lock = acquire(dir.path()).unwrap().expect("acquire");
    // SAFETY: fd は lock が所有していて有効。F_GETFD は何も変更しない。
    let flags = unsafe { libc::fcntl(lock._file.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0, "F_GETFD failed");
    assert!(flags & libc::FD_CLOEXEC != 0);
}
