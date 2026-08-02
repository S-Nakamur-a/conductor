//! Claude Code セッションの発見。
//!
//! ~/.claude/history.jsonl を読んで resume 可能な Claude Code セッションを
//! 探す。履歴ファイルの各行は次の形の JSON オブジェクト:
//!   { "display": "...", "timestamp": ..., "project": "...", "sessionId": "..." }
//!
//! 共有のデータ型とパスヘルパーはここに置く。セッション一覧化は
//! [discovery]、grab/ungrab のセッションシンボリックリンクは [migrate] に
//! ある。

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

/// ~/.claude/history.jsonl の1エントリ。
#[derive(Debug, Clone, Deserialize)]
struct ClaudeHistoryEntry {
    display: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    timestamp: u64,
    project: String,
}

/// resume 可能な Claude セッションと、そこから導出した表示用情報。
#[derive(Debug, Clone)]
pub struct ResumableSession {
    pub session_id: String,
    /// 元のプロンプトのテキスト(セッション内の最後のユーザメッセージ)。
    pub display: String,
    /// 短い名前(パスの末尾コンポーネント)。
    pub project_name: String,
    /// 人が読める形の経過時間文字列(例: "3h ago")。
    pub time_ago: String,
}

/// Claude 履歴ファイルへのパスを返す。
fn history_file_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("history.jsonl"))
}

/// プロジェクトパスを Claude Code のプロジェクトディレクトリと同じ方式で
/// エンコードする。/ と . はすべて - に置き換わる。
/// 例: /Users/foo/github.com/proj → -Users-foo-github-com-proj。
fn encode_project_path(path: &str) -> String {
    path.replace(['/', '.'], "-")
}

/// 指定した session ID とプロジェクトについて、セッションの JSONL
/// ファイルが存在するか確認する。
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
/// 起点は pinned_session_id — 起動時に --session-id で決め打ちした、この
/// パネル自身の id。1 つのプロジェクトディレクトリにはそのワークツリーで走った
/// 全セッションのログが同居するので、解決をディレクトリ単位の条件 (最新の
/// ログ、など) に広げてはならない。それをやると別の会話を表示する
/// ([session_log_in_dir] 参照)。
///
/// 唯一の例外が /clear によるローテーション。/clear は Claude Code の
/// 書き込み先を別 id の .jsonl に移すので、pin した id だけを見ていると
/// clear 前の会話で止まる。後続と認める条件は [rotation] にあり、いずれも
/// 「このパネルのログの続き」であることを担保するもの。
///
/// * spawned_at — このパネルの Claude プロセスを起動した時刻。
/// * claimed — 他の Claude パネルが pin している session id。
///
/// working dir を先に canonicalize するのは、Claude Code が *解決後* の cwd を
/// エンコードしてディレクトリ名にするため。シンボリックリンク越しのワークツリー
/// パスではディレクトリを取り違える。生のパスも 2 番目の候補として試すので、
/// 解決できなくなったワークツリー (削除・アンマウント) でもログを見つけられる。
/// どちらも同じ session id を引くので、広がるのは「どこを探すか」だけで
/// 「どのセッションを見せるか」ではない。
///
/// 後続が無ければ pin した id のログを返す。pin した id にログが無ければ
/// None。
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

/// 解決済みの Claude プロジェクトディレクトリの中にある、ちょうど
/// session_id のログ。そのディレクトリにそのセッションのログが無ければ
/// None。
///
/// 同じディレクトリ内の他の .jsonl はすべて意図的に無視する。兄弟ファイルは
/// *別の会話* — 同じ worktree の別の Conductor パネル、以前の実行、素の
/// claude 起動など — なので、mtime やその他のディレクトリ単位のヒューリス
/// ティクスで1つを選ぶと、他人の履歴をユーザに見せてしまう。id が解決しない
/// ときの答えは「履歴あり(別のもの)」ではなく「履歴なし」であるべき。
pub fn session_log_in_dir(project_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let path = project_dir.join(format!("{session_id}.jsonl"));
    path.exists().then_some(path)
}

/// 指定した working directory に対応する Claude プロジェクトディレクトリを
/// 返す。例: /Users/foo/project → ~/.claude/projects/-Users-foo-project/。
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
