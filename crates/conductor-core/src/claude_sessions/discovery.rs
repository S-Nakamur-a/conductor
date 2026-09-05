//! resume 可能なセッションの一覧化。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{ClaudeHome, HistoryEntry, ResumableSession, format_time_ago};

impl ClaudeHome {
    /// history.jsonl の有効なエントリを、ファイルの順 (時系列) で返す。ファイルが無ければ空。
    fn read_history(&self) -> Result<Vec<HistoryEntry>> {
        let path = self.history_file();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&path)?;
        Ok(content
            .lines()
            .filter_map(|line| serde_json::from_str::<HistoryEntry>(line).ok())
            .filter(|e| !e.session_id.is_empty())
            .collect())
    }

    /// resume 可能なセッションを新しい順に返す。filter_project は history の project との完全一致。
    /// ログがディスクに残っているものだけを含める。
    pub fn load_resumable_sessions(
        &self,
        filter_project: Option<&Path>,
    ) -> Result<Vec<ResumableSession>> {
        let now = now_ms();
        let filter = filter_project.map(|p| p.to_string_lossy().to_string());
        let mut seen = HashSet::new();
        let sessions = self
            .read_history()?
            .into_iter()
            .rev()
            .filter(|e| filter.as_ref().is_none_or(|p| &e.project == p))
            .filter(|e| seen.insert(e.session_id.clone()))
            .filter(|e| self.session_file_exists(e))
            .map(|e| ResumableSession::from_entry(e, now))
            .collect();
        Ok(sessions)
    }

    /// 各ワークツリーについて最新の resume 可能なセッション。キーは canonicalize したパス。
    ///
    /// ログが消えたエントリを先に除くのは、それが残っている古い有効なセッションを
    /// 覆い隠さないため。
    pub fn find_latest_sessions_for_paths(
        &self,
        paths: &[PathBuf],
    ) -> Result<HashMap<PathBuf, ResumableSession>> {
        let now = now_ms();
        let wanted: HashSet<PathBuf> = paths.iter().map(|p| canonical(p)).collect();
        let mut best: HashMap<PathBuf, HistoryEntry> = HashMap::new();
        for entry in self.read_history()? {
            let key = canonical(Path::new(&entry.project));
            if !wanted.contains(&key) || !self.session_file_exists(&entry) {
                continue;
            }
            if best
                .get(&key)
                .is_none_or(|prev| entry.timestamp >= prev.timestamp)
            {
                best.insert(key, entry);
            }
        }
        log::debug!(
            "find_latest_sessions: {} of {} paths have a session",
            best.len(),
            wanted.len()
        );
        Ok(best
            .into_iter()
            .map(|(key, entry)| (key, ResumableSession::from_entry(entry, now)))
            .collect())
    }
}

impl ResumableSession {
    fn from_entry(entry: HistoryEntry, now_ms: u64) -> Self {
        let project_name = Path::new(&entry.project)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| entry.project.clone());
        Self {
            session_id: entry.session_id,
            display: entry.display,
            project_name,
            time_ago: format_time_ago(now_ms, entry.timestamp),
        }
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
