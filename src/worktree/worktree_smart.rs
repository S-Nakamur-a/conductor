//! [App] の Smart Worktree 生成。
//!
//! 「Smart Worktree」は自由記述のタスク説明を受け取り、設定済みの LLM プロバイダに
//! ブランチ名/Claude Code プロンプト/セッション名を問い合わせ、worktree を作成して
//! 生成されたプロンプトで Claude Code を自動起動する。生成と作成はどちらも単一の
//! バックグラウンドスレッド上で実行し、進捗は [WorktreeOpResult] 経由で報告される。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::app::*;
use crate::git_engine;

/// このタスクがモデルに他のタスクと異なる振る舞いを求める部分は、設定済み
/// コマンド側ではなく全てここに置く。
///
/// 「ツールもコマンドも使うな」という一文は、[api] command が(claude -p の
/// ような)*エージェント的* CLI のときに効いてくる: 放っておくとそうしたツールは
/// Bash に手を伸ばし会話的に答えてしまい、パースに失敗する。純粋な補完 API では
/// この一文は無害に無視されるので、ユーザにラッパーで書かせるのではなく
/// 無条件に書いておいて問題ない。
const SMART_WORKTREE_SYSTEM_PROMPT: &str = r#"You are a helper that generates a git branch name, a Claude Code prompt, and a session name from a task description.

IMPORTANT: Do not use any tools. Do not run any commands. Do not explain. Answer immediately from the description alone.

Output ONLY a JSON object with three fields:
- "branch": a kebab-case branch name in English, 3-5 words, prefixed with "feature/", "fix/", or "refactor/" as appropriate.
- "prompt": a detailed, actionable prompt for Claude Code to implement the task. Write the prompt in the same language as the input description.
- "session_name": a short, descriptive session name (max 50 chars) for display in session lists. Write in the same language as the input description.
No markdown fences, no explanation, just the JSON object."#;

/// LLM の生出力を SmartGenResult にパースする。テキストに囲まれていても JSON を抽出する。
fn parse_smart_gen_result(raw: &str) -> Result<SmartGenResult, String> {
    // まず markdown のフェンスを取り除く。
    let stripped = raw
        .trim()
        .strip_prefix("```json")
        .or_else(|| raw.trim().strip_prefix("```"))
        .unwrap_or(raw.trim());
    let stripped = stripped.strip_suffix("```").unwrap_or(stripped).trim();

    // まず直接パースを試す。
    if let Ok(result) = serde_json::from_str::<SmartGenResult>(stripped) {
        return Ok(result);
    }

    // フォールバック: 最初の '{' と最後の '}' を探して JSON オブジェクトを抽出する。
    if let Some(start) = stripped.find('{')
        && let Some(end) = stripped.rfind('}')
        && start < end
    {
        let json_str = &stripped[start..=end];
        if let Ok(result) = serde_json::from_str::<SmartGenResult>(json_str) {
            return Ok(result);
        }
    }

    Err(format!(
        "JSON parse error: could not extract valid JSON\nRaw output: {raw}"
    ))
}

/// 返信に使えそうな JSON が含まれないとき、何回まで問い合わせ直すか。
///
/// エージェント的な CLI は、プロンプトをどれだけ強く書いても、たまに会話的に
/// 答えてしまうことがあり、1回のフォーマット崩れで smart-worktree フロー全体を
/// 潰すべきではない。このタスクは数秒の純粋なテキスト生成なので、ここでのリトライは
/// 安く済む。数分かかるタスクなら、同じ判断にはならない。
const SMART_WORKTREE_ATTEMPTS: usize = 3;

/// プロンプト・リトライ方針・パースはこの関数が持ち、設定済みコマンドはモデルに過ぎない。
/// [api] の command_timeout_secs をそのまま使う (数秒のテキスト生成なので独自の予算は不要)。
fn run_smart_generation(
    desc: &str,
    cancel_token: &Arc<AtomicBool>,
    api: &crate::config::ApiConfig,
    repo_path: &std::path::Path,
) -> Result<SmartGenResult, String> {
    let env = crate::ai_caller::TaskEnv {
        timeout_secs: None,
        working_dir: Some(repo_path.to_path_buf()),
    };
    let caller = crate::ai_caller::build_caller(api, &env)?;

    let mut last_err = String::new();
    for attempt in 1..=SMART_WORKTREE_ATTEMPTS {
        if cancel_token.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        let raw = caller.complete(SMART_WORKTREE_SYSTEM_PROMPT, desc, cancel_token)?;
        if cancel_token.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        match parse_smart_gen_result(&raw) {
            Ok(result) => return Ok(result),
            Err(e) => {
                log::warn!("smart worktree: unusable reply on attempt {attempt}: {e}");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

impl App {
    /// 単一のバックグラウンドスレッドで LLM 生成 + worktree 作成を非同期に実行する。
    pub fn start_smart_worktree_async(&mut self, description: &str) {
        let desc = description.to_string();
        let main_branch = self.config.general.main_branch.clone();
        let repo_path = self.repo.path.clone();
        // 実際に存在する参照へ解決する: リモートがあれば origin/<main>、なければ
        // ローカルの <main> ブランチ(または HEAD)。これがないと、ローカルのみの
        // リポジトリで worktree 作成が "invalid reference: origin/main" で
        // 失敗し、smart worktree が一向にできあがらない。
        let base_ref = match git_engine::GitEngine::open(&repo_path) {
            Ok(engine) => engine.resolve_base_ref(&main_branch),
            Err(_) => format!("origin/{main_branch}"),
        };
        let wt_dir = self.config.general.worktree_dir.clone();

        let cancel_token = Arc::new(AtomicBool::new(false));

        // 空のブランチ名で保留エントリを追加する(LLM が解決したら更新される)。
        let pending = PendingWorktree {
            branch: String::new(),
            op: PendingWorktreeOp::SmartCreating,
            base_ref: base_ref.clone(),
            worktree_path: None,
            auto_spawn: true,
            smart_prompt: String::new(),
            session_name: None,
            delete_branch_after: false,
            description: desc.clone(),
            created_at: std::time::Instant::now(),
            cancel_token: cancel_token.clone(),
        };
        self.worktree_mgr.pending_worktrees.push(pending);
        self.set_status(
            "Smart worktree: generating... (Esc to cancel)".to_string(),
            StatusLevel::Info,
        );

        let tx = self.worktree_op_sender();
        let api = self.config.api.clone();

        let cancel = cancel_token;
        std::thread::spawn(move || {
            let tx_panic = tx.clone();
            let desc_panic = desc.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // フェーズ1: LLM生成。
                let gen_result = match run_smart_generation(&desc, &cancel, &api, &repo_path) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(WorktreeOpResult::SmartFailed {
                            description: desc,
                            error: e,
                        });
                        return;
                    }
                };

                if gen_result.branch.is_empty() {
                    let _ = tx.send(WorktreeOpResult::SmartFailed {
                        description: desc,
                        error: "LLM returned empty branch name".to_string(),
                    });
                    return;
                }

                let branch = gen_result.branch.clone();
                let prompt = gen_result.prompt.clone();
                let session_name = gen_result.session_name.clone();

                // フェーズ2へ進む前にキャンセルされていないか確認する。
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(WorktreeOpResult::SmartFailed {
                        description: desc,
                        error: "Cancelled".to_string(),
                    });
                    return;
                }

                // ブランチが解決したことを報告する(UI更新用)。
                let _ = tx.send(WorktreeOpResult::SmartBranchResolved {
                    description: desc.clone(),
                    branch: branch.clone(),
                    prompt: prompt.clone(),
                    session_name: session_name.clone(),
                });

                // フェーズ2: worktree を作成する。
                let pending = PendingWorktree {
                    branch: branch.clone(),
                    op: PendingWorktreeOp::SmartCreating,
                    base_ref: base_ref.clone(),
                    worktree_path: None,
                    auto_spawn: true,
                    smart_prompt: prompt,
                    session_name,
                    delete_branch_after: false,
                    description: desc,
                    created_at: std::time::Instant::now(),
                    cancel_token: cancel.clone(),
                };
                let result = git_engine::GitEngine::open(&repo_path).and_then(|engine| {
                    engine.create_worktree_from_base(&branch, &base_ref, wt_dir.as_deref())
                });
                let msg = match result {
                    Ok(path) => WorktreeOpResult::Created { path, pending },
                    Err(e) => WorktreeOpResult::CreateFailed {
                        error: format!("{e}"),
                        pending,
                    },
                };
                let _ = tx.send(msg);
            }));

            if result.is_err() {
                let _ = tx_panic.send(WorktreeOpResult::SmartFailed {
                    description: desc_panic,
                    error: "Smart worktree thread panicked".to_string(),
                });
            }
        });
    }

    /// 保留中の smart worktree 作成を全てキャンセルする。
    ///
    /// キャンセルトークンをセットしてバックグラウンドスレッドを停止させ、
    /// 保留エントリをリストから取り除く。
    pub fn cancel_smart_worktrees(&mut self) -> bool {
        let smart_pending: Vec<_> = self
            .worktree_mgr
            .pending_worktrees
            .iter()
            .filter(|p| p.op == PendingWorktreeOp::SmartCreating)
            .map(|p| p.cancel_token.clone())
            .collect();

        if smart_pending.is_empty() {
            return false;
        }

        for token in &smart_pending {
            token.store(true, Ordering::Relaxed);
        }

        self.worktree_mgr
            .pending_worktrees
            .retain(|p| p.op != PendingWorktreeOp::SmartCreating);

        self.set_status(
            "Worktree creation cancelled.".to_string(),
            StatusLevel::Info,
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// このタスクの制約はタスク自身に属するものであり、たまたまその時の
    /// [api] command が何かには依存しない: エージェント的な CLI は指示しない
    /// 限りツールに手を伸ばし会話してしまい、以前はその指示をユーザが保守する
    /// ラッパースクリプト側に書く必要があった。
    #[test]
    fn system_prompt_forbids_tools_and_demands_json() {
        assert!(SMART_WORKTREE_SYSTEM_PROMPT.contains("Do not use any tools"));
        assert!(SMART_WORKTREE_SYSTEM_PROMPT.contains("Do not run any commands"));
        assert!(SMART_WORKTREE_SYSTEM_PROMPT.contains("Output ONLY a JSON object"));
    }

    #[test]
    fn bare_json_parses() {
        let raw = r#"{"branch": "feature/add-login", "prompt": "Add login page"}"#;
        let result = parse_smart_gen_result(raw).unwrap();
        assert_eq!(result.branch, "feature/add-login");
        assert_eq!(result.prompt, "Add login page");
    }

    #[test]
    fn json_inside_a_markdown_fence_parses() {
        let raw = "```json\n{\"branch\": \"fix/bug\", \"prompt\": \"Fix bug\"}\n```";
        let result = parse_smart_gen_result(raw).unwrap();
        assert_eq!(result.branch, "fix/bug");
    }

    #[test]
    fn json_wrapped_in_prose_parses() {
        let raw = r#"Here is the result:
{"branch": "feature/smart-parse", "prompt": "Implement smart parsing"}
Hope this helps!"#;
        let result = parse_smart_gen_result(raw).unwrap();
        assert_eq!(result.branch, "feature/smart-parse");
        assert_eq!(result.prompt, "Implement smart parsing");
    }

    #[test]
    fn json_after_a_preamble_parses() {
        let raw = r#"Now I have full understanding. The result is: {"branch": "fix/json-parse", "prompt": "Fix JSON parsing"}"#;
        let result = parse_smart_gen_result(raw).unwrap();
        assert_eq!(result.branch, "fix/json-parse");
    }

    #[test]
    fn a_reply_with_no_json_at_all_is_an_error() {
        let raw = "This has no JSON at all";
        assert!(parse_smart_gen_result(raw).is_err());
    }
}
