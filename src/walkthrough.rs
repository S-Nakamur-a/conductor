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

use std::collections::HashMap;
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
    /// The branch tip (HEAD commit OID) this walkthrough was generated
    /// against, or `None` for rows predating commit tracking. A regenerate
    /// request whose current HEAD matches this is skipped — the diff, and so
    /// the walkthrough, hasn't changed.
    pub head_commit: Option<String>,
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
///
/// The order of the slice is the walkthrough's order, deliberately: the MCP
/// tool also accepts a `seq` per step, but trusting it lets a caller that
/// numbers steps per-kind (intent 0,1 / core 0,1,2 / …) interleave the tour
/// while still reporting success. See `ReviewStore::save_walkthrough`.
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

/// Tools the headless generation session may use. `spawn_generation` always
/// passes `--strict-mcp-config`, so the session sees only the MCP server
/// registered by its own `--mcp-config` — whose server name is always
/// `conductor` (see `self_mcp_config`) — and never the user's ambient
/// marketplace-plugin server. That makes the tool-name form unambiguous:
/// only `mcp__conductor__*` needs listing here.
///
/// `create_comment` lets the session drop inline review comments on the
/// genuinely-hard-to-understand spots it finds while touring the diff, so a
/// single generation produces both the Explorer walkthrough and the in-Viewer
/// 💬 annotations. It only writes to the local review DB (no network), so
/// unlike a write-capable git subcommand it opens no exfiltration path.
///
/// This session reads a PR's diff, which may be adversarial (a malicious
/// contributor's prompt-injection attempt), so the git subcommands are
/// restricted to read-only ones — no `git push`, `git remote add`, etc. — to
/// close off any exfiltration path that would otherwise be reachable purely
/// through allowedTools.
const GENERATION_ALLOWED_TOOLS: &str = "mcp__conductor__save_walkthrough,\
mcp__conductor__create_comment,\
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

#[cfg(test)]
impl WalkthroughGeneration {
    /// Wrap an arbitrary child process in a generation handle. Lets the
    /// registry tests below drive real process lifecycles (exit, kill,
    /// timeout) without launching a `claude` session.
    fn for_test(branch: &str, child: Child, started: Instant) -> Self {
        Self {
            branch: branch.to_string(),
            child,
            started,
            log_path: PathBuf::from("/dev/null"),
        }
    }
}

/// A generation that stopped running, as handed back by
/// [`WalkthroughGenerations::take_finished`]. Carries the context the caller
/// needs to reconcile the database row it left behind — which branch it was
/// for, and where its log is — because the handle itself is gone by then.
pub struct FinishedGeneration {
    pub branch: String,
    pub log_path: PathBuf,
    pub outcome: GenerationPoll,
}

/// Every walkthrough generation in flight in this Conductor instance, at most
/// one per branch.
///
/// Keyed by branch, not by "one at a time", because the branch is the only
/// thing a generation actually contends for: `begin_walkthrough` deletes and
/// re-inserts the `walkthroughs` row for its branch and `save_walkthrough`
/// replaces it, so two sessions on one branch would race for a single row and
/// the loser's steps would vanish. Sessions on *different* branches — which
/// means different worktrees, since git won't check one branch out twice —
/// touch disjoint rows, disjoint log files, and a database that is already
/// WAL + `busy_timeout` (see `review_store::schema`), so they can run side by
/// side. Serializing them was over-broad: it made a reviewer touring one
/// worktree unable to start a walkthrough in another.
#[derive(Default)]
pub struct WalkthroughGenerations {
    by_branch: HashMap<String, WalkthroughGeneration>,
}

impl WalkthroughGenerations {
    /// Whether a generation for `branch` is currently in flight.
    pub fn is_generating(&self, branch: &str) -> bool {
        self.by_branch.contains_key(branch)
    }

    /// Register a freshly spawned generation. The caller is expected to have
    /// checked [`Self::is_generating`] first: inserting over a live handle
    /// would drop (and orphan) the running `claude` child and strand its
    /// branch's row in `generating` forever.
    pub fn insert(&mut self, generation: WalkthroughGeneration) {
        debug_assert!(
            !self.is_generating(&generation.branch),
            "would orphan the in-flight generation for {}",
            generation.branch
        );
        self.by_branch
            .insert(generation.branch.clone(), generation);
    }

    /// Poll every in-flight generation, removing the ones that are no longer
    /// running and returning what each one did.
    ///
    /// Removal is what makes a wedged or externally-killed session
    /// self-healing: whatever the child did — saved and exited, crashed, was
    /// `kill`ed from outside, or ran past [`GENERATION_TIMEOUT`] — its slot is
    /// released here, so the next request for that branch starts a fresh
    /// session instead of being told one is already running.
    pub fn take_finished(&mut self) -> Vec<FinishedGeneration> {
        let mut finished = Vec::new();
        self.by_branch.retain(|branch, generation| {
            let outcome = generation.poll();
            if matches!(outcome, GenerationPoll::Running) {
                return true;
            }
            finished.push(FinishedGeneration {
                branch: branch.clone(),
                log_path: generation.log_path.clone(),
                outcome,
            });
            false
        });
        finished
    }

    /// Whether nothing is in flight (lets the caller skip polling entirely).
    pub fn is_empty(&self) -> bool {
        self.by_branch.is_empty()
    }

    /// Kill every in-flight generation (used when the app shuts down).
    pub fn abort_all(&mut self) {
        for (_, mut generation) in self.by_branch.drain() {
            generation.abort();
        }
    }
}

/// Build the `--mcp-config` JSON that registers conductor's own `mcp-serve`
/// subcommand as the headless generation session's MCP server.
///
/// Points at [`std::env::current_exe`] rather than a path inside the repo, so
/// it works in any repository, not just conductor's own — see
/// `src/mcp_serve/mod.rs`'s module doc for why the server is embedded in the
/// binary at all.
///
/// Both paths are rejected outright (`bail!`, not lossy-converted) if they
/// aren't valid UTF-8: `to_string_lossy` would silently substitute U+FFFD
/// and register a server at a path that doesn't exist, so the session would
/// launch tool-less and exit 0 — the exact silent-failure mode this
/// function exists to close off (see `src/refresh_pipe.rs`'s `RefreshPipe::new`
/// for the same pattern).
fn self_mcp_config(db_path: &Path) -> Result<String> {
    let exe = std::env::current_exe().context("failed to resolve conductor's own path")?;
    let exe = exe.to_str().ok_or_else(|| {
        anyhow::anyhow!("conductor's own path is not valid UTF-8: {}", exe.display())
    })?;
    let db_path = db_path.to_str().ok_or_else(|| {
        anyhow::anyhow!("database path is not valid UTF-8: {}", db_path.display())
    })?;
    let config = serde_json::json!({
        "mcpServers": {
            "conductor": {
                "command": exe,
                "args": ["mcp-serve", "--db", db_path],
            }
        }
    });
    Ok(config.to_string())
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
branch = `{branch}`, a one-line title, a summary, and the steps (seq starting at 0).\n\
\n\
The summary is not throwaway text: it is stored as the branch's change summary and shown \
full-panel as Conductor's SUMMARY pseudo-file, so write it like a PR description — what the \
change is for, why these files are touched, and anything a reviewer should know up front \
(including what is deliberately out of scope). Markdown is rendered.\n\
\n\
After saving, for the few spots that are genuinely hard to understand — tricky logic whose \
intent isn't obvious at a glance, a non-obvious tradeoff, or a subtle edge case a reviewer \
could miss — drop an inline comment with the `create_comment` tool, anchored to that \
file_path and its new-side line number(s). Use kind = \"question\", keep each to 1-3 \
sentences, and explain *why* it works / where the subtlety is. This is high-signal and \
low-frequency: a handful per change at most, and none at all when nothing is genuinely \
tricky. Do NOT comment on self-evident changes (renames, boilerplate, formatting, imports).\n\
\n\
Then report the step count, kind breakdown, and how many inline comments you left, briefly, \
and stop.{language_hint}"
    )
}

/// Spawn the headless generation session in `worktree_path`.
///
/// The caller must have inserted the `generating` row first
/// ([`crate::review_store::ReviewStore::begin_walkthrough`]); on spawn
/// failure it should flip that row to `failed`.
pub fn spawn_generation(
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
    // `--strict-mcp-config` is placed *before* `--mcp-config`: the latter
    // takes a space-separated variable-length list of configs
    // (`<configs...>`), so a flag placed right after it risks being
    // swallowed as another config value instead of parsed as its own flag.
    // Together these two are what makes generation work in any repository,
    // not just conductor's own: the session sees only conductor's own MCP
    // server and never a user's ambient (and possibly stale) marketplace
    // plugin server, regardless of what that plugin's cache currently
    // exposes.
    cmd.arg("--strict-mcp-config");
    cmd.arg("--mcp-config").arg(self_mcp_config(db_path)?);

    let child = cmd
        .spawn()
        .context("failed to launch `claude` — is Claude Code installed and on PATH?")?;
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

    // ── self_mcp_config: points at the running binary, carries the db path ──

    #[test]
    fn self_mcp_config_points_at_current_exe_with_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("db.sqlite");

        let config = self_mcp_config(&db_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();

        let current_exe = std::env::current_exe().unwrap();
        assert_eq!(
            parsed["mcpServers"]["conductor"]["command"],
            current_exe.to_str().unwrap()
        );
        assert_eq!(
            parsed["mcpServers"]["conductor"]["args"],
            serde_json::json!(["mcp-serve", "--db", db_path.to_str().unwrap()])
        );
    }

    // ── allowedTools: MCP tool names present, git restricted to read-only ──

    #[test]
    fn generation_allowed_tools_uses_only_the_self_served_form() {
        // `--strict-mcp-config` makes the registered server name always
        // `conductor` (see self_mcp_config), so the marketplace-plugin form
        // is never reachable and must not be listed.
        assert!(GENERATION_ALLOWED_TOOLS.contains("mcp__conductor__save_walkthrough"));
        assert!(
            !GENERATION_ALLOWED_TOOLS.contains("mcp__plugin_conductor_conductor__save_walkthrough")
        );
        assert!(GENERATION_ALLOWED_TOOLS.contains("mcp__conductor__create_comment"));
        assert!(
            !GENERATION_ALLOWED_TOOLS.contains("mcp__plugin_conductor_conductor__create_comment")
        );
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
    fn generation_prompt_instructs_inline_comments_on_hard_spots() {
        let prompt = generation_prompt("pr-42", Some("main"), None);
        // The session must be told to annotate genuinely-tricky spots with
        // create_comment, high-signal and low-frequency.
        assert!(prompt.contains("create_comment"));
        assert!(prompt.contains("hard to understand"));
    }

    // ── WalkthroughGenerations: per-branch, not per-app, exclusion ──

    /// A child that stays alive well past the end of the test.
    fn long_running_child() -> Child {
        test_child("sleep 30")
    }

    fn test_child(script: &str) -> Child {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn test child")
    }

    /// Poll until something finishes. `take_finished` is non-blocking, so a
    /// child that has only just been spawned is legitimately still `Running`
    /// on the first call.
    fn wait_for_finished(generations: &mut WalkthroughGenerations) -> Vec<FinishedGeneration> {
        for _ in 0..400 {
            let finished = generations.take_finished();
            if !finished.is_empty() {
                return finished;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("no generation finished within 10 seconds");
    }

    #[test]
    fn different_branches_generate_side_by_side() {
        // The bug this replaced: one in-flight generation blocked every other
        // worktree's branch, not just its own.
        let mut generations = WalkthroughGenerations::default();
        generations.insert(WalkthroughGeneration::for_test(
            "feature/a",
            long_running_child(),
            Instant::now(),
        ));
        generations.insert(WalkthroughGeneration::for_test(
            "feature/b",
            long_running_child(),
            Instant::now(),
        ));

        assert!(generations.is_generating("feature/a"));
        assert!(generations.is_generating("feature/b"));
        // Neither displaced nor finished the other.
        assert!(generations.take_finished().is_empty());

        generations.abort_all();
        assert!(generations.is_empty());
    }

    #[test]
    fn only_the_same_branch_is_refused() {
        let mut generations = WalkthroughGenerations::default();
        generations.insert(WalkthroughGeneration::for_test(
            "feature/a",
            long_running_child(),
            Instant::now(),
        ));

        // This predicate is the guard `cmd_generate_walkthrough` consults.
        assert!(generations.is_generating("feature/a"));
        assert!(!generations.is_generating("feature/b"));

        generations.abort_all();
    }

    #[test]
    fn a_finished_generation_frees_its_branch() {
        let mut generations = WalkthroughGenerations::default();
        generations.insert(WalkthroughGeneration::for_test(
            "feature/a",
            test_child("exit 0"),
            Instant::now(),
        ));

        let finished = wait_for_finished(&mut generations);
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].branch, "feature/a");
        assert!(matches!(finished[0].outcome, GenerationPoll::Exited));
        assert!(!generations.is_generating("feature/a"));
    }

    #[test]
    fn an_externally_killed_generation_frees_its_branch() {
        // Stale-lock recovery: if the session dies without Conductor asking it
        // to, its slot must be released so the next request can regenerate
        // rather than being told one is already running.
        let mut generations = WalkthroughGenerations::default();
        generations.insert(WalkthroughGeneration::for_test(
            "feature/a",
            test_child("kill -9 $$"),
            Instant::now(),
        ));

        let finished = wait_for_finished(&mut generations);
        assert!(matches!(finished[0].outcome, GenerationPoll::Failed(_)));
        assert!(!generations.is_generating("feature/a"));

        // And the branch accepts a fresh generation immediately.
        generations.insert(WalkthroughGeneration::for_test(
            "feature/a",
            long_running_child(),
            Instant::now(),
        ));
        assert!(generations.is_generating("feature/a"));
        generations.abort_all();
    }

    #[test]
    fn a_wedged_generation_times_out_and_frees_its_branch() {
        let Some(started) = Instant::now().checked_sub(GENERATION_TIMEOUT + Duration::from_secs(1))
        else {
            // Machine booted less than GENERATION_TIMEOUT ago; no instant far
            // enough in the past exists to backdate against.
            return;
        };
        let mut generations = WalkthroughGenerations::default();
        generations.insert(WalkthroughGeneration::for_test(
            "feature/a",
            long_running_child(),
            started,
        ));

        let finished = generations.take_finished();
        assert_eq!(finished.len(), 1);
        assert!(matches!(finished[0].outcome, GenerationPoll::TimedOut));
        assert!(!generations.is_generating("feature/a"));
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
