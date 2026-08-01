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

/// Everything this task needs the model to do differently from other tasks
/// lives here, not in the configured command.
///
/// The "no tools, no commands" line matters when `[api] command` is an *agentic*
/// CLI (`claude -p` and the like): left to itself such a tool reaches for Bash
/// and answers conversationally, which fails the parse. A pure completion API
/// ignores the line harmlessly, so it is safe to state unconditionally rather
/// than having the user encode it in a wrapper.
const SMART_WORKTREE_SYSTEM_PROMPT: &str = r#"You are a helper that generates a git branch name, a Claude Code prompt, and a session name from a task description.

IMPORTANT: Do not use any tools. Do not run any commands. Do not explain. Answer immediately from the description alone.

Output ONLY a JSON object with three fields:
- "branch": a kebab-case branch name in English, 3-5 words, prefixed with "feature/", "fix/", or "refactor/" as appropriate.
- "prompt": a detailed, actionable prompt for Claude Code to implement the task. Write the prompt in the same language as the input description.
- "session_name": a short, descriptive session name (max 50 chars) for display in session lists. Write in the same language as the input description.
No markdown fences, no explanation, just the JSON object."#;

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

/// How many times to ask again when the reply contains no usable JSON.
///
/// An agentic CLI occasionally answers conversationally however firmly the
/// prompt is worded, and one off-format turn should not sink the whole
/// smart-worktree flow. Cheap to retry here — this task is seconds of pure text
/// generation, which is exactly why walkthrough generation does *not* retry.
const SMART_WORKTREE_ATTEMPTS: usize = 3;

/// Run the LLM generation for smart worktree (branch name + prompt).
///
/// The provider is selected from `[api]` config and built via [`crate::ai_caller`];
/// this function owns the prompt, the retry policy, and the parsing — the
/// configured command is just the model. Checks `cancel_token` before and after
/// each (blocking) call.
///
/// The task keeps `[api] command_timeout_secs` as-is: naming a branch is a few
/// seconds of text generation, unlike walkthrough generation, which asks for its own
/// far larger budget.
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
                    match run_smart_generation(&desc, &cancel, &api, &repo_path) {
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

    /// This task's constraints belong to the task, not to whatever `[api]
    /// command` happens to be: an agentic CLI reaches for tools and chats
    /// unless told not to, and that instruction used to live in a wrapper
    /// script the user had to maintain.
    #[test]
    fn system_prompt_forbids_tools_and_demands_json() {
        assert!(SMART_WORKTREE_SYSTEM_PROMPT.contains("Do not use any tools"));
        assert!(SMART_WORKTREE_SYSTEM_PROMPT.contains("Do not run any commands"));
        assert!(SMART_WORKTREE_SYSTEM_PROMPT.contains("Output ONLY a JSON object"));
    }

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
