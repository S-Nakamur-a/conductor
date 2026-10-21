//! PTY のマスター側を握り続けるホスト。
//!
//! マスターが閉じると子は SIGHUP で落ちるので、握る側をウィンドウと別のプロセスにすれば
//! ウィンドウを閉じても作業は続く。ウィンドウは 1 本の接続でつなぎ、挨拶の返事として
//! 各セッションの今の画面を受け取ってから差分を追う。
//!
//! 端末からの問い合わせ (CSI 6 n) とオルタネート画面のナッジもここで捌く。ウィンドウが
//! 居ない間に答えられるのはホストだけ。

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use conductor_core::cc_hook::{self, Notification};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use super::reader::trim_raw_history;
use super::screen::resize_parser;
use super::wire::{self, Frame, SessionInfo, SpawnSpec, ToHost, ToWindow};
use super::{MAX_RAW_HISTORY_BYTES, SessionKind, lock};
use crate::watch::CcNotifyListener;

/// ウィンドウもセッションも無い状態がこれだけ続いたらホストは退く。
const IDLE_EXIT: Duration = Duration::from_secs(3);
const REAP_INTERVAL: Duration = Duration::from_millis(200);

struct HostSession {
    info: Mutex<SessionInfo>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    parser: Mutex<vt100::Parser>,
    raw_history: Option<Mutex<VecDeque<u8>>>,
    exited: AtomicBool,
}

impl HostSession {
    fn id(&self) -> String {
        lock(&self.info).id.clone()
    }

    fn snapshot(&self) -> Vec<u8> {
        let history = self.raw_history.as_ref().map(|h| lock(h));
        snapshot(history.as_deref(), lock(&self.parser).screen())
    }
}

struct Window {
    stream: UnixStream,
    generation: u64,
}

#[derive(Default)]
struct Shared {
    sessions: Mutex<Vec<Arc<HostSession>>>,
    /// ロックの順序は window → 各セッションの parser。pump もこの順で取る。
    window: Mutex<Option<Window>>,
    generation: Mutex<u64>,
    /// 最後に届いた active / waiting を cwd ごとに。つなぎ直したウィンドウへ再生する。
    states: Mutex<HashMap<PathBuf, String>>,
    scrollback: usize,
    version: String,
    /// 退くときに片付けるもの。ウィンドウと同じプロセスで動くときは持たない。
    paths: Option<HostPaths>,
}

#[derive(Clone)]
pub struct Host {
    shared: Arc<Shared>,
}

impl Host {
    fn new(version: &str, scrollback: usize, paths: Option<HostPaths>) -> Self {
        Self {
            shared: Arc::new(Shared {
                scrollback,
                version: version.to_string(),
                paths,
                ..Shared::default()
            }),
        }
    }

    pub(super) fn serve(&self, mut stream: UnixStream) {
        if !matches!(
            wire::read_frame::<ToHost>(&mut stream),
            Ok(Frame::Control(ToHost::Hello { .. }))
        ) {
            return;
        }
        let Some(generation) = stream
            .try_clone()
            .ok()
            .and_then(|s| self.attach(s).ok().flatten())
        else {
            return;
        };
        loop {
            match wire::read_frame::<ToHost>(&mut stream) {
                Ok(Frame::Bytes { id, data }) => self.write(&id, &data),
                Ok(Frame::Control(msg)) => self.handle(msg, generation),
                Err(_) => break,
            }
        }
        self.detach(generation);
    }

    fn handle(&self, msg: ToHost, generation: u64) {
        match msg {
            ToHost::Spawn(spec) => {
                let id = spec.info.id.clone();
                let error = self.spawn(spec).err().map(|e| format!("{e:#}"));
                self.send(generation, &ToWindow::Spawned { id, error });
            }
            ToHost::Resize { id, rows, cols } => {
                if let Some(session) = self.find(&id) {
                    self.resize(&session, rows, cols);
                }
            }
            ToHost::Kill { id } => {
                if let Some(session) = self.find(&id)
                    && !session.exited.load(Ordering::Relaxed)
                {
                    let _ = lock(&session.child).kill();
                }
            }
            ToHost::Remove { id } => self.remove(&id),
            ToHost::Shutdown if lock(&self.shared.sessions).is_empty() => {
                if let Some(paths) = &self.shared.paths {
                    exit_host(paths);
                }
            }
            ToHost::Shutdown | ToHost::Hello { .. } => {}
        }
    }

    /// 別のウィンドウがまだつながっていれば断って None。ホストが話せる相手は 1 窓だけ。
    fn attach(&self, mut stream: UnixStream) -> std::io::Result<Option<u64>> {
        let mut window = lock(&self.shared.window);
        if let Some(current) = window.as_mut() {
            // 閉じたウィンドウの接続は、切断を読む前にここへ来ることがある。書けるかで見分ける。
            let probe = ToWindow::Notify {
                line: String::new(),
            };
            if wire::write_control(&mut current.stream, &probe).is_ok() {
                wire::write_control(&mut stream, &ToWindow::Busy)?;
                return Ok(None);
            }
            self.lose_window(&mut window);
        }
        // エディタは開いたウィンドウの持ち物。前のウィンドウのものを引き継いでも、
        // 新しいウィンドウには閉じた後に読み直す先が無い。
        self.drop_editors();
        let sessions = lock(&self.shared.sessions).clone();
        let welcome = ToWindow::Welcome {
            version: self.shared.version.clone(),
            protocol: wire::PROTOCOL,
            sessions: sessions.iter().map(|s| lock(&s.info).clone()).collect(),
            notifications: self.live_states(&sessions),
        };
        wire::write_control(&mut stream, &welcome)?;
        for session in &sessions {
            wire::write_bytes(&mut stream, &session.id(), &session.snapshot())?;
            if session.exited.load(Ordering::Relaxed) {
                wire::write_control(&mut stream, &ToWindow::Exited { id: session.id() })?;
            }
        }
        let generation = {
            let mut next = lock(&self.shared.generation);
            *next += 1;
            *next
        };
        if let Some(old) = window.replace(Window { stream, generation }) {
            let _ = old.stream.shutdown(std::net::Shutdown::Both);
        }
        Ok(Some(generation))
    }

    /// 生きた Claude が居る cwd の状態だけ。閉じたタブの waiting をホストの寿命の間
    /// 出し続けないため。
    fn live_states(&self, sessions: &[Arc<HostSession>]) -> Vec<String> {
        let live: Vec<PathBuf> = sessions
            .iter()
            .filter(|s| !s.exited.load(Ordering::Relaxed))
            .map(|s| lock(&s.info))
            .filter(|info| info.kind == SessionKind::ClaudeCode)
            .map(|info| info.working_dir.clone())
            .collect();
        lock(&self.shared.states)
            .iter()
            .filter(|(cwd, _)| live.contains(cwd))
            .map(|(_, line)| line.clone())
            .collect()
    }

    fn detach(&self, generation: u64) {
        let mut window = lock(&self.shared.window);
        if window.as_ref().is_some_and(|w| w.generation == generation) {
            *window = None;
            self.drop_editors();
        }
    }

    fn drop_editors(&self) {
        let editors: Vec<String> = lock(&self.shared.sessions)
            .iter()
            .filter(|s| lock(&s.info).kind == SessionKind::Editor)
            .map(|s| s.id())
            .collect();
        for id in editors {
            self.remove(&id);
        }
    }

    fn send(&self, generation: u64, msg: &ToWindow) {
        let mut window = lock(&self.shared.window);
        if let Some(w) = window.as_mut().filter(|w| w.generation == generation)
            && wire::write_control(&mut w.stream, msg).is_err()
        {
            self.lose_window(&mut window);
        }
    }

    fn send_any(&self, msg: &ToWindow) {
        let mut window = lock(&self.shared.window);
        if let Some(w) = window.as_mut()
            && wire::write_control(&mut w.stream, msg).is_err()
        {
            self.lose_window(&mut window);
        }
    }

    /// 書けなかった時点でウィンドウは居ない。後から来る detach は generation が
    /// 合わずに素通りするので、エディタはここで片付ける。
    fn lose_window(&self, window: &mut Option<Window>) {
        *window = None;
        self.drop_editors();
    }

    fn find(&self, id: &str) -> Option<Arc<HostSession>> {
        lock(&self.shared.sessions)
            .iter()
            .find(|s| lock(&s.info).id == id)
            .cloned()
    }

    fn write(&self, id: &str, data: &[u8]) {
        if let Some(session) = self.find(id) {
            let mut writer = lock(&session.writer);
            let _ = writer.write_all(data);
            let _ = writer.flush();
        }
    }

    /// master を握ったまま画面まで直す。nudge が読む大きさと食い違わせないため。
    fn resize(&self, session: &HostSession, rows: u16, cols: u16) {
        let master = lock(&session.master);
        let _ = master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        let mut info = lock(&session.info);
        (info.rows, info.cols) = (rows, cols);
        resize_parser(
            &mut lock(&session.parser),
            session.raw_history.as_ref().map(|h| lock(h)).as_deref(),
            rows,
            cols,
            self.shared.scrollback,
        );
    }

    /// 生きていても殺してから外す。reader が Arc を握っている間はマスターが閉じないので、
    /// drop だけでは子に SIGHUP が届かない。
    fn remove(&self, id: &str) {
        let mut sessions = lock(&self.shared.sessions);
        if let Some(at) = sessions.iter().position(|s| lock(&s.info).id == id) {
            let session = sessions.remove(at);
            // 回収済みの pid は別のプロセスに使い回されているかもしれない。
            if !session.exited.load(Ordering::Relaxed) {
                let _ = lock(&session.child).kill();
            }
        }
    }

    fn spawn(&self, spec: SpawnSpec) -> Result<()> {
        let SpawnSpec { info, argv, env } = spec;
        let pair = NativePtySystem::default()
            .openpty(PtySize {
                rows: info.rows,
                cols: info.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY pair")?;
        let mut cmd = CommandBuilder::from_argv(argv.into_iter().map(Into::into).collect());
        cmd.env_clear();
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.cwd(&info.working_dir);
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("failed to spawn command in PTY")?;
        let reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;

        let session = Arc::new(HostSession {
            parser: Mutex::new(vt100::Parser::new(
                info.rows,
                info.cols,
                self.shared.scrollback,
            )),
            raw_history: (info.kind == SessionKind::Shell).then(|| Mutex::new(VecDeque::new())),
            info: Mutex::new(info),
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            exited: AtomicBool::new(false),
        });
        lock(&self.shared.sessions).push(Arc::clone(&session));
        let host = self.clone();
        thread::Builder::new()
            .name(format!("pty-host-{}", session.id()))
            .spawn(move || host.pump(&session, reader))
            .context("failed to spawn PTY reader thread")?;
        Ok(())
    }

    fn pump(&self, session: &Arc<HostSession>, mut reader: Box<dyn Read + Send>) {
        let id = session.id();
        let mut buf = [0u8; 4096];
        let mut was_alt = false;
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let bytes = &buf[..n];
            let mut window = lock(&self.shared.window);
            let entered_alt = {
                let mut parser = lock(&session.parser);
                let queries = count_cursor_queries(bytes);
                parser.process(bytes);
                if let Some(history) = &session.raw_history {
                    let mut history = lock(history);
                    history.extend(bytes.iter().copied());
                    trim_raw_history(&mut history, MAX_RAW_HISTORY_BYTES);
                }
                if queries > 0 {
                    let (row, col) = parser.screen().cursor_position();
                    let answer = format!("\x1b[{};{}R", row + 1, col + 1).repeat(queries);
                    let mut writer = lock(&session.writer);
                    let _ = writer.write_all(answer.as_bytes());
                    let _ = writer.flush();
                }
                let is_alt = parser.screen().alternate_screen();
                let entered = is_alt && !was_alt;
                was_alt = is_alt;
                entered
            };
            if let Some(w) = window.as_mut()
                && wire::write_bytes(&mut w.stream, &id, bytes).is_err()
            {
                self.lose_window(&mut window);
            }
            drop(window);
            if entered_alt {
                let session = Arc::clone(session);
                thread::spawn(move || nudge(&session));
            }
        }
    }

    /// 終わった子を拾ってウィンドウへ知らせる。別プロセスのホストは、ウィンドウも
    /// セッションも無い時間が続いたところでプロセスごと退く。
    fn reap(&self) {
        let mut idle_since: Option<Instant> = None;
        loop {
            thread::sleep(REAP_INTERVAL);
            let sessions = lock(&self.shared.sessions).clone();
            for session in &sessions {
                if session.exited.load(Ordering::Relaxed) {
                    continue;
                }
                if matches!(lock(&session.child).try_wait(), Ok(Some(_))) {
                    session.exited.store(true, Ordering::Relaxed);
                    self.send_any(&ToWindow::Exited { id: session.id() });
                }
            }
            let Some(paths) = self.shared.paths.as_ref() else {
                continue;
            };
            let idle = sessions.is_empty() && lock(&self.shared.window).is_none();
            match (idle, idle_since) {
                (false, _) => idle_since = None,
                (true, None) => idle_since = Some(Instant::now()),
                (true, Some(since)) if since.elapsed() >= IDLE_EXIT => exit_host(paths),
                (true, Some(_)) => {}
            }
        }
    }

    fn on_notification(&self, notification: Notification) {
        let line = notification.to_line();
        match &notification {
            Notification::Session {
                panel_id,
                session_id,
            } => {
                if let Some(session) = self.find(panel_id) {
                    lock(&session.info).claude_session_id = Some(session_id.clone());
                }
            }
            Notification::Active { cwd } | Notification::Waiting { cwd } => {
                lock(&self.shared.states).insert(cwd.clone(), line.clone());
            }
        }
        self.send_any(&ToWindow::Notify { line });
    }

    fn listen(&self, listener: &UnixListener) {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let host = self.clone();
            thread::spawn(move || host.serve(stream));
        }
    }

    /// ウィンドウと同じプロセスの中で動かす。cc-notify は持たない (先客のウィンドウの宛先を奪うため)。
    pub(super) fn in_process(version: &str, scrollback: usize) -> std::io::Result<UnixStream> {
        let (window, host_end) = UnixStream::pair()?;
        let host = Self::new(version, scrollback, None);
        let reaper = host.clone();
        thread::spawn(move || reaper.reap());
        thread::spawn(move || host.serve(host_end));
        Ok(window)
    }
}

/// つないだばかりのウィンドウに渡す画面。生履歴は削られて先頭のモード切り替え
/// (代替画面、DECCKM、マウス) を失っていることがあるので、今の状態を後ろに足す。
/// 生履歴を先に渡すのは、ウィンドウ側でもスクロールバックとリフローを持たせるため。
fn snapshot(history: Option<&VecDeque<u8>>, screen: &vt100::Screen) -> Vec<u8> {
    let mut bytes: Vec<u8> = history
        .map(|h| h.iter().copied().collect())
        .unwrap_or_default();
    // vt100 の状態の書き出しは代替画面に居ることを含まない。
    if screen.alternate_screen() {
        bytes.extend_from_slice(b"\x1b[?1049h");
    }
    bytes.extend(screen.state_formatted());
    bytes
}

/// ホストが退くときに片付けるもの。プロセスごと抜けるので Drop は走らない。
struct HostPaths {
    socket: PathBuf,
    cc_socket: PathBuf,
    cc_pointer: PathBuf,
}

fn exit_host(paths: &HostPaths) -> ! {
    // 先に消す。退いている最中に来たウィンドウには、繋がらずに新しいホストを
    // 起こしてもらう。
    let _ = std::fs::remove_file(&paths.socket);
    let _ = std::fs::remove_file(&paths.cc_socket);
    let _ = std::fs::remove_file(&paths.cc_pointer);
    std::process::exit(0)
}

/// fzf のようなプログラムはリサイズを受けるまで初期画面を描かず、1 回では準備前に
/// 届いてしまうことがある。macOS は大きさが実際に変わったときしか SIGWINCH を
/// 送らないので、1 行縮めてから戻す。
fn nudge(session: &HostSession) {
    for _ in 0..5 {
        // 大きさは master を握ってから読む。間に Resize が入ると旧サイズへ戻してしまう。
        let master = lock(&session.master);
        let (rows, cols) = lock(&session.parser).screen().size();
        if rows > 1 {
            let _ = master.resize(PtySize {
                rows: rows - 1,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        let _ = master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        drop(master);
        thread::sleep(Duration::from_millis(100));
    }
}

/// 返事が無いと、問い合わせた側 (fzf やシェル) はタイムアウトまで止まる。
fn count_cursor_queries(bytes: &[u8]) -> usize {
    bytes.windows(4).filter(|w| *w == b"\x1b[6n").count()
}

/// `conductor pty-host <repo>`。ウィンドウが起こし、セッションが尽きるまで残る。
pub fn run(repo_root: &Path, version: &str, scrollback: usize) -> Result<()> {
    // 退いたばかりのホストは、ソケットを消してからロックを手放すまでに少し間がある。
    let deadline = Instant::now() + Duration::from_secs(2);
    let _lock = loop {
        match conductor_core::instance_lock::acquire_pty_host(repo_root)? {
            Some(lock) => break lock,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            // 先客が居る。起こした側はそちらへ繋ぐ。
            None => return Ok(()),
        }
    };
    cc_hook::ensure_runtime_dir()?;
    let paths = HostPaths {
        socket: cc_hook::pty_host_socket_path(repo_root),
        cc_socket: cc_hook::socket_path(repo_root),
        cc_pointer: cc_hook::socket_pointer_path(repo_root),
    };
    // ロックを取れた以上、残っているソケットは前のホストの亡骸。
    let _ = std::fs::remove_file(&paths.socket);
    let listener = UnixListener::bind(&paths.socket)
        .with_context(|| format!("could not bind {}", paths.socket.display()))?;

    let host = Host::new(version, scrollback, Some(paths));
    let notified = host.clone();
    let _cc_notify = CcNotifyListener::new(repo_root, move |n| notified.on_notification(n))
        .map_err(|e| log::warn!("cc-notify listener did not start: {e:#}"))
        .ok();
    let reaper = host.clone();
    thread::spawn(move || reaper.reap());

    host.listen(&listener);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::pty::client::Detached;
    use crate::pty::{Launch, PtyStore, Spawn};

    fn serve(dir: &Path) -> (Host, PathBuf) {
        let socket = dir.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let host = Host::new("test", 100, None);
        let (listening, reaping) = (host.clone(), host.clone());
        thread::spawn(move || reaping.reap());
        thread::spawn(move || listening.listen(&listener));
        (host, socket)
    }

    fn window_with(
        socket: &Path,
        on_notify: impl Fn(Notification) + Send + Sync + 'static,
    ) -> PtyStore {
        let mut store = PtyStore::new(100, 100);
        let socket = socket.to_path_buf();
        let detached = Detached {
            version: "test".into(),
            on_notify: Arc::new(on_notify),
            socket_of: Arc::new(move |_| socket.clone()),
            start: Arc::new(|_| anyhow::bail!("the host should already be up")),
        };
        store.attach_with(Path::new("/"), detached).unwrap();
        store
    }

    fn window(socket: &Path) -> PtyStore {
        window_with(socket, |_| {})
    }

    fn launch(store: &mut PtyStore, dir: &Path, launch: Launch<'_>) -> usize {
        store
            .spawn(Spawn {
                launch,
                worktree: "w",
                label: "SH:1",
                working_dir: dir,
                rows: 10,
                cols: 60,
            })
            .unwrap()
    }

    fn screen_text(store: &PtyStore, idx: usize) -> String {
        lock(&store.screen(idx).unwrap()).screen().contents()
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn ウィンドウを閉じても同じシェルが生き続けつなぎ直すと画面が戻る() {
        let dir = tempfile::tempdir().unwrap();
        let (_host, socket) = serve(dir.path());
        let mut first = window(&socket);
        let idx = launch(&mut first, dir.path(), Launch::Shell { program: "/bin/sh" });
        first
            .write_to_session(idx, b"KEPT=yes; echo before-$((1+1))\n")
            .unwrap();
        wait_until("the first window's output", || {
            screen_text(&first, idx).contains("before-2")
        });
        let id = first.sessions()[idx].id.clone();
        drop(first);

        let second = window(&socket);
        let ids: Vec<&str> = second.sessions().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, [id.as_str()]);
        wait_until("the screen handed over", || {
            screen_text(&second, 0).contains("before-2")
        });
        second.write_to_session(0, b"echo still-$KEPT\n").unwrap();
        wait_until("the same shell answering", || {
            screen_text(&second, 0).contains("still-yes")
        });
    }

    #[test]
    fn 別のウィンドウがつながっている間は割り込まずに断る() {
        let dir = tempfile::tempdir().unwrap();
        let (_host, socket) = serve(dir.path());
        let mut first = window(&socket);
        let idx = launch(&mut first, dir.path(), Launch::Shell { program: "/bin/sh" });

        let mut second = PtyStore::new(100, 100);
        let to = socket.clone();
        let detached = Detached {
            version: "test".into(),
            on_notify: Arc::new(|_| {}),
            socket_of: Arc::new(move |_| to.clone()),
            start: Arc::new(|_| anyhow::bail!("the host should already be up")),
        };
        assert!(second.attach_with(Path::new("/"), detached).is_err());

        first
            .write_to_session(idx, b"echo still-$((40+2))\n")
            .unwrap();
        wait_until("the first window keeping its shell", || {
            screen_text(&first, idx).contains("still-42")
        });
        assert!(first.is_session_alive(idx));
    }

    #[test]
    fn エディタはウィンドウと一緒に閉じる() {
        let dir = tempfile::tempdir().unwrap();
        let (_host, socket) = serve(dir.path());
        let mut first = window(&socket);
        let args = ["-c".to_string(), "sleep 30".to_string()];
        let editor = Launch::Editor {
            program: "/bin/sh",
            args: &args,
            file: Path::new("file"),
        };
        launch(&mut first, dir.path(), editor);
        drop(first);

        assert!(window(&socket).sessions().is_empty());
    }

    #[test]
    fn ウィンドウが居ない間の通知は_claudeが居るcwdの分だけ次のウィンドウに届く() {
        let dir = tempfile::tempdir().unwrap();
        let (host, socket) = serve(dir.path());
        let mut first = window(&socket);
        let idx = launch(&mut first, dir.path(), Launch::Shell { program: "/bin/sh" });
        let id = first.sessions()[idx].id.clone();
        drop(first);
        // 本物の claude は起こせないので、居るセッションを Claude ということにする。
        lock(&host.find(&id).unwrap().info).kind = SessionKind::ClaudeCode;

        let closed_tab = PathBuf::from("/closed-tab");
        host.on_notification(Notification::Waiting { cwd: closed_tab });
        host.on_notification(Notification::Waiting {
            cwd: dir.path().to_path_buf(),
        });
        host.on_notification(Notification::Session {
            panel_id: id,
            session_id: "after-clear".to_string(),
        });

        let (tx, rx) = mpsc::channel();
        let second = window_with(&socket, move |n| {
            let _ = tx.send(n);
        });
        let replayed: Vec<Notification> = rx.try_iter().collect();
        assert_eq!(
            replayed,
            [Notification::Waiting {
                cwd: dir.path().to_path_buf()
            }]
        );
        assert_eq!(
            second.claude_session_ref(0).map(|(_, id, _)| id).as_deref(),
            Some("after-clear")
        );
    }

    #[test]
    fn 削られた生履歴からでも写しは代替画面と入力モードを保つ() {
        let mut host_screen = vt100::Parser::new(10, 40, 100);
        host_screen.process(b"\x1b[?1049h\x1b[?1h\x1b[?1000h\x1b[?2004hTOP-LINE");
        let trimmed: VecDeque<u8> = b"later output\r\n".iter().copied().collect();

        let mut mirror = vt100::Parser::new(10, 40, 100);
        mirror.process(&snapshot(Some(&trimmed), host_screen.screen()));

        let screen = mirror.screen();
        assert!(screen.alternate_screen());
        assert!(screen.application_cursor());
        assert!(screen.bracketed_paste());
        assert_ne!(screen.mouse_protocol_mode(), vt100::MouseProtocolMode::None);
        assert!(
            screen.contents().starts_with("TOP-LINE"),
            "{:?}",
            screen.contents()
        );
    }

    #[test]
    fn 別のリポジトリで開いたセッションはそのリポジトリのホストに載る() {
        let (repo_a, repo_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (_a, socket_a) = serve(repo_a.path());
        let (_b, socket_b) = serve(repo_b.path());
        let mut store = PtyStore::new(100, 100);
        let root_b = repo_b.path().to_path_buf();
        let (to_a, to_b) = (socket_a.clone(), socket_b.clone());
        let detached = Detached {
            version: "test".into(),
            on_notify: Arc::new(|_| {}),
            socket_of: Arc::new(move |dir| {
                if dir.starts_with(&root_b) {
                    to_b.clone()
                } else {
                    to_a.clone()
                }
            }),
            start: Arc::new(|_| anyhow::bail!("the host should already be up")),
        };
        store.attach_with(repo_a.path(), detached).unwrap();
        launch(
            &mut store,
            repo_b.path(),
            Launch::Shell { program: "/bin/sh" },
        );
        drop(store);

        assert!(window(&socket_a).sessions().is_empty());
        assert_eq!(window(&socket_b).sessions().len(), 1);
    }

    #[test]
    fn 終わった子は生きていないと答える() {
        let dir = tempfile::tempdir().unwrap();
        let (_host, socket) = serve(dir.path());
        let mut store = window(&socket);
        let idx = launch(
            &mut store,
            dir.path(),
            Launch::Shell {
                program: "/usr/bin/true",
            },
        );
        wait_until("the exit", || !store.is_session_alive(idx));
    }
}
