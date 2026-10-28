//! 動いている Claude Code の写し (<設定ディレクトリ>/sessions/<pid>.json) を読む。
//!
//! 写しは Claude 自身が書く。ファイルが残っていることは生きている証拠にならないので、
//! pid のプロセスを引いて、ゾンビでなく起動時刻も合うものだけを残す。止めるときも
//! 同じ照合を通してから信号を送る。

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

/// pid の使い回しを見分ける時刻の許容差。procStart は秒単位。
const START_TOLERANCE: Duration = Duration::from_secs(2);
const STOP_POLL: Duration = Duration::from_millis(50);
const KILL_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub pid: u32,
    /// 止める直前に、一覧に載せたときと同じプロセスかを見分ける。
    pub started: SystemTime,
    pub cwd: PathBuf,
    pub session_id: String,
    pub status: Status,
    pub status_since: Option<SystemTime>,
    pub owner: Owner,
    /// 写しを読んだ sessions dir。止めるときはこの dir の写しで照合する。
    pub sessions_dir: PathBuf,
}

/// busy / idle / waiting 以外 (観測では shell) は意味が未確認なので生の文字列で残す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Working,
    Idle,
    Waiting(Waiting),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Waiting {
    Question,
    Permission,
    Other(String),
}

/// 親プロセスが conductor なら Conductor。それ以外は、conductor のシェルのタブで打った claude も Elsewhere。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Conductor,
    Elsewhere,
}

#[derive(Debug, Clone, Copy)]
pub struct ProcInfo {
    pub zombie: bool,
    pub parent: u32,
    pub started: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}

pub trait Processes {
    /// pid のプロセス。居なければ None。
    fn info(&self, pid: u32) -> Option<ProcInfo>;
    /// その pid の実行ファイル名が `conductor` か。
    fn is_conductor(&self, pid: u32) -> bool;
    fn signal(&self, pid: u32, signal: Signal) -> io::Result<()>;
}

pub struct SystemProcesses;

#[cfg(target_os = "macos")]
impl Processes for SystemProcesses {
    fn info(&self, pid: u32) -> Option<ProcInfo> {
        // SAFETY: proc_bsdinfo は全ビット 0 が有効な POD。proc_pidinfo は buffersize までしか書かない
        let mut raw: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let got = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut raw as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if got != size {
            return None;
        }
        Some(ProcInfo {
            zombie: raw.pbi_status == libc::SZOMB,
            parent: raw.pbi_ppid,
            started: UNIX_EPOCH
                + Duration::new(raw.pbi_start_tvsec, (raw.pbi_start_tvusec * 1000) as u32),
        })
    }

    fn is_conductor(&self, pid: u32) -> bool {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: buf は渡した長さぶん確保済み
        let len = unsafe {
            libc::proc_pidpath(
                pid as libc::c_int,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
            )
        };
        if len <= 0 {
            return false;
        }
        buf.truncate(len as usize);
        is_conductor_path(Path::new(&String::from_utf8_lossy(&buf).into_owned()))
    }

    fn signal(&self, pid: u32, signal: Signal) -> io::Result<()> {
        send_signal(pid, signal)
    }
}

#[cfg(target_os = "linux")]
impl Processes for SystemProcesses {
    fn info(&self, pid: u32) -> Option<ProcInfo> {
        let stat = parse_proc_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
        let boot = parse_btime(&std::fs::read_to_string("/proc/stat").ok()?)?;
        // SAFETY: 引数なしの問い合わせ
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if ticks <= 0 {
            return None;
        }
        Some(ProcInfo {
            zombie: stat.state == 'Z',
            parent: stat.parent,
            started: UNIX_EPOCH
                + Duration::from_secs(boot)
                + Duration::from_secs_f64(stat.start_ticks as f64 / ticks as f64),
        })
    }

    fn is_conductor(&self, pid: u32) -> bool {
        std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|exe| is_conductor_path(&exe))
    }

    fn signal(&self, pid: u32, signal: Signal) -> io::Result<()> {
        send_signal(pid, signal)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl Processes for SystemProcesses {
    fn info(&self, _pid: u32) -> Option<ProcInfo> {
        None
    }

    fn is_conductor(&self, _pid: u32) -> bool {
        false
    }

    fn signal(&self, _pid: u32, _signal: Signal) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn is_conductor_path(exe: &Path) -> bool {
    exe.file_name().is_some_and(|name| name == "conductor")
}

/// kill(2) は 0 と負の pid をプロセスグループ宛てに読み、1 は init。どれも一覧の Claude ではない。
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn signal_target(pid: u32) -> Option<libc::pid_t> {
    libc::pid_t::try_from(pid)
        .ok()
        .filter(|&target| target > 1 && pid != std::process::id())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn send_signal(pid: u32, signal: Signal) -> io::Result<()> {
    let target = signal_target(pid).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to signal pid {pid}"),
        )
    })?;
    let number = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // SAFETY: target は 1 より大きい単独のプロセスの pid、number は定数の信号番号
    if unsafe { libc::kill(target, number) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn already_gone(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn already_gone(_error: &io::Error) -> bool {
    false
}

#[cfg(any(target_os = "linux", test))]
struct ProcStat {
    state: char,
    parent: u32,
    start_ticks: u64,
}

/// comm は空白も括弧も含みうるので、最後の ')' より後ろを欄で割る。
#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat(stat: &str) -> Option<ProcStat> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let parent = fields.next()?.parse().ok()?;
    // 3 番目 (state) から数えて starttime は 20 番目
    let start_ticks = fields.nth(17)?.parse().ok()?;
    Some(ProcStat {
        state,
        parent,
        start_ticks,
    })
}

#[cfg(any(target_os = "linux", test))]
fn parse_btime(proc_stat: &str) -> Option<u64> {
    proc_stat
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()
}

pub fn sessions_dirs(config_dirs: &[PathBuf]) -> Vec<PathBuf> {
    config_dirs.iter().map(|dir| dir.join("sessions")).collect()
}

/// 一覧に出す写し。同じ dir で同じ会話が 2 つ動いていれば後から起きた方だけ。別の
/// アカウント (dir) で同じ会話を開いているのは別の Claude なので両方残す。pid の昇順。
pub fn read_live(dirs: &[PathBuf], procs: &dyn Processes) -> Vec<LiveSession> {
    let mut newest: HashMap<(PathBuf, String), (u64, LiveSession)> = HashMap::new();
    for (started_at, session) in read_all(dirs, procs) {
        let key = (session.sessions_dir.clone(), session.session_id.clone());
        let keep = newest.get(&key).is_none_or(|(seen, _)| started_at > *seen);
        if keep {
            newest.insert(key, (started_at, session));
        }
    }
    let mut sessions: Vec<LiveSession> = newest.into_values().map(|(_, s)| s).collect();
    sessions.sort_by(|a, b| {
        a.pid
            .cmp(&b.pid)
            .then_with(|| a.sessions_dir.cmp(&b.sessions_dir))
    });
    // sessions だけを symlink で共有した dir は同じ写しを二度読むので、pid で重ねる。
    sessions.dedup_by_key(|s| s.pid);
    sessions
}

/// 止める・消すの判定に使う写し。同じ会話でも隠さない。
pub fn read_every(dirs: &[PathBuf], procs: &dyn Processes) -> Vec<LiveSession> {
    read_all(dirs, procs).into_iter().map(|(_, s)| s).collect()
}

fn read_all(dirs: &[PathBuf], procs: &dyn Processes) -> Vec<(u64, LiveSession)> {
    dirs.iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| read_one(&path, procs))
        .collect()
}

#[derive(Deserialize)]
struct Record {
    pid: Option<u32>,
    cwd: Option<PathBuf>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    status: Option<String>,
    #[serde(rename = "waitingFor")]
    waiting_for: Option<String>,
    #[serde(rename = "statusUpdatedAt")]
    status_updated_at: Option<u64>,
    #[serde(rename = "startedAt")]
    started_at: Option<u64>,
    #[serde(rename = "procStart")]
    proc_start: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    Terminated,
    /// SIGTERM では終わらず、SIGKILL で止めた。
    Killed,
    AlreadyGone,
    Failed(String),
}

/// 写しを読み直し、一覧に載せたときと同じプロセスだと確かめられたときだけ SIGTERM を送る。
/// grace のうちに終わらなければ SIGKILL。
pub fn stop(
    dir: &Path,
    pid: u32,
    started: SystemTime,
    procs: &dyn Processes,
    grace: Duration,
) -> Stopped {
    let path = dir.join(format!("{pid}.json"));
    let confirmed = read_record(&path)
        // 起動時刻の無い写しは、使い回された pid と見分けられない
        .filter(|record| {
            record
                .proc_start
                .as_deref()
                .and_then(parse_proc_start)
                .is_some()
        })
        .and_then(|record| live_session(&path, record, procs))
        .is_some_and(|(_, session)| session.pid == pid && session.started == started);
    if !confirmed {
        return Stopped::AlreadyGone;
    }
    if let Err(e) = procs.signal(pid, Signal::Term) {
        return if already_gone(&e) {
            Stopped::AlreadyGone
        } else {
            Stopped::Failed(e.to_string())
        };
    }
    if ended_within(pid, started, procs, grace) {
        return Stopped::Terminated;
    }
    if let Err(e) = procs.signal(pid, Signal::Kill) {
        return if already_gone(&e) {
            Stopped::Terminated
        } else {
            Stopped::Failed(e.to_string())
        };
    }
    if ended_within(pid, started, procs, KILL_WAIT) {
        Stopped::Killed
    } else {
        Stopped::Failed(format!("pid {pid} survived SIGKILL"))
    }
}

/// ゾンビは親が回収するまで残るだけで、もう動いていない。起動時刻が変われば pid は別人に渡った。
pub fn ended_within(pid: u32, started: SystemTime, procs: &dyn Processes, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        let same = procs
            .info(pid)
            .is_some_and(|info| !info.zombie && info.started == started);
        if !same {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(STOP_POLL);
    }
}

fn read_one(path: &Path, procs: &dyn Processes) -> Option<(u64, LiveSession)> {
    live_session(path, read_record(path)?, procs)
}

fn read_record(path: &Path) -> Option<Record> {
    std::fs::read_to_string(path)
        .map_err(|e| log::debug!("{}: {e}", path.display()))
        .ok()
        .and_then(|text| {
            serde_json::from_str::<Record>(&text)
                .map_err(|e| log::debug!("{}: {e}", path.display()))
                .ok()
        })
}

fn live_session(path: &Path, record: Record, procs: &dyn Processes) -> Option<(u64, LiveSession)> {
    let (Some(pid), Some(cwd), Some(session_id), Some(status)) =
        (record.pid, record.cwd, record.session_id, record.status)
    else {
        log::debug!("{}: 必須の項目が無い", path.display());
        return None;
    };
    let info = procs.info(pid).filter(|info| !info.zombie)?;
    if let Some(proc_start) = record.proc_start.as_deref().and_then(parse_proc_start)
        && abs_diff(proc_start, info.started) > START_TOLERANCE
    {
        return None;
    }
    let session = LiveSession {
        pid,
        started: info.started,
        cwd,
        session_id,
        status: parse_status(&status, record.waiting_for),
        status_since: record
            .status_updated_at
            .map(|ms| UNIX_EPOCH + Duration::from_millis(ms)),
        owner: if procs.is_conductor(info.parent) {
            Owner::Conductor
        } else {
            Owner::Elsewhere
        },
        sessions_dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    Some((record.started_at.unwrap_or(0), session))
}

fn parse_status(status: &str, waiting_for: Option<String>) -> Status {
    match status {
        "busy" => Status::Working,
        "idle" => Status::Idle,
        "waiting" => Status::Waiting(match waiting_for.as_deref() {
            Some("input needed") => Waiting::Question,
            Some("permission prompt") => Waiting::Permission,
            other => Waiting::Other(other.unwrap_or_default().to_string()),
        }),
        other => Status::Other(other.to_string()),
    }
}

/// procStart は UTC の ctime 形式 (`Thu Oct 22 02:05:54 2026`)。印は付かないが local ではない。
fn parse_proc_start(text: &str) -> Option<SystemTime> {
    let parsed = chrono::NaiveDateTime::parse_from_str(text, "%a %b %e %H:%M:%S %Y")
        .map_err(|e| log::debug!("procStart {text:?}: {e}"))
        .ok()?;
    Some(parsed.and_utc().into())
}

fn abs_diff(a: SystemTime, b: SystemTime) -> Duration {
    a.duration_since(b).unwrap_or_else(|e| e.duration())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashSet;

    use chrono::{TimeZone, Utc};
    use serde_json::json;

    use super::*;

    const PROC_START: &str = "Thu Oct 22 02:05:54 2026";

    struct Fake {
        infos: RefCell<HashMap<u32, ProcInfo>>,
        conductors: HashSet<u32>,
        sent: RefCell<Vec<Signal>>,
        refuse: Option<i32>,
        /// 信号を受けた後のプロセス。None は消える。
        after: Vec<(Signal, Option<ProcInfo>)>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                infos: RefCell::new(HashMap::new()),
                conductors: HashSet::new(),
                sent: RefCell::new(Vec::new()),
                refuse: None,
                after: Vec::new(),
            }
        }

        fn with(self, pid: u32, info: ProcInfo) -> Self {
            self.infos.borrow_mut().insert(pid, info);
            self
        }

        fn on(mut self, signal: Signal, next: Option<ProcInfo>) -> Self {
            self.after.push((signal, next));
            self
        }

        fn refusing(mut self, errno: i32) -> Self {
            self.refuse = Some(errno);
            self
        }
    }

    impl Processes for Fake {
        fn info(&self, pid: u32) -> Option<ProcInfo> {
            self.infos.borrow().get(&pid).copied()
        }

        fn is_conductor(&self, pid: u32) -> bool {
            self.conductors.contains(&pid)
        }

        fn signal(&self, pid: u32, signal: Signal) -> io::Result<()> {
            if let Some(errno) = self.refuse {
                return Err(io::Error::from_raw_os_error(errno));
            }
            self.sent.borrow_mut().push(signal);
            if let Some((_, next)) = self.after.iter().find(|(s, _)| *s == signal) {
                match next {
                    Some(info) => self.infos.borrow_mut().insert(pid, *info),
                    None => self.infos.borrow_mut().remove(&pid),
                };
            }
            Ok(())
        }
    }

    fn started_utc() -> SystemTime {
        Utc.with_ymd_and_hms(2026, 10, 22, 2, 5, 54).unwrap().into()
    }

    fn alive(started: SystemTime) -> ProcInfo {
        ProcInfo {
            zombie: false,
            parent: 1,
            started,
        }
    }

    fn record(pid: u32, session_id: &str, status: &str, waiting_for: Option<&str>) -> String {
        let mut value = json!({
            "pid": pid,
            "sessionId": session_id,
            "cwd": "/Users/me/repo-worktrees/x",
            "startedAt": 1792634755219u64,
            "procStart": PROC_START,
            "kind": "interactive",
            "entrypoint": "cli",
            "status": status,
            "statusUpdatedAt": 1792636011219u64,
            "name": "something",
            "nameSource": "derived",
            "nameSince": 1792634755219u64,
            "pidDomain": "darwin",
            "peerProtocol": 1,
            "peerFeatures": [],
            "messagingSocketPath": "/tmp/x.sock",
        });
        if let Some(waiting_for) = waiting_for {
            value["waitingFor"] = json!(waiting_for);
        }
        value.to_string()
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn live(dir: &Path, procs: &Fake) -> Vec<LiveSession> {
        read_live(&[dir.to_path_buf()], procs)
    }

    #[test]
    fn 実物の形の三つがそれぞれの状態になる() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "10.json",
            &record(10, "a", "waiting", Some("input needed")),
        );
        write(
            dir.path(),
            "11.json",
            &record(11, "b", "waiting", Some("permission prompt")),
        );
        write(dir.path(), "12.json", &record(12, "c", "busy", None));
        let procs = Fake::new()
            .with(10, alive(started_utc()))
            .with(11, alive(started_utc()))
            .with(12, alive(started_utc()));

        let statuses: Vec<Status> = live(dir.path(), &procs)
            .into_iter()
            .map(|s| s.status)
            .collect();

        assert_eq!(
            statuses,
            vec![
                Status::Waiting(Waiting::Question),
                Status::Waiting(Waiting::Permission),
                Status::Working,
            ]
        );
    }

    #[test]
    fn 写しの値がそのまま載る() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "10.json", &record(10, "a", "idle", None));
        let procs = Fake::new().with(10, alive(started_utc()));

        let sessions = live(dir.path(), &procs);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].pid, 10);
        assert_eq!(sessions[0].session_id, "a");
        assert_eq!(sessions[0].cwd, PathBuf::from("/Users/me/repo-worktrees/x"));
        assert_eq!(sessions[0].status, Status::Idle);
        assert_eq!(
            sessions[0].status_since,
            Some(UNIX_EPOCH + Duration::from_millis(1792636011219))
        );
    }

    #[test]
    fn shellは生の文字列で残る() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "10.json", &record(10, "a", "shell", None));
        let procs = Fake::new().with(10, alive(started_utc()));

        let sessions = live(dir.path(), &procs);

        assert_eq!(sessions[0].status, Status::Other("shell".into()));
    }

    #[test]
    fn 未知の待ち理由は生の文字列で残る() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "10.json",
            &record(10, "a", "waiting", Some("elicitation")),
        );
        let procs = Fake::new().with(10, alive(started_utc()));

        let sessions = live(dir.path(), &procs);

        assert_eq!(
            sessions[0].status,
            Status::Waiting(Waiting::Other("elicitation".into()))
        );
    }

    #[test]
    fn ゾンビは出ない() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "10.json", &record(10, "a", "busy", None));
        let procs = Fake::new().with(
            10,
            ProcInfo {
                zombie: true,
                ..alive(started_utc())
            },
        );

        assert!(live(dir.path(), &procs).is_empty());
    }

    #[test]
    fn プロセスが居ない写しは出ない() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "10.json", &record(10, "a", "busy", None));

        assert!(live(dir.path(), &Fake::new()).is_empty());
    }

    #[test]
    fn 起動時刻が1時間ずれるpidの使い回しは出ない() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "10.json", &record(10, "a", "busy", None));
        let procs = Fake::new().with(10, alive(started_utc() + Duration::from_secs(3600)));

        assert!(live(dir.path(), &procs).is_empty());
    }

    #[test]
    fn procstartはutcとして読む() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "10.json", &record(10, "a", "busy", None));
        let jst_misread = started_utc() - Duration::from_secs(9 * 3600);
        let procs = Fake::new().with(10, alive(jst_misread));

        assert!(
            live(dir.path(), &procs).is_empty(),
            "UTC の起動時刻から 9 時間ずれたプロセスは別物"
        );
        let procs = Fake::new().with(10, alive(started_utc() + Duration::from_secs(1)));
        assert_eq!(live(dir.path(), &procs).len(), 1);
    }

    #[test]
    fn 日が1桁のprocstartも読める() {
        let dir = tempfile::tempdir().unwrap();
        let body = record(10, "a", "busy", None).replace(PROC_START, "Thu Oct  1 02:05:54 2026");
        write(dir.path(), "10.json", &body);
        let started: SystemTime = Utc.with_ymd_and_hms(2026, 10, 1, 2, 5, 54).unwrap().into();
        let procs = Fake::new().with(10, alive(started));

        assert_eq!(live(dir.path(), &procs).len(), 1);
    }

    #[test]
    fn 同じセッションidはstartedatが新しい方() {
        let dir = tempfile::tempdir().unwrap();
        let older = record(10, "same", "busy", None);
        let newer = record(11, "same", "idle", None).replace("1792634755219", "1792634999999");
        write(dir.path(), "10.json", &older);
        write(dir.path(), "11.json", &newer);
        let procs = Fake::new()
            .with(10, alive(started_utc()))
            .with(11, alive(started_utc()));

        let sessions = live(dir.path(), &procs);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].pid, 11);
    }

    #[test]
    fn keyファイルは中身がjsonでも読まない() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "10.abcdef.key", &record(10, "a", "busy", None));
        let procs = Fake::new().with(10, alive(started_utc()));

        assert!(live(dir.path(), &procs).is_empty());
    }

    #[test]
    fn 壊れた写しは飛ばして残りを読む() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "9.json", "{not json");
        write(
            dir.path(),
            "8.json",
            r#"{"pid":8,"cwd":"/x","sessionId":"z"}"#,
        );
        write(dir.path(), "10.json", &record(10, "a", "busy", None));
        let procs = Fake::new()
            .with(8, alive(started_utc()))
            .with(9, alive(started_utc()))
            .with(10, alive(started_utc()));

        let sessions = live(dir.path(), &procs);

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].pid, 10);
    }

    #[test]
    fn 親がconductorなら持ち主はconductor() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "10.json", &record(10, "a", "busy", None));
        write(dir.path(), "11.json", &record(11, "b", "busy", None));
        let mut procs = Fake::new()
            .with(
                10,
                ProcInfo {
                    parent: 500,
                    ..alive(started_utc())
                },
            )
            .with(
                11,
                ProcInfo {
                    parent: 600,
                    ..alive(started_utc())
                },
            );
        procs.conductors.insert(500);

        let owners: Vec<Owner> = live(dir.path(), &procs)
            .into_iter()
            .map(|s| s.owner)
            .collect();

        assert_eq!(owners, vec![Owner::Conductor, Owner::Elsewhere]);
    }

    #[test]
    fn 並びはpidの昇順() {
        let dir = tempfile::tempdir().unwrap();
        let mut procs = Fake::new();
        for pid in [30u32, 4, 200] {
            write(
                dir.path(),
                &format!("{pid}.json"),
                &record(pid, &format!("s{pid}"), "busy", None),
            );
            procs = procs.with(pid, alive(started_utc()));
        }

        let pids: Vec<u32> = live(dir.path(), &procs)
            .into_iter()
            .map(|s| s.pid)
            .collect();

        assert_eq!(pids, vec![4, 30, 200]);
    }

    #[test]
    fn ディレクトリが無ければ空() {
        let dir = tempfile::tempdir().unwrap();

        assert!(live(&dir.path().join("none"), &Fake::new()).is_empty());
    }

    #[test]
    fn 二つのディレクトリの写しはそれぞれ読んだディレクトリを持つ() {
        let (work, personal) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(work.path(), "10.json", &record(10, "a", "idle", None));
        write(personal.path(), "11.json", &record(11, "b", "busy", None));
        let procs = Fake::new()
            .with(10, alive(started_utc()))
            .with(11, alive(started_utc()));

        let dirs: Vec<(String, PathBuf)> = read_live(
            &[work.path().to_path_buf(), personal.path().to_path_buf()],
            &procs,
        )
        .into_iter()
        .map(|s| (s.session_id, s.sessions_dir))
        .collect();

        assert_eq!(
            dirs,
            vec![
                ("a".to_string(), work.path().to_path_buf()),
                ("b".to_string(), personal.path().to_path_buf()),
            ]
        );
    }

    fn started_later(pid: u32, session_id: &str) -> String {
        let mut value: serde_json::Value =
            serde_json::from_str(&record(pid, session_id, "busy", None)).unwrap();
        value["startedAt"] = json!(1792634999999u64);
        value.to_string()
    }

    #[test]
    fn 同じディレクトリで同じ会話が二つ動いていれば一覧は後の方だけで判定用は両方() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "10.json", &record(10, "same", "idle", None));
        write(dir.path(), "11.json", &started_later(11, "same"));
        let procs = Fake::new()
            .with(10, alive(started_utc()))
            .with(11, alive(started_utc()));
        let dirs = [dir.path().to_path_buf()];

        let listed: Vec<u32> = read_live(&dirs, &procs).iter().map(|s| s.pid).collect();
        let mut every: Vec<u32> = read_every(&dirs, &procs).iter().map(|s| s.pid).collect();
        every.sort();

        assert_eq!(listed, vec![11]);
        assert_eq!(every, vec![10, 11]);
    }

    #[test]
    fn 別のディレクトリで同じ会話を開いている二つは一覧に両方出る() {
        let (work, personal) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(work.path(), "10.json", &record(10, "same", "idle", None));
        write(personal.path(), "11.json", &started_later(11, "same"));
        let procs = Fake::new()
            .with(10, alive(started_utc()))
            .with(11, alive(started_utc()));

        let listed: Vec<u32> = read_live(
            &[work.path().to_path_buf(), personal.path().to_path_buf()],
            &procs,
        )
        .iter()
        .map(|s| s.pid)
        .collect();

        assert_eq!(listed, vec![10, 11]);
    }

    #[test]
    fn sessionsだけを共有した二つのディレクトリでも同じpidは一行() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("a/sessions");
        let linked = root.path().join("b/sessions");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        write(&real, "10.json", &record(10, "a", "idle", None));
        let procs = Fake::new().with(10, alive(started_utc()));

        let listed: Vec<u32> = read_live(&[real, linked], &procs)
            .iter()
            .map(|s| s.pid)
            .collect();

        assert_eq!(listed, vec![10]);
    }

    #[test]
    fn 本物のプロセス表で自分自身が引ける() {
        let info = SystemProcesses
            .info(std::process::id())
            .expect("自分自身のプロセスが引けない");

        assert!(!info.zombie);
        assert!(info.started <= SystemTime::now());
        assert!(info.started > SystemTime::now() - Duration::from_secs(24 * 3600));
        assert!(!SystemProcesses.is_conductor(std::process::id()));
    }

    #[test]
    fn 本物のプロセス表で終わって回収されない子は生きて見えない() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let running = SystemProcesses.info(pid).expect("動いている子が引けない");
        assert!(!running.zombie);
        assert_eq!(running.parent, std::process::id());

        child.kill().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let gone = loop {
            // macOS は回収前のゾンビを proc_pidinfo が返さない (ESRCH)。Linux は zombie: true
            if SystemProcesses.info(pid).is_none_or(|info| info.zombie) {
                break true;
            }
            if std::time::Instant::now() > deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        child.wait().unwrap();

        assert!(gone, "kill したが wait していない子が生きて見える");
    }

    #[test]
    fn procのstatから状態と親と起動を読む() {
        let stat = "4242 (my (weird) cmd) S 1 4242 4242 0 -1 4194560 100 0 0 0 5 3 0 0 20 0 1 0 987654 1000 100 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0";

        let parsed = parse_proc_stat(stat).unwrap();

        assert_eq!(parsed.state, 'S');
        assert_eq!(parsed.parent, 1);
        assert_eq!(parsed.start_ticks, 987654);
        assert_eq!(parse_proc_stat("4242 (x) Z 9 0").map(|s| s.state), None);
    }

    #[test]
    fn procのstatのbtimeを読む() {
        let proc_stat = "cpu  1 2 3\nintr 0\nbtime 1792000000\nprocesses 10\n";

        assert_eq!(parse_btime(proc_stat), Some(1792000000));
    }

    fn spawn_real(command: &mut std::process::Command) -> (std::process::Child, SystemTime) {
        let child = command.spawn().unwrap();
        let started = SystemProcesses
            .info(child.id())
            .expect("動いている子が引けない")
            .started;
        (child, started)
    }

    fn sleeper() -> (std::process::Child, SystemTime) {
        spawn_real(std::process::Command::new("sleep").arg("30"))
    }

    fn real_record(pid: u32, started: SystemTime) -> String {
        let proc_start = chrono::DateTime::<Utc>::from(started)
            .format("%a %b %e %H:%M:%S %Y")
            .to_string();
        record(pid, "a", "busy", None).replace(PROC_START, &proc_start)
    }

    fn signal_of(mut child: std::process::Child) -> Option<i32> {
        use std::os::unix::process::ExitStatusExt;
        child.wait().unwrap().signal()
    }

    fn assert_untouched(mut child: std::process::Child) {
        let running = child.try_wait().unwrap().is_none();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(running, "信号を送ってはいけない子が終わっている");
    }

    const GRACE: Duration = Duration::from_secs(3);

    #[test]
    fn 本物の子はsigtermで止まる() {
        let dir = tempfile::tempdir().unwrap();
        let (child, started) = sleeper();
        let pid = child.id();
        write(
            dir.path(),
            &format!("{pid}.json"),
            &real_record(pid, started),
        );

        let stopped = stop(dir.path(), pid, started, &SystemProcesses, GRACE);

        assert_eq!(stopped, Stopped::Terminated);
        assert_eq!(signal_of(child), Some(libc::SIGTERM));
    }

    #[test]
    fn sigtermを無視する子は猶予の後sigkillで止まる() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let (child, started) = spawn_real(
            std::process::Command::new("sh")
                .args(["-c", r#"trap "" TERM; echo > "$0"; exec sleep 30"#])
                .arg(&ready),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "子が TERM を無視する前に時間切れ"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let pid = child.id();
        write(
            dir.path(),
            &format!("{pid}.json"),
            &real_record(pid, started),
        );

        let stopped = stop(
            dir.path(),
            pid,
            started,
            &SystemProcesses,
            Duration::from_millis(100),
        );

        assert_eq!(stopped, Stopped::Killed);
        assert_eq!(signal_of(child), Some(libc::SIGKILL));
    }

    #[test]
    fn 写しが無ければ送らない() {
        let dir = tempfile::tempdir().unwrap();
        let (child, started) = sleeper();

        let stopped = stop(dir.path(), child.id(), started, &SystemProcesses, GRACE);

        assert_eq!(stopped, Stopped::AlreadyGone);
        assert_untouched(child);
    }

    #[test]
    fn 起動時刻の無い写しでは送らない() {
        let dir = tempfile::tempdir().unwrap();
        let (child, started) = sleeper();
        let pid = child.id();
        let mut body: serde_json::Value = serde_json::from_str(&real_record(pid, started)).unwrap();
        body.as_object_mut().unwrap().remove("procStart");
        write(dir.path(), &format!("{pid}.json"), &body.to_string());

        let stopped = stop(dir.path(), pid, started, &SystemProcesses, GRACE);

        assert_eq!(stopped, Stopped::AlreadyGone);
        assert_untouched(child);
    }

    #[test]
    fn 起動時刻の合わない写しでは送らない() {
        let dir = tempfile::tempdir().unwrap();
        let (child, started) = sleeper();
        let pid = child.id();
        let stale = real_record(pid, started - Duration::from_secs(3600));
        write(dir.path(), &format!("{pid}.json"), &stale);

        let stopped = stop(dir.path(), pid, started, &SystemProcesses, GRACE);

        assert_eq!(stopped, Stopped::AlreadyGone);
        assert_untouched(child);
    }

    #[test]
    fn 一覧に載せたときと別のプロセスなら送らない() {
        let dir = tempfile::tempdir().unwrap();
        let (child, started) = sleeper();
        let pid = child.id();
        write(
            dir.path(),
            &format!("{pid}.json"),
            &real_record(pid, started),
        );

        let listed = started - Duration::from_secs(1);
        let stopped = stop(dir.path(), pid, listed, &SystemProcesses, GRACE);

        assert_eq!(stopped, Stopped::AlreadyGone);
        assert_untouched(child);
    }

    fn fake_session(dir: &Path) -> Fake {
        write(dir, "10.json", &record(10, "a", "busy", None));
        Fake::new().with(10, alive(started_utc()))
    }

    const SHORT: Duration = Duration::from_millis(20);

    #[test]
    fn 写しの中のpidがファイル名と違えば送らない() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "10.json", &record(11, "a", "busy", None));
        // Linux の起動時刻は 10ms 刻みなので、同じ時刻に起きた別のプロセスはありうる
        let procs = Fake::new()
            .with(10, alive(started_utc()))
            .with(11, alive(started_utc()));

        assert_eq!(
            stop(dir.path(), 10, started_utc(), &procs, SHORT),
            Stopped::AlreadyGone
        );
        assert!(procs.sent.borrow().is_empty());
    }

    #[test]
    fn linux_の_claude_が書く_tick_数の起動時刻では送らない() {
        // 実物: Linux の Claude Code 2.1.310 が書く "procStart":"37242291"
        let dir = tempfile::tempdir().unwrap();
        let body = record(10, "a", "busy", None).replace(PROC_START, "37242291");
        write(dir.path(), "10.json", &body);
        let procs = Fake::new().with(10, alive(started_utc()));

        assert_eq!(
            stop(dir.path(), 10, started_utc(), &procs, SHORT),
            Stopped::AlreadyGone
        );
        assert!(procs.sent.borrow().is_empty());
    }

    #[test]
    fn 送る直前に終わっていれば終わっていた扱い() {
        let dir = tempfile::tempdir().unwrap();
        let procs = fake_session(dir.path()).refusing(libc::ESRCH);

        assert_eq!(
            stop(dir.path(), 10, started_utc(), &procs, SHORT),
            Stopped::AlreadyGone
        );
    }

    #[test]
    fn 送れなければ止められなかった扱い() {
        let dir = tempfile::tempdir().unwrap();
        let procs = fake_session(dir.path()).refusing(libc::EPERM);

        let stopped = stop(dir.path(), 10, started_utc(), &procs, SHORT);

        assert!(matches!(stopped, Stopped::Failed(_)), "{stopped:?}");
    }

    #[test]
    fn sigtermの後にゾンビになれば止まった扱い() {
        let dir = tempfile::tempdir().unwrap();
        let zombie = ProcInfo {
            zombie: true,
            ..alive(started_utc())
        };
        let procs = fake_session(dir.path()).on(Signal::Term, Some(zombie));

        assert_eq!(
            stop(dir.path(), 10, started_utc(), &procs, SHORT),
            Stopped::Terminated
        );
        assert_eq!(*procs.sent.borrow(), vec![Signal::Term]);
    }

    #[test]
    fn 猶予中にpidが別のプロセスへ渡ったらsigkillを送らない() {
        let dir = tempfile::tempdir().unwrap();
        let stranger = alive(started_utc() + Duration::from_secs(3600));
        let procs = fake_session(dir.path()).on(Signal::Term, Some(stranger));

        assert_eq!(
            stop(dir.path(), 10, started_utc(), &procs, SHORT),
            Stopped::Terminated
        );
        assert_eq!(*procs.sent.borrow(), vec![Signal::Term]);
    }

    #[test]
    fn 続けて2度止めると2度目はもう終わっていた扱いで送らない() {
        let dir = tempfile::tempdir().unwrap();
        let procs = fake_session(dir.path()).on(Signal::Term, None);

        assert_eq!(
            stop(dir.path(), 10, started_utc(), &procs, SHORT),
            Stopped::Terminated
        );
        assert_eq!(
            stop(dir.path(), 10, started_utc(), &procs, SHORT),
            Stopped::AlreadyGone
        );
        assert_eq!(*procs.sent.borrow(), vec![Signal::Term]);
    }

    #[test]
    fn sigkillの後も残れば止められなかった扱い() {
        let dir = tempfile::tempdir().unwrap();
        let procs = fake_session(dir.path());

        let stopped = stop(dir.path(), 10, started_utc(), &procs, SHORT);

        assert!(matches!(stopped, Stopped::Failed(_)), "{stopped:?}");
        assert_eq!(*procs.sent.borrow(), vec![Signal::Term, Signal::Kill]);
    }

    #[test]
    fn プロセスグループとinitと自分自身には送らない() {
        for pid in [0, 1, std::process::id(), u32::MAX] {
            assert_eq!(signal_target(pid), None, "pid {pid}");
        }
        assert_eq!(signal_target(4242), Some(4242));
    }
}
