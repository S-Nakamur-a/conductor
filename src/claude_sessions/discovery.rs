//! resume 可能なセッションの一覧化: 履歴全体のスキャンと worktree ごとの
//! 最新セッション検索。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{
    ClaudeHistoryEntry, ResumableSession, format_time_ago, history_file_path, session_file_exists,
};

/// resume 可能な Claude セッションを、必要なら特定のプロジェクトパスで
/// 絞り込んですべて読み込む。タイムスタンプの降順(最新が先)で返す。
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

    // 有効なエントリをすべてパースする。
    let mut entries: Vec<ClaudeHistoryEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<ClaudeHistoryEntry>(line).ok())
        .filter(|e| !e.session_id.is_empty())
        .collect();

    // 重複排除のため、最新のエントリから処理できるよう逆順にする。
    entries.reverse();

    let mut sessions = Vec::new();
    for entry in entries {
        if seen_sessions.contains(&entry.session_id) {
            continue;
        }

        // 任意のプロジェクトフィルタ。
        if let Some(proj) = filter_project {
            let proj_str = proj.to_string_lossy();
            if entry.project != *proj_str {
                continue;
            }
        }

        // セッションファイルがまだディスク上に存在するか確認する。
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

    // 上の逆順化により、すでに新しい順に並んでいる。
    Ok(sessions)
}

/// 指定した worktree パスそれぞれについて、最も新しい resume 可能な
/// セッションを見つける。
///
/// history.jsonl を一度だけ読み、worktree パスからその最新の有効な
/// セッションへのマップを返す。JSONL ファイルがディスク上に現存する
/// セッションだけを含める。
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

    // 高速な検索のため、正規化済みパス文字列の集合を作る。
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

    // 有効なエントリをすべてパースする(ファイルは時系列順なので、最新は末尾)。
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

    // セッションファイルが実際に存在するエントリの中で、パスごとに最新の
    // ものを追跡する。存在確認をここで前もって行うのは、ゴーストエントリ
    // (セッションファイルは削除済みだが履歴エントリは残っている)が
    // 有効な古いセッションを覆い隠してしまわないようにするため。
    let mut best: HashMap<String, ClaudeHistoryEntry> = HashMap::new();
    let mut match_count = 0u32;
    let mut skipped_missing = 0u32;
    for entry in entries {
        // 比較のため、履歴エントリのプロジェクトパスを正規化する。
        let entry_path =
            std::fs::canonicalize(&entry.project).unwrap_or_else(|_| PathBuf::from(&entry.project));
        let entry_key = entry_path.to_string_lossy().to_string();

        if !path_strs.contains_key(&entry_key) {
            continue;
        }
        match_count += 1;

        // セッションファイルがディスク上にもう存在しないエントリはスキップする。
        if !session_file_exists(&entry.session_id, &entry.project) {
            skipped_missing += 1;
            continue;
        }

        // タイムスタンプが最も新しいエントリを保持する。
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

    // ResumableSession へ変換する。
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

// 注記: ここには意図的に「プロジェクトディレクトリのログを mtime で並べて
// 一覧する」ヘルパーを置いていない。reflow トランスクリプトビューは以前
// その方法でソースを選んでおり、他セッションの会話がビューに漏れ出して
// いた(App::open_reflow を参照)。トランスクリプトは代わりに
// current_session_log 経由で、パネル自身の session id から解決する。
// この関数もディレクトリを読むが、それは /clear によるローテーションを
// 追跡するためだけであり、対象は /clear の記録自体で始まるログに限る —
// super::rotation を参照。
