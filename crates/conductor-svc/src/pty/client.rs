//! ウィンドウ側の接続。ホストへ操作を送り、届いたバイト列を写しへ流す。

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use conductor_core::cc_hook::{self, Notification};

use super::host::Host;
use super::wire::{self, Frame, SessionInfo, SpawnSpec, ToHost, ToWindow};
use super::{PtySession, PtyStore, SharedIo, lock, reader};

/// 起こしたホストがソケットを開くまで待つ上限。
const START_TIMEOUT: Duration = Duration::from_secs(5);
const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);

type OnNotify = Arc<dyn Fn(Notification) + Send + Sync>;

#[derive(Clone, Default)]
pub(super) struct Routes(Arc<Mutex<HashMap<String, SharedIo>>>);

impl Routes {
    pub(super) fn insert(&self, id: &str, io: SharedIo) {
        lock(&self.0).insert(id.to_string(), io);
    }

    pub(super) fn remove(&self, id: &str) {
        lock(&self.0).remove(id);
    }

    fn get(&self, id: &str) -> Option<SharedIo> {
        lock(&self.0).get(id).cloned()
    }

    fn mark_all_exited(&self) {
        for io in lock(&self.0).values() {
            io.exited.store(true, Ordering::Relaxed);
        }
    }
}

type Replies = Arc<Mutex<HashMap<String, mpsc::Sender<Option<String>>>>>;

pub(super) struct Link {
    stream: Mutex<UnixStream>,
    replies: Replies,
}

impl Link {
    fn start(
        stream: UnixStream,
        routes: Routes,
        output_notify: Arc<AtomicBool>,
        on_notify: OnNotify,
    ) -> Result<Self> {
        let replies = Replies::default();
        let incoming = stream.try_clone()?;
        let listen_replies = Arc::clone(&replies);
        thread::Builder::new()
            .name("pty-link".into())
            .spawn(move || {
                listen(
                    incoming,
                    &routes,
                    &listen_replies,
                    &output_notify,
                    &*on_notify,
                )
            })?;
        Ok(Self {
            stream: Mutex::new(stream),
            replies,
        })
    }

    pub(super) fn send(&self, msg: &ToHost) -> Result<()> {
        wire::write_control(&mut *lock(&self.stream), msg).context("the PTY host is gone")
    }

    pub(super) fn send_bytes(&self, id: &str, data: &[u8]) -> Result<()> {
        wire::write_bytes(&mut *lock(&self.stream), id, data).context("the PTY host is gone")
    }

    fn spawn(&self, spec: SpawnSpec) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let id = spec.info.id.clone();
        lock(&self.replies).insert(id.clone(), tx);
        let sent = self.send(&ToHost::Spawn(spec));
        let answer = sent.and_then(|()| {
            rx.recv_timeout(SPAWN_TIMEOUT)
                .context("the PTY host did not answer the spawn")
        });
        lock(&self.replies).remove(&id);
        match answer? {
            None => Ok(()),
            Some(error) => bail!(error),
        }
    }
}

impl Drop for Link {
    /// 受信スレッドが複製を持っているので、閉じるだけではホストに切断が見えない。
    fn drop(&mut self) {
        let _ = lock(&self.stream).shutdown(std::net::Shutdown::Both);
    }
}

fn listen(
    mut stream: UnixStream,
    routes: &Routes,
    replies: &Replies,
    output_notify: &AtomicBool,
    on_notify: &(dyn Fn(Notification) + Send + Sync),
) {
    loop {
        match wire::read_frame::<ToWindow>(&mut stream) {
            Ok(Frame::Bytes { id, data }) => {
                if let Some(io) = routes.get(&id) {
                    reader::feed(&io, &data);
                }
            }
            Ok(Frame::Control(ToWindow::Spawned { id, error })) => {
                if let Some(tx) = lock(replies).remove(&id) {
                    let _ = tx.send(error);
                }
            }
            Ok(Frame::Control(ToWindow::Exited { id })) => {
                if let Some(io) = routes.get(&id) {
                    reader::finish(&io);
                }
            }
            Ok(Frame::Control(ToWindow::Notify { line })) => {
                if let Some(notification) = Notification::parse(&line) {
                    on_notify(notification);
                }
            }
            Ok(Frame::Control(ToWindow::Welcome { .. })) => {}
            Err(e) => {
                log::warn!("lost the PTY host: {e}");
                routes.mark_all_exited();
                output_notify.store(true, Ordering::Relaxed);
                return;
            }
        }
    }
}

/// [PtyStore::attach] の結果。
pub struct Attached {
    /// つないだホストの版。ウィンドウより古いままのことがある。
    pub host_version: String,
}

struct Welcome {
    version: String,
    protocol: u32,
    sessions: Vec<SessionInfo>,
    notifications: Vec<String>,
}

fn handshake(mut stream: UnixStream, version: &str) -> Result<(UnixStream, Welcome)> {
    stream.set_read_timeout(Some(START_TIMEOUT))?;
    wire::write_control(
        &mut stream,
        &ToHost::Hello {
            version: version.to_string(),
            protocol: wire::PROTOCOL,
        },
    )?;
    let welcome = match wire::read_frame::<ToWindow>(&mut stream)
        .context("the PTY host did not greet back")?
    {
        Frame::Control(ToWindow::Welcome {
            version,
            protocol,
            sessions,
            notifications,
        }) => Welcome {
            version,
            protocol,
            sessions,
            notifications,
        },
        _ => bail!("the PTY host answered out of turn"),
    };
    stream.set_read_timeout(None)?;
    Ok((stream, welcome))
}

impl PtyStore {
    /// このリポジトリのホストにつなぐ。居なければ起こす。
    ///
    /// ホストの版が違っても、セッションを持っているなら殺さずにそのまま使う。
    /// 持っていなければ退かせて今の版で起こし直す。
    pub fn attach(
        &mut self,
        repo_root: &Path,
        version: &str,
        on_notify: impl Fn(Notification) + Send + Sync + 'static,
    ) -> Result<Attached> {
        let socket = cc_hook::pty_host_socket_path(repo_root);
        self.attach_to(&socket, version, Arc::new(on_notify), || {
            start_host(repo_root)
        })
    }

    pub(super) fn attach_to(
        &mut self,
        socket: &Path,
        version: &str,
        on_notify: OnNotify,
        start: impl Fn() -> Result<()>,
    ) -> Result<Attached> {
        anyhow::ensure!(self.link.is_none(), "already linked to a PTY host");
        let (mut stream, mut welcome) = handshake(connect_or_start(socket, &start)?, version)?;
        if welcome.version != version && welcome.sessions.is_empty() {
            let _ = wire::write_control(&mut stream, &ToHost::Shutdown);
            wait_until_gone(socket);
            (stream, welcome) = handshake(connect_or_start(socket, &start)?, version)?;
        }
        anyhow::ensure!(
            welcome.protocol == wire::PROTOCOL,
            "the PTY host (v{}) speaks protocol {}, this window speaks {}",
            welcome.version,
            welcome.protocol,
            wire::PROTOCOL
        );
        for info in welcome.sessions {
            self.adopt(info);
        }
        self.link = Some(Link::start(
            stream,
            self.routes.clone(),
            Arc::clone(&self.output_notify),
            Arc::clone(&on_notify),
        )?);
        for line in &welcome.notifications {
            if let Some(notification) = Notification::parse(line) {
                on_notify(notification);
            }
        }
        Ok(Attached {
            host_version: welcome.version,
        })
    }

    /// ホストに居るセッションの写しを作る。画面は挨拶の直後に届く。
    fn adopt(&mut self, info: SessionInfo) {
        let io = SharedIo::new(
            info.kind,
            info.rows,
            info.cols,
            self.inactive_scrollback,
            &self.output_notify,
        );
        self.routes.insert(&info.id, io.clone());
        self.sessions.push(PtySession {
            id: info.id,
            label: info.label,
            kind: info.kind,
            worktree: info.worktree,
            working_dir: info.working_dir,
            claude_session_id: info.claude_session_id,
            spawned_at: SystemTime::UNIX_EPOCH + Duration::from_millis(info.spawned_at_ms),
            io,
        });
    }

    /// ホストへの接続。[PtyStore::attach] していなければ、このプロセスの中にホストを置く。
    pub(super) fn link(&mut self) -> Result<&Link> {
        if self.link.is_none() {
            let stream = Host::in_process(env!("CARGO_PKG_VERSION"), self.inactive_scrollback)?;
            let (stream, _) = handshake(stream, env!("CARGO_PKG_VERSION"))?;
            self.link = Some(Link::start(
                stream,
                self.routes.clone(),
                Arc::clone(&self.output_notify),
                Arc::new(|_| {}),
            )?);
        }
        Ok(self.link.as_ref().expect("linked above"))
    }

    pub(super) fn linked(&self) -> Result<&Link> {
        self.link.as_ref().context("no PTY host is linked yet")
    }

    pub(super) fn spawn_on_host(&mut self, spec: SpawnSpec) -> Result<SharedIo> {
        let io = SharedIo::new(
            spec.info.kind,
            spec.info.rows,
            spec.info.cols,
            self.inactive_scrollback,
            &self.output_notify,
        );
        let id = spec.info.id.clone();
        self.routes.insert(&id, io.clone());
        let spawned = self.link().and_then(|link| link.spawn(spec));
        if spawned.is_err() {
            self.routes.remove(&id);
        }
        spawned.map(|()| io)
    }
}

fn connect_or_start(socket: &Path, start: &impl Fn() -> Result<()>) -> Result<UnixStream> {
    if let Ok(stream) = UnixStream::connect(socket) {
        return Ok(stream);
    }
    start()?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        match UnixStream::connect(socket) {
            Ok(stream) => return Ok(stream),
            Err(e) if Instant::now() > deadline => {
                return Err(e).with_context(|| format!("could not reach {}", socket.display()));
            }
            Err(_) => thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn wait_until_gone(socket: &Path) {
    let deadline = Instant::now() + START_TIMEOUT;
    while socket.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
}

/// 自分自身を `pty-host` として起こす。setsid で端末から切り離すので、ウィンドウを
/// 閉じても端末を閉じても SIGHUP は届かない。
fn start_host(repo_root: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let log_path = conductor_core::git_engine::conductor_dir(repo_root).join("pty-host.log");
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("could not open {}", log_path.display()))?;
    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.arg("pty-host")
        .arg(repo_root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log);
    // SAFETY: fork と exec の間で呼ぶのは async-signal-safe な setsid だけ。
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().context("could not start the PTY host")?;
    // 先に退いたときにゾンビとして残さない。
    thread::spawn(move || child.wait());
    Ok(())
}
