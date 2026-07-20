//! Unit tests for claude_log: wrapper-tag normalisation, content-to-block
//! conversion, `load_session` integration, and session timestamp bounds.

use super::convert::{content_to_display_blocks, result_lines, summarise_tool_input};
use super::model::{DisplayBlock, Role, TOOL_RESULT_PREVIEW_LINES};
use super::schema::{LogRecord, TextOnly, ToolResultContent};
use super::session::{load_session, session_first_timestamp, session_last_timestamp};

fn parse_msg_content(json: &str) -> Vec<DisplayBlock> {
    let r: LogRecord = serde_json::from_str(json).expect("valid test json");
    let msg = r.message.unwrap();
    let is_user = msg.role.as_deref() == Some("user");
    content_to_display_blocks(msg.content, is_user)
}

// ── Hidden-context normalisation (isMeta / wrappers) ─────────────────────

#[test]
fn meta_records_are_skipped() {
    // A skill invocation dumps the whole SKILL.md as an isMeta user turn;
    // Claude Code never displays it, so the transcript must not either.
    let f = write_jsonl(&[
        r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Base directory for this skill: ... 20k chars of SKILL.md"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"real prompt"}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 1);
    assert!(matches!(&entries[0].blocks[0], DisplayBlock::Text(t) if t == "real prompt"));
}

#[test]
fn command_wrapper_renders_as_slash_invocation() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<command-name>/merge-pr</command-name>\n<command-message>merge-pr</command-message>\n<command-args>--admin</command-args>"}}"#,
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == "/merge-pr --admin"));
}

#[test]
fn command_wrapper_without_args_shows_bare_command() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>"}}"#,
    );
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == "/clear"));
}

#[test]
fn local_command_stdout_is_unwrapped_and_sanitized() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>\u001b[2mCompacted\u001b[22m</local-command-stdout>"}}"#,
    );
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == "Compacted"));
}

#[test]
fn empty_local_command_stdout_is_dropped() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout></local-command-stdout>"}}"#,
    );
    assert!(blocks.is_empty());
}

#[test]
fn system_reminder_spans_are_stripped_from_user_text() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"fix the bug <system-reminder>hidden note</system-reminder>please"}}"#,
    );
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == "fix the bug please"));
}

#[test]
fn reminder_only_user_block_is_dropped() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"<system-reminder>only hidden</system-reminder>"}}"#,
    );
    assert!(blocks.is_empty());
}

#[test]
fn unterminated_system_reminder_strips_to_end() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"visible <system-reminder>truncated"}}"#,
    );
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == "visible"));
}

#[test]
fn unterminated_command_tag_at_start_is_left_as_prose() {
    // A real command record always carries the closing tag; a prompt that
    // merely *starts* with the literal tag must survive intact.
    let raw = "<command-name> is a wrapper the CLI writes";
    let blocks = parse_msg_content(&format!(
        r#"{{"type":"user","message":{{"role":"user","content":"{raw}"}}}}"#,
    ));
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == raw));
}

#[test]
fn mid_prompt_mention_of_command_tag_is_not_rewritten() {
    // The wrapper is only recognised at the start of the message; a user
    // *talking about* the tag keeps their full prompt (the reminder-strip
    // pass still runs, but there is no reminder here).
    let raw = "why does <command-name>/x</command-name> appear in my log?";
    let blocks = parse_msg_content(&format!(
        r#"{{"type":"user","message":{{"role":"user","content":"{raw}"}}}}"#,
    ));
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == raw));
}

#[test]
fn assistant_text_quoting_wrapper_tags_is_untouched() {
    // The assistant may legitimately discuss these tags; only user turns
    // get the wrapper normalisation.
    let raw = "use <system-reminder> and <command-name>/x</command-name> in docs";
    let blocks = parse_msg_content(&format!(
        r#"{{"type":"assistant","message":{{"role":"assistant","content":"{raw}"}}}}"#,
    ));
    assert!(matches!(&blocks[0], DisplayBlock::Text(t) if t == raw));
}

#[test]
fn string_content_becomes_single_text_block() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":"hello world"}}"#,
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0], DisplayBlock::Text(_)));
}

#[test]
fn array_content_multiple_block_types() {
    let blocks = parse_msg_content(
        r#"{
            "type": "assistant",
            "message": {
                "role": "assistant",
                "model": "claude-opus-4",
                "content": [
                    {"type":"text","text":"Here is the plan."},
                    {"type":"thinking","thinking":"","signature":"abc"},
                    {"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"ls -la"}},
                    {"type":"tool_result","tool_use_id":"tu1","content":"file1\nfile2\nfile3"}
                ]
            }
        }"#,
    );
    assert_eq!(blocks.len(), 4);
    assert!(matches!(blocks[0], DisplayBlock::Text(_)));
    assert!(matches!(blocks[1], DisplayBlock::Thinking { .. }));
    assert!(matches!(blocks[2], DisplayBlock::ToolUse { .. }));
    assert!(matches!(
        blocks[3],
        DisplayBlock::ToolResult {
            total_lines: 3,
            ..
        }
    ));
}

#[test]
fn sidechain_flag_is_parsed() {
    let r: LogRecord = serde_json::from_str(
        r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"secret"}}"#,
    )
    .unwrap();
    assert!(r.is_sidechain);
}

#[test]
fn tool_use_summary_picks_command_key() {
    let input = serde_json::json!({"command": "cargo build", "cwd": "/project"});
    assert_eq!(summarise_tool_input(&input), "cargo build");
}

#[test]
fn tool_use_summary_falls_back_to_file_path() {
    let input = serde_json::json!({"file_path": "/src/main.rs"});
    assert_eq!(summarise_tool_input(&input), "/src/main.rs");
}

#[test]
fn tool_use_summary_falls_back_to_pattern() {
    let input = serde_json::json!({"pattern": "fn open_reflow"});
    assert_eq!(summarise_tool_input(&input), "fn open_reflow");
}

#[test]
fn tool_result_string_counts_lines() {
    let content = ToolResultContent::Text("a\nb\nc".to_string());
    assert_eq!(result_lines(&content).len(), 3);
}

#[test]
fn preview_lines_are_sanitized_for_rendering() {
    // Tabs → spaces, ANSI color escapes stripped, control codes dropped, so a
    // rendered preview line contains no width-desyncing characters.
    let content = ToolResultContent::Text(
        "a\tb\n\u{1b}[31mred\u{1b}[0m\n\u{1b}]8;;http://x\u{07}link\u{1b}]8;;\u{07}\ncr\rlf"
            .to_string(),
    );
    let lines = result_lines(&content);
    assert_eq!(lines[0], "a    b", "tab expands to spaces");
    assert_eq!(lines[1], "red", "ANSI SGR escapes stripped");
    assert_eq!(lines[2], "link", "OSC hyperlink sequences stripped");
    assert_eq!(lines[3], "crlf", "carriage return dropped");
    for l in &lines {
        assert!(
            !l.chars().any(|c| c.is_control()),
            "no control chars remain in {l:?}"
        );
    }
}

#[test]
fn tool_result_block_array_counts_lines() {
    let content = ToolResultContent::Blocks(vec![
        TextOnly {
            text: "a\nb\nc".to_string(),
        },
        TextOnly {
            text: "d\ne".to_string(),
        },
    ]);
    assert_eq!(result_lines(&content).len(), 5);
}

#[test]
fn tool_result_preview_caps_and_reports_total() {
    // 10 output lines → preview capped at TOOL_RESULT_PREVIEW_LINES,
    // total_lines retains the real count for the "+N lines" summary.
    let body: String = (0..10)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let json = format!(
        r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t","content":{body:?}}}]}}}}"#
    );
    let blocks = parse_msg_content(&json);
    match &blocks[0] {
        DisplayBlock::ToolResult {
            preview,
            total_lines,
            is_error,
        } => {
            assert_eq!(*total_lines, 10);
            assert_eq!(preview.len(), TOOL_RESULT_PREVIEW_LINES);
            assert_eq!(preview[0], "line0");
            assert!(!*is_error);
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn tool_result_error_flag_is_captured() {
    let blocks = parse_msg_content(
        r#"{"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"t","is_error":true,"content":"boom"}
        ]}}"#,
    );
    assert!(matches!(
        blocks[0],
        DisplayBlock::ToolResult { is_error: true, .. }
    ));
}

#[test]
fn thinking_text_is_captured() {
    let blocks = parse_msg_content(
        r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"thinking","thinking":"let me reason","signature":"x"}
        ]}}"#,
    );
    match &blocks[0] {
        DisplayBlock::Thinking { text } => assert_eq!(text, "let me reason"),
        other => panic!("expected Thinking, got {other:?}"),
    }
}

#[test]
fn empty_text_block_excluded() {
    let blocks = parse_msg_content(
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":""}]}}"#,
    );
    assert!(blocks.is_empty());
}

#[test]
fn unknown_block_type_is_skipped() {
    let blocks = parse_msg_content(
        r#"{"type":"assistant","message":{"role":"assistant","content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"..."}},
            {"type":"text","text":"done"}
        ]}}"#,
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0], DisplayBlock::Text(_)));
}

// ── load_session integration tests (write a temp .jsonl, call load_session) ──

fn write_jsonl(lines: &[&str]) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::NamedTempFile::new().expect("tmp file");
    for line in lines {
        writeln!(f, "{line}").expect("write");
    }
    f
}

#[test]
fn load_session_skips_non_user_assistant_types() {
    let f = write_jsonl(&[
        r#"{"type":"system","message":{"role":"system","content":"sys prompt"}}"#,
        r#"{"type":"summary","message":{"role":"assistant","content":"summary"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"hello"}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].role, Role::User);
}

#[test]
fn load_session_skips_sidechain_records() {
    let f = write_jsonl(&[
        r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"hidden"}}"#,
        r#"{"type":"assistant","isSidechain":false,"message":{"role":"assistant","content":"visible"}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].role, Role::Assistant);
}

#[test]
fn load_session_skips_role_mismatch() {
    // A record with type=user but role=system should be silently dropped.
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"system","content":"not a user turn"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"real user"}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].role, Role::User);
}

#[test]
fn load_session_skips_empty_blocks() {
    // A message whose content produces zero display blocks is not emitted.
    let f = write_jsonl(&[
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":""}]}}"#,
        r#"{"type":"user","message":{"role":"user","content":"valid"}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].role, Role::User);
}

#[test]
fn load_session_mixed_records_correct_count() {
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"q1"}}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":"a1"}}"#,
        r#"{"type":"system","message":{"role":"system","content":"noise"}}"#,
        r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"chain"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"q2"}}"#,
        r#"not-json-at-all"#,
    ]);
    let entries = load_session(f.path());
    // system noise, sidechain, and malformed line are excluded → 3 remain.
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].role, Role::User);
    assert_eq!(entries[1].role, Role::Assistant);
    assert_eq!(entries[2].role, Role::User);
}

#[test]
fn load_session_includes_enqueued_prompts_skips_removes() {
    // A prompt typed while Claude is busy is an `enqueue` queue-operation
    // (top-level content); the dequeue is a contentless `remove`.
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"first"}}"#,
        r#"{"type":"queue-operation","operation":"enqueue","content":"typed while busy"}"#,
        r#"{"type":"queue-operation","operation":"remove","content":""}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":"reply"}}"#,
    ]);
    let entries = load_session(f.path());
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].role, Role::User);
    assert_eq!(entries[1].role, Role::User);
    assert!(
        matches!(&entries[1].blocks[0], DisplayBlock::Text(t) if t == "typed while busy"),
        "enqueued prompt text should be surfaced as a user turn"
    );
    assert_eq!(entries[2].role, Role::Assistant);
}

#[test]
fn load_session_missing_file_returns_empty() {
    let entries = load_session(std::path::Path::new("/nonexistent/path.jsonl"));
    assert!(entries.is_empty());
}

// ── Session timestamp bounds (mid-session /clear rotation detection) ──────

#[test]
fn timestamp_bounds_skip_leading_bookkeeping_records() {
    // A fresh session opens with `mode` / `file-history-snapshot` records
    // that carry no top-level timestamp; the bounds must come from the
    // first and last *turn* records instead.
    let f = write_jsonl(&[
        r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
        r#"{"type":"file-history-snapshot","messageId":"m"}"#,
        r#"{"type":"user","message":{"role":"user","content":"hi"},"timestamp":"2026-07-06T03:15:21.896Z"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":"yo"},"timestamp":"2026-07-06T03:16:00.000Z"}"#,
    ]);
    assert_eq!(
        session_first_timestamp(f.path()).as_deref(),
        Some("2026-07-06T03:15:21.896Z")
    );
    assert_eq!(
        session_last_timestamp(f.path()).as_deref(),
        Some("2026-07-06T03:16:00.000Z")
    );
}

#[test]
fn timestamp_bounds_none_for_timestampless_log() {
    // A pre-timestamp log format yields None, which disables rotation
    // detection (the caller falls back to the pinned session).
    let f = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
    ]);
    assert!(session_first_timestamp(f.path()).is_none());
    assert!(session_last_timestamp(f.path()).is_none());
}

#[test]
fn continuation_starts_at_or_after_pinned_last_turn() {
    // The rotation-detection predicate: a post-`/clear` continuation begins
    // at/after the pinned session's last turn (string compare == chrono
    // order for uniform UTC stamps), while a concurrent sibling panel's
    // first turn predates it and is rejected.
    let pinned = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"a"},"timestamp":"2026-07-06T03:00:00.000Z"}"#,
        r#"{"type":"assistant","message":{"role":"assistant","content":"b"},"timestamp":"2026-07-06T03:10:00.000Z"}"#,
    ]);
    let continuation = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"c"},"timestamp":"2026-07-06T03:10:00.000Z"}"#,
    ]);
    let sibling = write_jsonl(&[
        r#"{"type":"user","message":{"role":"user","content":"d"},"timestamp":"2026-07-06T03:05:00.000Z"}"#,
    ]);
    let pinned_last = session_last_timestamp(pinned.path()).unwrap();
    assert!(session_first_timestamp(continuation.path()).unwrap() >= pinned_last);
    assert!(session_first_timestamp(sibling.path()).unwrap() < pinned_last);
}
