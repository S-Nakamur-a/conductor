//! Data model and generation for AI-generated PR walkthroughs.
//!
//! A walkthrough is a model-authored, ordered tour of a branch's diff
//! (`walkthroughs` + `walkthrough_steps` in the review database, persisted
//! and queried via [`crate::review_store::ReviewStore`]). This module holds
//! the plain data types shared by the store and the UI pane, plus the
//! generation task itself.
//!
//! ## How generation runs
//!
//! Through the one configurable AI seam, [`crate::ai_caller`], exactly like
//! smart-worktree naming: Conductor owns the prompt and the parsing, and the
//! user owns *which* model answers via `[api] provider` / `[api] command`.
//! Conductor never spawns a `claude` process of its own — that rule holds
//! across this entire codebase, and this module used to be the last exception.
//!
//! The task is agentic in nature: the model has to read the branch's diff, and
//! usually the callers/callees around it, before it can narrate anything. The
//! command therefore runs with its working directory set to the reviewed
//! worktree (see the protocol notes in [`crate::ai_caller`]), and the reply
//! comes back as one JSON object that [`parse_generated`] turns into steps.
//!
//! ## Why the reply is JSON rather than an MCP tool call
//!
//! The MCP `save_walkthrough` tool still exists and is still how the external
//! `/conductor-walkthrough` command saves its work. It is not how *this* path
//! saves, because the seam above is a plain stdin/stdout text protocol: there
//! is no argv Conductor controls, so there is nowhere to inject an
//! `--mcp-config`. Conductor parses the JSON and writes the rows itself, which
//! also means a malformed reply fails loudly here instead of leaving a row
//! stuck in `generating`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// Generation status of a walkthrough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkthroughStatus {
    /// A background generation is producing the steps.
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

/// Wall-clock budget for one generation, handed to the AI seam as this task's
/// timeout so it is not capped by `[api] command_timeout_secs` (which is sized
/// for a few seconds of smart-worktree naming). Reading a branch diff and
/// narrating it takes minutes; past this we assume the session is wedged.
pub const GENERATION_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The system prompt: what a walkthrough is, and the exact reply shape.
///
/// Mirrors `plugins/conductor/commands/conductor-walkthrough.md` (kept for
/// marketplace-plugin users, which saves through the MCP tool instead) but is
/// embedded here so generation works regardless of which slash commands the
/// installed plugin cache has.
const GENERATION_SYSTEM_PROMPT: &str = r#"You build reviewer walkthroughs: an ordered tour of a branch's change that a reviewer follows step by step, each step anchored to a file and line range.

Use your tools freely: this task cannot be done without reading the repository in your working directory. Run git to see the diff, and read the changed files and the code around them. Only when you have finished exploring, answer with the JSON described below and nothing else.

Order the steps as a story: intent -> core -> ripple -> test.
- intent: what this change wanted to achieve (background, motivation).
- core: what was changed to achieve it, and its effect on existing code. Do NOT compare alternative designs — reviewers ask those questions themselves.
- ripple: knock-on changes (call-site updates, config/schema follow-ups).
- test: a summary of what behavior each test verifies, detailed enough that a reviewer can skip reading the full test diff.

There is no fixed step count — match the actual change.

Output ONLY a JSON object, no markdown fences and no explanation, with these fields:
- "title": a one-line title for the whole walkthrough.
- "summary": the overview of the change. This is stored as the branch's change summary and shown full-panel as Conductor's SUMMARY pseudo-file, so write it like a PR description — what the change is for, why these files are touched, and anything a reviewer should know up front (including what is deliberately out of scope). Markdown is rendered.
- "steps": an array, in tour order, of objects with:
    "file_path"  (string, repo-relative, e.g. "src/foo.rs" — never absolute, never prefixed with "a/" or "b/", never starting with "./")
    "line_start" (integer or null, 1-based line number on the NEW side)
    "line_end"   (integer or null, 1-based)
    "kind"       ("intent" | "core" | "ripple" | "test")
    "title"      (string, one line)
    "body"       (string, the explanation; Markdown is rendered)
- "comments": an array (possibly empty) of inline notes for the few spots that are genuinely hard to understand — tricky logic whose intent isn't obvious at a glance, a non-obvious tradeoff, or a subtle edge case a reviewer could miss. Each object has "file_path", "line_start", "line_end" (integer or null), and "body" (1-3 sentences explaining *why* it works / where the subtlety is). This is high-signal and low-frequency: a handful per change at most, and an empty array when nothing is genuinely tricky. Do NOT comment on self-evident changes (renames, boilerplate, formatting, imports).

Every file_path must be repo-relative: the reviewer's diff list matches these against git's own paths, so a step whose path is spelled any other way cannot be opened."#;

/// The per-run instruction: which branch, which base, which language.
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
walkthrough of it.\n\
\n\
{base_hint} The branch under review is `{branch}`, checked out in your working directory. \
Use `git diff <base>...HEAD` (three-dot, merge-base) to see the change. Read not only the \
changed files but, where needed, their callers/callees so you understand the whole \
picture.\n\
\n\
When you have explored enough, reply with the JSON object described above and nothing \
else.{language_hint}"
    )
}

/// One inline note the generation asked for, saved as a `question` comment.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GeneratedComment {
    pub file_path: String,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub body: String,
}

/// A step as it arrives from the model, before validation.
#[derive(Debug, Clone, serde::Deserialize)]
struct GeneratedStep {
    file_path: String,
    #[serde(default)]
    line_start: Option<i64>,
    #[serde(default)]
    line_end: Option<i64>,
    kind: String,
    title: String,
    body: String,
}

/// The whole reply, before validation.
#[derive(Debug, Clone, serde::Deserialize)]
struct GeneratedWalkthrough {
    title: String,
    summary: String,
    steps: Vec<GeneratedStep>,
    #[serde(default)]
    comments: Vec<GeneratedComment>,
}

/// A validated generation result, ready for [`crate::review_store::ReviewStore`].
#[derive(Debug, Clone)]
pub struct Generated {
    pub title: String,
    pub summary: String,
    pub steps: Vec<NewWalkthroughStep>,
    pub comments: Vec<GeneratedComment>,
}

/// Turn the model's raw reply into a saveable walkthrough, or explain what is
/// wrong with it.
///
/// Tolerant about the *envelope* (markdown fences, a sentence before the JSON)
/// because models add those regardless of instructions, and strict about the
/// *contents*: an unknown `kind` or a path that cannot anchor to a file would
/// otherwise be stored and only show up as a step the reviewer can't open.
/// Mirrors the checks `mcp_serve::tools::save_walkthrough` runs on the same
/// data arriving over MCP, so both entry points reject the same things.
pub fn parse_generated(raw: &str) -> Result<Generated, String> {
    let json = extract_json_object(raw)
        .ok_or_else(|| format!("no JSON object in the model's reply\nRaw output: {raw}"))?;
    let parsed: GeneratedWalkthrough = serde_json::from_str(json)
        .map_err(|e| format!("could not parse the walkthrough JSON: {e}\nRaw output: {raw}"))?;

    if parsed.title.trim().is_empty() {
        return Err("the walkthrough has no title".to_string());
    }
    if parsed.summary.trim().is_empty() {
        return Err("the walkthrough has no summary".to_string());
    }
    if parsed.steps.is_empty() {
        return Err("the walkthrough has no steps".to_string());
    }

    let mut steps = Vec::with_capacity(parsed.steps.len());
    for (i, step) in parsed.steps.into_iter().enumerate() {
        let kind = WalkthroughStepKind::from_str(step.kind.trim())
            .ok_or_else(|| format!("step {i} has an unknown kind '{}'", step.kind))?;
        let file_path = crate::repo_path::normalize(&step.file_path);
        if file_path.is_empty() {
            return Err(format!("step {i} has no file_path"));
        }
        if file_path.starts_with('/') || file_path.split('/').any(|s| s == "..") {
            return Err(format!(
                "step {i} file_path must be repo-relative, got: {}",
                step.file_path
            ));
        }
        if step.title.trim().is_empty() {
            return Err(format!("step {i} ({file_path}) has no title"));
        }
        if step.body.trim().is_empty() {
            return Err(format!("step {i} ({file_path}) has no body"));
        }
        // Line numbers are 1-based everywhere they are read back, and a
        // reversed range would underline nothing; drop the range rather than
        // failing the whole walkthrough over an anchor detail.
        let (line_start, line_end) = sane_range(step.line_start, step.line_end);
        steps.push(NewWalkthroughStep {
            file_path,
            line_start,
            line_end,
            kind,
            title: step.title,
            body: step.body,
        });
    }

    // Comments are the optional extra: a bad one is dropped with a log line
    // rather than failing a walkthrough that is otherwise fine.
    let comments = parsed
        .comments
        .into_iter()
        .filter_map(|mut c| {
            let path = crate::repo_path::normalize(&c.file_path);
            if path.is_empty()
                || path.starts_with('/')
                || path.split('/').any(|s| s == "..")
                || c.body.trim().is_empty()
                || c.line_start.is_none_or(|l| l == 0)
            {
                log::warn!("dropping malformed inline comment for {:?}", c.file_path);
                return None;
            }
            if let (Some(start), Some(end)) = (c.line_start, c.line_end)
                && end < start
            {
                c.line_end = None;
            }
            c.file_path = path;
            Some(c)
        })
        .collect();

    Ok(Generated {
        title: parsed.title,
        summary: parsed.summary,
        steps,
        comments,
    })
}

/// Keep a 1-based, non-reversed line range; anything else anchors to the file
/// as a whole rather than to a wrong span.
fn sane_range(start: Option<i64>, end: Option<i64>) -> (Option<i64>, Option<i64>) {
    let start = start.filter(|s| *s >= 1);
    let end = end.filter(|e| *e >= 1);
    match (start, end) {
        (Some(s), Some(e)) if e < s => (Some(s), None),
        (None, Some(_)) => (None, None),
        pair => pair,
    }
}

/// Find the JSON object in a reply that may be fenced or prefaced with prose.
///
/// Scans for the first `{` and then tracks brace depth, skipping over braces
/// that appear inside string literals — a `body` containing `}` (very likely,
/// since bodies quote code) would otherwise truncate the object at the wrong
/// place and produce a parse error the user cannot act on.
fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Run one generation to completion and return the parsed walkthrough.
///
/// Blocking: the caller runs this on a background thread and reports the result
/// through a channel (see `App::cmd_generate_walkthrough`). `cancel` is checked
/// by the underlying caller too, so a cancelled generation kills its child
/// rather than waiting out the timeout.
pub fn generate(
    api: &crate::config::ApiConfig,
    worktree_path: &Path,
    branch: &str,
    base_ref: Option<&str>,
    language: Option<&str>,
    cancel: &Arc<AtomicBool>,
) -> Result<Generated, String> {
    let env = crate::ai_caller::TaskEnv {
        timeout_secs: Some(GENERATION_TIMEOUT.as_secs()),
        working_dir: Some(worktree_path.to_path_buf()),
    };
    let caller = crate::ai_caller::build_caller(api, &env)?;
    let raw = caller.complete(
        GENERATION_SYSTEM_PROMPT,
        &generation_prompt(branch, base_ref, language),
        cancel,
    )?;
    parse_generated(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── generation_prompt: branch, base, language ───────────────────────

    #[test]
    fn generation_prompt_includes_the_branch_name() {
        let prompt = generation_prompt("pr-42", Some("main"), None);
        assert!(prompt.contains("pr-42"));
        assert!(prompt.contains("main"));
    }

    #[test]
    fn generation_prompt_falls_back_to_discovering_the_base() {
        let prompt = generation_prompt("pr-42", None, None);
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

    /// The instruction not to compare alternative designs, and the step-order
    /// story, live in the system prompt now — they must not have been lost in
    /// the move off the headless session.
    #[test]
    fn system_prompt_keeps_the_walkthrough_contract() {
        assert!(GENERATION_SYSTEM_PROMPT.to_lowercase().contains("do not compare"));
        assert!(GENERATION_SYSTEM_PROMPT.contains("intent -> core -> ripple -> test"));
        // Path spelling is the whole reason walkthrough steps used to fail to
        // open, so the prompt has to be explicit about it.
        assert!(GENERATION_SYSTEM_PROMPT.contains("repo-relative"));
        assert!(GENERATION_SYSTEM_PROMPT.contains("never starting with \"./\""));
        // Inline notes are asked for here rather than through an MCP tool call,
        // but the contract is the same one the plugin command states: annotate
        // the genuinely-tricky spots, high-signal and low-frequency.
        assert!(GENERATION_SYSTEM_PROMPT.contains("hard to understand"));
        assert!(GENERATION_SYSTEM_PROMPT.contains("\"comments\""));
    }

    /// The opposite constraint from smart-worktree naming, and it has to be
    /// stated here rather than in a wrapper the user maintains: an agentic
    /// command told nothing would answer from the prompt alone, and this task
    /// is impossible without reading the repo.
    #[test]
    fn system_prompt_tells_the_model_to_read_the_repo() {
        assert!(GENERATION_SYSTEM_PROMPT.contains("Use your tools freely"));
        assert!(GENERATION_SYSTEM_PROMPT.contains("working directory"));
    }

    /// Conductor must not spawn `claude` itself anywhere; this module was the
    /// last place that did. The prompt names no CLI and no MCP tool call — the
    /// reply comes back as JSON over whatever `[api]` is pointed at.
    #[test]
    fn generation_never_names_a_cli_to_run() {
        assert!(!GENERATION_SYSTEM_PROMPT.contains("claude -p"));
        assert!(!GENERATION_SYSTEM_PROMPT.contains("save_walkthrough"));
        let prompt = generation_prompt("pr-42", Some("main"), None);
        assert!(!prompt.contains("claude -p"));
        assert!(!prompt.contains("save_walkthrough"));
    }

    // ── parse_generated ────────────────────────────────────────────────

    fn reply(steps: &str) -> String {
        format!(
            r#"{{"title":"T","summary":"S","steps":[{steps}]}}"#
        )
    }

    #[test]
    fn parses_a_well_formed_reply() {
        let raw = reply(
            r#"{"file_path":"src/a.rs","line_start":10,"line_end":12,"kind":"core","title":"t","body":"b"}"#,
        );
        let g = parse_generated(&raw).unwrap();
        assert_eq!(g.title, "T");
        assert_eq!(g.summary, "S");
        assert_eq!(g.steps.len(), 1);
        assert_eq!(g.steps[0].file_path, "src/a.rs");
        assert_eq!(g.steps[0].line_start, Some(10));
        assert_eq!(g.steps[0].line_end, Some(12));
        assert_eq!(g.steps[0].kind, WalkthroughStepKind::Core);
        assert!(g.comments.is_empty());
    }

    /// Models wrap JSON in fences and prefaces no matter what the prompt says,
    /// so the envelope is tolerated even though the contents are not.
    #[test]
    fn parses_through_fences_and_preamble() {
        let inner = reply(r#"{"file_path":"src/a.rs","kind":"intent","title":"t","body":"b"}"#);
        for wrapped in [
            format!("```json\n{inner}\n```"),
            format!("Here you go:\n{inner}\nHope that helps!"),
            format!("```\n{inner}\n```"),
        ] {
            let g = parse_generated(&wrapped).unwrap();
            assert_eq!(g.steps.len(), 1, "wrapped: {wrapped}");
        }
    }

    /// A body quoting code will contain braces. Counting to the *matching*
    /// close brace, and skipping braces inside strings, is what keeps such a
    /// reply from being truncated into a parse error.
    #[test]
    fn parses_a_body_containing_braces() {
        let raw = r#"prose {"title":"T","summary":"S","steps":[{"file_path":"src/a.rs","kind":"core","title":"t","body":"fn main() { let x = {1}; }"}]} trailing"#;
        let g = parse_generated(raw).unwrap();
        assert_eq!(g.steps[0].body, "fn main() { let x = {1}; }");
    }

    /// The same spellings the diff list cannot match are canonicalised here, so
    /// a generated step can always be opened.
    #[test]
    fn normalises_step_paths() {
        for spelling in ["./src/a.rs", "src//a.rs", "src/a.rs/", "  src/a.rs  "] {
            let raw = reply(&format!(
                r#"{{"file_path":"{spelling}","kind":"core","title":"t","body":"b"}}"#
            ));
            let g = parse_generated(&raw).unwrap();
            assert_eq!(g.steps[0].file_path, "src/a.rs", "spelling: {spelling}");
        }
    }

    #[test]
    fn rejects_paths_that_escape_the_repo() {
        for bad in ["/etc/passwd", "../secret", ""] {
            let raw = reply(&format!(
                r#"{{"file_path":"{bad}","kind":"core","title":"t","body":"b"}}"#
            ));
            assert!(parse_generated(&raw).is_err(), "path: {bad}");
        }
    }

    #[test]
    fn rejects_an_unknown_step_kind() {
        let raw = reply(r#"{"file_path":"src/a.rs","kind":"summary","title":"t","body":"b"}"#);
        let err = parse_generated(&raw).unwrap_err();
        assert!(err.contains("summary"), "should echo the bad kind: {err}");
    }

    #[test]
    fn rejects_empty_title_summary_and_steps() {
        assert!(parse_generated(r#"{"title":"","summary":"S","steps":[]}"#).is_err());
        assert!(
            parse_generated(
                r#"{"title":"T","summary":"S","steps":[{"file_path":"a.rs","kind":"core","title":"t","body":"b"}]}"#
            )
            .is_ok()
        );
        assert!(parse_generated(r#"{"title":"T","summary":"S","steps":[]}"#).is_err());
        assert!(parse_generated("no json here").is_err());
    }

    /// A bad line range anchors the step to its file rather than failing the
    /// whole walkthrough or underlining a nonsense span.
    #[test]
    fn sanitises_line_ranges() {
        assert_eq!(sane_range(Some(10), Some(5)), (Some(10), None));
        assert_eq!(sane_range(Some(0), Some(5)), (None, None));
        assert_eq!(sane_range(None, Some(5)), (None, None));
        assert_eq!(sane_range(Some(3), None), (Some(3), None));
        assert_eq!(sane_range(Some(3), Some(3)), (Some(3), Some(3)));
    }

    /// Inline comments are the optional extra: a malformed one is dropped, and
    /// the walkthrough it came with still saves.
    #[test]
    fn drops_malformed_comments_but_keeps_the_walkthrough() {
        let raw = r#"{"title":"T","summary":"S",
            "steps":[{"file_path":"src/a.rs","kind":"core","title":"t","body":"b"}],
            "comments":[
              {"file_path":"./src/a.rs","line_start":4,"line_end":6,"body":"why"},
              {"file_path":"/etc/passwd","line_start":1,"body":"escape"},
              {"file_path":"src/a.rs","line_start":0,"body":"zero line"},
              {"file_path":"src/a.rs","line_start":9,"body":"   "}
            ]}"#;
        let g = parse_generated(raw).unwrap();
        assert_eq!(g.steps.len(), 1);
        assert_eq!(g.comments.len(), 1);
        assert_eq!(g.comments[0].file_path, "src/a.rs");
        assert_eq!(g.comments[0].line_start, Some(4));
    }

    /// A reversed comment range collapses to a single line instead of being
    /// stored as a span that renders backwards.
    #[test]
    fn reversed_comment_range_collapses() {
        let raw = r#"{"title":"T","summary":"S",
            "steps":[{"file_path":"a.rs","kind":"core","title":"t","body":"b"}],
            "comments":[{"file_path":"a.rs","line_start":9,"line_end":2,"body":"why"}]}"#;
        let g = parse_generated(raw).unwrap();
        assert_eq!(g.comments[0].line_start, Some(9));
        assert_eq!(g.comments[0].line_end, None);
    }
}
