//! classify が知らなかったツール名を `.conductor/unknown-tools.log` に書き残す。
//!
//! トランスクリプトは開くたびに読み直されるので、素直に毎回書くと同じ行が延々積まれる。

use std::collections::{BTreeSet, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use conductor_core::claude_log::{DisplayBlock, LogEntry, ToolCategory, classify};
use conductor_core::git_engine::conductor_dir;

const FILE_NAME: &str = "unknown-tools.log";
const MAX_NAME_BYTES: usize = 200;
/// `semantic_index::history` と同じ上限。リポジトリに置きっぱなしになるので際限なく伸ばさない。
const MAX_BYTES: u64 = 512 * 1024;

fn seen_names() -> &'static Mutex<HashSet<String>> {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashSet::new()))
}

/// `mcp__` 始まりは対象にしない — MCP ツールは名前が利用者ごとに任意で、列挙できないから。
pub(crate) fn record(working_dir: &Path, entries: &[LogEntry]) {
    // 書けなかった名前を seen に入れると二度と書かれないので、ロックを持ったまま書く。
    let mut seen = seen_names().lock().unwrap_or_else(|e| e.into_inner());
    let fresh: BTreeSet<String> = entries
        .iter()
        .flat_map(|entry| &entry.blocks)
        .filter_map(|block| match block {
            DisplayBlock::ToolUse { name, input, .. } => {
                matches!(classify(name, input), ToolCategory::Unknown { .. }).then_some(name)
            }
            _ => None,
        })
        .filter(|name| !name.starts_with("mcp__") && loggable(name))
        .filter(|name| !seen.contains(*name))
        .cloned()
        .collect();

    if !fresh.is_empty() && append(&conductor_dir(working_dir), &fresh) {
        seen.extend(fresh);
    }
}

/// 改行や制御文字を含む名前は、1 行 1 件の書式を壊して偽の行を作れてしまう。
fn loggable(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME_BYTES && !name.chars().any(char::is_control)
}

/// `.conductor` が無ければ諦める。作りに行くと、消えた worktree のディレクトリを
/// 復活させて `git worktree add` を塞ぐ (conductor_dir は解決に失敗すると引数を返す)。
fn append(dir: &Path, names: &BTreeSet<String>) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let path = dir.join(FILE_NAME);
    truncate_if_large(&path);

    let stamp = chrono::Utc::now().to_rfc3339();
    let mut body = String::new();
    for name in names {
        let _ = writeln!(body, "{stamp} tool={name}");
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(body.as_bytes()))
        .is_ok()
}

fn truncate_if_large(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_BYTES) {
        return;
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = content.lines().collect();
    let kept = lines[lines.len() / 2..].join("\n");
    let _ = std::fs::write(path, format!("{kept}\n"));
}

#[cfg(test)]
mod tests {
    use conductor_core::claude_log::Role;
    use serde_json::json;

    use super::*;

    fn tool_use_entry(name: &str) -> LogEntry {
        LogEntry {
            role: Role::Assistant,
            blocks: vec![DisplayBlock::ToolUse {
                name: name.to_string(),
                input: json!({}),
                errored: false,
            }],
        }
    }

    fn log_lines(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join(".conductor").join(FILE_NAME))
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default()
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".conductor")).unwrap();
        dir
    }

    // seen_names() はプロセス全体で共有する static なので、テスト間でツール名を
    // 使い回さない。
    #[test]
    fn 未知のツールは初回だけ書く() {
        let dir = repo();
        let entry = tool_use_entry("初回だけ書くツール");

        record(dir.path(), std::slice::from_ref(&entry));
        record(dir.path(), std::slice::from_ref(&entry));

        assert_eq!(log_lines(dir.path()).len(), 1);
    }

    #[test]
    fn mcp_ツールは書かない() {
        let dir = repo();
        let entry = tool_use_entry("mcp__conductor__search_symbols");

        record(dir.path(), &[entry]);

        assert!(log_lines(dir.path()).is_empty());
    }

    #[test]
    fn 既知のツールは書かない() {
        let dir = repo();
        let entry = tool_use_entry("Read");

        record(dir.path(), &[entry]);

        assert!(log_lines(dir.path()).is_empty());
    }

    #[test]
    fn conductorディレクトリが無ければ作らない() {
        let dir = tempfile::tempdir().unwrap();
        let entry = tool_use_entry("ディレクトリを作らないツール");

        record(dir.path(), &[entry]);

        assert!(
            !dir.path().join(".conductor").exists(),
            "消えた worktree のディレクトリを復活させると git worktree add が塞がる"
        );
    }

    #[test]
    fn 制御文字を含む名前は書かない() {
        let dir = repo();
        let entry = tool_use_entry("偽造ツール\n2000-01-01T00:00:00+00:00 tool=Forged");

        record(dir.path(), &[entry]);

        assert!(
            log_lines(dir.path()).is_empty(),
            "1 行 1 件の書式を壊せてしまう"
        );
    }
}
