//! Listing resumable sessions: full history scan and per-worktree
//! latest-session lookup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    ClaudeHistoryEntry, ResumableSession, format_time_ago, history_file_path, session_file_exists,
};

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
pub fn find_latest_sessions_for_paths(
    paths: &[PathBuf],
) -> Result<HashMap<PathBuf, ResumableSession>> {
    let history_path = match history_file_path() {
        Some(p) if p.exists() => p,
        _ => {
            log::debug!("find_latest_sessions: history file not found");
            return Ok(HashMap::new());
        }
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
            log::info!(
                "find_latest_sessions: lookup key: raw={} canonical={}",
                p.display(),
                canonical.display()
            );
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
    log::info!(
        "find_latest_sessions: total history entries={}",
        entries.len()
    );

    // Track latest entry per path whose session file actually exists.
    // We validate existence eagerly so that ghost entries (session file
    // deleted but history entry remains) don't shadow valid older sessions.
    let mut best: HashMap<String, ClaudeHistoryEntry> = HashMap::new();
    let mut match_count = 0u32;
    let mut skipped_missing = 0u32;
    for entry in entries {
        // Canonicalize the project path from the history entry for comparison.
        let entry_path =
            std::fs::canonicalize(&entry.project).unwrap_or_else(|_| PathBuf::from(&entry.project));
        let entry_key = entry_path.to_string_lossy().to_string();

        if !path_strs.contains_key(&entry_key) {
            continue;
        }
        match_count += 1;

        // Skip entries whose session file no longer exists on disk.
        if !session_file_exists(&entry.session_id, &entry.project) {
            skipped_missing += 1;
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
    log::info!(
        "find_latest_sessions: matched {} history entries, {} skipped (file missing), {} valid unique paths",
        match_count,
        skipped_missing,
        best.len()
    );

    // Convert to ResumableSession.
    let mut result = HashMap::new();
    for (key, entry) in best {
        let project_name = Path::new(&entry.project)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.project.clone());
        let time_ago = format_time_ago(now_ms, entry.timestamp);

        if let Some(canonical) = path_strs.get(&key) {
            log::info!(
                "find_latest_sessions: found session id={} for path={}",
                entry.session_id,
                key
            );
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

// NOTE: there is deliberately no "list the project dir's logs by mtime" helper
// here. The reflow transcript view used to select its source that way and it
// leaked other sessions' conversations into the view (see `App::open_reflow`);
// a transcript is resolved from the panel's own session id via
// `session_jsonl_path` instead.
