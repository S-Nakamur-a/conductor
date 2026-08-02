//! grab/ungrab のためのセッション移行: あるセッションを別のプロジェクト
//! ディレクトリへシンボリックリンクして claude --resume から見つかるようにし、
//! またそれを元に戻す。

use std::path::Path;

use anyhow::{Context, Result};

use super::{history_file_path, projects_dir_for};

/// シンボリックリンクを作ることで、Claude Code のセッションをあるプロジェクト
/// ディレクトリから別のディレクトリへ移す。これにより、別の working
/// directory から実行した claude --resume <id> でもそのセッションが見つかる
/// ようになる(例: grab 後のメイン worktree)。
///
/// 以下にシンボリックリンクを作る:
///   - <session_id>.jsonl (会話ログ)
///   - <session_id>/ ディレクトリ (サブエージェントのデータ、存在すれば)
///
/// history.jsonl にもエントリを追記し、移行先プロジェクトの resume 一覧に
/// このセッションが現れるようにする。
///
/// 移行を実行した場合は Ok(true)、スキップした場合(セッションファイルが
/// 見つからない、など)は Ok(false)、I/O 失敗時は Err を返す。
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

    // 移行先ディレクトリが存在することを保証する。
    if !dst_dir.exists() {
        std::fs::create_dir_all(&dst_dir)?;
    }

    // .jsonl ファイルをシンボリックリンクする。
    let dst_jsonl = dst_dir.join(&session_file);
    if !dst_jsonl.exists() {
        std::os::unix::fs::symlink(&src_jsonl, &dst_jsonl).with_context(|| {
            format!("symlink {} -> {}", dst_jsonl.display(), src_jsonl.display())
        })?;
        log::info!("migrate_session: symlinked {}", dst_jsonl.display());
    }

    // サブエージェントのディレクトリが存在すればシンボリックリンクする。
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

    // claude --resume が移行先プロジェクト配下にこのセッションを一覧
    // できるよう、履歴エントリを追記する。
    append_history_entry(session_id, dest_working_dir, display_hint)?;

    Ok(true)
}

/// migrate_session が作ったシンボリックリンクを取り除き、Claude Code が
/// (元のシンボリックリンクを置き換えて)実ファイルとして書き込んだかも
/// しれないセッションデータをコピーして戻す。
///
/// Claude Code がセッションファイルをアトミックに書く場合(一時ファイル+
/// リネーム)、シンボリックリンクは実ファイルに置き換わる。その場合、
/// 最新の会話は移行先ディレクトリにしか存在しない。元の worktree から
/// 見たときにセッションが揃うよう、それを移行元へコピーし戻す。
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
            // まだシンボリックリンクのまま — 書き込みは移行元ファイルへ
            // そのまま通っている。
            std::fs::remove_file(&dst_jsonl)?;
            log::info!("unmigrate_session: removed symlink {}", dst_jsonl.display());
        } else {
            // 実ファイル — Claude Code がシンボリックリンクを置き換えた。
            // 内容を移行元プロジェクトディレクトリへコピーし戻してから削除する。
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

    // サブエージェントのディレクトリを処理する: シンボリックリンクなら削除、
    // 実ディレクトリならコピーし戻す。
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

/// ディレクトリツリーを再帰的にコピーし、移行先へマージする。
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

/// ~/.claude/history.jsonl にエントリを1件追記する。
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
