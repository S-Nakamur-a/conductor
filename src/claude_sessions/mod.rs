//! Claude Code session discovery.
//!
//! Reads `~/.claude/history.jsonl` to find resumable Claude Code sessions.
//! Each line in the history file is a JSON object with:
//!   { "display": "...", "timestamp": ..., "project": "...", "sessionId": "..." }
//!
//! Shared data types and path helpers live here; session listing is in
//! [`discovery`] and grab/ungrab session symlinking is in [`migrate`].

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

mod discovery;
mod migrate;
mod rotation;
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

/// パネルがいま書き込んでいるセッションログのパス。
///
/// 起点は `pinned_session_id` — 起動時に `--session-id` で決め打ちした、この
/// パネル自身の id。1 つのプロジェクトディレクトリにはそのワークツリーで走った
/// 全セッションのログが同居するので、解決をディレクトリ単位の条件 (最新の
/// ログ、など) に広げてはならない。それをやると別の会話を表示する
/// ([`session_log_in_dir`] 参照)。
///
/// 唯一の例外が `/clear` によるローテーション。`/clear` は Claude Code の
/// 書き込み先を別 id の `.jsonl` に移すので、pin した id だけを見ていると
/// clear 前の会話で止まる。後続と認める条件は [`rotation`] にあり、いずれも
/// 「このパネルのログの続き」であることを担保するもの。
///
/// * `spawned_at` — このパネルの Claude プロセスを起動した時刻。
/// * `claimed` — 他の Claude パネルが pin している session id。
///
/// working dir を先に canonicalize するのは、Claude Code が *解決後* の cwd を
/// エンコードしてディレクトリ名にするため。シンボリックリンク越しのワークツリー
/// パスではディレクトリを取り違える。生のパスも 2 番目の候補として試すので、
/// 解決できなくなったワークツリー (削除・アンマウント) でもログを見つけられる。
/// どちらも同じ session id を引くので、広がるのは「どこを探すか」だけで
/// 「どのセッションを見せるか」ではない。
///
/// 後続が無ければ pin した id のログを返す。pin した id にログが無ければ
/// `None`。
pub fn current_session_log(
    working_dir: &Path,
    pinned_session_id: &str,
    spawned_at: SystemTime,
    claimed: &HashSet<String>,
) -> Option<PathBuf> {
    let canonical =
        std::fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf());
    for dir in [canonical.as_path(), working_dir] {
        let Some(project_dir) = projects_dir_for(dir) else {
            continue;
        };
        let Some(pinned_path) = session_log_in_dir(&project_dir, pinned_session_id) else {
            continue;
        };
        let current = rotation::resolve_current_session_id(
            &project_dir,
            pinned_session_id,
            spawned_at,
            claimed,
        );
        return session_log_in_dir(&project_dir, &current).or(Some(pinned_path));
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
