//! Tests for transcript-source resolution.
//!
//! One Claude project directory holds the logs of every session ever run in a
//! worktree, so these tests pin down the property the reflow transcript view
//! depends on: the *session id* selects the log, and nothing about the sibling
//! logs sharing the directory (count, mtime, first/last turn time) can change
//! which one is picked.

use std::fs::{File, FileTimes};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::session_log_in_dir;
use crate::claude_log::{DisplayBlock, load_session};

/// Write `<session_id>.jsonl` into `dir` with one user turn per line of `turns`,
/// and force its mtime to `now - age`.
fn write_log(dir: &Path, session_id: &str, turns: &[&str], age: Duration) -> PathBuf {
    let path = dir.join(format!("{session_id}.jsonl"));
    let mut f = File::create(&path).expect("create log");
    for turn in turns {
        writeln!(
            f,
            r#"{{"type":"user","sessionId":"{session_id}","message":{{"role":"user","content":"{turn}"}}}}"#
        )
        .expect("write turn");
    }
    let mtime = SystemTime::now() - age;
    f.set_times(FileTimes::new().set_modified(mtime))
        .expect("set mtime");
    path
}

/// The text of every entry in the transcript resolved for `session_id`.
fn transcript_texts(dir: &Path, session_id: &str) -> Vec<String> {
    let path = session_log_in_dir(dir, session_id).expect("session log resolves");
    load_session(&path)
        .iter()
        .flat_map(|e| e.blocks.clone())
        .map(|b| match b {
            DisplayBlock::Text(t) => t,
            other => panic!("unexpected block: {other:?}"),
        })
        .collect()
}

#[test]
fn session_id_selects_the_log_among_siblings() {
    // Three sessions share the project dir (three Claude panels on the same
    // worktree). Each id must resolve to its own log, whatever the mtimes.
    let dir = tempfile::tempdir().expect("tmp dir");
    write_log(dir.path(), "aaa", &["from aaa"], Duration::from_secs(3600));
    write_log(dir.path(), "bbb", &["from bbb"], Duration::from_secs(60));
    write_log(dir.path(), "ccc", &["from ccc"], Duration::ZERO);

    assert_eq!(transcript_texts(dir.path(), "aaa"), vec!["from aaa"]);
    assert_eq!(transcript_texts(dir.path(), "bbb"), vec!["from bbb"]);
    assert_eq!(transcript_texts(dir.path(), "ccc"), vec!["from ccc"]);
}

#[test]
fn idle_session_is_not_displaced_by_a_newer_sibling() {
    // Regression: a panel whose main agent is stopped while a subagent still
    // works writes nothing to its session log (subagent turns land in
    // `<session-id>/subagents/*.jsonl`), so its log is both the stalest in the
    // directory and frozen before a session started later in the same worktree.
    // Resolution must still return *this* panel's log — the old view followed
    // the freshest log, or the one starting after this one's last turn, and
    // opened onto the other session's conversation.
    let dir = tempfile::tempdir().expect("tmp dir");
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
    // A subagent transcript of the idle session, in its own subdirectory.
    let sub = dir.path().join("idle-with-subagent").join("subagents");
    std::fs::create_dir_all(&sub).expect("subagent dir");
    write_log(&sub, "agent-aivy-1234", &["subagent work"], Duration::ZERO);

    assert_eq!(
        transcript_texts(dir.path(), "idle-with-subagent"),
        vec!["my prompt", "my later prompt"]
    );
}

#[test]
fn unknown_session_id_resolves_to_nothing() {
    // No history is the correct answer for an unresolvable id: falling back to
    // whatever log the directory happens to hold shows another conversation.
    let dir = tempfile::tempdir().expect("tmp dir");
    write_log(dir.path(), "aaa", &["from aaa"], Duration::ZERO);

    assert!(session_log_in_dir(dir.path(), "no-such-session").is_none());
}

#[test]
fn empty_project_dir_resolves_to_nothing() {
    let dir = tempfile::tempdir().expect("tmp dir");
    assert!(session_log_in_dir(dir.path(), "aaa").is_none());
}

#[test]
fn symlinked_session_log_resolves() {
    // `migrate_session` symlinks a grabbed branch's session into the new
    // worktree's project dir; resolution must follow the link.
    let source = tempfile::tempdir().expect("tmp dir");
    let target = tempfile::tempdir().expect("tmp dir");
    let real = write_log(source.path(), "aaa", &["from aaa"], Duration::ZERO);
    std::os::unix::fs::symlink(&real, target.path().join("aaa.jsonl")).expect("symlink");

    assert_eq!(transcript_texts(target.path(), "aaa"), vec!["from aaa"]);
}
