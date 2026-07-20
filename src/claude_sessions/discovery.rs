//! Listing resumable sessions: full history scan, per-worktree latest-session
//! lookup, and mtime-ordered session logs for the reflow transcript view.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    ClaudeHistoryEntry, ResumableSession, format_time_ago, history_file_path, projects_dir_for,
    session_file_exists,
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

/// List the session `.jsonl` files in a worktree's Claude project directory,
/// most recently modified first.
///
/// This is the basis for the reflow transcript view's session selection: the
/// file Claude is *currently appending to* always has the freshest mtime, so
/// picking by mtime tracks whatever session the live pane shows — including a
/// session the user switched to with a manual `/resume` (which can mint a new
/// session ID). That is more reliable than re-deriving the session from
/// `history.jsonl`, whose newest entry can point at an unrelated auxiliary
/// session (e.g. a one-shot security review run in the same directory) or at a
/// freshly-spawned-but-empty session.
///
/// The working dir is canonicalized first because Claude Code encodes its
/// *resolved* cwd; symlinked worktree paths would otherwise miss the directory.
/// Symlinked session files (created by `migrate_session` for grabbed branches)
/// are followed via `metadata()`; dangling or unreadable entries are skipped.
pub fn session_logs_by_mtime(working_dir: &Path) -> Vec<PathBuf> {
    let canonical = std::fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf());
    let dir = match projects_dir_for(&canonical) {
        Some(d) => d,
        None => return Vec::new(),
    };

    let read_dir = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };

    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        // metadata() follows symlinks, so a migrated session resolves to its
        // real file's mtime; a dangling symlink errors out and is skipped.
        if let Ok(meta) = std::fs::metadata(&path)
            && let Ok(mtime) = meta.modified()
        {
            files.push((mtime, path));
        }
    }

    files.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    files.into_iter().map(|(_, p)| p).collect()
}
