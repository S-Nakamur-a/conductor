//! Pluggable AI caller abstraction.
//!
//! Every LLM provider satisfies one tiny seam: `(system_prompt, user_message) -> String`.
//! Conductor owns the prompt assembly *and* the response parsing — providers only ever
//! return raw text. That is what keeps the user-facing extension point trivial: the
//! output format never crosses the provider boundary.
//!
//! The one built-in provider ([`GeminiCaller`]) ships inside the binary. The
//! user-extensible path is [`CommandCaller`]: the user names a command in their config
//! and Conductor speaks a minimal, stable protocol to it. Conductor never hard-codes
//! any CLI of its own — every other provider is whatever the config names.
//!
//! ## External LLM Command Protocol (v2)
//!
//! `[api] command` names the **AI tool itself**: any CLI that takes a prompt and
//! prints a completion. It is not a place for per-task behaviour — which output
//! format a task needs, whether the model may use tools, and which directory it
//! should look at are all decided by the *feature* asking for the completion, so
//! no wrapper script is needed to adapt one to the other.
//!
//! - **Invocation:** Conductor runs the configured argv directly (no shell).
//! - **Placeholders:** any argument may contain `{prompt}` (the assembled
//!   prompt) or `{workdir}` (the task's directory). A tool that takes its prompt
//!   as a positional argument puts `{prompt}` where that argument goes; one that
//!   reads stdin needs no placeholder at all.
//! - **Input:** with `{prompt}` present the prompt goes in that argument and
//!   stdin is closed immediately. Without it, the prompt (system prompt + two
//!   newlines + user message) is written to **stdin** as UTF-8, then closed.
//! - **Working directory:** the child runs in the task's directory, so a tool
//!   that resolves paths relatively (`-w .`) lands in the right place even
//!   without `{workdir}`.
//! - **Output:** the command writes the model's completion to **stdout** and
//!   Conductor parses it on its own side. The command does no formatting and no
//!   JSON extraction — the feature's own parser tolerates fences and prose.
//! - **Exit code:** `0` = success. Non-zero = failure; stderr is surfaced in the error.
//! - **stderr:** diagnostics only, never parsed.
//! - **Timeout / cancel:** Conductor enforces a per-task wall-clock timeout (see
//!   [`TaskEnv`]) and kills the child if the user cancels. The command is simply
//!   killed; it is not notified.
//!
//! ### What belongs to the feature, not to the command
//!
//! Each task writes its own system prompt, and that is where its constraints
//! live: smart-worktree naming tells the model not to use tools and to answer
//! with one JSON object, while walkthrough generation tells it the opposite —
//! go read the diff. Retrying an off-format reply is likewise the feature's
//! call, because only it knows what a retry costs (seconds for a branch name,
//! minutes for a walkthrough).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config::ApiConfig;

/// Hard-coded token budget for the (single) smart-worktree generation task.
/// Lives here because it is a Gemini-request knob, not part of the seam.
const GEMINI_MAX_TOKENS: u32 = 1024;

/// How often [`CommandCaller`] wakes to check for child exit / cancel / timeout.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A provider that turns a prompt into raw completion text.
///
/// The caller owns the system prompt and owns parsing the result; an implementation
/// must only return the model's text (or an error). `cancel` should be honored where
/// the underlying call can be interrupted (e.g. a subprocess).
pub trait AiCaller {
    fn complete(
        &self,
        system_prompt: &str,
        user_message: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String>;
}

/// Built-in: Google Gemini HTTP API.
pub struct GeminiCaller {
    pub model: String,
    pub max_tokens: u32,
}

impl AiCaller for GeminiCaller {
    fn complete(
        &self,
        system_prompt: &str,
        user_message: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        crate::gemini_api::call_messages_api(
            system_prompt,
            user_message,
            Some(&self.model),
            self.max_tokens,
        )
        .map_err(|e| format!("{e}"))
    }
}

// There is deliberately no built-in Claude provider — Conductor does not spawn a
// `claude` process for completions. Wanting one is fine; it is just config, and
// needs no wrapper script:
//
//     [api]
//     provider = "command"
//     command = ["claude", "-p", "{prompt}"]
//
// Any other prompt-in/completion-out CLI is configured the same way.

/// Placeholder replaced with the assembled prompt. Its presence also switches
/// prompt delivery from stdin to argv.
const PROMPT_PLACEHOLDER: &str = "{prompt}";

/// Placeholder replaced with the task's working directory.
const WORKDIR_PLACEHOLDER: &str = "{workdir}";

/// User-extensible: an external command speaking the protocol in the module docs.
pub struct CommandCaller {
    /// argv: `cmd[0]` is the executable, the rest are fixed arguments, any of
    /// which may contain `{prompt}` / `{workdir}`.
    pub cmd: Vec<String>,
    /// Wall-clock timeout in seconds. `0` disables the timeout.
    pub timeout_secs: u64,
    /// Directory to run the command in — the repository or worktree the task is
    /// about, and what `{workdir}` expands to. `None` inherits Conductor's cwd.
    pub working_dir: Option<PathBuf>,
}

/// Expand the placeholders in a configured argv.
///
/// Returns the expanded argv and whether `{prompt}` was found — the caller
/// needs that to decide between argv and stdin delivery, and "no placeholder
/// anywhere" has to mean stdin rather than a command that silently receives no
/// prompt at all.
fn expand_argv(cmd: &[String], prompt: &str, workdir: Option<&Path>) -> (Vec<String>, bool) {
    let workdir = workdir.map(|d| d.to_string_lossy().into_owned());
    let mut saw_prompt = false;
    let expanded = cmd
        .iter()
        .map(|arg| {
            let mut out = arg.clone();
            if out.contains(PROMPT_PLACEHOLDER) {
                saw_prompt = true;
                out = out.replace(PROMPT_PLACEHOLDER, prompt);
            }
            if let Some(dir) = &workdir {
                out = out.replace(WORKDIR_PLACEHOLDER, dir);
            }
            out
        })
        .collect();
    (expanded, saw_prompt)
}

impl AiCaller for CommandCaller {
    fn complete(
        &self,
        system_prompt: &str,
        user_message: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String> {
        let payload = format!("{system_prompt}\n\n{user_message}");
        let (argv, prompt_in_argv) =
            expand_argv(&self.cmd, &payload, self.working_dir.as_deref());
        let program = argv
            .first()
            .ok_or_else(|| "AI command is empty".to_string())?
            .clone();
        log::info!(
            "AI caller: using external command '{program}' (prompt via {})",
            if prompt_in_argv { "argv" } else { "stdin" }
        );

        let mut command = Command::new(&program);
        command.args(&argv[1..]);
        if let Some(dir) = &self.working_dir {
            command.current_dir(dir);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn AI command '{program}': {e}"))?;

        // Drain stdout/stderr on their own threads so a chatty command (e.g. a tool that
        // logs progress to stderr) can't fill a pipe buffer and deadlock before exiting.
        let stdout_reader = child.stdout.take().map(spawn_pipe_reader);
        let stderr_reader = child.stderr.take().map(spawn_pipe_reader);

        // Deliver the prompt. When it already went out as an argument, stdin is
        // still closed immediately (dropping the handle) — a tool that reads
        // stdin anyway must see EOF rather than block forever, and sending the
        // prompt twice would be worse than not sending it at all.
        if let Some(mut stdin) = child.stdin.take()
            && !prompt_in_argv
        {
            stdin
                .write_all(payload.as_bytes())
                .map_err(|e| format!("Failed to write to AI command stdin: {e}"))?;
        }

        // Poll for exit, honoring cancellation and the wall-clock timeout.
        let start = Instant::now();
        let exit_status = loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Cancelled".to_string());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if self.timeout_secs > 0 && start.elapsed().as_secs() >= self.timeout_secs {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(format!(
                            "AI command '{program}' timed out after {}s",
                            self.timeout_secs
                        ));
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(e) => return Err(format!("Failed to wait on AI command: {e}")),
            }
        };

        let stdout = join_pipe_reader(stdout_reader);
        let stderr = join_pipe_reader(stderr_reader);

        if !exit_status.success() {
            return Err(format!(
                "AI command '{program}' failed ({exit_status}): {}",
                tail_chars(stderr.trim(), 500)
            ));
        }
        if stdout.trim().is_empty() {
            return Err(format!("AI command '{program}' returned empty output"));
        }
        Ok(stdout)
    }
}

/// Read a child pipe to end on a worker thread, decoding lossily as UTF-8.
fn spawn_pipe_reader<R: Read + Send + 'static>(mut pipe: R) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// Join a pipe-reader thread, yielding its captured text (empty on join failure).
fn join_pipe_reader(handle: Option<JoinHandle<String>>) -> String {
    handle.and_then(|h| h.join().ok()).unwrap_or_default()
}

/// Last `n` characters of `s` (char-boundary safe; no intermediate allocation).
fn tail_chars(s: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    match s.char_indices().nth_back(n - 1) {
        Some((i, _)) => &s[i..],
        None => s,
    }
}

/// What a task needs from the AI beyond its prompt: how long it may take and
/// which directory it is about.
///
/// Both are per-task, not per-provider. Smart-worktree naming is a few seconds of
/// pure text generation; a walkthrough is minutes of an agent reading a diff. One
/// `[api] command_timeout_secs` cannot serve both, and only the second needs a
/// working directory at all.
#[derive(Debug, Clone, Default)]
pub struct TaskEnv {
    /// Overrides `[api] command_timeout_secs` when set. `0` disables the timeout.
    pub timeout_secs: Option<u64>,
    /// Directory to run an external command in — the worktree the task concerns.
    pub working_dir: Option<PathBuf>,
}

/// Build the configured AI caller for a task.
///
/// Providers (`[api] provider`). Each provider stands alone — a failure surfaces to the
/// user rather than silently falling back to another provider.
/// - `"gemini"` (default): Gemini HTTP API.
/// - `"command"`: the user's external command (`[api] command`).
///
/// There is deliberately **no built-in `claude` provider**: spawning the `claude` CLI
/// from inside Conductor is not allowed anywhere in this codebase. Driving Claude is
/// exactly what `provider = "command"` is for — the user names the CLI directly and
/// Conductor never needs to know which model is behind it.
///
/// Note that `"gemini"` is a plain HTTP completion: it cannot read the repository, so
/// a task that needs the code (walkthrough generation) only works behind `"command"`
/// pointed at an agentic CLI.
pub fn build_caller(api: &ApiConfig, env: &TaskEnv) -> Result<Box<dyn AiCaller>, String> {
    match api.provider.trim().to_lowercase().as_str() {
        "gemini" => Ok(Box::new(GeminiCaller {
            model: api.model.clone(),
            max_tokens: GEMINI_MAX_TOKENS,
        })),
        "command" => {
            if api.command.is_empty() {
                return Err(
                    "provider = \"command\" but [api] command is empty; set command = [\"...\"]"
                        .to_string(),
                );
            }
            Ok(Box::new(CommandCaller {
                cmd: api.command.clone(),
                timeout_secs: env.timeout_secs.unwrap_or(api.command_timeout_secs),
                working_dir: env.working_dir.clone(),
            }))
        }
        other => Err(format!(
            "unknown AI provider '{other}' (expected: gemini, command)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(provider: &str) -> ApiConfig {
        ApiConfig {
            provider: provider.to_string(),
            ..Default::default()
        }
    }

    // ── build_caller selection / validation ──────────────────────────

    #[test]
    fn build_caller_accepts_known_providers() {
        assert!(build_caller(&api("gemini"), &TaskEnv::default()).is_ok());
        assert!(build_caller(&ApiConfig::default(), &TaskEnv::default()).is_ok());
    }

    #[test]
    fn build_caller_is_case_and_whitespace_insensitive() {
        assert!(build_caller(&api("GEMINI"), &TaskEnv::default()).is_ok());
        assert!(build_caller(&api("  gemini  "), &TaskEnv::default()).is_ok());
    }

    /// Conductor must never spawn the `claude` CLI itself, so the provider name
    /// that used to do exactly that is now an ordinary unknown value — and the
    /// error has to point at `command`, which is how a Claude-backed setup is
    /// wired instead.
    #[test]
    fn build_caller_rejects_the_removed_claude_provider() {
        let err = build_caller(&api("claude"), &TaskEnv::default()).err().unwrap();
        assert!(err.contains("claude"), "should echo the bad value: {err}");
        assert!(err.contains("command"), "should point at the way in: {err}");
    }

    #[test]
    fn build_caller_rejects_unknown_provider() {
        let err = build_caller(&api("ollama"), &TaskEnv::default()).err().unwrap();
        assert!(err.contains("ollama"), "should echo the bad value: {err}");
        assert!(err.contains("gemini"), "should list valid values: {err}");
    }

    #[test]
    fn build_caller_rejects_empty_command() {
        let cfg = ApiConfig {
            provider: "command".to_string(),
            command: Vec::new(),
            ..Default::default()
        };
        let err = build_caller(&cfg, &TaskEnv::default()).err().unwrap();
        assert!(err.contains("command"), "actionable message: {err}");
    }

    #[test]
    fn build_caller_accepts_nonempty_command() {
        let cfg = ApiConfig {
            provider: "command".to_string(),
            command: vec!["cat".to_string()],
            ..Default::default()
        };
        assert!(build_caller(&cfg, &TaskEnv::default()).is_ok());
    }

    // ── tail_chars ───────────────────────────────────────────────────

    #[test]
    fn tail_chars_takes_last_n() {
        assert_eq!(tail_chars("hello", 3), "llo");
        assert_eq!(tail_chars("hi", 5), "hi");
        assert_eq!(tail_chars("hi", 0), "");
        // char-boundary safe with multibyte input
        assert_eq!(tail_chars("あいうえお", 2), "えお");
    }

    // ── CommandCaller (real subprocess, Unix only) ───────────────────

    #[cfg(unix)]
    mod command {
        use super::*;

        fn sh(script: &str, timeout_secs: u64) -> CommandCaller {
            CommandCaller {
                cmd: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
                timeout_secs,
                working_dir: None,
            }
        }

        #[test]
        fn echoes_prompt_via_stdin() {
            let caller = sh("cat", 5);
            let cancel = Arc::new(AtomicBool::new(false));
            let out = caller.complete("SYS", "USER", &cancel).unwrap();
            assert!(out.contains("SYS") && out.contains("USER"), "got: {out}");
        }

        /// The command runs in the directory the task is about. This is the only
        /// way an agentic command can reach the code it is being asked about, so
        /// it is part of the protocol, not a detail.
        #[test]
        fn runs_in_the_task_working_directory() {
            let dir = tempfile::tempdir().unwrap();
            let caller = CommandCaller {
                cmd: vec!["sh".to_string(), "-c".to_string(), "pwd".to_string()],
                timeout_secs: 5,
                working_dir: Some(dir.path().to_path_buf()),
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let out = caller.complete("s", "u", &cancel).unwrap();
            // macOS reports /private/var… for a /var… tempdir, so compare the
            // resolved forms rather than the strings we started with.
            let reported = std::fs::canonicalize(out.trim()).unwrap();
            assert_eq!(reported, std::fs::canonicalize(dir.path()).unwrap());
        }

        /// A task that names a timeout gets that one; the config value only fills in
        /// when it doesn't. Walkthrough generation depends on this — it runs for
        /// minutes under a `command_timeout_secs` meant for a few seconds of naming.
        ///
        /// Asserted by behaviour rather than by inspecting the built caller: the
        /// config disables the timeout entirely, so the command can only be killed
        /// if the task's own value is what reached it.
        #[test]
        fn task_timeout_overrides_the_configured_one() {
            let cfg = ApiConfig {
                provider: "command".to_string(),
                command: vec!["sh".to_string(), "-c".to_string(), "sleep 30".to_string()],
                command_timeout_secs: 0,
                ..Default::default()
            };
            let caller = build_caller(
                &cfg,
                &TaskEnv {
                    timeout_secs: Some(1),
                    working_dir: None,
                },
            )
            .unwrap();

            let start = Instant::now();
            let err = caller
                .complete("s", "u", &Arc::new(AtomicBool::new(false)))
                .unwrap_err();
            assert!(err.contains("timed out after 1s"), "got: {err}");
            assert!(start.elapsed() < Duration::from_secs(10));
        }

        /// A tool that takes its prompt as a positional argument (`claude -p`,
        /// and most agentic CLIs) is named directly in the config; `{prompt}` is
        /// what makes that possible without a wrapper script.
        #[test]
        fn prompt_placeholder_delivers_via_argv() {
            let caller = CommandCaller {
                // `printf %s` echoes its argument, so stdout is exactly what
                // landed in argv.
                cmd: vec![
                    "printf".to_string(),
                    "%s".to_string(),
                    "PRE[{prompt}]POST".to_string(),
                ],
                timeout_secs: 5,
                working_dir: None,
            };
            let out = caller
                .complete("SYS", "USER", &Arc::new(AtomicBool::new(false)))
                .unwrap();
            assert_eq!(out, "PRE[SYS\n\nUSER]POST");
        }

        /// …and the prompt must not also arrive on stdin, or the model sees it
        /// twice. `cat` would append whatever stdin carried.
        #[test]
        fn prompt_placeholder_leaves_stdin_empty() {
            let caller = CommandCaller {
                cmd: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "printf 'argv=%s;' \"$1\"; printf 'stdin='; cat".to_string(),
                    "sh".to_string(),
                    "{prompt}".to_string(),
                ],
                timeout_secs: 5,
                working_dir: None,
            };
            let out = caller
                .complete("SYS", "USER", &Arc::new(AtomicBool::new(false)))
                .unwrap();
            assert_eq!(out, "argv=SYS\n\nUSER;stdin=");
        }

        /// No placeholder anywhere keeps the stdin delivery that stdin-shaped
        /// tools (`ollama run …`) rely on.
        #[test]
        fn without_a_placeholder_the_prompt_still_goes_to_stdin() {
            let caller = sh("cat", 5);
            let out = caller
                .complete("SYS", "USER", &Arc::new(AtomicBool::new(false)))
                .unwrap();
            assert_eq!(out, "SYS\n\nUSER");
        }

        /// `{workdir}` expands to the task's directory, so a tool that wants it
        /// as an explicit flag rather than as its cwd gets it without the user
        /// hard-coding any one worktree in their config.
        #[test]
        fn workdir_placeholder_expands_to_the_task_directory() {
            let dir = tempfile::tempdir().unwrap();
            let caller = CommandCaller {
                cmd: vec![
                    "printf".to_string(),
                    "%s".to_string(),
                    "{workdir}".to_string(),
                ],
                timeout_secs: 5,
                working_dir: Some(dir.path().to_path_buf()),
            };
            let out = caller
                .complete("s", "u", &Arc::new(AtomicBool::new(false)))
                .unwrap();
            assert_eq!(
                std::fs::canonicalize(out.trim()).unwrap(),
                std::fs::canonicalize(dir.path()).unwrap()
            );
        }

        #[test]
        fn nonzero_exit_surfaces_stderr() {
            let caller = sh("echo boom >&2; exit 1", 5);
            let cancel = Arc::new(AtomicBool::new(false));
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert!(err.contains("boom"), "stderr tail: {err}");
            assert!(err.contains("failed"));
        }

        #[test]
        fn empty_success_is_an_error() {
            let caller = sh("exit 0", 5);
            let cancel = Arc::new(AtomicBool::new(false));
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert!(err.contains("empty"), "got: {err}");
        }

        #[test]
        fn times_out_without_waiting_for_the_command() {
            let caller = sh("sleep 5", 1);
            let cancel = Arc::new(AtomicBool::new(false));
            let start = Instant::now();
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert!(err.contains("timed out"), "got: {err}");
            assert!(start.elapsed() < Duration::from_secs(4), "should not wait 5s");
        }

        #[test]
        fn preset_cancel_returns_immediately() {
            let caller = sh("sleep 5", 0);
            let cancel = Arc::new(AtomicBool::new(true));
            let start = Instant::now();
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert_eq!(err, "Cancelled");
            assert!(start.elapsed() < Duration::from_secs(4));
        }

        #[test]
        fn missing_program_is_a_spawn_error() {
            let caller = CommandCaller {
                cmd: vec!["definitely_not_a_real_binary_xyzzy".to_string()],
                timeout_secs: 5,
                working_dir: None,
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert!(err.contains("definitely_not_a_real_binary_xyzzy"), "got: {err}");
        }
    }
}
