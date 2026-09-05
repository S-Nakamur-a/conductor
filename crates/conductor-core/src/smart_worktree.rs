//! 自由記述のタスクから、ブランチ名・Claude Code へのプロンプト・セッション名を作る。
//!
//! worktree を作るのも Claude を起こすのも呼び出し側の仕事で、ここは AI に訊いて
//! [Plan] にするところまでを持つ。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Deserialize;

use crate::ai_caller::{self, TaskEnv};
use crate::config::ApiConfig;

/// ツールを禁じる一文は、[api] command が claude -p のようなエージェント型 CLI の
/// ときに効く。放っておくと Bash に手を伸ばして会話的に答え、パースに失敗する。
/// 素の補完 API は黙って無視するので、ユーザにラッパーを書かせずここに置ける。
pub const SYSTEM_PROMPT: &str = r#"You are a helper that generates a git branch name, a Claude Code prompt, and a session name from a task description.

IMPORTANT: Do not use any tools. Do not run any commands. Do not explain. Answer immediately from the description alone.

Output ONLY a JSON object with three fields:
- "branch": a kebab-case branch name in English, 3-5 words, prefixed with "feature/", "fix/", or "refactor/" as appropriate.
- "prompt": a detailed, actionable prompt for Claude Code to implement the task. Write the prompt in the same language as the input description.
- "session_name": a short, descriptive session name (max 50 chars) for display in session lists. Write in the same language as the input description.
No markdown fences, no explanation, just the JSON object."#;

/// AI が答えた 1 件分。
#[derive(Debug, Clone, Deserialize)]
pub struct Plan {
    pub branch: String,
    pub prompt: String,
    #[serde(default)]
    pub session_name: Option<String>,
}

/// エージェント型 CLI はどれだけ強く書いても時折会話で答えるが、これは数秒の
/// テキスト生成なので訊き直しは安い。1 回の形式崩れで作成まるごとを潰さない。
const ATTEMPTS: usize = 3;

/// 設定された AI に訊いて [Plan] にする。[api] の command_timeout_secs をそのまま使う。
pub fn generate(description: &str, api: &ApiConfig, working_dir: &Path) -> Result<Plan, String> {
    let env = TaskEnv {
        timeout_secs: None,
        working_dir: Some(working_dir.to_path_buf()),
    };
    let caller = ai_caller::build_caller(api, &env)?;
    let cancel = Arc::new(AtomicBool::new(false));

    let mut last_err = String::new();
    for attempt in 1..=ATTEMPTS {
        let raw = caller.complete(SYSTEM_PROMPT, description, &cancel)?;
        match parse(&raw) {
            Ok(plan) => return Ok(plan),
            Err(e) => {
                log::warn!("smart worktree: unusable reply on attempt {attempt}: {e}");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

/// 生の応答から [Plan] を取り出す。地の文に包まれていても JSON だけを拾う。
pub fn parse(raw: &str) -> Result<Plan, String> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let stripped = stripped.strip_suffix("```").unwrap_or(stripped).trim();

    if let Ok(plan) = serde_json::from_str::<Plan>(stripped) {
        return Ok(plan);
    }
    if let Some(start) = stripped.find('{')
        && let Some(end) = stripped.rfind('}')
        && start < end
        && let Ok(plan) = serde_json::from_str::<Plan>(&stripped[start..=end])
    {
        return Ok(plan);
    }
    Err(format!(
        "JSON parse error: could not extract valid JSON\nRaw output: {raw}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// この制約はタスクに属するもので、そのときの [api] command が何かに依らない。
    /// 以前はユーザが保守するラッパースクリプト側に書く必要があった。
    #[test]
    fn システムプロンプトはツールを禁じjsonを求める() {
        assert!(SYSTEM_PROMPT.contains("Do not use any tools"));
        assert!(SYSTEM_PROMPT.contains("Do not run any commands"));
        assert!(SYSTEM_PROMPT.contains("Output ONLY a JSON object"));
    }

    #[test]
    fn 素のjsonを読める() {
        let raw = r#"{"branch": "feature/add-login", "prompt": "Add login page"}"#;
        let plan = parse(raw).unwrap();
        assert_eq!(plan.branch, "feature/add-login");
        assert_eq!(plan.prompt, "Add login page");
    }

    #[test]
    fn コードフェンスの中のjsonを読める() {
        let raw = "```json\n{\"branch\": \"fix/bug\", \"prompt\": \"Fix bug\"}\n```";
        assert_eq!(parse(raw).unwrap().branch, "fix/bug");
    }

    #[test]
    fn 地の文に包まれたjsonを読める() {
        let raw = r#"Here is the result:
{"branch": "feature/smart-parse", "prompt": "Implement smart parsing"}
Hope this helps!"#;
        let plan = parse(raw).unwrap();
        assert_eq!(plan.branch, "feature/smart-parse");
        assert_eq!(plan.prompt, "Implement smart parsing");
    }

    #[test]
    fn 前置きの後ろのjsonを読める() {
        let raw = r#"Now I have full understanding. The result is: {"branch": "fix/json-parse", "prompt": "Fix JSON parsing"}"#;
        assert_eq!(parse(raw).unwrap().branch, "fix/json-parse");
    }

    #[test]
    fn jsonが1つも無い応答はエラー() {
        assert!(parse("This has no JSON at all").is_err());
    }
}
