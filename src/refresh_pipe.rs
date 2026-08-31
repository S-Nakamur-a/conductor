//! MCP をきっかけに UI をリフレッシュするための名前付きパイプ (FIFO) のリスナ。
//!
//! MCP サーバはレビューデータを変更したあと (返信、解決など) に
//! .conductor/refresh.pipe へ書き込む。バックグラウンドスレッドがパイプから
//! 読み取り、mpsc チャネルでメインループへイベントを転送する。メインループは
//! それを受けて refresh_reviews() を呼ぶ。

use std::io::{Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

/// 終了時に読み取りスレッドの終了を待つ上限。
///
/// 突いても起きないスレッド (FIFO が外から消された等) のために終了そのものを
/// 諦めるより、後始末を捨てて抜けられるほうがよい。
const SHUTDOWN_JOIN_TIMEOUT: Duration = Duration::from_millis(500);

/// MCP がリフレッシュ用パイプへ書いたときに送られるイベント。
#[derive(Debug)]
pub struct RefreshEvent;

/// MCP サーバからの UI リフレッシュ信号を名前付きパイプで待ち受ける。
pub struct RefreshPipe {
    rx: mpsc::Receiver<RefreshEvent>,
    pipe_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RefreshPipe {
    /// 指定したリポジトリルート配下の .conductor/refresh.pipe に紐づけた
    /// リスナを作る。
    pub fn new(repo_path: &Path) -> anyhow::Result<Self> {
        let conductor_dir = crate::git_engine::GitEngine::open(repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| repo_path.to_path_buf())
            .join(".conductor");
        std::fs::create_dir_all(&conductor_dir)?;

        let pipe_path = conductor_dir.join("refresh.pipe");

        // 前回の実行で残ったパイプを消して作り直す。
        if pipe_path.exists() {
            let _ = std::fs::remove_file(&pipe_path);
        }

        let path_cstr = std::ffi::CString::new(
            pipe_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("non-UTF-8 path"))?,
        )?;
        // SAFETY: mkfifo は標準的な POSIX 呼び出しで、path_cstr は有効かつ
        // null 終端されている。モード 0o660 は所有者とグループに読み書きを与える。
        let ret = unsafe { libc::mkfifo(path_cstr.as_ptr(), 0o660) };
        if ret != 0 {
            return Err(anyhow::anyhow!(
                "mkfifo failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let (tx, rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);
        let path_for_thread = pipe_path.clone();

        let thread = std::thread::Builder::new()
            .name("refresh-pipe".into())
            .spawn(move || {
                Self::read_loop(path_for_thread, tx, shutdown_flag);
            })?;

        Ok(Self {
            rx,
            pipe_path,
            shutdown,
            thread: Some(thread),
        })
    }

    pub fn poll(&self) -> Option<RefreshEvent> {
        self.rx.try_recv().ok()
    }

    fn read_loop(pipe_path: PathBuf, tx: mpsc::Sender<RefreshEvent>, shutdown: Arc<AtomicBool>) {
        // パイプを開き直すループになっているのは、FIFO は書き手が全員閉じると
        // EOF を返すから。EOF のたびに開き直して次の書き手を待つ。
        while !shutdown.load(Ordering::Relaxed) {
            // FIFO を読み取り用に開く (書き手がつながるまでブロックする)。
            // 生の libc::open を使うのは、Rust の File::open が FIFO に対して
            // 開く時点での O_NONBLOCK に対応していないため。
            let path_cstr = match std::ffi::CString::new(pipe_path.to_string_lossy().as_ref()) {
                Ok(c) => c,
                Err(_) => break,
            };

            // SAFETY: 標準的な POSIX の open。path_cstr は有効かつ null 終端されている。
            let fd: RawFd = unsafe { libc::open(path_cstr.as_ptr(), libc::O_RDONLY) };
            if fd < 0 {
                // パイプが消えた (終了処理か後始末)。
                break;
            }

            // SAFETY: fd はこちらが排他的に所有する有効な open 済みファイルディスクリプタ。
            let mut file = unsafe { std::fs::File::from_raw_fd(fd) };

            let mut buf = [0u8; 64];
            loop {
                if shutdown.load(Ordering::Relaxed) {
                    return;
                }
                match file.read(&mut buf) {
                    Ok(0) => {
                        // EOF。書き手が全員閉じた。開き直す。
                        break;
                    }
                    Ok(_) => {
                        if tx.send(RefreshEvent).is_err() {
                            // 受信側が落ちた = メインループが終了した。
                            return;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                        continue;
                    }
                    Err(_) => break,
                }
            }

            // 開き直す前に少し待つ。EOF が高速に繰り返されたときのビジーループを避ける。
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// パイプのパスを明示してリスナを作る (テスト用)。
    #[cfg(test)]
    fn from_path(pipe_path: PathBuf) -> anyhow::Result<Self> {
        if pipe_path.exists() {
            let _ = std::fs::remove_file(&pipe_path);
        }

        let path_cstr = std::ffi::CString::new(
            pipe_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("non-UTF-8 path"))?,
        )?;
        let ret = unsafe { libc::mkfifo(path_cstr.as_ptr(), 0o660) };
        if ret != 0 {
            return Err(anyhow::anyhow!(
                "mkfifo failed: {}",
                std::io::Error::last_os_error()
            ));
        }

        let (tx, rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);
        let path_for_thread = pipe_path.clone();

        let thread = std::thread::Builder::new()
            .name("refresh-pipe-test".into())
            .spawn(move || {
                Self::read_loop(path_for_thread, tx, shutdown_flag);
            })?;

        Ok(Self {
            rx,
            pipe_path,
            shutdown,
            thread: Some(thread),
        })
    }
}

/// TUI のリフレッシュ用 FIFO を突いて、レビューデータを読み直させる。
///
/// mcp-serve が書き込みのたびに呼ぶ。ベストエフォートなのは意図的で、よくある
/// 「失敗」は conductor が動いていないこと。その場合 FIFO が存在しないか読み手が
/// いないかで、後者は O_NONBLOCK により ENXIO になる。どちらも表に出す価値は
/// ない。書き込み自体は既に成功しているし、次に TUI を開いたときにはどのみち
/// データベースを読み直すため。
///
/// ハングしないのは O_NONBLOCK のおかげ。FIFO を書き込み用に開くのは読み手が
/// つながるまでブロックするので、これが無いとツール呼び出しが固まってしまう。
pub fn signal_refresh(pipe_path: &Path) {
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

    // SAFETY: fd はこちらが排他的に所有する有効なディスクリプタで、drop 時に閉じられる。
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };

    // 書き込むのは本物の FIFO に対してだけにする。refresh.pipe が通常ファイルや
    // その symlink として戻ってきた場合 (特殊ファイルを含まないバックアップの復元、
    // 特殊ファイルを運ばないツールで展開したアーカイブなど)、この書き込みは
    // オフセット 0 に着地して、その実体が何であれ先頭バイトを潰してしまう。
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

impl Drop for RefreshPipe {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);

        // パイプを短時間だけ書き込み用に開いて、読み手のブロックを解く。
        if self.pipe_path.exists() {
            let path_cstr = std::ffi::CString::new(self.pipe_path.to_string_lossy().as_ref());
            if let Ok(cstr) = path_cstr {
                // SAFETY: O_WRONLY | O_NONBLOCK を指定した標準的な POSIX の open。
                // 読み手がいなくても O_NONBLOCK によりブロックしない。
                unsafe {
                    let fd = libc::open(cstr.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK);
                    if fd >= 0 {
                        libc::close(fd);
                    }
                }
            }
        }

        if let Some(thread) = self.thread.take() {
            crate::background::join_or_abandon(thread, SHUTDOWN_JOIN_TIMEOUT);
        }
        let _ = std::fs::remove_file(&self.pipe_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 自前の複製ではなく本物の signal_refresh を通す。複製すると、本番で書き込み系の
    /// mcp-serve ツールが全部呼んでいるその関数が壊れてもテストが通ってしまう。
    fn write_to_pipe(pipe_path: &Path) {
        super::signal_refresh(pipe_path);
    }

    #[test]
    fn 書き込み1回でイベントが出る() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let listener = RefreshPipe::from_path(pipe_path.clone()).unwrap();

        // バックグラウンドスレッドがパイプを読み取り用に開くのを待つ。
        std::thread::sleep(Duration::from_millis(100));

        write_to_pipe(&pipe_path);
        std::thread::sleep(Duration::from_millis(200));

        assert!(listener.poll().is_some(), "expected a RefreshEvent");

        drop(listener);
        assert!(!pipe_path.exists(), "pipe should be cleaned up on drop");
    }

    #[test]
    fn 複数回の書き込みでその数だけイベントが出る() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let listener = RefreshPipe::from_path(pipe_path.clone()).unwrap();

        std::thread::sleep(Duration::from_millis(100));

        write_to_pipe(&pipe_path);
        // 書き手が閉じる → EOF → 読み手が開き直す。開き直しを待つ。
        std::thread::sleep(Duration::from_millis(200));

        write_to_pipe(&pipe_path);
        std::thread::sleep(Duration::from_millis(200));

        // 少なくとも 2 件のイベントを受け取っているはず。
        let mut count = 0;
        while listener.poll().is_some() {
            count += 1;
        }
        assert!(count >= 2, "expected at least 2 events, got {count}");
    }

    #[test]
    fn 書き込みが無ければイベントも出ない() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let listener = RefreshPipe::from_path(pipe_path).unwrap();

        std::thread::sleep(Duration::from_millis(100));
        assert!(listener.poll().is_none(), "expected no event");
    }

    /// データベースの .conductor/ にまだ refresh.pipe すら無いときの
    /// 「conductor が動いていない」経路。libc::open は ENOENT で失敗するが、
    /// ここで panic してはいけない。
    #[test]
    fn パイプが無ければ即座に返る() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("does-not-exist.pipe");
        signal_refresh(&pipe_path); // panic しないこと
    }

    /// refresh.pipe が (過去の実行で作られて) 存在するのに、今は誰も読んでいない
    /// ことがある。mcp-serve は自分で RefreshPipe のリスナを立てないまま書き込む。
    /// ハングしないのは O_NONBLOCK のおかげで、FIFO を書き込み用に開くのは本来
    /// 読み手がつながるまでブロックし、ツール呼び出しが永遠に固まってしまう。
    /// ここで退行したときに CI がハングするのではなくテストが落ちるよう、
    /// バックグラウンドスレッドでタイムアウト付きで実行する。
    #[test]
    fn 読み手がいなければ即座に返る() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let path_cstr = std::ffi::CString::new(pipe_path.to_str().unwrap()).unwrap();
        // SAFETY: 標準的な POSIX の mkfifo。path_cstr は有効かつ null 終端されている。
        let ret = unsafe { libc::mkfifo(path_cstr.as_ptr(), 0o660) };
        assert_eq!(ret, 0, "failed to create test FIFO");

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            signal_refresh(&pipe_path);
            let _ = tx.send(());
        });

        rx.recv_timeout(Duration::from_secs(2))
            .expect("signal_refresh hung on a reader-less FIFO");
    }

    /// 何かの理由で refresh.pipe が通常ファイルだった場合 (特殊ファイル抜きで
    /// 展開されたアーカイブ、復元されたバックアップなど)、書き込みはオフセット 0 に
    /// 着地して、その実体が何であれ先頭バイトを潰す。FIFO かどうかの確認は書き込みの
    /// 前に来なければならない。
    #[test]
    fn 普通のファイルには書き込まない() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_pipe = dir.path().join("refresh.pipe");
        let original = "IMPORTANT PRE-EXISTING CONTENT";
        std::fs::write(&not_a_pipe, original).unwrap();

        signal_refresh(&not_a_pipe);

        assert_eq!(
            std::fs::read_to_string(&not_a_pipe).unwrap(),
            original,
            "signal_refresh overwrote a regular file"
        );
    }

    /// Ctrl+Q が効かなくなる退行の番人。
    ///
    /// FIFO が外から消えると読み取りスレッドは誰も辿り着けない inode の
    /// open() で寝たままになり、突いても起きない。終了はそれでも進むこと。
    #[test]
    fn パイプが消えたリスナを畳んでもハングしない() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let listener = RefreshPipe::from_path(pipe_path.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        std::fs::remove_file(&pipe_path).unwrap();

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            drop(listener);
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("shutdown hung on a vanished FIFO");
    }
}
