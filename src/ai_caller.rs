//! Pluggable AI caller abstraction.
//!
//! Every LLM provider satisfies one tiny seam: `(system_prompt, user_message) -> String`.
//! Conductor owns the prompt assembly *and* the response parsing — providers only ever
//! return raw text. That is what keeps the user-facing extension point trivial: the
//! output format never crosses the provider boundary.
//!
//! Built-in providers ([`GeminiCaller`], [`ClaudeCliCaller`]) ship inside the binary.
//! The user-extensible path is [`CommandCaller`]: the user names a command in their
//! config and Conductor speaks a minimal, stable protocol to it.
//!
//! ## External LLM Command Protocol (v1)
//!
//! - **Invocation:** Conductor runs the configured argv directly (no shell).
//! - **Input:** the assembled prompt (system prompt + two newlines + user message) is
//!   written to the command's **stdin** as UTF-8, then stdin is closed (EOF).
//! - **Output:** the command writes the model's completion to **stdout** and Conductor
//!   parses it on its own side. The command does no formatting itself — it only relays
//!   the model's answer (see the output requirement below).
//! - **Exit code:** `0` = success. Non-zero = failure; stderr is surfaced in the error.
//! - **stderr:** diagnostics only, never parsed.
//! - **Timeout / cancel:** Conductor enforces a wall-clock timeout and kills the child if
//!   the user cancels. The command is simply killed; it is not notified.
//!
//! ### Output requirement (current task)
//!
//! Today Conductor's only LLM task is smart-worktree generation, which parses the
//! returned text into `{ branch, prompt, session_name }`. So the relayed text must
//! ultimately *contain* that JSON object. The command author does no JSON work — the
//! model produces it, because Conductor's system prompt instructs it to, and the parser
//! tolerates surrounding prose / code fences. A weak model that ignores the prompt and
//! returns prose will make smart-worktree fail with a parse error; pair `command` with a
//! model capable of following a "reply with only this JSON" instruction.

use std::io::{Read, Write};
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

/// Built-in: the `claude -p` CLI.
///
/// `json_schema` is *injected* by the caller (the task owns its schema), so this
/// provider stays generic. When present, structured output is requested.
pub struct ClaudeCliCaller {
    pub json_schema: Option<String>,
}

impl AiCaller for ClaudeCliCaller {
    fn complete(
        &self,
        system_prompt: &str,
        user_message: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        log::info!("AI caller: using claude -p CLI");

        let mut cmd = Command::new("claude");
        cmd.args(["-p", "--output-format", "json"]);
        if let Some(schema) = &self.json_schema {
            cmd.arg("--json-schema").arg(schema);
        }
        cmd.arg("--system-prompt").arg(system_prompt).arg(user_message);

        let output = cmd
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
        extract_claude_payload(&stdout)
    }
}

/// Pull the model's payload out of `claude -p --output-format json` output.
///
/// `structured_output` (with `--json-schema`) or `result` (plain) carries the text;
/// a string field is returned as-is, an object is re-serialized for the caller's parser.
fn extract_claude_payload(stdout: &str) -> Result<String, String> {
    let wrapper: serde_json::Value = serde_json::from_str(stdout)
        .map_err(|e| format!("Failed to parse claude CLI JSON wrapper: {e}"))?;
    let payload = wrapper
        .get("structured_output")
        .or_else(|| wrapper.get("result"))
        .ok_or_else(|| {
            format!("claude CLI response missing structured_output/result\nRaw: {stdout}")
        })?;
    Ok(match payload.as_str() {
        Some(s) => s.to_string(),
        None => payload.to_string(),
    })
}

/// User-extensible: an external command speaking the v1 protocol (see module docs).
pub struct CommandCaller {
    /// argv: `cmd[0]` is the executable, the rest are fixed arguments.
    pub cmd: Vec<String>,
    /// Wall-clock timeout in seconds. `0` disables the timeout.
    pub timeout_secs: u64,
}

impl AiCaller for CommandCaller {
    fn complete(
        &self,
        system_prompt: &str,
        user_message: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String> {
        let program = self
            .cmd
            .first()
            .ok_or_else(|| "AI command is empty".to_string())?;
        log::info!("AI caller: using external command '{program}'");

        let mut child = Command::new(program)
            .args(&self.cmd[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn AI command '{program}': {e}"))?;

        // Drain stdout/stderr on their own threads so a chatty command (e.g. a tool that
        // logs progress to stderr) can't fill a pipe buffer and deadlock before exiting.
        let stdout_reader = child.stdout.take().map(spawn_pipe_reader);
        let stderr_reader = child.stderr.take().map(spawn_pipe_reader);

        // Write the assembled prompt and close stdin (EOF) so the command can proceed.
        if let Some(mut stdin) = child.stdin.take() {
            let payload = format!("{system_prompt}\n\n{user_message}");
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

/// Build the configured AI caller.
///
/// `json_schema` is the task's optional output schema, handed to the `claude` provider.
///
/// Providers (`[api] provider`). Each provider stands alone — a failure surfaces to the
/// user rather than silently falling back to another provider.
/// - `"gemini"` (default): Gemini HTTP API.
/// - `"claude"`: `claude -p` CLI.
/// - `"command"`: the user's external command (`[api] command`).
pub fn build_caller(
    api: &ApiConfig,
    json_schema: Option<String>,
) -> Result<Box<dyn AiCaller>, String> {
    match api.provider.trim().to_lowercase().as_str() {
        "gemini" => Ok(Box::new(GeminiCaller {
            model: api.model.clone(),
            max_tokens: GEMINI_MAX_TOKENS,
        })),
        "claude" => Ok(Box::new(ClaudeCliCaller { json_schema })),
        "command" => {
            if api.command.is_empty() {
                return Err(
                    "provider = \"command\" but [api] command is empty; set command = [\"...\"]"
                        .to_string(),
                );
            }
            Ok(Box::new(CommandCaller {
                cmd: api.command.clone(),
                timeout_secs: api.command_timeout_secs,
            }))
        }
        other => Err(format!(
            "unknown AI provider '{other}' (expected: gemini, claude, command)"
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
        assert!(build_caller(&api("gemini"), None).is_ok());
        assert!(build_caller(&api("claude"), None).is_ok());
        assert!(build_caller(&ApiConfig::default(), None).is_ok());
    }

    #[test]
    fn build_caller_is_case_and_whitespace_insensitive() {
        assert!(build_caller(&api("GEMINI"), None).is_ok());
        assert!(build_caller(&api("Claude"), None).is_ok());
        assert!(build_caller(&api("  gemini  "), None).is_ok());
    }

    #[test]
    fn build_caller_rejects_unknown_provider() {
        let err = build_caller(&api("ollama"), None).err().unwrap();
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
        let err = build_caller(&cfg, None).err().unwrap();
        assert!(err.contains("command"), "actionable message: {err}");
    }

    #[test]
    fn build_caller_accepts_nonempty_command() {
        let cfg = ApiConfig {
            provider: "command".to_string(),
            command: vec!["cat".to_string()],
            ..Default::default()
        };
        assert!(build_caller(&cfg, None).is_ok());
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

    // ── extract_claude_payload ───────────────────────────────────────

    #[test]
    fn extract_claude_payload_reads_structured_object() {
        let out = r#"{"structured_output":{"branch":"x","prompt":"p"}}"#;
        let payload = extract_claude_payload(out).unwrap();
        // Re-serialized object the caller's parser can consume.
        assert!(payload.contains("\"branch\""));
        assert!(payload.contains("\"x\""));
    }

    #[test]
    fn extract_claude_payload_reads_result_string() {
        let out = r#"{"result":"hello world"}"#;
        assert_eq!(extract_claude_payload(out).unwrap(), "hello world");
    }

    #[test]
    fn extract_claude_payload_errors_on_missing_field() {
        let out = r#"{"other":1}"#;
        assert!(extract_claude_payload(out).is_err());
    }

    // ── CommandCaller (real subprocess, Unix only) ───────────────────

    #[cfg(unix)]
    mod command {
        use super::*;

        fn sh(script: &str, timeout_secs: u64) -> CommandCaller {
            CommandCaller {
                cmd: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
                timeout_secs,
            }
        }

        #[test]
        fn echoes_prompt_via_stdin() {
            let caller = sh("cat", 5);
            let cancel = Arc::new(AtomicBool::new(false));
            let out = caller.complete("SYS", "USER", &cancel).unwrap();
            assert!(out.contains("SYS") && out.contains("USER"), "got: {out}");
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
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let err = caller.complete("s", "u", &cancel).unwrap_err();
            assert!(err.contains("definitely_not_a_real_binary_xyzzy"), "got: {err}");
        }
    }
}
