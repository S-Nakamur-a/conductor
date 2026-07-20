//! Smart Worktree generation for [`App`].
//!
//! "Smart Worktree" takes a free-form task description, asks the configured
//! LLM provider for a branch name / Claude Code prompt / session name, then
//! creates the worktree and auto-spawns Claude Code with the generated
//! prompt. Generation and creation both run on a single background thread;
//! progress is reported back via [`WorktreeOpResult`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

const SMART_WORKTREE_SYSTEM_PROMPT: &str = r#"You are a helper that generates a git branch name, a Claude Code prompt, and a session name from a task description.
Output ONLY a JSON object with three fields:
- "branch": a kebab-case branch name in English, 3-5 words, prefixed with "feature/", "fix/", or "refactor/" as appropriate.
- "prompt": a detailed, actionable prompt for Claude Code to implement the task. Write the prompt in the same language as the input description.
- "session_name": a short, descriptive session name (max 50 chars) for display in session lists. Write in the same language as the input description.
No markdown fences, no explanation, just the JSON object."#;

const SMART_WORKTREE_JSON_SCHEMA: &str = r#"{"type":"object","properties":{"branch":{"type":"string"},"prompt":{"type":"string"},"session_name":{"type":"string"}},"required":["branch","prompt","session_name"]}"#;

/// Parse LLM raw output into `SmartGenResult`, extracting JSON even if surrounded by text.
fn parse_smart_gen_result(raw: &str) -> Result<SmartGenResult, String> {
    // Strip markdown fences first.
    let stripped = raw
        .trim()
        .strip_prefix("```json")
        .or_else(|| raw.trim().strip_prefix("```"))
        .unwrap_or(raw.trim());
    let stripped = stripped.strip_suffix("```").unwrap_or(stripped).trim();

    // Try direct parse first.
    if let Ok(result) = serde_json::from_str::<SmartGenResult>(stripped) {
        return Ok(result);
    }

    // Fallback: find the first '{' and last '}' to extract a JSON object.
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

/// Run the LLM generation for smart worktree (branch name + prompt).
///
/// The provider is selected from `[api]` config and built via [`crate::ai_caller`];
/// this function owns the prompt, the output schema, and the parsing — the provider
/// only returns raw text. Checks `cancel_token` before and after the (blocking) call.
fn run_smart_generation(
    desc: &str,
    cancel_token: &Arc<AtomicBool>,
    api: &crate::config::ApiConfig,
) -> Result<SmartGenResult, String> {
    if cancel_token.load(Ordering::Relaxed) {
        return Err("Cancelled".to_string());
    }

    let caller =
        crate::ai_caller::build_caller(api, Some(SMART_WORKTREE_JSON_SCHEMA.to_string()))?;
    let raw = caller.complete(SMART_WORKTREE_SYSTEM_PROMPT, desc, cancel_token)?;

    if cancel_token.load(Ordering::Relaxed) {
        return Err("Cancelled".to_string());
    }

    parse_smart_gen_result(&raw)
}

impl App {
    /// Run LLM generation + worktree creation asynchronously in a single background thread.
    pub fn start_smart_worktree_async(&mut self, description: &str) {
        let desc = description.to_string();
        let main_branch = self.config.general.main_branch.clone();
        let repo_path = self.repo_path.clone();
        // Resolve to a ref that actually exists: origin/<main> if there is a
        // remote, otherwise the local <main> branch (or HEAD). Without this,
        // worktree creation fails with "invalid reference: origin/main" in a
        // local-only repo and the smart worktree never materializes.
        let base_ref = match git_engine::GitEngine::open(&repo_path) {
            Ok(engine) => engine.resolve_base_ref(&main_branch),
            Err(_) => format!("origin/{main_branch}"),
        };
        let wt_dir = self.config.general.worktree_dir.clone();

        let cancel_token = Arc::new(AtomicBool::new(false));

        // Add pending entry with empty branch (will be updated when LLM resolves).
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
                // Phase 1: LLM generation.
                let gen_result =
                    match run_smart_generation(&desc, &cancel, &api) {
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

                // Check cancellation before proceeding to Phase 2.
                if cancel.load(Ordering::Relaxed) {
                    let _ = tx.send(WorktreeOpResult::SmartFailed {
                        description: desc,
                        error: "Cancelled".to_string(),
                    });
                    return;
                }

                // Report branch resolved (for UI update).
                let _ = tx.send(WorktreeOpResult::SmartBranchResolved {
                    description: desc.clone(),
                    branch: branch.clone(),
                    prompt: prompt.clone(),
                    session_name: session_name.clone(),
                });

                // Phase 2: Create worktree.
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

    /// Cancel all pending smart worktree creations.
    ///
    /// Sets the cancel token so the background thread stops, and removes
    /// the pending entries from the list.
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

    #[test]
    fn test_parse_plain_json() {
        let raw = r#"{"branch": "feature/add-login", "prompt": "Add login page"}"#;
        let result = parse_smart_gen_result(raw).unwrap();
        assert_eq!(result.branch, "feature/add-login");
        assert_eq!(result.prompt, "Add login page");
    }

    #[test]
    fn test_parse_markdown_fenced_json() {
        let raw = "```json\n{\"branch\": \"fix/bug\", \"prompt\": \"Fix bug\"}\n```";
        let result = parse_smart_gen_result(raw).unwrap();
        assert_eq!(result.branch, "fix/bug");
    }

    #[test]
    fn test_parse_json_with_surrounding_text() {
        let raw = r#"Here is the result:
{"branch": "feature/smart-parse", "prompt": "Implement smart parsing"}
Hope this helps!"#;
        let result = parse_smart_gen_result(raw).unwrap();
        assert_eq!(result.branch, "feature/smart-parse");
        assert_eq!(result.prompt, "Implement smart parsing");
    }

    #[test]
    fn test_parse_json_with_preamble_only() {
        let raw = r#"Now I have full understanding. The result is: {"branch": "fix/json-parse", "prompt": "Fix JSON parsing"}"#;
        let result = parse_smart_gen_result(raw).unwrap();
        assert_eq!(result.branch, "fix/json-parse");
    }

    #[test]
    fn test_parse_no_json_returns_error() {
        let raw = "This has no JSON at all";
        assert!(parse_smart_gen_result(raw).is_err());
    }
}
