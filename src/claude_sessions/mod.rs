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

pub use discovery::{find_latest_sessions_for_paths, load_resumable_sessions, session_logs_by_mtime};
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
/// Mirrors the path that `session_file_exists` checks so callers can open
/// the file directly. Returns `None` if the home directory is unavailable.
pub fn session_jsonl_path(working_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let dir = projects_dir_for(working_dir)?;
    Some(dir.join(format!("{session_id}.jsonl")))
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
