//! Claude Code の状態通知を受ける Unix ドメインソケットのリスナ。
//!
//! ソケットのパスと電文の書式は [conductor_core::cc_hook] が定める
//! (フック側とここが同じ規約を共有する必要があるため)。ここはバックグラウンド
//! スレッドで接続を受け付け、パースした結果を渡された関数へ渡すだけ。
//! 持ち主は PTY ホストで、ウィンドウが閉じている間に届いた通知も取りこぼさない。

use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use conductor_core::cc_hook::Notification;

/// 1 接続から読み取るメッセージの上限。行儀の悪いクライアントにメモリを食わせないための歯止め。
const MAX_MESSAGE_BYTES: usize = 8 * 1024;

/// 終了時に accept ループの終了を待つ上限。
const SHUTDOWN_JOIN_TIMEOUT: Duration = Duration::from_millis(500);

/// Claude Code の状態通知を Unix ドメインソケットで待ち受ける。
pub struct CcNotifyListener {
    socket_path: PathBuf,
    pointer_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CcNotifyListener {
    /// repo_path のリポジトリの cc-notify ソケットに bind したリスナを作る。
    pub fn new(
        repo_path: &std::path::Path,
        on_notification: impl Fn(Notification) + Send + 'static,
    ) -> anyhow::Result<Self> {
        conductor_core::cc_hook::ensure_runtime_dir()?;
        let socket_path = conductor_core::cc_hook::socket_path(repo_path);

        // 前回のクラッシュで残ったソケットを処理する。
        if socket_path.exists() {
            if UnixStream::connect(&socket_path).is_err() {
                // 待ち受けている相手がいない = 取り残されたソケットファイル。
                let _ = std::fs::remove_file(&socket_path);
            } else {
                // 別の Conductor インスタンスが既に待ち受けている。
                anyhow::bail!("socket already in use: {}", socket_path.display());
            }
        }

        // bind より先に書く。後回しだと、書けなかったときに誰も片付けないソケットが残る。
        let pointer_path = conductor_core::cc_hook::socket_pointer_path(repo_path);
        if let Some(dir) = pointer_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&pointer_path, format!("{}\n", socket_path.display()))?;

        let listener = UnixListener::bind(&socket_path)?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);

        let thread = std::thread::Builder::new()
            .name("cc-notify".into())
            .spawn(move || Self::accept_loop(listener, on_notification, shutdown_flag))?;

        Ok(Self {
            socket_path,
            pointer_path,
            shutdown,
            thread: Some(thread),
        })
    }

    fn accept_loop(
        listener: UnixListener,
        on_notification: impl Fn(Notification),
        shutdown: Arc<AtomicBool>,
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
            // 行儀の悪いクライアントでブロックしないよう、読み取りタイムアウトは短く。
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));

            // 相手が閉じるまで読む。1 回の read で足りるとは限らない —
            // 送り手が複数回 write すると (フォーマット済み文字列を書く場合に
            // 起きる) 先頭だけ拾って残りを捨ててしまう。
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while buf.len() < MAX_MESSAGE_BYTES {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
            let msg = String::from_utf8_lossy(&buf);
            let msg = msg.trim();
            if msg.is_empty() {
                continue;
            }

            match Notification::parse(msg) {
                Some(notification) => on_notification(notification),
                None => log::debug!("cc-notify: unrecognized message: {msg:?}"),
            }
        }
    }
}

impl Drop for CcNotifyListener {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // accept() のブロックを解くためにソケットへ接続する。
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(thread) = self.thread.take() {
            // accept() は突いても起きないことがある (ソケットが外から消された等)。
            // 後始末より、端末を戻して抜けられることを優先する。
            crate::join_or_abandon(thread, SHUTDOWN_JOIN_TIMEOUT);
        }
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_file(&self.pointer_path);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::mpsc;

    use super::*;

    fn listen(repo: &std::path::Path) -> (CcNotifyListener, mpsc::Receiver<Notification>) {
        let (tx, rx) = mpsc::channel();
        let listener = CcNotifyListener::new(repo, move |n| {
            let _ = tx.send(n);
        })
        .unwrap();
        (listener, rx)
    }

    fn send(socket_path: &std::path::Path, msg: &str) {
        let mut stream = UnixStream::connect(socket_path).unwrap();
        stream.write_all(msg.as_bytes()).unwrap();
    }

    #[test]
    fn 状態のメッセージが届く() {
        let repo = tempfile::tempdir().unwrap();
        let (listener, rx) = listen(repo.path());

        send(&listener.socket_path, "waiting /tmp/wt");

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Notification::Waiting {
                cwd: PathBuf::from("/tmp/wt")
            }
        );
    }

    #[test]
    fn セッションのローテーションが届く() {
        let repo = tempfile::tempdir().unwrap();
        let (listener, rx) = listen(repo.path());

        send(&listener.socket_path, "session panel-1 sess-2");

        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Notification::Session {
                panel_id: "panel-1".to_string(),
                session_id: "sess-2".to_string()
            }
        );
    }

    #[test]
    fn dropでソケットファイルが片付く() {
        let repo = tempfile::tempdir().unwrap();
        let (listener, _rx) = listen(repo.path());
        let socket_path = listener.socket_path.clone();
        assert!(socket_path.exists());

        drop(listener);

        assert!(!socket_path.exists());
    }
}
