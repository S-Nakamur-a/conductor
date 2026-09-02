//! MCP をきっかけに UI をリフレッシュするための名前付きパイプ (FIFO) の読み手。
//!
//! MCP サーバはレビューデータを変更したあと (返信、解決など) に
//! .conductor/refresh.pipe へ書き込む。書く側は conductor-mcp が持つ。ここは
//! バックグラウンドスレッドがパイプから読み取り、[WatchEvent::RefreshRequested]
//! を送るだけ。

use std::io::Read;
use std::os::unix::io::{FromRawFd, RawFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use conductor_core::git_engine::conductor_dir;

use super::WatchEvent;
use crate::EventSender;

/// 終了時に読み取りスレッドの終了を待つ上限。
///
/// 突いても起きないスレッド (FIFO が外から消された等) のために終了そのものを
/// 諦めるより、後始末を捨てて抜けられるほうがよい。
const SHUTDOWN_JOIN_TIMEOUT: Duration = Duration::from_millis(500);

/// MCP サーバからの UI リフレッシュ信号を名前付きパイプで待ち受ける。
pub struct RefreshPipe {
    pipe_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RefreshPipe {
    /// repo_path のリポジトリの .conductor/refresh.pipe に紐づけたリスナを作る。
    pub fn new<P: Send + 'static>(
        repo_path: &std::path::Path,
        sender: EventSender<P>,
    ) -> anyhow::Result<Self> {
        let dir = conductor_dir(repo_path);
        std::fs::create_dir_all(&dir)?;
        Self::from_path(dir.join("refresh.pipe"), sender)
    }

    fn from_path<P: Send + 'static>(
        pipe_path: PathBuf,
        sender: EventSender<P>,
    ) -> anyhow::Result<Self> {
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

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);
        let path_for_thread = pipe_path.clone();

        let thread = std::thread::Builder::new()
            .name("refresh-pipe".into())
            .spawn(move || Self::read_loop(path_for_thread, sender, shutdown_flag))?;

        Ok(Self {
            pipe_path,
            shutdown,
            thread: Some(thread),
        })
    }

    fn read_loop<P: Send + 'static>(
        pipe_path: PathBuf,
        sender: EventSender<P>,
        shutdown: Arc<AtomicBool>,
    ) {
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
                    Ok(0) => break, // EOF。書き手が全員閉じた。開き直す。
                    Ok(_) => sender.send_watch(WatchEvent::RefreshRequested),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }

            // 開き直す前に少し待つ。EOF が高速に繰り返されたときのビジーループを避ける。
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for RefreshPipe {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);

        // パイプを短時間だけ書き込み用に開いて、読み手のブロックを解く。
        if self.pipe_path.exists()
            && let Ok(cstr) = std::ffi::CString::new(self.pipe_path.to_string_lossy().as_ref())
        {
            // SAFETY: O_WRONLY | O_NONBLOCK を指定した標準的な POSIX の open。
            // 読み手がいなくても O_NONBLOCK によりブロックしない。
            unsafe {
                let fd = libc::open(cstr.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK);
                if fd >= 0 {
                    libc::close(fd);
                }
            }
        }

        if let Some(thread) = self.thread.take() {
            crate::join_or_abandon(thread, SHUTDOWN_JOIN_TIMEOUT);
        }
        let _ = std::fs::remove_file(&self.pipe_path);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use crate::Services;

    fn recv_watch(svc: &Services<()>) -> Option<WatchEvent> {
        for _ in 0..200 {
            if let Some(event) = svc.try_recv()
                && let crate::EventKind::Watch(w) = event.kind
            {
                return Some(w);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// テスト用の書き込み。読み手 (RefreshPipe) が先に FIFO を開いている前提で、
    /// ブロッキングの open で足りる — signal_refresh の O_NONBLOCK は「読み手が
    /// いないかもしれない」書き手側の都合であり、conductor-mcp へ移った。
    fn write_to_pipe(pipe_path: &std::path::Path) {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(pipe_path)
            .unwrap();
        file.write_all(b"r").unwrap();
    }

    #[test]
    fn 書き込み1回でイベントが出る() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let svc = Services::<()>::new();
        let listener = RefreshPipe::from_path(pipe_path.clone(), svc.sender()).unwrap();

        // バックグラウンドスレッドがパイプを読み取り用に開くのを待つ。
        std::thread::sleep(Duration::from_millis(100));
        write_to_pipe(&pipe_path);

        let event = recv_watch(&svc).expect("event should arrive");
        assert!(matches!(event, WatchEvent::RefreshRequested));

        drop(listener);
        assert!(!pipe_path.exists(), "pipe should be cleaned up on drop");
    }

    #[test]
    fn 複数回の書き込みでその数だけイベントが出る() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let svc = Services::<()>::new();
        let _listener = RefreshPipe::from_path(pipe_path.clone(), svc.sender()).unwrap();

        std::thread::sleep(Duration::from_millis(100));
        write_to_pipe(&pipe_path);
        recv_watch(&svc).expect("first event should arrive");

        // 書き手が閉じる → EOF → 読み手が開き直す。開き直しを待ってから 2 通目を送る。
        std::thread::sleep(Duration::from_millis(200));
        write_to_pipe(&pipe_path);
        recv_watch(&svc).expect("second event should arrive");
    }

    #[test]
    fn 書き込みが無ければイベントも出ない() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let svc = Services::<()>::new();
        let _listener = RefreshPipe::from_path(pipe_path, svc.sender()).unwrap();

        std::thread::sleep(Duration::from_millis(100));
        assert!(recv_watch(&svc).is_none(), "expected no event");
    }

    /// Ctrl+Q が効かなくなる退行の番人。
    ///
    /// FIFO が外から消えると読み取りスレッドは誰も辿り着けない inode の
    /// open() で寝たままになり、突いても起きない。終了はそれでも進むこと。
    #[test]
    fn パイプが消えたリスナを畳んでもハングしない() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let svc = Services::<()>::new();
        let listener = RefreshPipe::from_path(pipe_path.clone(), svc.sender()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        std::fs::remove_file(&pipe_path).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(listener);
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("shutdown hung on a vanished FIFO");
    }
}
