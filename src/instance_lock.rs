//! 同じリポジトリを 2 つの Conductor が同時に開かないようにする排他ロック。
//!
//! .conductor/ に置くリソース (レビュー DB、cc-notify ソケット、リフレッシュ用
//! FIFO) はどれもリポジトリにつき 1 つを前提にしていて、2 つ目のインスタンスは
//! それを奪うか諦めるかしかできない。取り合いを個別に捌くより、そもそも
//! 2 つ目を起動させないほうが単純で、壊れ方も読める。
//!
//! flock を使うのは、ロックの寿命をプロセスの寿命に縛れるため。pid を書いた
//! ファイルだと、クラッシュや SIGKILL のあとに「誰も持っていないのに開けない」
//! ロックが残り、それを掃除するコードがまた別の壊れ方をする。flock なら
//! fd が閉じた時点でカーネルが解放するので、後始末そのものが要らない。

use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 保持している間だけ、このリポジトリを開く権利を持つ。
///
/// ロックはファイル記述子に紐づくので、drop (プロセス終了を含む) で解放される。
pub struct InstanceLock {
    _file: std::fs::File,
}

/// このリポジトリのロックファイルのパス。
pub fn lock_path(repo_path: &Path) -> PathBuf {
    crate::git_engine::conductor_dir(repo_path).join("conductor.lock")
}

/// このロックが覆うリポジトリのルート。
///
/// linked worktree から起動してもメインワークツリーを返す。ロックは
/// リポジトリ単位なので、断るときに「どれが開いているのか」として見せるべき
/// なのは、ユーザーがいま打ったパスではなくこちら。
pub fn locked_repo_root(repo_path: &Path) -> PathBuf {
    crate::git_engine::conductor_dir(repo_path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo_path.to_path_buf())
}

/// リポジトリのロックを取る。
///
/// - `Ok(Some(_))`: 取れた。保持している間だけ起動してよい。
/// - `Ok(None)`: 別の Conductor が持っている。
/// - `Err(_)`: ロックの仕組みが使えなかった (権限、flock 非対応のファイル
///   システムなど)。呼び出し側は起動を止めずに続ける — 排他できないことを
///   理由にリポジトリを開けなくするほうが害が大きい。
pub fn acquire(repo_path: &Path) -> Result<Option<InstanceLock>> {
    let path = lock_path(repo_path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;

    // SAFETY: 標準的な POSIX の flock。fd は file が所有していて有効。
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(InstanceLock { _file: file }));
    }

    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(None);
    }
    Err(anyhow::Error::new(err).context(format!("could not lock {}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ロックファイルは残る。掃除しないのが正しい — 消しに行くと、別の
    /// プロセスが既に作って掴んだ実体を消してしまう。
    #[test]
    fn a_second_acquire_is_refused_until_the_first_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path())
            .unwrap()
            .expect("first acquire succeeds");
        assert!(
            acquire(dir.path()).unwrap().is_none(),
            "a second instance must be refused"
        );

        drop(first);
        assert!(
            acquire(dir.path()).unwrap().is_some(),
            "the lock must be free once the holder is gone"
        );
    }

    #[test]
    fn different_repositories_do_not_block_each_other() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let _lock_a = acquire(a.path()).unwrap().expect("repo a");
        assert!(acquire(b.path()).unwrap().is_some(), "repo b is unrelated");
    }

    /// 更新後の再起動は exec でプロセスイメージを置き換える (startup.rs)。
    /// ロックの fd がそこを生き延びると、新しいイメージは自分自身が握った
    /// ロックに弾かれて「既に開いています」で起動できなくなる。
    #[test]
    fn the_lock_does_not_survive_exec() {
        let dir = tempfile::tempdir().unwrap();
        let lock = acquire(dir.path()).unwrap().expect("acquire");
        // SAFETY: fd は lock が所有していて有効。F_GETFD は何も変更しない。
        let flags = unsafe { libc::fcntl(lock._file.as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0, "F_GETFD failed");
        assert!(
            flags & libc::FD_CLOEXEC != 0,
            "the lock fd must be close-on-exec or the post-update restart refuses to start"
        );
    }
}
