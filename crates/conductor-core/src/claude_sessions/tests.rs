//! 1 つのプロジェクトディレクトリには、あるワークツリーで走った全セッションのログが同居する。
//! ログを選ぶのは session id だけで、兄弟ログの数・mtime・ターン時刻で答えが変わってはならない。

use std::fs::{self, File, FileTimes};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{ClaudeHome, encode_project_path, format_time_ago, session_log_in_dir};
use crate::claude_log::{DisplayBlock, load_session};

fn home() -> (tempfile::TempDir, ClaudeHome) {
    let dir = tempfile::tempdir().unwrap();
    let home = ClaudeHome::at(dir.path().to_path_buf());
    (dir, home)
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

/// dir に <session_id>.jsonl を書く。turns は 1 ユーザターン 1 行、mtime は now - age。
fn write_log(dir: &Path, session_id: &str, turns: &[&str], age: Duration) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{session_id}.jsonl"));
    let mut f = File::create(&path).unwrap();
    for turn in turns {
        writeln!(
            f,
            r#"{{"type":"user","sessionId":"{session_id}","message":{{"role":"user","content":"{turn}"}}}}"#
        )
        .unwrap();
    }
    f.set_times(FileTimes::new().set_modified(SystemTime::now() - age))
        .unwrap();
    path
}

fn transcript_texts(dir: &Path, session_id: &str) -> Vec<String> {
    let path = session_log_in_dir(dir, session_id).expect("session log resolves");
    load_session(&path)
        .into_iter()
        .flat_map(|e| e.blocks)
        .map(|b| match b {
            DisplayBlock::Text(t) => t,
            other => panic!("unexpected block: {other:?}"),
        })
        .collect()
}

/// history.jsonl の 1 行 (実測の形)。
fn history_line(display: &str, session_id: &str, timestamp: u64, project: &Path) -> String {
    format!(
        r#"{{"display":"{display}","timestamp":{timestamp},"project":"{}","sessionId":"{session_id}"}}"#,
        project.display()
    )
}

fn write_history(home: &ClaudeHome, lines: &[String]) {
    fs::write(home.history_file(), lines.join("\n") + "\n").unwrap();
}

/// project の projects ディレクトリに空のセッションログを置く。
fn touch_session(home: &ClaudeHome, project: &Path, session_id: &str) {
    let dir = home.projects_dir_for(project);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{session_id}.jsonl")), "").unwrap();
}

fn ids(sessions: &[super::ResumableSession]) -> Vec<&str> {
    sessions.iter().map(|s| s.session_id.as_str()).collect()
}

// session id によるログの解決

#[test]
fn session_idが兄弟の中からログを選ぶ() {
    let dir = tempfile::tempdir().unwrap();
    write_log(dir.path(), "aaa", &["from aaa"], Duration::from_secs(3600));
    write_log(dir.path(), "bbb", &["from bbb"], Duration::from_secs(60));
    write_log(dir.path(), "ccc", &["from ccc"], Duration::ZERO);

    assert_eq!(transcript_texts(dir.path(), "aaa"), ["from aaa"]);
    assert_eq!(transcript_texts(dir.path(), "bbb"), ["from bbb"]);
    assert_eq!(transcript_texts(dir.path(), "ccc"), ["from ccc"]);
}

#[test]
fn 止まっているセッションは新しい兄弟に追い出されない() {
    // サブエージェントだけが動いているパネルは自分のログに何も書かない (サブエージェントの
    // ターンは <session-id>/subagents/*.jsonl に入る)。ディレクトリ内で最古のログでも、
    // 解決はこのパネル自身のログを返す。
    let dir = tempfile::tempdir().unwrap();
    write_log(
        dir.path(),
        "idle-with-subagent",
        &["my prompt", "my later prompt"],
        Duration::from_secs(3600),
    );
    write_log(
        dir.path(),
        "other-live-session",
        &["someone else's prompt"],
        Duration::ZERO,
    );
    let sub = dir.path().join("idle-with-subagent").join("subagents");
    write_log(&sub, "agent-aivy-1234", &["subagent work"], Duration::ZERO);

    assert_eq!(
        transcript_texts(dir.path(), "idle-with-subagent"),
        ["my prompt", "my later prompt"]
    );
}

#[test]
fn 解決できないidは兄弟にフォールバックしない() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        session_log_in_dir(dir.path(), "aaa").is_none(),
        "空のディレクトリ"
    );
    write_log(dir.path(), "aaa", &["from aaa"], Duration::ZERO);
    assert!(session_log_in_dir(dir.path(), "no-such-session").is_none());
}

#[test]
fn symlinkされたセッションログも解決する() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let real = write_log(source.path(), "aaa", &["from aaa"], Duration::ZERO);
    std::os::unix::fs::symlink(&real, target.path().join("aaa.jsonl")).unwrap();

    assert_eq!(transcript_texts(target.path(), "aaa"), ["from aaa"]);
}

#[test]
fn working_dirは解決後のパスで探し消えたワークツリーは生のパスで探す() {
    let (_h, home) = home();
    let wd = tempfile::tempdir().unwrap();
    let expected = write_log(
        &home.projects_dir_for(&canonical(wd.path())),
        "aaa",
        &["x"],
        Duration::ZERO,
    );
    assert_eq!(home.session_log(wd.path(), "aaa"), Some(expected));
    assert_eq!(home.session_log(wd.path(), "zzz"), None);

    let gone = wd.path().join("gone");
    let expected = write_log(&home.projects_dir_for(&gone), "bbb", &["y"], Duration::ZERO);
    assert_eq!(home.session_log(&gone, "bbb"), Some(expected));
}

#[test]
fn プロジェクトパスのエンコード() {
    assert_eq!(
        encode_project_path("/Users/foo/github.com/proj"),
        "-Users-foo-github-com-proj"
    );
}

#[test]
fn 経過時間の表記() {
    let cases = [
        (0, "just now"),
        (59_000, "just now"),
        (5 * 60_000, "5m ago"),
        (3 * 3_600_000, "3h ago"),
        (2 * 86_400_000, "2d ago"),
        (45 * 86_400_000, "1mo ago"),
    ];
    for (ago_ms, want) in cases {
        assert_eq!(
            format_time_ago(10_000_000_000, 10_000_000_000 - ago_ms),
            want
        );
    }
    assert_eq!(format_time_ago(10, 20), "just now", "未来の時刻");
}

// history.jsonl からの発見

#[test]
fn resume一覧は新しい順で重複とログの消えたものを除く() {
    let (_h, home) = home();
    let p = canonical(tempfile::tempdir().unwrap().path());
    let q = canonical(tempfile::tempdir().unwrap().path());
    for id in ["a", "b", "c"] {
        touch_session(&home, if id == "c" { &q } else { &p }, id);
    }
    write_history(
        &home,
        &[
            history_line("a first", "a", 1000, &p),
            history_line("b", "b", 2000, &p),
            history_line("a again", "a", 3000, &p),
            history_line("ghost", "ghost", 4000, &p),
            history_line("c", "c", 2500, &q),
            r#"{"broken": true}"#.to_string(),
        ],
    );

    let all = home.load_resumable_sessions(None).unwrap();
    assert_eq!(
        ids(&all),
        ["c", "a", "b"],
        "順序はファイルの逆順で、timestamp では並べない"
    );
    assert_eq!(all[1].display, "a again", "重複は最後のエントリを採る");
    assert_eq!(
        all[1].project_name,
        p.file_name().unwrap().to_string_lossy()
    );

    let only_p = home.load_resumable_sessions(Some(&p)).unwrap();
    assert_eq!(ids(&only_p), ["a", "b"]);
}

#[test]
fn ワークツリーごとの最新セッションはログの消えたものに隠されない() {
    let (_h, home) = home();
    let p = canonical(tempfile::tempdir().unwrap().path());
    let q = canonical(tempfile::tempdir().unwrap().path());
    let unknown = canonical(tempfile::tempdir().unwrap().path());
    touch_session(&home, &p, "a");
    touch_session(&home, &p, "b");
    touch_session(&home, &q, "c");
    write_history(
        &home,
        &[
            history_line("b", "b", 2000, &p),
            history_line("a", "a", 1000, &p),
            history_line("ghost", "ghost", 4000, &p),
            history_line("c", "c", 100, &q),
        ],
    );

    let found = home
        .find_latest_sessions_for_paths(&[p.clone(), q.clone(), unknown.clone()])
        .unwrap();
    assert_eq!(found[&p].session_id, "b");
    assert_eq!(found[&q].session_id, "c");
    assert!(!found.contains_key(&unknown));
}

#[test]
fn historyが無ければ空() {
    let (_h, home) = home();
    assert!(home.load_resumable_sessions(None).unwrap().is_empty());
    assert!(
        home.find_latest_sessions_for_paths(&[PathBuf::from("/x")])
            .unwrap()
            .is_empty()
    );
}

// grab/ungrab の移行

struct Migration {
    _home_dir: tempfile::TempDir,
    home: ClaudeHome,
    source: PathBuf,
    dest: PathBuf,
}

impl Migration {
    fn new() -> Self {
        let (home_dir, home) = home();
        let source = canonical(tempfile::tempdir().unwrap().path());
        let dest = canonical(tempfile::tempdir().unwrap().path());
        let src_dir = home.projects_dir_for(&source);
        write_log(&src_dir, "sess", &["hello"], Duration::ZERO);
        write_log(
            &src_dir.join("sess").join("subagents"),
            "agent-1",
            &["sub"],
            Duration::ZERO,
        );
        Self {
            _home_dir: home_dir,
            home,
            source,
            dest,
        }
    }

    fn src(&self, rel: &str) -> PathBuf {
        self.home.projects_dir_for(&self.source).join(rel)
    }

    fn dst(&self, rel: &str) -> PathBuf {
        self.home.projects_dir_for(&self.dest).join(rel)
    }

    fn migrate(&self) -> bool {
        self.home
            .migrate_session("sess", &self.source, &self.dest, "hint")
            .unwrap()
    }

    fn unmigrate(&self) {
        self.home
            .unmigrate_session("sess", &self.source, &self.dest)
            .unwrap();
    }
}

#[test]
fn 移行はログとサブエージェントをリンクしhistoryに追記する() {
    let m = Migration::new();
    assert!(m.migrate());

    assert!(m.dst("sess.jsonl").symlink_metadata().unwrap().is_symlink());
    assert!(m.dst("sess").symlink_metadata().unwrap().is_symlink());
    assert_eq!(transcript_texts(&m.dst(""), "sess"), ["hello"]);

    let latest = m
        .home
        .find_latest_sessions_for_paths(std::slice::from_ref(&m.dest))
        .unwrap();
    assert_eq!(latest[&m.dest].session_id, "sess");
    assert_eq!(latest[&m.dest].display, "hint");

    assert!(m.migrate(), "2 回目もリンクがあるので何もせず成功");
}

#[test]
fn 移行元にログが無ければ何もしない() {
    let m = Migration::new();
    assert!(
        !m.home
            .migrate_session("nope", &m.source, &m.dest, "hint")
            .unwrap()
    );
    assert!(!m.dst("").exists());
}

#[test]
fn 戻すときリンクは外すだけ() {
    let m = Migration::new();
    m.migrate();
    m.unmigrate();
    assert!(!m.dst("sess.jsonl").exists());
    assert!(!m.dst("sess").exists());
    assert_eq!(transcript_texts(&m.src(""), "sess"), ["hello"]);
}

#[test]
fn 戻すとき実体に置き換わったものは移行元へコピーする() {
    let m = Migration::new();
    m.migrate();
    fs::remove_file(m.dst("sess.jsonl")).unwrap();
    write_log(&m.dst(""), "sess", &["hello", "newer"], Duration::ZERO);
    fs::remove_file(m.dst("sess")).unwrap();
    write_log(
        &m.dst("sess").join("subagents"),
        "agent-2",
        &["sub2"],
        Duration::ZERO,
    );

    m.unmigrate();

    assert!(!m.dst("sess.jsonl").exists());
    assert!(!m.dst("sess").exists());
    assert_eq!(transcript_texts(&m.src(""), "sess"), ["hello", "newer"]);
    let subagents = m.src("sess/subagents");
    assert!(subagents.join("agent-1.jsonl").exists());
    assert!(subagents.join("agent-2.jsonl").exists());
}
