//! Claude Code の状態通知を受ける Unix ドメインソケットのリスナ。
//!
//! フック側がソケット経由で次のいずれかを送る:
//!
//!   active <cwd>
//!   waiting <cwd>
//!   session <panel id> <claude session id>
//!
//! 前の 2 つは waiting/active 表示用。3 つ目は SessionStart フック
//! ([crate::cc_hook]) が送る「このパネルが書き込んでいる Claude セッションが
//! 変わった」通知で、/clear や /resume によるログのローテーションを
//! 推測ではなく事実として受け取るためのもの。
//!
//! バックグラウンドスレッドが接続を受け付け、パースしたイベントを mpsc
//! チャネルでメインループへ転送する。

use std::io::Read;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

/// 1 接続から読み取るメッセージの上限。行儀の悪いクライアントに
/// メモリを食わせないための歯止め。
const MAX_MESSAGE_BYTES: usize = 8 * 1024;

/// Claude Code の状態変化の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CcNotifyKind {
    Active,
    Waiting,
}

/// フックから受け取った通知イベント 1 件。
#[derive(Debug)]
pub enum CcNotifyEvent {
    /// ワークツリーの waiting/active 状態が変わった。
    State { kind: CcNotifyKind, cwd: PathBuf },
    /// このパネルが書き込んでいる Claude セッションの id が変わった。
    ///
    /// panel_id は spawn 時に CONDUCTOR_PANEL_ID として PTY に注入した
    /// [crate::pty_manager::PtySession::id]。フックはそのパネル自身の
    /// Claude プロセスの中で走るので、パネルと session id の対応は推測ではなく
    /// 同一性として得られる — 同一ワークツリーで複数パネルが同時に /clear
    /// しても取り違えない。
    SessionRotated {
        panel_id: String,
        session_id: String,
    },
}

/// Claude Code の状態通知を Unix ドメインソケットで待ち受ける。
pub struct CcNotifyListener {
    rx: mpsc::Receiver<CcNotifyEvent>,
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// このリポジトリの cc-notify ソケットのパス。
///
/// bind 側 ([CcNotifyListener::new]) と、フックへ渡す側
/// ([crate::pty_manager]) の両方がここを使う。片方だけ変わって行き違うことが
/// 無いように定義は 1 箇所に置く。ワークツリーではなくメインワークツリーの
/// .conductor/ に置くのは、1 リポジトリにつきリスナが 1 つだからで、
/// 同じ理由でフック側も同じ解決をしなければならない。
pub fn socket_path(repo_path: &Path) -> PathBuf {
    crate::git_engine::GitEngine::open(repo_path)
        .and_then(|e| e.main_worktree_path())
        .unwrap_or_else(|_| repo_path.to_path_buf())
        .join(".conductor")
        .join("cc-notify.sock")
}

impl CcNotifyListener {
    /// 指定したリポジトリルート配下の .conductor/cc-notify.sock に bind した
    /// リスナを作る。
    pub fn new(repo_path: &Path) -> anyhow::Result<Self> {
        let socket_path = socket_path(repo_path);
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
        // バックグラウンドスレッドは accept() でブロックするので、ノンブロッキングは不要。

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

    /// 次のイベントをノンブロッキングで取り出す。
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
                    // 読み取りタイムアウト等。ここまでに届いた分で解釈を試みる。
                    Err(_) => break,
                }
            }
            let msg = String::from_utf8_lossy(&buf);
            let msg = msg.trim();
            if msg.is_empty() {
                continue;
            }

            if let Some(event) = Self::parse_message(msg) {
                if tx.send(event).is_err() {
                    // 受信側が落ちた = メインループが終了した。
                    break;
                }
            } else {
                log::debug!("cc-notify: unrecognized message: {msg:?}");
            }
        }
    }

    fn parse_message(msg: &str) -> Option<CcNotifyEvent> {
        let (verb, rest) = msg.split_once(' ')?;
        match verb {
            "active" => Some(CcNotifyEvent::State {
                kind: CcNotifyKind::Active,
                cwd: PathBuf::from(rest),
            }),
            "waiting" => Some(CcNotifyEvent::State {
                kind: CcNotifyKind::Waiting,
                cwd: PathBuf::from(rest),
            }),
            // どちらの id も UUID なので空白区切りで曖昧さは無い。
            "session" => {
                let (panel_id, session_id) = rest.split_once(' ')?;
                let (panel_id, session_id) = (panel_id.trim(), session_id.trim());
                if panel_id.is_empty() || session_id.is_empty() {
                    return None;
                }
                Some(CcNotifyEvent::SessionRotated {
                    panel_id: panel_id.to_string(),
                    session_id: session_id.to_string(),
                })
            }
            _ => None,
        }
    }
}

impl Drop for CcNotifyListener {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // accept() のブロックを解くためにソケットへ接続する。
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state_messages() {
        let CcNotifyEvent::State { kind, cwd } =
            CcNotifyListener::parse_message("waiting /tmp/wt").expect("parsed")
        else {
            panic!("expected a state event");
        };
        assert_eq!(kind, CcNotifyKind::Waiting);
        assert_eq!(cwd, PathBuf::from("/tmp/wt"));
    }

    #[test]
    fn parses_session_rotation() {
        let CcNotifyEvent::SessionRotated {
            panel_id,
            session_id,
        } = CcNotifyListener::parse_message("session panel-1 sess-2").expect("parsed")
        else {
            panic!("expected a rotation event");
        };
        assert_eq!(panel_id, "panel-1");
        assert_eq!(session_id, "sess-2");
    }

    #[test]
    fn rejects_malformed_messages() {
        // 動詞だけ / id 片方だけ / 未知の動詞は捨てる。
        assert!(CcNotifyListener::parse_message("session").is_none());
        assert!(CcNotifyListener::parse_message("session only-one-id").is_none());
        assert!(CcNotifyListener::parse_message("session  sess").is_none());
        assert!(CcNotifyListener::parse_message("bogus /tmp/wt").is_none());
    }

    #[test]
    fn cwd_with_spaces_survives() {
        // パスは行末まで丸ごと。空白を含むワークツリー名でも壊れない。
        let CcNotifyEvent::State { cwd, .. } =
            CcNotifyListener::parse_message("active /tmp/my worktree").expect("parsed")
        else {
            panic!("expected a state event");
        };
        assert_eq!(cwd, PathBuf::from("/tmp/my worktree"));
    }
}
