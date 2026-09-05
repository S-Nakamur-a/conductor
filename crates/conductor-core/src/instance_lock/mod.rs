//! 同じリポジトリを 2 つの conductor が同時に開かないようにする排他ロック。
//!
//! .conductor/ に置くリソース (レビュー DB、cc-notify ソケット、リフレッシュ用 FIFO) は
//! どれもリポジトリにつき 1 つを前提にしている。取り合いを個別に捌くより、2 つ目を
//! 起動させないほうが単純で、壊れ方も読める。
//!
//! pid ファイルでなく flock なのは、ロックの寿命をプロセスの寿命に縛るため。fd が閉じた
//! 時点でカーネルが解放するので、クラッシュや SIGKILL の後始末そのものが要らない。

use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[cfg(test)]
mod tests;

/// 保持している間だけ、このリポジトリを開く権利を持つ。drop (プロセス終了を含む) で解放される。
pub struct InstanceLock {
    _file: std::fs::File,
}

/// このリポジトリのロックファイルのパス。linked worktree から見てもメインワークツリーの 1 つ。
pub fn lock_path(repo_path: &Path) -> PathBuf {
    crate::git_engine::conductor_dir(repo_path).join("conductor.lock")
}

/// このロックが覆うリポジトリのルート。
///
/// 断るときに「どれが開いているのか」として見せるのは、ユーザーがいま打ったパスではなく
/// ロックの単位であるメインワークツリー。
pub fn locked_repo_root(repo_path: &Path) -> PathBuf {
    crate::git_engine::conductor_dir(repo_path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo_path.to_path_buf())
}

/// リポジトリのロックを取る。`Ok(None)` は別の conductor が持っている。
///
/// `Err` はロックの仕組み自体が使えなかった (権限、flock 非対応のファイルシステム)。
/// 呼び出し側は警告だけ出して起動を続ける。排他できないことを理由にリポジトリを
/// 開けなくするほうが害が大きい。
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

    // SAFETY: fd は file が所有していて有効。
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(Some(InstanceLock { _file: file }));
    }

    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(None);
    }
    Err(anyhow::Error::new(err).context(format!("could not lock {}", path.display())))
}
