//! Claude Code session discovery.
//!
//! Reads `~/.claude/history.jsonl` to find resumable Claude Code sessions.
//! Each line in the history file is a JSON object with:
//!   { "display": "...", "timestamp": ..., "project": "...", "sessionId": "..." }

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Deserialize;

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

/// Encode a project path the way Claude does for its project directories.
/// E.g. `/Users/foo/project` → `-Users-foo-project`.
fn encode_project_path(path: &str) -> String {
    path.replace('/', "-")
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

/// Load all resumable Claude sessions, optionally filtered to a specific project path.
/// Returns sessions sorted by timestamp descending (most recent first).
pub fn load_resumable_sessions(filter_project: Option<&Path>) -> Result<Vec<ResumableSession>> {
    let history_path = match history_file_path() {
        Some(p) if p.exists() => p,
        _ => return Ok(Vec::new()),
    };

    let content = std::fs::read_to_string(&history_path)?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut seen_sessions = std::collections::HashSet::new();

    // Parse all valid entries.
    let mut entries: Vec<ClaudeHistoryEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<ClaudeHistoryEntry>(line).ok())
        .filter(|e| !e.session_id.is_empty())
        .collect();

    // Reverse so we process most-recent entries first for deduplication.
    entries.reverse();

    let mut sessions = Vec::new();
    for entry in entries {
        if seen_sessions.contains(&entry.session_id) {
            continue;
        }

        // Optional project filter.
        if let Some(proj) = filter_project {
            let proj_str = proj.to_string_lossy();
            if entry.project != *proj_str {
                continue;
            }
        }

        // Verify the session file still exists on disk.
        if !session_file_exists(&entry.session_id, &entry.project) {
            continue;
        }

        seen_sessions.insert(entry.session_id.clone());

        let project_name = Path::new(&entry.project)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.project.clone());

        let time_ago = format_time_ago(now_ms, entry.timestamp);

        sessions.push(ResumableSession {
            session_id: entry.session_id,
            display: entry.display,
            project_name,
            time_ago,
            project_path: entry.project.clone(),
        });
    }

    // Already in reverse chronological order from the reversal above.
    Ok(sessions)
}

/// Find the most recent resumable session for each of the given worktree paths.
///
/// Reads `history.jsonl` once and returns a map from worktree path to its latest
/// valid session. Only sessions whose JSONL file still exists on disk are included.
pub fn find_latest_sessions_for_paths(paths: &[PathBuf]) -> Result<HashMap<PathBuf, ResumableSession>> {
    let history_path = match history_file_path() {
        Some(p) if p.exists() => p,
        _ => return Ok(HashMap::new()),
    };

    let content = std::fs::read_to_string(&history_path)?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Build a set of canonical path strings for fast lookup.
    let path_strs: HashMap<String, PathBuf> = paths
        .iter()
        .map(|p| {
            let canonical = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            (canonical.to_string_lossy().to_string(), canonical)
        })
        .collect();

    // Parse all valid entries, most recent last (file is in chronological order).
    let entries: Vec<ClaudeHistoryEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<ClaudeHistoryEntry>(line).ok())
        .filter(|e| !e.session_id.is_empty())
        .collect();

    // Track latest entry per path (later entries overwrite earlier ones).
    let mut best: HashMap<String, ClaudeHistoryEntry> = HashMap::new();
    for entry in entries {
        // Canonicalize the project path from the history entry for comparison.
        let entry_path = std::fs::canonicalize(&entry.project)
            .unwrap_or_else(|_| PathBuf::from(&entry.project));
        let entry_key = entry_path.to_string_lossy().to_string();

        if !path_strs.contains_key(&entry_key) {
            continue;
        }

        // Keep the entry with the highest timestamp.
        let dominated = best
            .get(&entry_key)
            .is_none_or(|prev| entry.timestamp >= prev.timestamp);
        if dominated {
            best.insert(entry_key, entry);
        }
    }

    // Convert to ResumableSession, validating that session files exist.
    let mut result = HashMap::new();
    for (key, entry) in best {
        if !session_file_exists(&entry.session_id, &entry.project) {
            continue;
        }

        let project_name = Path::new(&entry.project)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.project.clone());
        let time_ago = format_time_ago(now_ms, entry.timestamp);

        if let Some(canonical) = path_strs.get(&key) {
            result.insert(
                canonical.clone(),
                ResumableSession {
                    session_id: entry.session_id,
                    display: entry.display,
                    project_name,
                    time_ago,
                    project_path: entry.project,
                },
            );
        }
    }

    Ok(result)
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
