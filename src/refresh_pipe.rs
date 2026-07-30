//! Named pipe (FIFO) listener for MCP-triggered UI refresh.
//!
//! The MCP server writes to `.conductor/refresh.pipe` after modifying review
//! data (reply, resolve, etc.).  A background thread reads from the pipe and
//! forwards events through an `mpsc` channel to the main loop, which then
//! calls `refresh_reviews()`.

use std::io::{Read, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

/// Event sent when MCP writes to the refresh pipe.
#[derive(Debug)]
pub struct RefreshEvent;

/// Listens on a named pipe for UI refresh signals from the MCP server.
pub struct RefreshPipe {
    rx: mpsc::Receiver<RefreshEvent>,
    pipe_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RefreshPipe {
    /// Create a new listener bound to `.conductor/refresh.pipe` under the
    /// given repository root.
    pub fn new(repo_path: &Path) -> anyhow::Result<Self> {
        let conductor_dir = crate::git_engine::GitEngine::open(repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| repo_path.to_path_buf())
            .join(".conductor");
        std::fs::create_dir_all(&conductor_dir)?;

        let pipe_path = conductor_dir.join("refresh.pipe");

        // Remove stale pipe from a previous run and recreate.
        if pipe_path.exists() {
            let _ = std::fs::remove_file(&pipe_path);
        }

        // Create the FIFO.
        let path_cstr = std::ffi::CString::new(
            pipe_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("non-UTF-8 path"))?,
        )?;
        // SAFETY: mkfifo is a standard POSIX call; path_cstr is valid and
        // null-terminated.  Mode 0o660 gives owner+group read/write.
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

    /// Non-blocking poll for the next event.
    pub fn poll(&self) -> Option<RefreshEvent> {
        self.rx.try_recv().ok()
    }

    fn read_loop(pipe_path: PathBuf, tx: mpsc::Sender<RefreshEvent>, shutdown: Arc<AtomicBool>) {
        // We loop re-opening the pipe because a FIFO returns EOF when all
        // writers close.  After each EOF we re-open to wait for the next
        // writer.
        while !shutdown.load(Ordering::Relaxed) {
            // Open the FIFO for reading (blocking until a writer connects).
            // We use raw libc::open because Rust's File::open does not
            // support O_NONBLOCK at open time on FIFOs.
            let path_cstr = match std::ffi::CString::new(pipe_path.to_string_lossy().as_ref()) {
                Ok(c) => c,
                Err(_) => break,
            };

            // SAFETY: standard POSIX open; path_cstr is valid and null-terminated.
            let fd: RawFd = unsafe { libc::open(path_cstr.as_ptr(), libc::O_RDONLY) };
            if fd < 0 {
                // Pipe was removed (shutdown or cleanup).
                break;
            }

            // Wrap in a File for convenient reading. SAFETY: fd is a valid
            // open file descriptor that we own exclusively.
            let mut file = unsafe { std::fs::File::from_raw_fd(fd) };

            let mut buf = [0u8; 64];
            loop {
                if shutdown.load(Ordering::Relaxed) {
                    return;
                }
                match file.read(&mut buf) {
                    Ok(0) => {
                        // EOF — all writers closed. Re-open.
                        break;
                    }
                    Ok(_) => {
                        if tx.send(RefreshEvent).is_err() {
                            // Receiver dropped — main loop exited.
                            return;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                        continue;
                    }
                    Err(_) => break,
                }
            }

            // Small sleep before re-opening to avoid busy-loop on rapid
            // EOF cycles.
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Create a listener from an explicit pipe path (for testing).
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

/// Poke the TUI's refresh FIFO so it reloads review data.
///
/// Called by `mcp-serve` after every write. Best-effort on purpose: the common
/// "failure" is that no conductor is running, so the FIFO either does not exist
/// or has no reader, and `O_NONBLOCK` turns the latter into `ENXIO`. Neither is
/// worth surfacing — the write already succeeded, and the next time the TUI
/// opens it reads the database fresh anyway.
///
/// `O_NONBLOCK` is what keeps this from being a hang: opening a FIFO for writing
/// blocks until a reader attaches, which would wedge the tool call.
pub fn signal_refresh(pipe_path: &Path) {
    let Some(path) = pipe_path.to_str() else {
        log::warn!("refresh pipe path is not UTF-8: {}", pipe_path.display());
        return;
    };
    let Ok(path_cstr) = std::ffi::CString::new(path) else {
        return;
    };

    // SAFETY: standard POSIX open; path_cstr is valid and null-terminated.
    let fd = unsafe { libc::open(path_cstr.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK) };
    if fd < 0 {
        log::debug!(
            "refresh pipe not writable ({}): {}",
            pipe_path.display(),
            std::io::Error::last_os_error()
        );
        return;
    }

    // SAFETY: fd is a valid descriptor we own exclusively; closed on drop.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };

    // Only ever write into an actual FIFO. If `refresh.pipe` came back as a
    // regular file or a symlink to one — a backup restored without special
    // files, an archive extracted by a tool that doesn't carry them — this
    // write would land at offset 0 and overwrite the first byte of whatever
    // that file really is.
    // SAFETY: `fd` is open and owned by `file`; `stat` is written only on success.
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

        // Unblock the reader by opening the pipe for writing briefly.
        if self.pipe_path.exists() {
            let path_cstr = std::ffi::CString::new(self.pipe_path.to_string_lossy().as_ref());
            if let Ok(cstr) = path_cstr {
                // SAFETY: standard POSIX open with O_WRONLY | O_NONBLOCK.
                // O_NONBLOCK prevents blocking if no reader exists.
                unsafe {
                    let fd = libc::open(cstr.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK);
                    if fd >= 0 {
                        libc::close(fd);
                    }
                }
            }
        }

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.pipe_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write to the FIFO the way the mcp-serve tool handlers do — through the
    /// real `signal_refresh`, not a hand-rolled copy of it. A copy here would
    /// mean these tests stay green even if `signal_refresh` itself broke,
    /// which is exactly the function every write-capable mcp-serve tool calls
    /// in production.
    fn write_to_pipe(pipe_path: &Path) {
        super::signal_refresh(pipe_path);
    }

    #[test]
    fn single_write_produces_event() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let listener = RefreshPipe::from_path(pipe_path.clone()).unwrap();

        // Give the background thread time to open the pipe for reading.
        std::thread::sleep(Duration::from_millis(100));

        write_to_pipe(&pipe_path);
        std::thread::sleep(Duration::from_millis(200));

        assert!(listener.poll().is_some(), "expected a RefreshEvent");

        drop(listener);
        assert!(!pipe_path.exists(), "pipe should be cleaned up on drop");
    }

    #[test]
    fn multiple_writes_produce_events() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let listener = RefreshPipe::from_path(pipe_path.clone()).unwrap();

        std::thread::sleep(Duration::from_millis(100));

        write_to_pipe(&pipe_path);
        // Writer closes → EOF → reader re-opens. Wait for re-open.
        std::thread::sleep(Duration::from_millis(200));

        write_to_pipe(&pipe_path);
        std::thread::sleep(Duration::from_millis(200));

        // Should have received at least 2 events.
        let mut count = 0;
        while listener.poll().is_some() {
            count += 1;
        }
        assert!(count >= 2, "expected at least 2 events, got {count}");
    }

    #[test]
    fn no_event_without_write() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let listener = RefreshPipe::from_path(pipe_path).unwrap();

        std::thread::sleep(Duration::from_millis(100));
        assert!(listener.poll().is_none(), "expected no event");
    }

    /// `signal_refresh` is the "no conductor is running" path when the
    /// database's `.conductor/` directory doesn't even have a `refresh.pipe`
    /// yet — `libc::open` fails with `ENOENT`, and this must not panic.
    #[test]
    fn signal_refresh_on_nonexistent_path_returns_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("does-not-exist.pipe");
        signal_refresh(&pipe_path); // must not panic
    }

    /// A `refresh.pipe` can exist (created by a past run) with nothing
    /// currently reading it — `mcp-serve` writes without ever having started
    /// a `RefreshPipe` listener itself. `O_NONBLOCK` is what keeps this from
    /// hanging: opening a FIFO for writing normally blocks until a reader
    /// attaches, which would wedge the tool call forever. Run on a background
    /// thread with a timeout so a regression here fails this test instead of
    /// hanging CI.
    #[test]
    fn signal_refresh_with_no_reader_returns_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let pipe_path = dir.path().join("refresh.pipe");
        let path_cstr = std::ffi::CString::new(pipe_path.to_str().unwrap()).unwrap();
        // SAFETY: standard POSIX mkfifo; path_cstr is valid and null-terminated.
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

    /// If `refresh.pipe` is somehow a regular file — an archive extracted
    /// without special files, a restored backup — writing to it would land at
    /// offset 0 and clobber the first byte of whatever it actually is. The
    /// FIFO check has to come before the write.
    #[test]
    fn signal_refresh_will_not_write_into_a_regular_file() {
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
}
