//! Claude Code のセッションの発見と、パネルを裏打ちする .jsonl の解決。
//!
//! ~/.claude/history.jsonl の各行は { "display", "timestamp", "project", "sessionId" }。
//! 会話ログは ~/.claude/projects/<cwd の / と . を - にした名前>/<session id>.jsonl で、
//! 1 つのディレクトリにそのワークツリーで走った全セッションが同居する。
//! セッション一覧は [discovery]、grab/ungrab のシンボリックリンクは [migrate]。

use std::path::{Path, PathBuf};

use serde::Deserialize;

mod discovery;
mod migrate;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Deserialize)]
struct HistoryEntry {
    display: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    timestamp: u64,
    project: String,
}

/// resume 可能なセッションと、一覧表示用に導出した情報。
#[derive(Debug, Clone)]
pub struct ResumableSession {
    pub session_id: String,
    /// セッション内の最後のユーザプロンプト。
    pub display: String,
    /// プロジェクトパスの末尾。
    pub project_name: String,
    /// "3h ago" の形。
    pub time_ago: String,
}

/// ~/.claude。
#[derive(Debug, Clone)]
pub struct ClaudeHome {
    root: PathBuf,
}

impl ClaudeHome {
    pub fn detect() -> Option<Self> {
        dirs::home_dir().map(|home| Self::at(home.join(".claude")))
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    fn history_file(&self) -> PathBuf {
        self.root.join("history.jsonl")
    }

    pub fn projects_dir_for(&self, working_dir: &Path) -> PathBuf {
        self.root
            .join("projects")
            .join(encode_project_path(&working_dir.to_string_lossy()))
    }

    fn session_file_exists(&self, entry: &HistoryEntry) -> bool {
        session_log_in_dir(
            &self.projects_dir_for(Path::new(&entry.project)),
            &entry.session_id,
        )
        .is_some()
    }

    /// パネルが書き込んでいるセッションログ。無ければ None。
    ///
    /// Claude Code は解決後の cwd でディレクトリ名を作るので canonicalize してから探し、
    /// 解決できなくなったワークツリー (削除・アンマウント) のために生のパスも試す。
    /// 広がるのは「どこを探すか」だけで、どの id を見せるかは変わらない。
    pub fn session_log(&self, working_dir: &Path, session_id: &str) -> Option<PathBuf> {
        let canonical =
            std::fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf());
        [canonical.as_path(), working_dir]
            .into_iter()
            .find_map(|dir| session_log_in_dir(&self.projects_dir_for(dir), session_id))
    }
}

/// project_dir の中の、ちょうど session_id のログ。
///
/// 兄弟の .jsonl は別の会話 (同じワークツリーの別パネル、以前の実行、素の claude) なので、
/// mtime などで 1 つを選ぶと他人の履歴を見せる。答えは「履歴なし」であるべき。
fn session_log_in_dir(project_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let path = project_dir.join(format!("{session_id}.jsonl"));
    path.exists().then_some(path)
}

fn encode_project_path(path: &str) -> String {
    path.replace(['/', '.'], "-")
}

fn format_time_ago(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    let (mins, hours, days) = (secs / 60, secs / 3600, secs / 86400);
    match () {
        _ if mins == 0 => "just now".to_string(),
        _ if hours == 0 => format!("{mins}m ago"),
        _ if days == 0 => format!("{hours}h ago"),
        _ if days < 30 => format!("{days}d ago"),
        _ => format!("{}mo ago", days / 30),
    }
}
