//! Claude Code session discovery.
//!
//! Reads `~/.claude/history.jsonl` to find resumable Claude Code sessions.
//! Each line in the history file is a JSON object with:
//!   { "display": "...", "timestamp": ..., "project": "...", "sessionId": "..." }
//!
//! Shared data types and path helpers live here; session listing is in
//! [`discovery`] and grab/ungrab session symlinking is in [`migrate`].

use std::path::{Path, PathBuf};

use serde::Deserialize;

mod discovery;
mod migrate;
#[cfg(test)]
mod tests;

pub use discovery::{find_latest_sessions_for_paths, load_resumable_sessions};
pub use migrate::{migrate_session, unmigrate_session};

/// A single entry from `~/.claude/history.jsonl`.
#[derive(Debug, Clone, Deserialize)]
struct ClaudeHistoryEntry {
    display: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    timestamp: u64,
    project: String,
}

/// A resumable Claude session with derived display info.
#[derive(Debug, Clone)]
pub struct ResumableSession {
    pub session_id: String,
    /// The original prompt text (last user message in the session).
    pub display: String,
    /// Short name (last path component).
    pub project_name: String,
    /// Human-readable time ago string (e.g. "3h ago").
    pub time_ago: String,
    /// The full project path from the history entry.
    #[allow(dead_code)]
    pub project_path: String,
}

/// Return the path to the Claude history file.
fn history_file_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("history.jsonl"))
}

/// Encode a project path the way Claude Code does for its project directories.
/// All `/` and `.` are replaced with `-`.
/// E.g. `/Users/foo/github.com/proj` → `-Users-foo-github-com-proj`.
fn encode_project_path(path: &str) -> String {
    path.replace(['/', '.'], "-")
}

/// Check if a session JSONL file exists for the given session ID and project.
fn session_file_exists(session_id: &str, project: &str) -> bool {
    if let Some(home) = dirs::home_dir() {
        let encoded = encode_project_path(project);
        let session_file = home
            .join(".claude")
            .join("projects")
            .join(&encoded)
            .join(format!("{session_id}.jsonl"));
        session_file.exists()
    } else {
        false
    }
}

/// Return the `.jsonl` path for the given working directory and session ID.
///
/// The session id is the only key: one project directory holds the logs of
/// *every* session ever run in that directory, so resolution must never widen to
/// a directory-level criterion (see [`session_log_in_dir`]). Returns `None` when
/// the home directory is unavailable or that session has no log on disk.
///
/// The working dir is canonicalized first because Claude Code encodes its
/// *resolved* cwd; a symlinked worktree path would otherwise miss the directory.
/// The raw path is tried as a second encoding so a worktree that no longer
/// resolves (deleted, unmounted) still finds its log — both attempts look up the
/// same session id, so this widens only *where* the log is looked for, never
/// *which* session is shown.
pub fn session_jsonl_path(working_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let canonical =
        std::fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf());
    for dir in [canonical.as_path(), working_dir] {
        if let Some(project_dir) = projects_dir_for(dir)
            && let Some(path) = session_log_in_dir(&project_dir, session_id)
        {
            return Some(path);
        }
    }
    None
}

/// The log of exactly `session_id` inside an already-resolved Claude project
/// directory, or `None` when that session has no log there.
///
/// Deliberately ignores every sibling `.jsonl` in the directory. Siblings are
/// *different conversations* — other Conductor panels on the same worktree,
/// earlier runs, plain `claude` invocations — so picking one by mtime or by any
/// other directory-level heuristic shows the user someone else's history. When
/// the id does not resolve, the answer is "no history", not "some history".
pub fn session_log_in_dir(project_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let path = project_dir.join(format!("{session_id}.jsonl"));
    path.exists().then_some(path)
}

/// Return the Claude projects directory for a given working directory path.
/// E.g. `/Users/foo/project` → `~/.claude/projects/-Users-foo-project/`.
fn projects_dir_for(working_dir: &Path) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let encoded = encode_project_path(&working_dir.to_string_lossy());
    Some(home.join(".claude").join("projects").join(encoded))
}

fn format_time_ago(now_ms: u64, then_ms: u64) -> String {
    if now_ms <= then_ms {
        return "just now".to_string();
    }
    let diff_secs = (now_ms - then_ms) / 1000;
    if diff_secs < 60 {
        return "just now".to_string();
    }
    let diff_mins = diff_secs / 60;
    if diff_mins < 60 {
        return format!("{diff_mins}m ago");
    }
    let diff_hours = diff_mins / 60;
    if diff_hours < 24 {
        return format!("{diff_hours}h ago");
    }
    let diff_days = diff_hours / 24;
    if diff_days < 30 {
        return format!("{diff_days}d ago");
    }
    format!("{}mo ago", diff_days / 30)
}
