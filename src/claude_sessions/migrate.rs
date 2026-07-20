//! Session migration for grab/ungrab: symlinks a session into another
//! project directory so `claude --resume` finds it, and reverses that.

use std::path::Path;

use anyhow::{Context, Result};

use super::{history_file_path, projects_dir_for};

/// Migrate a Claude Code session from one project directory to another by
/// creating symlinks. This allows `claude --resume <id>` to find the session
/// when run from a different working directory (e.g. main worktree after grab).
///
/// Creates symlinks for:
///   - `<session_id>.jsonl` (the conversation log)
///   - `<session_id>/` directory (subagent data, if present)
///
/// Also appends an entry to `history.jsonl` so the session appears in the
/// resume list for the destination project.
///
/// Returns `Ok(true)` if migration was performed, `Ok(false)` if skipped
/// (e.g. session file not found), and `Err` on I/O failure.
pub fn migrate_session(
    session_id: &str,
    source_working_dir: &Path,
    dest_working_dir: &Path,
    display_hint: &str,
) -> Result<bool> {
    let src_dir = projects_dir_for(source_working_dir)
        .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    let dst_dir = projects_dir_for(dest_working_dir)
        .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;

    let session_file = format!("{session_id}.jsonl");
    let src_jsonl = src_dir.join(&session_file);
    if !src_jsonl.exists() {
        log::warn!(
            "migrate_session: source file not found: {}",
            src_jsonl.display()
        );
        return Ok(false);
    }

    // Ensure destination directory exists.
    if !dst_dir.exists() {
        std::fs::create_dir_all(&dst_dir)?;
    }

    // Symlink the .jsonl file.
    let dst_jsonl = dst_dir.join(&session_file);
    if !dst_jsonl.exists() {
        std::os::unix::fs::symlink(&src_jsonl, &dst_jsonl).with_context(|| {
            format!("symlink {} -> {}", dst_jsonl.display(), src_jsonl.display())
        })?;
        log::info!("migrate_session: symlinked {}", dst_jsonl.display());
    }

    // Symlink the subagent directory if it exists.
    let src_subdir = src_dir.join(session_id);
    if src_subdir.is_dir() {
        let dst_subdir = dst_dir.join(session_id);
        if !dst_subdir.exists() {
            std::os::unix::fs::symlink(&src_subdir, &dst_subdir).with_context(|| {
                format!(
                    "symlink {} -> {}",
                    dst_subdir.display(),
                    src_subdir.display()
                )
            })?;
            log::info!("migrate_session: symlinked subdir {}", dst_subdir.display());
        }
    }

    // Append a history entry so `claude --resume` lists this session
    // under the destination project.
    append_history_entry(session_id, dest_working_dir, display_hint)?;

    Ok(true)
}

/// Remove symlinks created by `migrate_session` and copy back any session
/// data that Claude Code may have written as real files (replacing the
/// original symlinks).
///
/// When Claude Code atomically writes session files (temp + rename), the
/// symlink is replaced with a real file.  In that case the latest
/// conversation lives only in the *destination* directory.  We copy it back
/// to the source so the session is complete when viewed from the original
/// worktree.
pub fn unmigrate_session(
    session_id: &str,
    source_working_dir: &Path,
    dest_working_dir: &Path,
) -> Result<()> {
    let dst_dir = match projects_dir_for(dest_working_dir) {
        Some(d) => d,
        None => return Ok(()),
    };

    let session_file = format!("{session_id}.jsonl");
    let dst_jsonl = dst_dir.join(&session_file);

    if let Ok(meta) = dst_jsonl.symlink_metadata() {
        if meta.file_type().is_symlink() {
            // Still a symlink — writes went through to the source file.
            std::fs::remove_file(&dst_jsonl)?;
            log::info!("unmigrate_session: removed symlink {}", dst_jsonl.display());
        } else {
            // Real file — Claude Code replaced the symlink.  Copy content
            // back to the source project directory, then remove.
            if let Some(src_dir) = projects_dir_for(source_working_dir) {
                let src_jsonl = src_dir.join(&session_file);
                std::fs::copy(&dst_jsonl, &src_jsonl).with_context(|| {
                    format!(
                        "copy back session {} -> {}",
                        dst_jsonl.display(),
                        src_jsonl.display(),
                    )
                })?;
                log::info!(
                    "unmigrate_session: copied back real file {} -> {}",
                    dst_jsonl.display(),
                    src_jsonl.display(),
                );
            }
            std::fs::remove_file(&dst_jsonl)?;
            log::info!(
                "unmigrate_session: removed real file {}",
                dst_jsonl.display()
            );
        }
    }

    // Handle subagent directory: symlink → remove, real dir → copy back.
    let dst_subdir = dst_dir.join(session_id);
    if let Ok(meta) = dst_subdir.symlink_metadata() {
        if meta.file_type().is_symlink() {
            std::fs::remove_file(&dst_subdir)?;
            log::info!(
                "unmigrate_session: removed symlink {}",
                dst_subdir.display()
            );
        } else if meta.is_dir() {
            if let Some(src_dir) = projects_dir_for(source_working_dir) {
                let src_subdir = src_dir.join(session_id);
                copy_dir_recursive(&dst_subdir, &src_subdir).with_context(|| {
                    format!(
                        "copy back subdir {} -> {}",
                        dst_subdir.display(),
                        src_subdir.display(),
                    )
                })?;
                log::info!(
                    "unmigrate_session: copied back real subdir {} -> {}",
                    dst_subdir.display(),
                    src_subdir.display(),
                );
            }
            std::fs::remove_dir_all(&dst_subdir)?;
            log::info!(
                "unmigrate_session: removed real subdir {}",
                dst_subdir.display()
            );
        }
    }

    Ok(())
}

/// Recursively copy a directory tree, merging into the destination.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

/// Append a single entry to `~/.claude/history.jsonl`.
fn append_history_entry(session_id: &str, project_path: &Path, display: &str) -> Result<()> {
    use std::io::Write;
    let history_path =
        history_file_path().ok_or_else(|| anyhow::anyhow!("cannot determine history file path"))?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let entry = serde_json::json!({
        "display": display,
        "sessionId": session_id,
        "timestamp": now_ms,
        "project": project_path.to_string_lossy(),
    });

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&history_path)?;
    writeln!(file, "{}", entry)?;

    Ok(())
}
