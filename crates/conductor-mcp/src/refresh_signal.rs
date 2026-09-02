//! TUI のリフレッシュ用 FIFO (`<main worktree>/.conductor/refresh.pipe`) を突く側。
//! 読む側は TUI が持つ。

use std::io::Write;
use std::os::unix::io::FromRawFd;
use std::path::Path;

/// FIFO を突いて、TUI にレビューデータを読み直させる。
///
/// ベストエフォートなのは意図的で、よくある「失敗」は conductor が動いていない
/// こと (FIFO が無いか、読み手がおらず O_NONBLOCK で ENXIO)。書き込み自体は既に
/// 成功しているし、次に TUI を開けばどのみち読み直される。
///
/// O_NONBLOCK が無いと、FIFO を書き込み用に開く時点で読み手が繋がるまでブロック
/// してツール呼び出しが固まる。
pub(crate) fn signal_refresh(pipe_path: &Path) {
    let Some(path) = pipe_path.to_str() else {
        log::warn!("refresh pipe path is not UTF-8: {}", pipe_path.display());
        return;
    };
    let Ok(path_cstr) = std::ffi::CString::new(path) else {
        return;
    };

    // SAFETY: 標準的な POSIX の open。path_cstr は有効かつ null 終端されている。
    let fd = unsafe { libc::open(path_cstr.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK) };
    if fd < 0 {
        log::debug!(
            "refresh pipe not writable ({}): {}",
            pipe_path.display(),
            std::io::Error::last_os_error()
        );
        return;
    }

    // SAFETY: fd はこちらが排他的に所有する有効なディスクリプタで、drop で閉じる。
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };

    // 通常ファイルだと書き込みがオフセット 0 に着地して先頭バイトを潰す。
    // refresh.pipe が通常ファイルとして戻ってくることは実際にある (特殊ファイルを
    // 含まないバックアップの復元、特殊ファイルを運ばないツールでの展開)。
    // SAFETY: fd は open 済みで file が所有している。stat は成功時のみ書かれる。
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    let is_fifo = unsafe { libc::fstat(fd, &mut stat) } == 0
        && (stat.st_mode & libc::S_IFMT) == libc::S_IFIFO;
    if !is_fifo {
        log::warn!(
            "refresh pipe is not a FIFO, refusing to write: {}",
            pipe_path.display()
        );
        return;
    }

    if let Err(e) = file.write_all(b"r") {
        log::debug!("refresh pipe write failed: {e}");
    }
}
