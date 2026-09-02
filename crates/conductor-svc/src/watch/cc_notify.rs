//! Claude Code の状態通知を受ける Unix ドメインソケットのリスナ。
//!
//! ソケットのパスと電文の書式は [conductor_core::cc_hook] が定める
//! (フック側とここが同じ規約を共有する必要があるため)。ここはバックグラウンド
//! スレッドで接続を受け付け、パースした結果を [WatchEvent] として送るだけ。

use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use conductor_core::cc_hook::Notification;

use super::{CcState, WatchEvent};
use crate::EventSender;

/// 1 接続から読み取るメッセージの上限。行儀の悪いクライアントにメモリを食わせないための歯止め。
const MAX_MESSAGE_BYTES: usize = 8 * 1024;

/// 終了時に accept ループの終了を待つ上限。
const SHUTDOWN_JOIN_TIMEOUT: Duration = Duration::from_millis(500);

/// Claude Code の状態通知を Unix ドメインソケットで待ち受ける。
pub struct CcNotifyListener {
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl CcNotifyListener {
    /// repo_path のリポジトリの cc-notify ソケットに bind したリスナを作る。
    pub fn new<P: Send + 'static>(
        repo_path: &std::path::Path,
        sender: EventSender<P>,
    ) -> anyhow::Result<Self> {
        let socket_path = conductor_core::cc_hook::socket_path(repo_path);
        if let Some(dir) = socket_path.parent() {
            std::fs::create_dir_all(dir)?;
        }

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

        let listener = UnixListener::bind(&socket_path)?;

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = Arc::clone(&shutdown);

        let thread = std::thread::Builder::new()
            .name("cc-notify".into())
            .spawn(move || Self::accept_loop(listener, sender, shutdown_flag))?;

        Ok(Self {
            socket_path,
            shutdown,
            thread: Some(thread),
        })
    }

    fn accept_loop<P: Send + 'static>(
        listener: UnixListener,
        sender: EventSender<P>,
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
                Some(Notification::Active { cwd }) => {
                    sender.send_watch(WatchEvent::CcState {
                        kind: CcState::Active,
                        cwd,
                    });
                }
                Some(Notification::Waiting { cwd }) => {
                    sender.send_watch(WatchEvent::CcState {
                        kind: CcState::Waiting,
                        cwd,
                    });
                }
                Some(Notification::Session {
                    panel_id,
                    session_id,
                }) => {
                    sender.send_watch(WatchEvent::CcSessionRotated {
                        panel_id,
                        session_id,
                    });
                }
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

    fn send(socket_path: &std::path::Path, msg: &str) {
        let mut stream = UnixStream::connect(socket_path).unwrap();
        stream.write_all(msg.as_bytes()).unwrap();
    }

    #[test]
    fn 状態のメッセージが届く() {
        let repo = tempfile::tempdir().unwrap();
        let svc = Services::<()>::new();
        let listener = CcNotifyListener::new(repo.path(), svc.sender()).unwrap();

        send(&listener.socket_path, "waiting /tmp/wt");

        let event = recv_watch(&svc).expect("event should arrive");
        assert_eq!(
            event,
            WatchEvent::CcState {
                kind: CcState::Waiting,
                cwd: PathBuf::from("/tmp/wt")
            }
        );
    }

    #[test]
    fn セッションのローテーションが届く() {
        let repo = tempfile::tempdir().unwrap();
        let svc = Services::<()>::new();
        let listener = CcNotifyListener::new(repo.path(), svc.sender()).unwrap();

        send(&listener.socket_path, "session panel-1 sess-2");

        let event = recv_watch(&svc).expect("event should arrive");
        assert_eq!(
            event,
            WatchEvent::CcSessionRotated {
                panel_id: "panel-1".to_string(),
                session_id: "sess-2".to_string()
            }
        );
    }

    #[test]
    fn dropでソケットファイルが片付く() {
        let repo = tempfile::tempdir().unwrap();
        let svc = Services::<()>::new();
        let listener = CcNotifyListener::new(repo.path(), svc.sender()).unwrap();
        let socket_path = listener.socket_path.clone();
        assert!(socket_path.exists());

        drop(listener);

        assert!(!socket_path.exists());
    }
}
