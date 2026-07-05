//! Data model and generation trigger for AI-generated PR walkthroughs.
//!
//! A walkthrough is a Claude-authored, ordered tour of a branch's diff
//! (`walkthroughs` + `walkthrough_steps` in the review database, persisted
//! and queried via [`crate::review_store::ReviewStore`]). This module holds
//! the plain data types shared by the store, the UI pane, and the generator:
//! a headless `claude -p` process spawned in the reviewed worktree that
//! explores the diff and saves the result through the conductor MCP server's
//! `save_walkthrough` tool. The database row's `status` column is the source
//! of truth for completion; the process handle here only detects failures
//! (spawn error, non-zero exit, exit without saving, timeout).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Generation status of a walkthrough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkthroughStatus {
    /// A background Claude Code session is producing the steps.
    Generating,
    /// Steps are saved and ready to display.
    Ready,
    /// Generation failed; `Walkthrough::error` holds the reason.
    Failed,
}

impl WalkthroughStatus {
    /// Convert to the string representation stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            WalkthroughStatus::Generating => "generating",
            WalkthroughStatus::Ready => "ready",
            WalkthroughStatus::Failed => "failed",
        }
    }

    /// Parse the string representation stored in the database.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "generating" => Some(WalkthroughStatus::Generating),
            "ready" => Some(WalkthroughStatus::Ready),
            "failed" => Some(WalkthroughStatus::Failed),
            _ => None,
        }
    }
}

impl std::fmt::Display for WalkthroughStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The kind of a walkthrough step, driving its icon/emphasis in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkthroughStepKind {
    /// Why the change exists — the motivating problem or request.
    Intent,
    /// The main implementation of the change.
    Core,
    /// A knock-on effect of the core change (call-site updates, config, etc).
    Ripple,
    /// Test coverage added or updated for the change.
    Test,
}

impl WalkthroughStepKind {
    /// Convert to the string representation stored in the database.
    pub fn as_str(&self) -> &'static str {
        match self {
            WalkthroughStepKind::Intent => "intent",
            WalkthroughStepKind::Core => "core",
            WalkthroughStepKind::Ripple => "ripple",
            WalkthroughStepKind::Test => "test",
        }
    }

    /// Parse the string representation stored in the database.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "intent" => Some(WalkthroughStepKind::Intent),
            "core" => Some(WalkthroughStepKind::Core),
            "ripple" => Some(WalkthroughStepKind::Ripple),
            "test" => Some(WalkthroughStepKind::Test),
            _ => None,
        }
    }
}

impl std::fmt::Display for WalkthroughStepKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A branch's walkthrough header row (`walkthroughs` table). One per branch —
/// re-generating deletes and recreates this row rather than keeping history.
#[derive(Debug, Clone)]
pub struct Walkthrough {
    pub id: String,
    // Row-identifying/audit fields, kept for parity with the `walkthroughs`
    // table and for `Debug` output when troubleshooting, but no caller reads
    // them today: lookups key on the branch string they already have, and the
    // UI doesn't surface timestamps.
    #[allow(dead_code)]
    pub branch: String,
    pub title: Option<String>,
    // The pane title carries `title` and the intent step carries the same
    // narrative as `summary`, so the compact walkthrough pane doesn't render
    // this separately; kept for round-trip fidelity with `save_walkthrough`.
    #[allow(dead_code)]
    pub summary: Option<String>,
    pub status: WalkthroughStatus,
    pub error: Option<String>,
    #[allow(dead_code)]
    pub created_at: String,
    #[allow(dead_code)]
    pub updated_at: String,
}

/// A single ordered step of a walkthrough (`walkthrough_steps` table),
/// anchored to a file and optional line range.
#[derive(Debug, Clone)]
pub struct WalkthroughStep {
    pub id: String,
    /// Foreign key to the owning `Walkthrough`, kept for parity with the
    /// `walkthrough_steps` table; steps are always accessed already scoped to
    /// their walkthrough (via `get_walkthrough`), so nothing re-derives it.
    #[allow(dead_code)]
    pub walkthrough_id: String,
    /// Display order, kept for parity with the table; the UI reads steps
    /// from the already-ordered `Vec` `get_walkthrough` returns instead of
    /// re-sorting by this field.
    #[allow(dead_code)]
    pub seq: i64,
    pub file_path: String,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub kind: WalkthroughStepKind,
    pub title: String,
    pub body: String,
}

/// A step as supplied when saving a completed walkthrough — no `id` or
/// `walkthrough_id`, since the store assigns those (`seq` is likewise
/// implied by the slice's order, not repeated here).
#[derive(Debug, Clone)]
pub struct NewWalkthroughStep {
    pub file_path: String,
    pub line_start: Option<i64>,
    pub line_end: Option<i64>,
    pub kind: WalkthroughStepKind,
    pub title: String,
    pub body: String,
}

/// Kill a generation that has been running longer than this. Claude writes
/// its result through the MCP tool well before this on any reasonable PR;
/// past it we assume the session is wedged.
pub const GENERATION_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Tools the headless generation session may use. Both `save_walkthrough`
/// tool-name forms are listed so the session works whether the MCP server
/// comes from the bundled `--mcp-config` below (dogfooding: `conductor`) or
/// from the user's installed marketplace plugin
/// (`plugin_conductor_conductor`).
///
/// This session reads a PR's diff, which may be adversarial (a malicious
/// contributor's prompt-injection attempt), so the git subcommands are
/// restricted to read-only ones — no `git push`, `git remote add`, etc. — to
/// close off any exfiltration path that would otherwise be reachable purely
/// through allowedTools.
const GENERATION_ALLOWED_TOOLS: &str = "mcp__conductor__save_walkthrough,\
mcp__plugin_conductor_conductor__save_walkthrough,\
Read,Grep,Glob,\
Bash(git diff:*),Bash(git log:*),Bash(git show:*),Bash(git merge-base:*),\
Bash(git rev-parse:*),Bash(git status:*),Bash(git branch:*)";

/// What a [`WalkthroughGeneration::poll`] observed about the child process.
pub enum GenerationPoll {
    /// Still running (and within the timeout).
    Running,
    /// Exited with success. Whether a walkthrough was actually saved is up
    /// to the database row — the caller must check `walkthroughs.status`.
    Exited,
    /// Exited with a non-zero status, or polling the process failed.
    Failed(String),
    /// Ran past [`GENERATION_TIMEOUT`]; the process has been killed.
    TimedOut,
}

/// Handle to an in-flight walkthrough generation: the headless `claude`
/// child plus enough context to report failures against the right branch.
pub struct WalkthroughGeneration {
    pub branch: String,
    child: Child,
    started: Instant,
    /// Where the child's stdout/stderr go — named in error messages so the
    /// user can inspect what the session actually did.
    pub log_path: PathBuf,
}

impl WalkthroughGeneration {
    /// Non-blocking check of the child process, enforcing the timeout.
    pub fn poll(&mut self) -> GenerationPoll {
        match self.child.try_wait() {
            Ok(Some(status)) if status.success() => GenerationPoll::Exited,
            Ok(Some(status)) => GenerationPoll::Failed(format!(
                "claude exited with {status} (log: {})",
                self.log_path.display()
            )),
            Ok(None) => {
                if self.started.elapsed() > GENERATION_TIMEOUT {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    GenerationPoll::TimedOut
                } else {
                    GenerationPoll::Running
                }
            }
            Err(e) => GenerationPoll::Failed(format!("failed to poll claude process: {e}")),
        }
    }

    /// Kill the child (used when the app shuts down mid-generation).
    pub fn abort(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Build the `--mcp-config` JSON registering the repo-bundled conductor MCP
/// server, or `None` when this repository doesn't carry the server (e.g. a
/// non-conductor repo reviewed by a marketplace-plugin install — there the
/// headless session inherits the user's ambient plugin MCP instead).
fn bundled_mcp_config(repo_root: &Path, db_path: &Path) -> Option<String> {
    let server_dir = repo_root.join("plugins/conductor/mcp/conductor-comment");
    let dist = server_dir.join("dist/index.js");
    let src = server_dir.join("src/index.ts");
    let (command, args) = if dist.is_file() {
        ("node", vec![dist.to_string_lossy().into_owned()])
    } else if src.is_file() {
        (
            "npx",
            vec![
                "--yes".to_string(),
                "tsx".to_string(),
                src.to_string_lossy().into_owned(),
            ],
        )
    } else {
        return None;
    };
    let config = serde_json::json!({
        "mcpServers": {
            "conductor": {
                "command": command,
                "args": args,
                "env": { "CONDUCTOR_DB_PATH": db_path.to_string_lossy() },
            }
        }
    });
    Some(config.to_string())
}

/// The generation instructions handed to the headless session. Mirrors
/// `plugins/conductor/commands/conductor-walkthrough.md` (kept for
/// marketplace-plugin users) but is embedded here so generation works
/// regardless of which slash commands the installed plugin cache has.
fn generation_prompt(branch: &str, base_ref: Option<&str>, language: Option<&str>) -> String {
    let base_hint = match base_ref {
        Some(b) => format!("The base branch is `{b}`."),
        None => "Determine the base branch (origin/HEAD, usually main).".to_string(),
    };
    let language_hint = match language {
        Some(lang) => format!(
            "\n\nWrite the walkthrough title, summary, and every step's title and body in {lang}."
        ),
        None => String::new(),
    };
    format!(
        "Read this branch's merge-base diff against its base branch and build a reviewer \
walkthrough (an ordered tour of the change), then save it with the conductor MCP server's \
`save_walkthrough` tool.\n\
\n\
{base_hint} The branch under review is `{branch}`. Use `git diff <base>...HEAD` (three-dot, \
merge-base) to see the change. Read not only the changed files but, where needed, their \
callers/callees so you understand the whole picture.\n\
\n\
Order the steps as a story: intent -> core -> ripple -> test.\n\
- intent: what this change wanted to achieve (background, motivation).\n\
- core: what was changed to achieve it, and its effect on existing code. Do NOT compare \
alternative designs — reviewers ask those questions themselves.\n\
- ripple: knock-on changes (call-site updates, config/schema follow-ups).\n\
- test: a summary of what behavior each test verifies, detailed enough that a reviewer can \
skip reading the full test diff.\n\
\n\
Each step needs: file_path (repo-relative), optional line_start/line_end (new-side line \
numbers), kind, title, body. There is no fixed step count — match the actual change.\n\
\n\
When all steps are assembled, call the `save_walkthrough` tool exactly once with: \
branch = `{branch}`, a one-line title, a short summary, and the steps (seq starting at 0). \
Then report the step count and kind breakdown briefly and stop.{language_hint}"
    )
}

/// Spawn the headless generation session in `worktree_path`.
///
/// The caller must have inserted the `generating` row first
/// ([`crate::review_store::ReviewStore::begin_walkthrough`]); on spawn
/// failure it should flip that row to `failed`.
pub fn spawn_generation(
    repo_root: &Path,
    worktree_path: &Path,
    db_path: &Path,
    branch: &str,
    base_ref: Option<&str>,
    model: Option<&str>,
    language: Option<&str>,
) -> Result<WalkthroughGeneration> {
    let log_path = db_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!("walkthrough-{}.log", branch.replace('/', "-")));
    let log_file = std::fs::File::create(&log_path)
        .with_context(|| format!("failed to create log file {}", log_path.display()))?;
    let log_err = log_file
        .try_clone()
        .context("failed to clone log file handle")?;

    let mut cmd = Command::new("claude");
    cmd.arg("-p")
        .arg(generation_prompt(branch, base_ref, language))
        .arg("--allowedTools")
        .arg(GENERATION_ALLOWED_TOOLS)
        .current_dir(worktree_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err));
    if let Some(model) = model {
        cmd.arg("--model").arg(model);
    }
    if let Some(config) = bundled_mcp_config(repo_root, db_path) {
        cmd.arg("--mcp-config").arg(config);
    }

    let child = cmd.spawn().context(
        "failed to launch `claude` — is Claude Code installed and on PATH?",
    )?;
    Ok(WalkthroughGeneration {
        branch: branch.to_string(),
        child,
        started: Instant::now(),
        log_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── bundled_mcp_config: dist takes priority over src, else None ──

    #[test]
    fn bundled_mcp_config_prefers_compiled_dist_over_ts_source() {
        let dir = tempfile::tempdir().unwrap();
        let server_dir = dir.path().join("plugins/conductor/mcp/conductor-comment");
        std::fs::create_dir_all(server_dir.join("dist")).unwrap();
        std::fs::create_dir_all(server_dir.join("src")).unwrap();
        std::fs::write(server_dir.join("dist/index.js"), "").unwrap();
        std::fs::write(server_dir.join("src/index.ts"), "").unwrap();

        let config = bundled_mcp_config(dir.path(), &dir.path().join("db.sqlite")).unwrap();
        assert!(config.contains("\"node\""));
        assert!(config.contains("dist/index.js"));
    }

    #[test]
    fn bundled_mcp_config_falls_back_to_ts_source_via_npx_tsx() {
        let dir = tempfile::tempdir().unwrap();
        let server_dir = dir.path().join("plugins/conductor/mcp/conductor-comment");
        std::fs::create_dir_all(server_dir.join("src")).unwrap();
        std::fs::write(server_dir.join("src/index.ts"), "").unwrap();

        let config = bundled_mcp_config(dir.path(), &dir.path().join("db.sqlite")).unwrap();
        assert!(config.contains("\"npx\""));
        assert!(config.contains("tsx"));
        assert!(config.contains("src/index.ts"));
    }

    #[test]
    fn bundled_mcp_config_is_none_when_neither_dist_nor_src_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert!(bundled_mcp_config(dir.path(), &dir.path().join("db.sqlite")).is_none());
    }

    // ── allowedTools: MCP tool names present, git restricted to read-only ──

    #[test]
    fn generation_allowed_tools_lists_both_save_walkthrough_tool_name_forms() {
        assert!(GENERATION_ALLOWED_TOOLS.contains("mcp__conductor__save_walkthrough"));
        assert!(GENERATION_ALLOWED_TOOLS.contains("mcp__plugin_conductor_conductor__save_walkthrough"));
    }

    #[test]
    fn generation_allowed_tools_has_no_write_git_subcommands() {
        // Matches the FIX-2 restriction: no bare `Bash(git:*)`, and none of
        // the write-capable subcommands that would open an exfiltration path
        // for an adversarial PR diff (push, remote, fetch with refspec-write
        // side effects, config, etc).
        assert!(!GENERATION_ALLOWED_TOOLS.contains("Bash(git:*)"));
        for write_subcommand in ["push", "remote", "config", "commit", "checkout", "reset"] {
            assert!(
                !GENERATION_ALLOWED_TOOLS.contains(&format!("git {write_subcommand}")),
                "allowedTools should not permit `git {write_subcommand}`"
            );
        }
    }

    // ── generation_prompt: branch name and no-alternatives instruction ──

    #[test]
    fn generation_prompt_includes_the_branch_name() {
        let prompt = generation_prompt("pr-42", Some("main"), None);
        assert!(prompt.contains("pr-42"));
        assert!(prompt.contains("main"));
    }

    #[test]
    fn generation_prompt_tells_the_model_not_to_compare_alternative_designs() {
        let prompt = generation_prompt("pr-42", None, None);
        assert!(prompt.to_lowercase().contains("do not compare"));
        assert!(prompt.contains("Determine the base branch"));
    }

    #[test]
    fn generation_prompt_requests_the_configured_language() {
        let prompt = generation_prompt("pr-42", Some("main"), Some("日本語"));
        assert!(prompt.contains("in 日本語"));
        // No language configured → no language directive at all.
        let unconstrained = generation_prompt("pr-42", Some("main"), None);
        assert!(!unconstrained.contains("Write the walkthrough title"));
    }
}
