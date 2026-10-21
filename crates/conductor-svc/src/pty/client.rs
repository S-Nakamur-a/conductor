//! ウィンドウ側の接続。ホストへ操作を送り、届いたバイト列を写しへ流す。
//!
//! ホストはリポジトリごとに 1 つで、ウィンドウはリポジトリを切り替えても前のセッションを
//! 落とさないので、訪れたリポジトリの数だけ接続を持つ。セッションは自分の作業ディレクトリが
//! 属するリポジトリのホストに置く。

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
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
type StartHost = Arc<dyn Fn(&Path) -> Result<()> + Send + Sync>;

/// 別プロセスのホストへのつなぎ方。持っていなければ、ホストはウィンドウと同じプロセスに置く。
#[derive(Clone)]
pub(super) struct Detached {
    pub version: String,
    pub on_notify: OnNotify,
    pub socket_of: Arc<dyn Fn(&Path) -> PathBuf + Send + Sync>,
    pub start: StartHost,
}

#[derive(Clone, Default)]
struct Routes(Arc<Mutex<HashMap<String, SharedIo>>>);

impl Routes {
    fn insert(&self, id: &str, io: SharedIo) {
        lock(&self.0).insert(id.to_string(), io);
    }

    fn remove(&self, id: &str) {
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
    routes: Routes,
    alive: Arc<AtomicBool>,
}

impl Link {
    fn start(
        stream: UnixStream,
        routes: Routes,
        output_notify: Arc<AtomicBool>,
        on_notify: OnNotify,
    ) -> Result<Self> {
        let replies = Replies::default();
        let alive = Arc::new(AtomicBool::new(true));
        let incoming = stream.try_clone()?;
        let (listen_routes, listen_replies, listen_alive) =
            (routes.clone(), Arc::clone(&replies), Arc::clone(&alive));
        thread::Builder::new()
            .name("pty-link".into())
            .spawn(move || {
                listen(incoming, &listen_routes, &listen_replies, &*on_notify);
                listen_alive.store(false, Ordering::Relaxed);
                output_notify.store(true, Ordering::Relaxed);
            })?;
        Ok(Self {
            stream: Mutex::new(stream),
            replies,
            routes,
            alive,
        })
    }

    pub(super) fn send(&self, msg: &ToHost) -> Result<()> {
        wire::write_control(&mut *lock(&self.stream), msg).context("the PTY host is gone")
    }

    pub(super) fn send_bytes(&self, id: &str, data: &[u8]) -> Result<()> {
        wire::write_bytes(&mut *lock(&self.stream), id, data).context("the PTY host is gone")
    }

    pub(super) fn forget(&self, id: &str) {
        self.routes.remove(id);
    }

    fn spawn(&self, spec: SpawnSpec, io: SharedIo) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let id = spec.info.id.clone();
        self.routes.insert(&id, io);
        lock(&self.replies).insert(id.clone(), tx);
        let answer = self.send(&ToHost::Spawn(spec)).and_then(|()| {
            rx.recv_timeout(SPAWN_TIMEOUT)
                .context("the PTY host did not answer the spawn")
        });
        lock(&self.replies).remove(&id);
        let result = match answer {
            Ok(None) => return Ok(()),
            Ok(Some(error)) => Err(anyhow::anyhow!(error)),
            // 遅れて起きたセッションを、誰も知らないタブとしてホストに残さない。
            Err(e) => {
                let _ = self.send(&ToHost::Remove { id: id.clone() });
                Err(e)
            }
        };
        self.routes.remove(&id);
        result
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
    pub fn attach(
        &mut self,
        repo_root: &Path,
        version: &str,
        on_notify: impl Fn(Notification) + Send + Sync + 'static,
    ) -> Result<Attached> {
        self.attach_with(
            repo_root,
            Detached {
                version: version.to_string(),
                on_notify: Arc::new(on_notify),
                socket_of: Arc::new(cc_hook::pty_host_socket_path),
                start: Arc::new(start_host),
            },
        )
    }

    pub(super) fn attach_with(&mut self, repo_root: &Path, detached: Detached) -> Result<Attached> {
        let attached = self.connect(repo_root, &detached);
        if attached.is_ok() {
            self.detached = Some(detached);
        }
        attached
    }

    fn connect(&mut self, working_dir: &Path, detached: &Detached) -> Result<Attached> {
        let socket = (detached.socket_of)(working_dir);
        let start = || (detached.start)(working_dir);
        // 退く直前のホストに当たると挨拶が返らない。そのときは起こし直してもう 1 回。
        let (mut stream, mut welcome) =
            match handshake(connect_or_start(&socket, &start)?, &detached.version) {
                Ok(greeted) => greeted,
                Err(_) => {
                    wait_until_gone(&socket);
                    handshake(connect_or_start(&socket, &start)?, &detached.version)?
                }
            };
        if welcome.version != detached.version && welcome.sessions.is_empty() {
            let _ = wire::write_control(&mut stream, &ToHost::Shutdown);
            wait_until_gone(&socket);
            (stream, welcome) = handshake(connect_or_start(&socket, &start)?, &detached.version)?;
        }
        anyhow::ensure!(
            welcome.protocol == wire::PROTOCOL,
            "the PTY host (v{}) speaks protocol {}, this window speaks {}",
            welcome.version,
            welcome.protocol,
            wire::PROTOCOL
        );
        // 受信を始める前に流す。後にすると、届いたばかりの通知を古い再生が上書きしうる。
        for line in &welcome.notifications {
            if let Some(notification) = Notification::parse(line) {
                (detached.on_notify)(notification);
            }
        }
        let routes = Routes::default();
        for info in welcome.sessions {
            self.adopt(info, &socket, &routes);
        }
        let link = Link::start(
            stream,
            routes,
            Arc::clone(&self.output_notify),
            Arc::clone(&detached.on_notify),
        )?;
        self.links.insert(socket, link);
        Ok(Attached {
            host_version: welcome.version,
        })
    }

    fn adopt(&mut self, info: SessionInfo, host: &Path, routes: &Routes) {
        let io = SharedIo::new(
            info.kind,
            info.rows,
            info.cols,
            self.inactive_scrollback,
            &self.output_notify,
        );
        routes.insert(&info.id, io.clone());
        self.sessions.push(PtySession {
            id: info.id,
            label: info.label,
            kind: info.kind,
            worktree: info.worktree,
            working_dir: info.working_dir,
            claude_session_id: info.claude_session_id,
            spawned_at: SystemTime::UNIX_EPOCH + Duration::from_millis(info.spawned_at_ms),
            host: host.to_path_buf(),
            io,
        });
    }

    /// working_dir のセッションを置くホストの鍵。同じプロセスのホストは空のパス。
    pub(super) fn host_of(&self, working_dir: &Path) -> PathBuf {
        self.detached
            .as_ref()
            .map_or_else(PathBuf::new, |d| (d.socket_of)(working_dir))
    }

    /// 死んだ接続は捨てて張り直す。ホストが落ちた後もこのウィンドウで新しいセッションを開ける。
    fn link_for(&mut self, working_dir: &Path) -> Result<&Link> {
        let key = self.host_of(working_dir);
        if self
            .links
            .get(&key)
            .is_some_and(|l| !l.alive.load(Ordering::Relaxed))
        {
            self.links.remove(&key);
        }
        if !self.links.contains_key(&key) {
            match self.detached.clone() {
                Some(detached) => {
                    self.connect(working_dir, &detached)?;
                }
                None => {
                    let version = env!("CARGO_PKG_VERSION");
                    let stream = Host::in_process(version, self.inactive_scrollback)?;
                    let (stream, _) = handshake(stream, version)?;
                    let link = Link::start(
                        stream,
                        Routes::default(),
                        Arc::clone(&self.output_notify),
                        Arc::new(|_| {}),
                    )?;
                    self.links.insert(key.clone(), link);
                }
            }
        }
        self.links.get(&key).context("no PTY host for this session")
    }

    pub(super) fn link_of(&self, session: &PtySession) -> Result<&Link> {
        self.links
            .get(&session.host)
            .context("the PTY host of this session is gone")
    }

    pub(super) fn spawn_on_host(&mut self, spec: SpawnSpec) -> Result<(SharedIo, PathBuf)> {
        let io = SharedIo::new(
            spec.info.kind,
            spec.info.rows,
            spec.info.cols,
            self.inactive_scrollback,
            &self.output_notify,
        );
        let working_dir = spec.info.working_dir.clone();
        self.link_for(&working_dir)?.spawn(spec, io.clone())?;
        Ok((io, self.host_of(&working_dir)))
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
