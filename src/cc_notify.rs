//! Unix domain socket listener for CC state notifications.
//!
//! Hooks send `"active <cwd>\n"` or `"waiting <cwd>\n"` messages via the
//! socket.  A background thread accepts connections and forwards parsed
//! events through an `mpsc` channel to the main loop.

use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

/// The kind of CC state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CcNotifyKind {
    Active,
    Waiting,
}

/// A single notification event received from a hook.
#[derive(Debug)]
pub struct CcNotifyEvent {
    pub kind: CcNotifyKind,
    pub cwd: PathBuf,
}

/// Listens on a Unix domain socket for CC state notifications.
pub struct CcNotifyListener {
    rx: mpsc::Receiver<CcNotifyEvent>,
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CcNotifyListener {
    /// Create a new listener bound to `.conductor/cc-notify.sock` under the
    /// given repository root.
    pub fn new(repo_path: &Path) -> anyhow::Result<Self> {
        let conductor_dir = crate::git_engine::GitEngine::open(repo_path)
            .and_then(|e| e.main_worktree_path())
            .unwrap_or_else(|_| repo_path.to_path_buf())
            .join(".conductor");
        std::fs::create_dir_all(&conductor_dir)?;

        let socket_path = conductor_dir.join("cc-notify.sock");

        // Handle stale socket from a previous crash.
        if socket_path.exists() {
            if UnixStream::connect(&socket_path).is_err() {
                // No listener — stale socket file.
                let _ = std::fs::remove_file(&socket_path);
            } else {
                // Another Conductor instance is already listening.
                anyhow::bail!("socket already in use: {}", socket_path.display());
            }
        }

        let listener = UnixListener::bind(&socket_path)?;
        // The background thread blocks on accept(); non-blocking is not needed.

        let (tx, rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);
        let path_for_thread = socket_path.clone();

        let thread = std::thread::Builder::new()
            .name("cc-notify".into())
            .spawn(move || {
                Self::accept_loop(listener, tx, shutdown_flag, &path_for_thread);
            })?;

        Ok(Self {
            rx,
            socket_path,
            shutdown,
            thread: Some(thread),
        })
    }

    /// Non-blocking poll for the next event.
    pub fn poll(&self) -> Option<CcNotifyEvent> {
        self.rx.try_recv().ok()
    }

    fn accept_loop(
        listener: UnixListener,
        tx: mpsc::Sender<CcNotifyEvent>,
        shutdown: Arc<AtomicBool>,
        _socket_path: &Path,
    ) {
        for stream in listener.incoming() {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            let mut stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("cc-notify accept error: {e}");
                    continue;
                }
            };
            // Short read timeout to avoid blocking on misbehaving clients.
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));

            let mut buf = [0u8; 1024];
            let n = match stream.read(&mut buf) {
                Ok(n) => n,
                Err(_) => continue,
            };
            let msg = String::from_utf8_lossy(&buf[..n]);
            let msg = msg.trim();
            if msg.is_empty() {
                continue;
            }

            if let Some(event) = Self::parse_message(msg) {
                if tx.send(event).is_err() {
                    // Receiver dropped — main loop exited.
                    break;
                }
            } else {
                log::debug!("cc-notify: unrecognized message: {msg:?}");
            }
        }
    }

    fn parse_message(msg: &str) -> Option<CcNotifyEvent> {
        let (kind_str, cwd_str) = msg.split_once(' ')?;
        let kind = match kind_str {
            "active" => CcNotifyKind::Active,
            "waiting" => CcNotifyKind::Waiting,
            _ => return None,
        };
        let cwd = PathBuf::from(cwd_str);
        Some(CcNotifyEvent { kind, cwd })
    }
}

impl Drop for CcNotifyListener {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Connect to the socket to unblock the accept() call.
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}
