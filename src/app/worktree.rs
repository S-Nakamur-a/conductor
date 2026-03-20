//! Worktree CRUD and operations for [`App`].
//!
//! This module contains methods for creating, deleting, switching, and
//! managing worktrees, including smart worktree generation, grep search,
//! cherry-pick, grab/ungrab, and background operations.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use super::*;

const SMART_WORKTREE_SYSTEM_PROMPT: &str = r#"You are a helper that generates a git branch name and a Claude Code prompt from a task description.
Output ONLY a JSON object with two fields:
- "branch": a kebab-case branch name in English, 3-5 words, prefixed with "feature/", "fix/", or "refactor/" as appropriate.
- "prompt": a detailed, actionable prompt for Claude Code to implement the task. Write the prompt in the same language as the input description.
No markdown fences, no explanation, just the JSON object."#;

/// Parse LLM raw output into `SmartGenResult`, stripping markdown fences if present.
fn parse_smart_gen_result(raw: &str) -> Result<SmartGenResult, String> {
    let json_str = raw
        .trim()
        .strip_prefix("```json")
        .or_else(|| raw.trim().strip_prefix("```"))
        .unwrap_or(raw.trim());
    let json_str = json_str
        .strip_suffix("```")
        .unwrap_or(json_str)
        .trim();
    serde_json::from_str::<SmartGenResult>(json_str)
        .map_err(|e| format!("JSON parse error: {e}\nRaw output: {raw}"))
}

/// Fallback: call `claude -p` CLI with the same system prompt and description.
fn run_smart_generation_claude_cli(desc: &str) -> Result<String, String> {
    log::info!("Smart worktree: falling back to claude -p CLI");
    let output = std::process::Command::new("claude")
        .args(["-p", "--output-format", "text"])
        .arg("--system-prompt")
        .arg(SMART_WORKTREE_SYSTEM_PROMPT)
        .arg(desc)
        .output()
        .map_err(|e| format!("Failed to run claude CLI: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("claude CLI failed ({}): {stderr}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.trim().is_empty() {
        return Err("claude CLI returned empty output".to_string());
    }
    Ok(stdout)
}

/// Run the LLM generation for smart worktree (branch name + prompt).
///
/// Tries the Gemini API first; if it fails, falls back to `claude -p` CLI.
/// Checks `cancel_token` before each call; the API calls are blocking.
fn run_smart_generation(desc: &str, cancel_token: &Arc<AtomicBool>, model: &str) -> Result<SmartGenResult, String> {
    if cancel_token.load(Ordering::Relaxed) {
        return Err("Cancelled".to_string());
    }

    // Phase 1: Try Gemini API.
    let raw = match crate::gemini_api::call_messages_api(SMART_WORKTREE_SYSTEM_PROMPT, desc, Some(model), 1024) {
        Ok(raw) => raw,
        Err(gemini_err) => {
            log::warn!("Gemini API failed, falling back to claude CLI: {gemini_err}");

            if cancel_token.load(Ordering::Relaxed) {
                return Err("Cancelled".to_string());
            }

            // Phase 1b: Fallback to claude -p CLI.
            run_smart_generation_claude_cli(desc)?
        }
    };

    if cancel_token.load(Ordering::Relaxed) {
        return Err("Cancelled".to_string());
    }

    parse_smart_gen_result(&raw)
}

impl App {
    // ── Worktree create / delete helpers ──────────────────────────

    /// Select a worktree by its path and trigger UI updates.
    fn select_worktree_by_path(&mut self, path: &std::path::Path) {
        if let Some(idx) = self.worktrees.iter().position(|w| w.path == path) {
            self.selected_worktree = idx;
            self.on_worktree_changed();
        }
    }

    /// Create a worktree from a base ref (2-step flow) — runs in a background thread.
    pub fn create_worktree_from_base(&mut self, branch_name: &str, base_ref: &str) {
        let base = if base_ref.is_empty() { "origin/main" } else { base_ref };

        let pending = PendingWorktree {
            branch: branch_name.to_string(),
            op: PendingWorktreeOp::Creating,
            base_ref: base.to_string(),
            worktree_path: None,
            auto_spawn: false,
            smart_prompt: String::new(),
            delete_branch_after: false,
            description: String::new(),
            created_at: std::time::Instant::now(),
            cancel_token: Arc::new(AtomicBool::new(false)),
        };
        self.worktree_mgr.pending_worktrees.push(pending.clone());
        self.set_status(format!("Creating worktree '{branch_name}'..."), StatusLevel::Info);

        let tx = self.worktree_op_sender();
        let repo_path = self.repo_path.clone();
        let branch = branch_name.to_string();
        let base_owned = base.to_string();
        let wt_dir = self.config.general.worktree_dir.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.create_worktree_from_base(&branch, &base_owned, wt_dir.as_deref()));
            let msg = match result {
                Ok(path) => WorktreeOpResult::Created { path, pending },
                Err(e) => WorktreeOpResult::CreateFailed { error: format!("{e}"), pending },
            };
            let _ = tx.send(msg);
        });
    }

    /// Create a worktree from a remote branch — runs in a background thread.
    pub fn create_worktree_from_remote(&mut self, remote_branch: &str) {
        let local_branch = remote_branch
            .strip_prefix("origin/")
            .unwrap_or(remote_branch);

        let pending = PendingWorktree {
            branch: local_branch.to_string(),
            op: PendingWorktreeOp::Creating,
            base_ref: remote_branch.to_string(),
            worktree_path: None,
            auto_spawn: false,
            smart_prompt: String::new(),
            delete_branch_after: false,
            description: String::new(),
            created_at: std::time::Instant::now(),
            cancel_token: Arc::new(AtomicBool::new(false)),
        };
        self.worktree_mgr.pending_worktrees.push(pending.clone());
        self.set_status(format!("Creating worktree '{local_branch}'..."), StatusLevel::Info);

        let tx = self.worktree_op_sender();
        let repo_path = self.repo_path.clone();
        let remote = remote_branch.to_string();
        let wt_dir = self.config.general.worktree_dir.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.create_worktree_from_remote(&remote, wt_dir.as_deref()));
            let msg = match result {
                Ok(path) => WorktreeOpResult::Created { path, pending },
                Err(e) => WorktreeOpResult::CreateFailed { error: format!("{e}"), pending },
            };
            let _ = tx.send(msg);
        });
    }

    /// Delete a branch (optionally force).
    pub fn delete_branch(&mut self, name: &str, force: bool) {
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.delete_branch(name, force) {
                    Ok(()) => {
                        let mode = if force { "force-deleted" } else { "deleted" };
                        self.set_status(format!("Branch {mode}: {name}"), StatusLevel::Success);
                    }
                    Err(e) => {
                        self.set_status(format!("Branch delete error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Execute grab: checkout main to the selected worktree's branch.
    ///
    /// Also looks up the source worktree's latest Claude Code session and,
    /// if found, auto-resumes it on the main worktree after grabbing.
    pub fn execute_grab(&mut self, branch_name: &str) {
        // Pre-check: already grabbing another branch
        if let Some(ref grabbed) = self.worktree_mgr.grabbed_branch {
            self.set_status(
                format!(
                    "Already grabbed: {}. Ungrab first (Y).",
                    grabbed.branch
                ),
                StatusLevel::Warning,
            );
            return;
        }

        let main_path = match self.worktrees.iter().find(|w| w.is_main) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("Main worktree not found.".to_string(), StatusLevel::Error);
                return;
            }
        };
        let source_path = match self.worktrees.iter().find(|w| w.branch == branch_name) {
            Some(w) => w.path.clone(),
            None => {
                self.set_status(
                    format!("Worktree for '{branch_name}' not found."),
                    StatusLevel::Error,
                );
                return;
            }
        };

        // Look up the latest Claude Code session for the source worktree.
        let claude_session = crate::claude_sessions::find_latest_sessions_for_paths(&[source_path.clone()])
            .ok()
            .and_then(|mut map| {
                let canonical = std::fs::canonicalize(&source_path).unwrap_or_else(|_| source_path.clone());
                map.remove(&canonical)
            });
        let session_id = claude_session.as_ref().map(|s| s.session_id.as_str());

        let selected_path = self.worktrees.get(self.selected_worktree).map(|w| w.path.clone());
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.grab_branch(&main_path, &source_path, branch_name, session_id) {
                    Ok(()) => {
                        let claude_session_id = claude_session.as_ref().map(|s| s.session_id.clone());
                        self.worktree_mgr.grabbed_branch = Some(GrabbedBranch {
                            branch: branch_name.to_string(),
                            source_worktree: source_path,
                            claude_session_id: claude_session_id.clone(),
                        });

                        // Auto-resume the Claude Code session on main worktree.
                        let resume_msg = if let Some(ref session) = claude_session {
                            match self.resume_claude_session_on_main(&session.session_id, &session.display) {
                                Ok(_) => format!(
                                    "Grabbed '{branch_name}' + resumed session {}. Press Y to ungrab.",
                                    &session.session_id[..8.min(session.session_id.len())]
                                ),
                                Err(e) => {
                                    log::warn!("grab: failed to resume session: {e}");
                                    format!("Grabbed '{branch_name}' (session resume failed). Press Y to ungrab.")
                                }
                            }
                        } else {
                            format!("Grabbed '{branch_name}' — main is now on this branch. Press Y to ungrab.")
                        };
                        self.set_status(resume_msg, StatusLevel::Success);

                        self.refresh_worktrees();
                        if let Some(path) = selected_path {
                            self.select_worktree_by_path(&path);
                        }
                    }
                    Err(e) => {
                        self.set_status(format!("Grab error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Resume a Claude Code session on the main worktree.
    fn resume_claude_session_on_main(&mut self, session_id: &str, display: &str) -> anyhow::Result<usize> {
        let main_wt = self.worktrees.iter().find(|w| w.is_main);
        let (worktree_name, working_dir) = match main_wt {
            Some(w) => (w.branch.clone(), w.path.clone()),
            None => anyhow::bail!("main worktree not found"),
        };

        let label: String = display.chars().take(40).collect();
        let label = if label.is_empty() {
            format!("Resume:{}", &session_id[..8.min(session_id.len())])
        } else {
            label
        };
        let shell = self.config.general.shell.clone();
        let (rows, cols) = self.terminal.size_claude;
        let idx = self.terminal.pty_manager.spawn_session(
            pty_manager::SessionKind::ClaudeCode,
            &worktree_name,
            &label,
            &shell,
            &working_dir,
            rows,
            cols,
            Some(session_id),
            &self.repo_path,
        )?;
        self.terminal.pty_manager.activate_session(idx);
        self.terminal.active_claude_session = Some(idx);
        Ok(idx)
    }

    /// Execute ungrab: return main to main branch, restore worktree to original branch.
    pub fn execute_ungrab(&mut self) {
        let grabbed = match self.worktree_mgr.grabbed_branch.clone() {
            Some(g) => g,
            None => {
                self.set_status("Not grabbing any branch.".to_string(), StatusLevel::Warning);
                return;
            }
        };
        let main_path = match self.worktrees.iter().find(|w| w.is_main) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("Main worktree not found.".to_string(), StatusLevel::Error);
                return;
            }
        };
        let selected_path = self.worktrees.get(self.selected_worktree).map(|w| w.path.clone());
        let main_branch = self.config.general.main_branch.clone();
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.ungrab_branch(
                    &main_path,
                    &grabbed.source_worktree,
                    &grabbed.branch,
                    &main_branch,
                ) {
                    Ok(()) => {
                        let branch = grabbed.branch.clone();
                        self.worktree_mgr.grabbed_branch = None;
                        self.set_status(
                            format!("Ungrabbed '{branch}' — main restored."),
                            StatusLevel::Success,
                        );
                        self.refresh_worktrees();
                        if let Some(path) = selected_path {
                            self.select_worktree_by_path(&path);
                        }
                    }
                    Err(e) => {
                        self.set_status(format!("Ungrab error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Prune all stale worktrees.
    pub fn execute_prune(&mut self) {
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                let mut pruned = 0;
                for name in &self.overlays.prune.stale {
                    match engine.prune_stale_worktree(name) {
                        Ok(()) => pruned += 1,
                        Err(e) => {
                            log::warn!("failed to prune worktree '{name}': {e}");
                        }
                    }
                }
                self.set_status(format!("Pruned {pruned} stale worktree(s)."), StatusLevel::Success);
                self.overlays.prune.stale.clear();
                self.refresh_worktrees();
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Load remote branches for the switch overlay.
    ///
    /// Immediately populates the list from cached refs, then kicks off a
    /// background fetch. When the fetch completes, `poll_bg_branches()`
    /// picks up the refreshed list so the overlay updates without blocking.
    pub fn load_switch_branches(&mut self) {
        // Show cached refs instantly.
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.list_remote_branches() {
                    Ok(branches) => {
                        self.overlays.switch_branch.branches = branches;
                        self.overlays.switch_branch.selected = 0;
                        self.overlays.switch_branch.filter.clear();
                    }
                    Err(e) => {
                        self.set_status(format!("Error listing branches: {e}"), StatusLevel::Error);
                        self.overlays.switch_branch.branches.clear();
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
                return;
            }
        }

        // Fetch in background and send updated branch list back.
        let repo_path = self.repo_path.clone();
        self.bg_branch_op.start(move |tx| {
            let engine = match git_engine::GitEngine::open(&repo_path) {
                Ok(e) => e,
                Err(err) => {
                    log::warn!("bg fetch: failed to open repo: {err}");
                    return;
                }
            };
            if let Err(e) = engine.fetch_origin() {
                log::warn!("bg fetch failed: {e}");
            }
            match engine.list_remote_branches() {
                Ok(branches) => { let _ = tx.send(branches); }
                Err(e) => { log::warn!("bg list_remote_branches failed: {e}"); }
            }
        });
    }

    /// Check whether the background fetch has finished and update the
    /// switch-branch list if new data is available. Non-blocking.
    pub fn poll_bg_branches(&mut self) {
        if let Some(branches) = self.bg_branch_op.poll() {
            // Preserve the user's current filter/selection as best we can.
            let prev_selected_name = self.filtered_switch_branches()
                .get(self.overlays.switch_branch.selected)
                .map(|(_, name)| (*name).clone());
            self.overlays.switch_branch.branches = branches;
            // Try to restore selection by name.
            if let Some(name) = prev_selected_name {
                if let Some(pos) = self.filtered_switch_branches()
                    .iter()
                    .position(|(_, b)| **b == name)
                {
                    self.overlays.switch_branch.selected = pos;
                }
            }
            self.bg_branch_op.clear();
        }
    }

    // ── Pull worktree (fetch + fast-forward) ──────────────────────────

    /// Start a background pull (fetch + fast-forward) for the selected worktree.
    pub fn start_pull_worktree(&mut self) {
        if self.bg_pull_op.is_running() {
            self.set_status("A pull is already in progress.".to_string(), StatusLevel::Warning);
            return;
        }

        let wt = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt,
            None => return,
        };

        let branch = wt.branch.clone();
        let wt_path = wt.path.clone();
        let repo_path = self.repo_path.clone();

        self.set_status(format!("Pulling '{branch}'..."), StatusLevel::Info);

        self.bg_pull_op.start(move |tx| {
            let result = (|| -> Result<String, String> {
                let engine = git_engine::GitEngine::open(&repo_path)
                    .map_err(|e| format!("Failed to open repo: {e}"))?;
                engine.pull_worktree(&wt_path)
                    .map_err(|e| format!("{e}"))
            })();
            let _ = tx.send(result);
        });
    }

    /// Poll the background pull channel. Non-blocking.
    pub fn poll_bg_pull(&mut self) {
        if let Some(result) = self.bg_pull_op.poll() {
            match result {
                Ok(msg) => {
                    let level = if msg.contains("up-to-date") {
                        StatusLevel::Info
                    } else if msg.contains("fast-forward") {
                        StatusLevel::Success
                    } else {
                        StatusLevel::Warning
                    };
                    self.set_status(msg, level);
                    self.refresh_worktrees();
                }
                Err(err) => {
                    self.set_status(format!("Pull failed: {err}"), StatusLevel::Error);
                }
            }
        }
    }

    // ── Async worktree operations ──────────────────────────────────────

    /// Poll for completed background worktree create/delete results.
    pub fn poll_worktree_ops(&mut self) {
        let rx = match self.worktree_mgr.bg_worktree_rx.as_ref() {
            Some(rx) => rx,
            None => return,
        };
        let mut results = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(result) => results.push(result),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worktree_mgr.bg_worktree_rx = None;
                    self.worktree_mgr.bg_worktree_tx = None;
                    // Clean up any pending create/smart-create entries that will never complete.
                    let orphaned: Vec<_> = self.worktree_mgr.pending_worktrees.iter()
                        .filter(|p| matches!(p.op, PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating))
                        .map(|p| p.description.clone())
                        .collect();
                    if !orphaned.is_empty() {
                        self.worktree_mgr.pending_worktrees.retain(|p| {
                            !matches!(p.op, PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating)
                        });
                        log::warn!("Cleaned up {} orphaned pending worktrees on channel disconnect", orphaned.len());
                        self.set_status(
                            "Worktree creation interrupted (channel disconnected)".to_string(),
                            StatusLevel::Error,
                        );
                    }
                    break;
                }
            }
        }
        for result in results {
            self.handle_worktree_op_result(result);
        }

        // Timeout detection: warn if any pending create/smart-create has been running too long.
        const TIMEOUT_SECS: u64 = 120;
        let now = std::time::Instant::now();
        let timed_out: Vec<_> = self.worktree_mgr.pending_worktrees.iter()
            .filter(|p| {
                matches!(p.op, PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating)
                    && now.duration_since(p.created_at).as_secs() >= TIMEOUT_SECS
            })
            .map(|p| {
                if p.description.is_empty() { p.branch.clone() } else { p.description.clone() }
            })
            .collect();
        if !timed_out.is_empty() {
            self.worktree_mgr.pending_worktrees.retain(|p| {
                !(matches!(p.op, PendingWorktreeOp::Creating | PendingWorktreeOp::SmartCreating)
                    && now.duration_since(p.created_at).as_secs() >= TIMEOUT_SECS)
            });
            let names = timed_out.join(", ");
            log::warn!("Timed out pending worktrees: {names}");
            self.set_status(
                format!("Worktree creation timed out: {names}"),
                StatusLevel::Error,
            );
        }
    }

    fn handle_worktree_op_result(&mut self, result: WorktreeOpResult) {
        match result {
            WorktreeOpResult::Created { path, pending } => {
                // Remove from pending list (matches both Creating and SmartCreating).
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !((p.op == PendingWorktreeOp::Creating || p.op == PendingWorktreeOp::SmartCreating)
                        && p.branch == pending.branch)
                });

                self.record_stat("branches_created");
                if let Some(store) = &self.review_store {
                    let _ = store.save_worktree_base_branch(&pending.branch, &pending.base_ref);
                }
                self.refresh_worktrees();
                // Preserve the current focus and selected worktree — don't
                // switch the user's view to the newly created worktree.
                let prev_selected = self.selected_worktree;
                let prev_focus = self.focus;
                self.set_status(
                    format!("Created worktree: {} (from {})", path.display(), pending.base_ref),
                    StatusLevel::Success,
                );

                // Smart Worktree: auto-spawn Claude Code and defer prompt
                // until the session is ready for input.
                if pending.auto_spawn {
                    // Temporarily select the new worktree so spawn_claude_code
                    // picks up the correct working directory.
                    self.select_worktree_by_path(&path);
                    match self.spawn_claude_code() {
                        Ok(idx) => {
                            if !pending.smart_prompt.is_empty() {
                                self.terminal.deferred_prompts.insert(idx, pending.smart_prompt.clone());
                            }
                        }
                        Err(e) => {
                            log::warn!("Failed to auto-spawn Claude Code: {e}");
                        }
                    }
                    // Restore the previous worktree selection and focus.
                    self.selected_worktree = prev_selected;
                    self.on_worktree_changed();
                    self.focus = prev_focus;
                }
            }
            WorktreeOpResult::CreateFailed { error, pending } => {
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !((p.op == PendingWorktreeOp::Creating || p.op == PendingWorktreeOp::SmartCreating)
                        && p.branch == pending.branch)
                });
                self.set_status(format!("Error: {error}"), StatusLevel::Error);
            }
            WorktreeOpResult::Deleted { ref branch } => {
                let delete_branch_after = self.worktree_mgr.pending_worktrees.iter().any(|p| {
                    p.op == PendingWorktreeOp::Deleting && p.branch == *branch && p.delete_branch_after
                });
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !(p.op == PendingWorktreeOp::Deleting && p.branch == *branch)
                });
                self.refresh_worktrees();
                self.set_status(format!("Deleted worktree: {branch}"), StatusLevel::Success);

                if delete_branch_after {
                    self.delete_branch(branch, true);
                }
            }
            WorktreeOpResult::DeleteFailed { error, ref branch } => {
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !(p.op == PendingWorktreeOp::Deleting && p.branch == *branch)
                });
                self.set_status(format!("Error: {error}"), StatusLevel::Error);
            }
            WorktreeOpResult::Skipped { ref branch, ref reason } => {
                self.worktree_mgr.pending_worktrees.retain(|p| p.branch != *branch);
                self.worktree_mgr.skip_reason = Some(reason.clone());
            }
            WorktreeOpResult::SmartBranchResolved { ref description, ref branch, ref prompt } => {
                // Update the pending entry: set branch name and prompt.
                for p in &mut self.worktree_mgr.pending_worktrees {
                    if p.op == PendingWorktreeOp::SmartCreating && p.description == *description {
                        p.branch = branch.clone();
                        p.smart_prompt = prompt.clone();
                        break;
                    }
                }
                self.set_status(
                    format!("Smart worktree: creating '{branch}'... (Esc to cancel)"),
                    StatusLevel::Info,
                );
            }
            WorktreeOpResult::SmartFailed { ref description, ref error } => {
                self.worktree_mgr.pending_worktrees.retain(|p| {
                    !(p.op == PendingWorktreeOp::SmartCreating && p.description == *description)
                });
                // Suppress error message if the operation was cancelled by user.
                if error == "Cancelled" {
                    log::info!("Smart worktree cancelled for: {description}");
                } else {
                    log::warn!("Smart worktree failed: {error}");
                    self.set_status(
                        format!("Smart worktree failed: {error}"),
                        StatusLevel::Error,
                    );
                }
            }
        }
    }

    // ── Smart Worktree generation ──────────────────────────────────────

    /// Run LLM generation + worktree creation asynchronously in a single background thread.
    pub fn start_smart_worktree_async(&mut self, description: &str) {
        // Guard: skip if a smart worktree creation is already in progress.
        if self.worktree_mgr.pending_worktrees.iter().any(|p| p.op == PendingWorktreeOp::SmartCreating) {
            self.set_status(
                "Smart worktree creation is already in progress.".to_string(),
                StatusLevel::Warning,
            );
            return;
        }

        let desc = description.to_string();
        let main_branch = self.config.general.main_branch.clone();
        let base_ref = format!("origin/{main_branch}");
        let repo_path = self.repo_path.clone();
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
            delete_branch_after: false,
            description: desc.clone(),
            created_at: std::time::Instant::now(),
            cancel_token: cancel_token.clone(),
        };
        self.worktree_mgr.pending_worktrees.push(pending);
        self.set_status("Smart worktree: generating... (Esc to cancel)".to_string(), StatusLevel::Info);

        let tx = self.worktree_op_sender();
        let api_model = self.config.api.model.clone();

        let cancel = cancel_token;
        std::thread::spawn(move || {
            let tx_panic = tx.clone();
            let desc_panic = desc.clone();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Phase 1: LLM generation.
                let gen_result = match run_smart_generation(&desc, &cancel, &api_model) {
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
                });

                // Phase 2: Create worktree.
                let pending = PendingWorktree {
                    branch: branch.clone(),
                    op: PendingWorktreeOp::SmartCreating,
                    base_ref: base_ref.clone(),
                    worktree_path: None,
                    auto_spawn: true,
                    smart_prompt: prompt,
                    delete_branch_after: false,
                    description: desc,
                    created_at: std::time::Instant::now(),
                    cancel_token: cancel.clone(),
                };
                let result = git_engine::GitEngine::open(&repo_path)
                    .and_then(|engine| engine.create_worktree_from_base(&branch, &base_ref, wt_dir.as_deref()));
                let msg = match result {
                    Ok(path) => WorktreeOpResult::Created { path, pending },
                    Err(e) => WorktreeOpResult::CreateFailed { error: format!("{e}"), pending },
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
        let smart_pending: Vec<_> = self.worktree_mgr.pending_worktrees.iter()
            .filter(|p| p.op == PendingWorktreeOp::SmartCreating)
            .map(|p| p.cancel_token.clone())
            .collect();

        if smart_pending.is_empty() {
            return false;
        }

        for token in &smart_pending {
            token.store(true, Ordering::Relaxed);
        }

        self.worktree_mgr.pending_worktrees.retain(|p| {
            p.op != PendingWorktreeOp::SmartCreating
        });

        self.set_status(
            "Worktree creation cancelled.".to_string(),
            StatusLevel::Info,
        );
        true
    }

    /// Schedule an incremental grep search with debounce (200ms).
    ///
    /// Called on every keystroke that modifies the query. Sets a deadline;
    /// `check_grep_debounce()` fires the actual search when the deadline passes.
    pub fn schedule_grep_search(&mut self) {
        let query = self.overlays.grep_search.query.text().to_string();
        if query.is_empty() {
            // Clear everything immediately.
            self.overlays.grep_search.results.clear();
            self.overlays.grep_search.selected = 0;
            self.overlays.grep_search.scroll = 0;
            self.overlays.grep_search.running = false;
            self.overlays.grep_search.bg_op.clear();
            self.overlays.grep_search.bg_op_phase2.clear();
            self.overlays.grep_search.debounce_deadline = None;
            self.overlays.grep_search.phase1_active = false;
            return;
        }
        self.overlays.grep_search.debounce_deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(200));
    }

    /// Check if the debounce deadline has passed; if so, start the search.
    /// Returns `true` if a search was started (caller should trigger redraw).
    pub fn check_grep_debounce(&mut self) -> bool {
        if let Some(deadline) = self.overlays.grep_search.debounce_deadline {
            if std::time::Instant::now() >= deadline {
                self.overlays.grep_search.debounce_deadline = None;
                self.start_incremental_grep_search();
                return true;
            }
        }
        false
    }

    /// Start an incremental grep search.
    ///
    /// For short queries (≤3 chars), uses 2-phase search:
    ///   phase1 — search only recently modified files (fast)
    ///   phase2 — full search (runs in parallel, replaces phase1 results)
    /// For longer queries, runs only a full search.
    fn start_incremental_grep_search(&mut self) {
        let query = self.overlays.grep_search.query.text().to_string();
        if query.is_empty() {
            return;
        }

        let wt_path = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt.path.clone(),
            None => return,
        };

        // Cancel any previous search.
        self.overlays.grep_search.bg_op.clear();
        self.overlays.grep_search.bg_op_phase2.clear();

        // Reset results.
        self.overlays.grep_search.results.clear();
        self.overlays.grep_search.selected = 0;
        self.overlays.grep_search.scroll = 0;
        self.overlays.grep_search.running = true;

        let regex_mode = self.overlays.grep_search.regex_mode;
        let case_sensitive = self.overlays.grep_search.case_sensitive;

        if query.chars().count() <= 3 {
            // 2-phase search for short queries.
            self.overlays.grep_search.phase1_active = true;

            // Get recently modified files (synchronous, fast).
            let recent_files = crate::git_engine::recently_modified_files(&wt_path, 200)
                .unwrap_or_default();

            // Phase1: search only recent files.
            if !recent_files.is_empty() {
                let wt1 = wt_path.clone();
                let q1 = query.clone();
                let files1 = recent_files;
                self.overlays.grep_search.bg_op.start(move |tx| {
                    crate::grep_search::run_search_files(&wt1, &q1, regex_mode, case_sensitive, files1, tx);
                });
            }

            // Phase2: full search (runs in parallel).
            let wt2 = wt_path.clone();
            let q2 = query.clone();
            self.overlays.grep_search.bg_op_phase2.start(move |tx| {
                crate::grep_search::run_search(&wt2, &q2, regex_mode, case_sensitive, tx);
            });
        } else {
            // Single-phase full search for longer queries.
            self.overlays.grep_search.phase1_active = false;
            let wt2 = wt_path.clone();
            let q2 = query.clone();
            self.overlays.grep_search.bg_op.start(move |tx| {
                crate::grep_search::run_search(&wt2, &q2, regex_mode, case_sensitive, tx);
            });
        }
    }

    /// Poll for background grep search results.
    pub fn poll_grep_search(&mut self) {
        // Poll phase1 / single-phase bg_op.
        let messages = self.overlays.grep_search.bg_op.poll_all();
        for msg in messages {
            match msg {
                GrepProgress::Results(batch) => {
                    self.overlays.grep_search.results.extend(batch);
                }
                GrepProgress::Done(total) => {
                    // If phase1 completed but phase2 is still running, keep running = true.
                    if !self.overlays.grep_search.phase1_active || !self.overlays.grep_search.bg_op_phase2.is_running() {
                        self.overlays.grep_search.running = false;
                        self.overlays.grep_search.bg_op.clear();
                        if total >= 5000 {
                            self.set_status(
                                format!("Search truncated at {total} results."),
                                StatusLevel::Warning,
                            );
                        }
                    } else {
                        self.overlays.grep_search.bg_op.clear();
                    }
                }
                GrepProgress::Error(msg) => {
                    self.overlays.grep_search.running = false;
                    self.overlays.grep_search.bg_op.clear();
                    self.set_status(format!("Search error: {msg}"), StatusLevel::Error);
                    return;
                }
            }
        }

        // Poll phase2 bg_op.
        if self.overlays.grep_search.phase1_active {
            let messages2 = self.overlays.grep_search.bg_op_phase2.poll_all();
            let mut got_phase2_results = false;
            for msg in messages2 {
                match msg {
                    GrepProgress::Results(batch) => {
                        if !got_phase2_results {
                            // Replace phase1 results with phase2 results.
                            self.overlays.grep_search.results.clear();
                            self.overlays.grep_search.selected = 0;
                            self.overlays.grep_search.scroll = 0;
                            self.overlays.grep_search.phase1_active = false;
                            got_phase2_results = true;
                        }
                        self.overlays.grep_search.results.extend(batch);
                    }
                    GrepProgress::Done(total) => {
                        if !got_phase2_results {
                            // Phase2 done with no results — clear phase1 results too
                            // only if phase1 also had no results; otherwise keep phase1.
                            self.overlays.grep_search.phase1_active = false;
                        }
                        self.overlays.grep_search.running = false;
                        self.overlays.grep_search.bg_op_phase2.clear();
                        if total >= 5000 {
                            self.set_status(
                                format!("Search truncated at {total} results."),
                                StatusLevel::Warning,
                            );
                        }
                    }
                    GrepProgress::Error(msg) => {
                        self.overlays.grep_search.phase1_active = false;
                        self.overlays.grep_search.running = false;
                        self.overlays.grep_search.bg_op_phase2.clear();
                        self.set_status(format!("Search error: {msg}"), StatusLevel::Error);
                        return;
                    }
                }
            }
        }
    }

    /// Return the filtered list of switch branches based on the current filter.
    pub fn filtered_switch_branches(&self) -> Vec<(usize, &String)> {
        if self.overlays.switch_branch.filter.is_empty() {
            self.overlays.switch_branch.branches.iter().enumerate().collect()
        } else {
            let filter_lower = self.overlays.switch_branch.filter.to_lowercase();
            self.overlays.switch_branch.branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    /// Load branches available as base for worktree creation.
    /// Lists remote branches and pre-selects `origin/<main_branch>`.
    pub fn load_base_branches(&mut self) {
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.list_remote_branches() {
                    Ok(branches) => {
                        self.worktree_mgr.base_branch_list = branches;
                        self.worktree_mgr.base_branch_selected = 0;
                        self.worktree_mgr.base_branch_filter.clear();
                        // Pre-select origin/<main_branch> if it exists.
                        let default_base = format!("origin/{}", self.config.general.main_branch);
                        if let Some(pos) = self.worktree_mgr.base_branch_list.iter().position(|b| b == &default_base) {
                            self.worktree_mgr.base_branch_selected = pos;
                        }
                    }
                    Err(e) => {
                        self.set_status(format!("Error listing branches: {e}"), StatusLevel::Error);
                        self.worktree_mgr.base_branch_list.clear();
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Return the filtered list of base branches based on the current filter.
    pub fn filtered_base_branches(&self) -> Vec<(usize, &String)> {
        if self.worktree_mgr.base_branch_filter.is_empty() {
            self.worktree_mgr.base_branch_list.iter().enumerate().collect()
        } else {
            let filter_lower = self.worktree_mgr.base_branch_filter.to_lowercase();
            self.worktree_mgr.base_branch_list
                .iter()
                .enumerate()
                .filter(|(_, b)| b.to_lowercase().contains(&filter_lower))
                .collect()
        }
    }

    /// Load grab branch candidates (non-main worktree branches).
    pub fn load_grab_branches(&mut self) {
        self.overlays.grab.branches = self.worktrees
            .iter()
            .filter(|w| !w.is_main)
            .map(|w| w.branch.clone())
            .collect();
        self.overlays.grab.selected = 0;
    }

    pub fn delete_selected_worktree(&mut self, delete_branch_after: bool) {
        let wt = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt,
            None => return,
        };

        if wt.is_main {
            self.set_status("Cannot delete the main worktree.".to_string(), StatusLevel::Error);
            return;
        }

        let wt_path = wt.path.clone();
        let branch = wt.branch.clone();

        // Kill all PTY sessions (Claude Code + Shell) associated with this worktree
        // before removing the worktree directory. Walk backwards so removals don't
        // shift indices we haven't processed yet.
        let session_indices: Vec<usize> = self
            .terminal.pty_manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.working_dir == wt_path)
            .map(|(idx, _)| idx)
            .collect();
        for &idx in session_indices.iter().rev() {
            log::info!(
                "killing PTY session {idx} for deleted worktree '{branch}'"
            );
            self.close_terminal_session(idx);
        }

        // Add pending entry and run git removal in a background thread.
        let pending = PendingWorktree {
            branch: branch.clone(),
            op: PendingWorktreeOp::Deleting,
            base_ref: String::new(),
            worktree_path: Some(wt_path.clone()),
            auto_spawn: false,
            smart_prompt: String::new(),
            delete_branch_after,
            description: String::new(),
            created_at: std::time::Instant::now(),
            cancel_token: Arc::new(AtomicBool::new(false)),
        };
        self.worktree_mgr.pending_worktrees.push(pending);
        self.set_status(format!("Deleting worktree '{branch}'..."), StatusLevel::Info);

        let tx = self.worktree_op_sender();
        let repo_path = self.repo_path.clone();

        std::thread::spawn(move || {
            let result = git_engine::GitEngine::open(&repo_path)
                .and_then(|engine| engine.remove_worktree(&wt_path));
            let msg = match result {
                Ok(()) => WorktreeOpResult::Deleted { branch },
                Err(e) => WorktreeOpResult::DeleteFailed { error: format!("{e}"), branch },
            };
            let _ = tx.send(msg);
        });
    }

    // ── Cherry-pick helpers ────────────────────────────────────────────

    pub fn load_cherry_pick_commits(&mut self) {
        let branch = self.overlays.cherry_pick.source_branch.clone();
        if branch.is_empty() {
            self.overlays.cherry_pick.commits.clear();
            return;
        }
        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.list_branch_commits(&branch, 20) {
                    Ok(commits) => {
                        self.overlays.cherry_pick.commits = commits;
                        self.overlays.cherry_pick.selected = 0;
                    }
                    Err(e) => {
                        log::warn!("failed to list commits for branch '{branch}': {e}");
                        self.overlays.cherry_pick.commits.clear();
                    }
                }
            }
            Err(e) => {
                log::warn!("failed to open git repository for cherry-pick: {e}");
                self.overlays.cherry_pick.commits.clear();
            }
        }
    }

    pub fn execute_cherry_pick(&mut self) {
        let commit = match self.overlays.cherry_pick.commits.get(self.overlays.cherry_pick.selected) {
            Some(c) => c.clone(),
            None => {
                self.set_status("No commit selected.".to_string(), StatusLevel::Error);
                return;
            }
        };
        let wt_path = match self.worktrees.get(self.selected_worktree) {
            Some(wt) => wt.path.clone(),
            None => {
                self.set_status("No worktree selected.".to_string(), StatusLevel::Error);
                return;
            }
        };

        match git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => {
                match engine.cherry_pick_to_worktree(&wt_path, &commit.oid) {
                    Ok(msg) => {
                        self.set_status(msg, StatusLevel::Success);
                        self.refresh_worktrees();
                    }
                    Err(e) => {
                        self.set_status(format!("Cherry-pick error: {e}"), StatusLevel::Error);
                    }
                }
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

    /// Called when the selected worktree changes — refreshes viewer, diff, sessions.
    ///
    /// Heavy operations (file tree walk, diff computation, branch details) are
    /// dispatched to background threads so the UI stays responsive. Results are
    /// applied in `poll_worktree_switch_ops()`.
    pub fn on_worktree_changed(&mut self) {
        self.viewer_state = ViewerState::default();

        // Reviews are fast (SQLite) — keep synchronous.
        self.refresh_reviews();

        // Snapshot baseline so the next poll cycle doesn't trigger a redundant refresh.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            self.last_poll_head_oid = self.worktree_heads.get(&wt.branch).cloned();
            self.last_poll_status = Some((wt.added, wt.modified, wt.deleted));
        }

        // Update active sessions to match the new worktree.
        let wt_name = self.selected_worktree_branch();
        let claude_sessions = self.current_worktree_claude_sessions();
        self.terminal.active_claude_session = claude_sessions.first().map(|(idx, _)| *idx);
        let shell_sessions = self.current_worktree_shell_sessions();
        self.terminal.active_shell_session = shell_sessions.first().map(|(idx, _)| *idx);

        // Activate the PTY sessions.
        if let Some(idx) = self.terminal.active_claude_session {
            self.terminal.pty_manager.activate_session(idx);
        }
        if let Some(idx) = self.terminal.active_shell_session {
            self.terminal.pty_manager.activate_session(idx);
        }

        self.terminal.scroll_claude = 0;
        self.terminal.scroll_shell = 0;
        self.terminal.cache_claude = Default::default();
        self.terminal.cache_shell = Default::default();

        // Dispatch heavy operations to background threads.
        if let Some(wt) = self.worktrees.get(self.selected_worktree) {
            let wt_path = wt.path.clone();

            // Background file tree walk.
            {
                let path = wt_path.clone();
                self.bg_file_tree_op.start(move |tx| {
                    let gi = ViewerState::build_gitignore(&path);
                    let mut entries = Vec::new();
                    ViewerState::walk_dir(&path, &path, 0, &mut entries, Some(&gi));
                    let _ = tx.send(entries);
                });
            }

            // Background diff computation.
            {
                let path = wt_path.clone();
                let base_branch = self.config.general.main_branch.clone();
                let word_diff = self.config.diff.word_diff;
                let tab_width = self.config.viewer.tab_width;
                self.bg_diff_op.start(move |tx| {
                    let mut result = BgDiffResult {
                        committed: Vec::new(),
                        uncommitted: Vec::new(),
                        error: None,
                    };
                    match DiffState::compute_diff_range_static(
                        &path, &base_branch, true, word_diff, tab_width,
                    ) {
                        Ok(mut files) => {
                            files.sort_by(|a, b| a.path.cmp(&b.path));
                            result.committed = files;
                        }
                        Err(e) => {
                            result.error = Some(format!("{e:#}"));
                            let _ = tx.send(result);
                            return;
                        }
                    }
                    match DiffState::compute_diff_range_static(
                        &path, &base_branch, false, word_diff, tab_width,
                    ) {
                        Ok(mut files) => {
                            files.sort_by(|a, b| a.path.cmp(&b.path));
                            result.uncommitted = files;
                        }
                        Err(e) => {
                            log::warn!("failed to compute uncommitted diff: {e:#}");
                        }
                    }
                    let _ = tx.send(result);
                });
            }

            // Background branch details computation.
            self.start_bg_branch_details();
        }

        self.set_status(format!("Switched to worktree: {wt_name}"), StatusLevel::Success);
    }

    /// Spawn background branch details computation.
    fn start_bg_branch_details(&mut self) {
        let Some(wt) = self.worktrees.get(self.selected_worktree) else {
            self.branch_details = Default::default();
            return;
        };
        let branch = wt.branch.clone();
        let is_main = wt.is_main;
        let repo_path = self.repo_path.clone();
        let main_branch = self.config.general.main_branch.clone();
        let worktree_branches: Vec<String> = self
            .worktrees
            .iter()
            .filter(|w| !w.is_main && w.branch != branch)
            .map(|w| w.branch.clone())
            .collect();

        // Check DB for cached parent/children before spawning the thread.
        let db_initial_branch = if !is_main {
            self.review_store
                .as_ref()
                .and_then(|store| store.get_worktree_base_branch(&branch).ok().flatten())
        } else {
            None
        };

        let active_branches: std::collections::HashSet<String> =
            self.worktrees.iter().map(|w| w.branch.clone()).collect();
        let db_children: Vec<String> = self
            .review_store
            .as_ref()
            .and_then(|store| store.get_worktree_children(&branch).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|c| active_branches.contains(c))
            .collect();

        // Reset branch_details and start PR lookup (already async).
        self.branch_details = Default::default();
        if !is_main && self.gh_available {
            self.branch_details.pr_loading = true;
            self.start_pr_url_lookup(&branch);
        }

        self.bg_branch_details_op.start(move |tx| {
            let mut details = git_engine::BranchDetails::default();

            if !is_main {
                details.initial_branch = db_initial_branch.or_else(|| {
                    git_engine::GitEngine::open(&repo_path)
                        .ok()
                        .and_then(|engine| {
                            engine.detect_parent_branch(&branch, &main_branch, &worktree_branches)
                        })
                });
            }

            if !db_children.is_empty() {
                details.derived_branches = db_children;
            } else if let Ok(engine) = git_engine::GitEngine::open(&repo_path) {
                if let Ok(derived) =
                    engine.find_derived_branches(&branch, &main_branch, &worktree_branches)
                {
                    details.derived_branches = derived;
                }
            }

            let _ = tx.send(details);
        });
    }

    /// Poll background worktree-switch operations (file tree, diff, branch details).
    pub fn poll_worktree_switch_ops(&mut self) {
        // File tree result.
        if let Some(entries) = self.bg_file_tree_op.poll() {
            self.viewer_state.tree.file_tree = entries;
            self.viewer_state.invalidate_visible_cache();
            // Re-open the previously viewed file if it still exists.
            if let Some(wt) = self.worktrees.get(self.selected_worktree) {
                let wt_path = wt.path.clone();
                if let Some(ref rel_path) = self.viewer_state.content.current_file.clone() {
                    let full = wt_path.join(rel_path);
                    if full.is_file() {
                        let tab_width = self.config.viewer.tab_width;
                        self.viewer_state.open_file(&wt_path, rel_path, tab_width);
                    }
                }
            }
            self.rehighlight_viewer();
        }

        // Diff result.
        if let Some(result) = self.bg_diff_op.poll() {
            if let Some(error) = result.error {
                self.diff_state.committed_files.clear();
                self.diff_state.uncommitted_files.clear();
                self.diff_state.error = Some(error);
            } else {
                self.diff_state.committed_files = result.committed;
                self.diff_state.uncommitted_files = result.uncommitted;
                self.diff_state.error = None;
            }
            self.diff_state.rebuild_display_list();
        }

        // Branch details result.
        if let Some(details) = self.bg_branch_details_op.poll() {
            // Preserve pr_url and pr_loading from the already-running PR lookup.
            let pr_url = self.branch_details.pr_url.take();
            let pr_loading = self.branch_details.pr_loading;
            self.branch_details = details;
            self.branch_details.pr_url = pr_url;
            self.branch_details.pr_loading = pr_loading;
        }
    }

    // ── Branch details (worktree detail panel) ───────────────────

    /// Check whether the `gh` CLI is available on this system.
    pub(super) fn check_gh_available() -> bool {
        std::process::Command::new("gh")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Get (or lazily create) a sender for worktree operation results.
    fn worktree_op_sender(&mut self) -> mpsc::Sender<WorktreeOpResult> {
        if self.worktree_mgr.bg_worktree_tx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.worktree_mgr.bg_worktree_tx = Some(tx);
            self.worktree_mgr.bg_worktree_rx = Some(rx);
        }
        self.worktree_mgr.bg_worktree_tx.as_ref().unwrap().clone()
    }

    /// Check if a worktree at the given path is pending deletion.
    pub fn is_worktree_pending_delete(&self, path: &Path) -> bool {
        self.worktree_mgr.pending_worktrees.iter().any(|p| {
            p.op == PendingWorktreeOp::Deleting && p.worktree_path.as_deref() == Some(path)
        })
    }

    /// Spawn a background thread to look up the PR URL via `gh pr view`.
    fn start_pr_url_lookup(&mut self, branch: &str) {
        let branch = branch.to_string();
        let repo_path = self.repo_path.clone();

        self.bg_pr_url_op.start(move |tx| {
            let result = std::process::Command::new("gh")
                .args(["pr", "view", "--head", &branch, "--json", "url", "-q", ".url"])
                .current_dir(&repo_path)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .and_then(|output| {
                    if output.status.success() {
                        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if url.is_empty() { None } else { Some(url) }
                    } else {
                        None
                    }
                });
            let _ = tx.send(result);
        });
    }

    /// Poll the background PR URL lookup for a result.
    pub fn poll_pr_url(&mut self) {
        if let Some(result) = self.bg_pr_url_op.poll() {
            self.branch_details.pr_url = result;
            self.branch_details.pr_loading = false;
        }
    }

    // ── Open PR in browser ───────────────────────────────────────

    /// Open the pull-request page for the selected worktree's branch in the
    /// default web browser.
    pub fn open_pr_in_browser(&mut self) {
        let branch = self.selected_worktree_branch();
        if branch.is_empty() {
            self.set_status("No worktree selected.".to_string(), StatusLevel::Warning);
            return;
        }

        match crate::git_engine::GitEngine::open(&self.repo_path) {
            Ok(engine) => match engine.pr_url_for_branch(&branch) {
                Some(url) => {
                    log::info!("Opening PR URL: {url}");
                    if let Err(e) = open::that(&url) {
                        self.set_status(format!("Failed to open browser: {e}"), StatusLevel::Error);
                    } else {
                        self.set_status(format!("Opened PR for '{branch}'"), StatusLevel::Success);
                    }
                }
                None => {
                    self.set_status("Could not determine remote URL.".to_string(), StatusLevel::Error);
                }
            },
            Err(e) => {
                self.set_status(format!("Error: {e}"), StatusLevel::Error);
            }
        }
    }

}
