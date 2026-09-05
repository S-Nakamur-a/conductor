//! grab/ungrab のためのセッション移行。別の working directory から claude --resume <id> が
//! 見つけられるよう、セッションをそちらのプロジェクトディレクトリへシンボリックリンクする。

use std::path::Path;

use anyhow::{Context, Result};

use super::ClaudeHome;
use super::discovery::now_ms;

impl ClaudeHome {
    /// <session_id>.jsonl と、あればサブエージェントの <session_id>/ を dest 側へリンクし、
    /// dest の resume 一覧に出るよう history.jsonl にも追記する。
    /// 移行元にログが無ければ何もせず Ok(false)。
    pub fn migrate_session(
        &self,
        session_id: &str,
        source_working_dir: &Path,
        dest_working_dir: &Path,
        display_hint: &str,
    ) -> Result<bool> {
        let src_dir = self.projects_dir_for(source_working_dir);
        let dst_dir = self.projects_dir_for(dest_working_dir);
        let log_name = format!("{session_id}.jsonl");
        if !src_dir.join(&log_name).exists() {
            log::warn!(
                "migrate_session: source file not found: {}",
                src_dir.join(&log_name).display()
            );
            return Ok(false);
        }
        std::fs::create_dir_all(&dst_dir)?;
        link(&src_dir.join(&log_name), &dst_dir.join(&log_name))?;
        if src_dir.join(session_id).is_dir() {
            link(&src_dir.join(session_id), &dst_dir.join(session_id))?;
        }
        self.append_history_entry(session_id, dest_working_dir, display_hint)?;
        Ok(true)
    }

    /// migrate_session が作ったリンクを外す。
    ///
    /// Claude Code はセッションファイルを一時ファイル + rename で書くので、リンクは実体に
    /// 置き換わっていることがある。そのとき最新の会話は dest にしか無いので、source へ
    /// コピーして戻してから消す。
    pub fn unmigrate_session(
        &self,
        session_id: &str,
        source_working_dir: &Path,
        dest_working_dir: &Path,
    ) -> Result<()> {
        let src_dir = self.projects_dir_for(source_working_dir);
        let dst_dir = self.projects_dir_for(dest_working_dir);
        let log_name = format!("{session_id}.jsonl");
        take_back(&dst_dir.join(&log_name), &src_dir.join(&log_name))?;
        take_back(&dst_dir.join(session_id), &src_dir.join(session_id))
    }

    fn append_history_entry(
        &self,
        session_id: &str,
        project_path: &Path,
        display: &str,
    ) -> Result<()> {
        use std::io::Write;
        let entry = serde_json::json!({
            "display": display,
            "sessionId": session_id,
            "timestamp": now_ms(),
            "project": project_path.to_string_lossy(),
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.history_file())?;
        writeln!(file, "{entry}")?;
        Ok(())
    }
}

fn link(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        return Ok(());
    }
    std::os::unix::fs::symlink(src, dst)
        .with_context(|| format!("symlink {} -> {}", dst.display(), src.display()))?;
    log::info!("migrate_session: symlinked {}", dst.display());
    Ok(())
}

/// dst がリンクなら外すだけ。実体なら src へコピーしてから消す。無ければ何もしない。
fn take_back(dst: &Path, src: &Path) -> Result<()> {
    let Ok(meta) = dst.symlink_metadata() else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        std::fs::remove_file(dst)?;
    } else if meta.is_dir() {
        copy_dir_recursive(dst, src)
            .with_context(|| format!("copy back {} -> {}", dst.display(), src.display()))?;
        std::fs::remove_dir_all(dst)?;
    } else {
        std::fs::copy(dst, src)
            .with_context(|| format!("copy back {} -> {}", dst.display(), src.display()))?;
        std::fs::remove_file(dst)?;
    }
    log::info!("unmigrate_session: removed {}", dst.display());
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}
