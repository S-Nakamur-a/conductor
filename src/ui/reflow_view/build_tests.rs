//! Tests for [`super::build::build_lines`] — the tool-rendering pipeline that
//! turns a session log's [`LogEntry`] list into transcript lines. Exercised
//! directly against a hand-built [`BuildCtx`], with no `App` in sight: the
//! whole point of the `BuildCtx` split (S0) is that this pipeline is
//! testable without constructing the application state.
//!
//! The tool-call cases below assert against the classification table in
//! `crate::claude_log::tool_class` (§2.1 of
//! `docs/plans/2026-07-31-native-render-parity.md`), reconstructed from a
//! raw-byte capture of Claude Code's own transcript — not a guess.

use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use serde_json::json;
use syntect::highlighting::ThemeSet;

use crate::claude_log::{CountedBucket, DisplayBlock, LogEntry, ResultKind, Role};
use crate::ui::markdown::MarkdownCache;

use super::build::{BuildCtx, build_lines};
use super::glyphs::{ASSISTANT_MARKER, THINKING_GLYPH, TOOL_RESULT_GLYPH, USER_MARKER};
use super::palette;

fn fixtures() -> (
    crate::theme::Theme,
    syntect::parsing::SyntaxSet,
    syntect::highlighting::Theme,
) {
    let theme = crate::theme::Theme::default();
    let syntax_set = two_face::syntax::extra_newlines();
    let syntect_theme = ThemeSet::load_defaults()
        .themes
        .remove("base16-ocean.dark")
        .unwrap();
    (theme, syntax_set, syntect_theme)
}

/// `build_lines` must be callable with only borrowed fixtures — no `App`
/// constructed anywhere in this test. An empty entry list is a degenerate
/// but valid input.
#[test]
fn build_lines_runs_without_an_app() {
    let (theme, syntax_set, syntect_theme) = fixtures();
    let cache = MarkdownCache::new();
    let entries: Vec<LogEntry> = Vec::new();
    let ctx = BuildCtx {
        entries: &entries,
        cache: &cache,
        theme: &theme,
        syntax_set: &syntax_set,
        syntect_theme: &syntect_theme,
        expanded: false,
    };

    let built = build_lines(&ctx, 80);

    assert!(built.lines.is_empty());
    assert!(built.meta.is_empty());
}

// ── Fixture helpers for the tool-call rendering table below ──────────────

fn tool_use(name: &str, input: serde_json::Value) -> DisplayBlock {
    tool_use_errored(name, input, false)
}

fn tool_use_errored(name: &str, input: serde_json::Value, errored: bool) -> DisplayBlock {
    DisplayBlock::ToolUse {
        name: name.to_string(),
        input,
        errored,
    }
}

/// `kind` is the *already-resolved* pairing-map value — the same value
/// `crate::claude_log::convert::content_to_display_blocks` would have
/// written during parsing. Building it pre-resolved here keeps these tests
/// focused on `build_lines`'s rendering rules rather than re-testing the
/// pairing map (covered separately by `claude_log::tests` and
/// `tool_class::tests`).
fn tool_result(kind: ResultKind, lines: &[&str], is_error: bool) -> DisplayBlock {
    DisplayBlock::ToolResult {
        kind,
        lines: lines.iter().map(|s| s.to_string()).collect(),
        is_error,
    }
}

fn thinking(text: &str, duration_secs: u64) -> DisplayBlock {
    DisplayBlock::Thinking {
        text: text.to_string(),
        duration_secs,
    }
}

fn teammate_message(id: &str, body: &str) -> DisplayBlock {
    DisplayBlock::TeammateMessage {
        id: id.to_string(),
        body: body.to_string(),
    }
}

fn entry(role: Role, blocks: Vec<DisplayBlock>) -> LogEntry {
    LogEntry {
        role,
        model: None,
        blocks,
    }
}

fn build(entries: &[LogEntry], expanded: bool) -> Vec<Line<'static>> {
    let (theme, syntax_set, syntect_theme) = fixtures();
    let cache = MarkdownCache::new();
    let ctx = BuildCtx {
        entries,
        cache: &cache,
        theme: &theme,
        syntax_set: &syntax_set,
        syntect_theme: &syntect_theme,
        expanded,
    };
    build_lines(&ctx, 80).lines
}

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// Every rendered line's text, trimmed and with blank (entry-separator)
/// lines dropped — the shape most of these tests care about.
fn non_blank_texts(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(line_text)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// The single visible (non-blank) line, panicking if there isn't exactly one
/// — used by the style-assertion tests below, which need the actual `Line`
/// (not just its text) to inspect span styles.
fn only_visible_line<'a>(lines: &'a [Line<'a>]) -> &'a Line<'a> {
    let visible: Vec<&Line<'_>> = lines
        .iter()
        .filter(|l| !line_text(l).trim().is_empty())
        .collect();
    assert_eq!(visible.len(), 1, "expected exactly one visible line, got {visible:?}");
    visible[0]
}

// ── Counted category: aggregation into one summary line ──────────────────

#[test]
fn read_results_collapse_into_aggregated_count_line() {
    let entries = vec![
        entry(Role::Assistant, vec![tool_use("Read", json!({"file_path": "/a"}))]),
        entry(
            Role::User,
            vec![
                tool_result(ResultKind::Counted { bucket: CountedBucket::Read, from_bash: false }, &["a"], false),
                tool_result(ResultKind::Counted { bucket: CountedBucket::Read, from_bash: false }, &["b"], false),
                tool_result(ResultKind::Counted { bucket: CountedBucket::Read, from_bash: false }, &["c"], false),
            ],
        ),
    ];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec!["Read 3 files (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn read_results_singular_count_uses_singular_noun() {
    let entries = vec![entry(
        Role::User,
        vec![tool_result(ResultKind::Counted { bucket: CountedBucket::Read, from_bash: false }, &["a"], false)],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec!["Read 1 file (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn grep_and_glob_share_one_search_bucket_summary() {
    // Both classify to `CountedBucket::Search`, so results from either
    // resolve to the same bucket and merge here.
    let entries = vec![entry(
        Role::User,
        vec![
            tool_result(ResultKind::Counted { bucket: CountedBucket::Search, from_bash: false }, &["match1"], false),
            tool_result(ResultKind::Counted { bucket: CountedBucket::Search, from_bash: false }, &["match2"], false),
        ],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec!["Searched for 2 patterns (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn bash_ls_collapses_to_listed_directories_summary() {
    let entries = vec![entry(
        Role::User,
        vec![
            tool_result(ResultKind::Counted { bucket: CountedBucket::List, from_bash: true }, &["a"], false),
            tool_result(ResultKind::Counted { bucket: CountedBucket::List, from_bash: true }, &["b"], false),
        ],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec!["Listed 2 directories (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn bash_cat_merges_with_read_bucket_summary() {
    // One result from a real `Read` tool call, one from `cat` via `Bash` —
    // both resolve to the `Read` bucket and merge into one line.
    let entries = vec![entry(
        Role::User,
        vec![
            tool_result(ResultKind::Counted { bucket: CountedBucket::Read, from_bash: false }, &["file a contents"], false),
            tool_result(ResultKind::Counted { bucket: CountedBucket::Read, from_bash: false }, &["file b contents"], false),
        ],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec!["Read 2 files (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn counted_result_ignores_is_error_and_still_aggregates_normally() {
    // Corrected spec (measured): `Counted` completely ignores `is_error` — a
    // failed `Read` still folds into the plain gray summary line, with no
    // error styling at all.
    let entries = vec![entry(
        Role::User,
        vec![tool_result(ResultKind::Counted { bucket: CountedBucket::Read, from_bash: false }, &["boom: file not found"], true)],
    )];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);

    assert_eq!(line_text(line).trim(), "Read 1 file (ctrl+o to expand)");
    for span in &line.spans {
        assert_ne!(span.style, Style::default().fg(palette::ERROR));
    }
}

// ── Inline category: per-call `⏺ Name(arg)` line ──────────────────────────

#[test]
fn edit_tool_collapses_to_update_display_name() {
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use("Edit", json!({"file_path": "/tmp/out.txt"}))],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec![format!("{ASSISTANT_MARKER} Update(/tmp/out.txt)")]
    );
}

#[test]
fn task_tool_collapses_to_agent_display_name() {
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use("Task", json!({"description": "investigate bug"}))],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec![format!("{ASSISTANT_MARKER} Agent(investigate bug)")]
    );
}

#[test]
fn webfetch_tool_collapses_to_fetch_display_name() {
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use("WebFetch", json!({"url": "https://example.com"}))],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec![format!("{ASSISTANT_MARKER} Fetch(https://example.com)")]
    );
}

#[test]
fn unknown_tool_falls_back_to_raw_name_and_generic_arg() {
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use("WebSearch", json!({"query": "some search term"}))],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec![format!("{ASSISTANT_MARKER} WebSearch(some search term)")]
    );
}

#[test]
fn inline_tool_use_renders_bold_name_and_text_colored_arg_not_gray() {
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use("Write", json!({"file_path": "/tmp/out.txt"}))],
    )];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);

    assert_eq!(line.spans.len(), 3, "marker + name + arg spans, got {line:?}");
    assert_eq!(line.spans[0].style, Style::default().fg(palette::SUCCESS));
    assert_eq!(line.spans[1].content.as_ref(), "Write");
    assert_eq!(
        line.spans[1].style,
        Style::default().fg(palette::TEXT).add_modifier(Modifier::BOLD)
    );
    assert_eq!(line.spans[2].content.as_ref(), "(/tmp/out.txt)");
    // The native capture shows the argument in the same color as body text,
    // not dimmed — this must NOT be `palette::INACTIVE`.
    assert_eq!(line.spans[2].style, Style::default().fg(palette::TEXT));
    assert_ne!(line.spans[2].style, Style::default().fg(palette::INACTIVE));
}

// ── Hidden category: draws nothing, in either position ────────────────────

#[test]
fn todowrite_renders_nothing_in_collapsed_mode() {
    let entries = vec![
        entry(Role::Assistant, vec![tool_use("TodoWrite", json!({"todos": []}))]),
        entry(Role::User, vec![tool_result(ResultKind::Inline, &["ok"], false)]),
    ];
    let lines = build(&entries, false);
    assert!(non_blank_texts(&lines).is_empty());
}

// ── Errors: only `Inline` category draws a result line; `Counted` is
// covered above (it ignores `is_error` completely) ────────────────────────

#[test]
fn inline_error_result_draws_multiline_error_block() {
    // Measured column layout for a failed `Bash(false)` call: `⎿` at col2,
    // body (with a prepended "Error: ") from col4 on the first line, body
    // from col5 on continuation lines, no "Error: " prefix past the first.
    let entries = vec![entry(
        Role::User,
        vec![tool_result(ResultKind::Inline,
            &[
                "bash: command failed with exit code 1",
                "second line of the error",
            ],
            true,
        )],
    )];
    let lines = build(&entries, false);
    let visible: Vec<&Line<'_>> = lines
        .iter()
        .filter(|l| !line_text(l).trim().is_empty())
        .collect();
    assert_eq!(visible.len(), 2, "first + continuation error line, got {visible:?}");

    let first = visible[0];
    assert_eq!(
        line_text(first),
        format!("  {TOOL_RESULT_GLYPH}  Error: bash: command failed with exit code 1")
    );
    assert_eq!(first.spans[0].content.as_ref(), " ");
    assert_ne!(first.spans[0].style, Style::default().fg(palette::ERROR));
    assert_eq!(first.spans[1].style, Style::default().fg(palette::ERROR));
    assert_eq!(first.spans[2].style, Style::default().fg(palette::ERROR));

    let cont = visible[1];
    assert_eq!(line_text(cont), "     second line of the error");
    assert!(
        !line_text(cont).contains("Error:"),
        "only the first error line gets the \"Error: \" prefix"
    );
    for span in &cont.spans {
        if !span.content.trim().is_empty() {
            assert_eq!(span.style, Style::default().fg(palette::ERROR));
        }
    }
}

#[test]
fn non_erroring_inline_result_renders_nothing() {
    let entries = vec![entry(
        Role::User,
        vec![tool_result(ResultKind::Inline, &["all good"], false)],
    )];
    let lines = build(&entries, false);
    assert!(non_blank_texts(&lines).is_empty());
}

#[test]
fn errored_tool_use_marker_turns_error_colored() {
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use_errored("Bash", json!({"command": "false"}), true)],
    )];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);

    assert_eq!(line.spans[0].style, Style::default().fg(palette::ERROR));
    assert_ne!(line.spans[0].style, Style::default().fg(palette::SUCCESS));
}

// ── Thinking blocks: collapsed one-liner vs. expanded header+body (S2b) ───

#[test]
fn thinking_block_collapsed_renders_one_line_summary() {
    let entries = vec![entry(Role::Assistant, vec![thinking("let me reason", 12)])];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec!["Thought for 12s (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn thinking_block_collapsed_has_no_glyph_and_starts_at_column_two() {
    // Spec: col2, no glyph — a plain two-space indent, not the `*` marker the
    // expanded header uses.
    let entries = vec![entry(Role::Assistant, vec![thinking("let me reason", 3)])];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);
    assert_eq!(line_text(line), "  Thought for 3s (ctrl+o to expand)");
}

#[test]
fn thinking_block_collapsed_bolds_only_the_duration_span() {
    let entries = vec![entry(Role::Assistant, vec![thinking("let me reason", 12)])];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);

    let bold_span = line
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "12s")
        .expect("a span with exactly the duration text");
    assert!(bold_span.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(bold_span.style.fg, Some(palette::INACTIVE));

    for span in &line.spans {
        if span.content.as_ref() != "12s" {
            assert!(
                !span.style.add_modifier.contains(Modifier::BOLD),
                "only the duration span should be bold, got bold: {span:?}"
            );
            // The leading gutter is blank and carries no style — native writes
            // nothing there at all, jumping straight to column 3 — so only the
            // text spans are checked for colour.
            if !span.content.trim().is_empty() {
                assert_eq!(span.style.fg, Some(palette::INACTIVE));
            }
        }
    }
}

#[test]
fn thinking_block_expanded_shows_header_and_body_unchanged() {
    let entries = vec![entry(Role::Assistant, vec![thinking("let me reason", 12)])];
    let lines = build(&entries, true);
    let texts = non_blank_texts(&lines);
    assert_eq!(texts[0], format!("{THINKING_GLYPH} Thinking\u{2026}"));
    assert!(
        texts.iter().any(|t| t.contains("let me reason")),
        "expanded mode must still render the reasoning body: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("Thought for")),
        "expanded mode must not show the collapsed one-liner: {texts:?}"
    );
}

// ── Teammate-message blocks: collapsed summary vs. expanded body (S4) ─────

#[test]
fn teammate_message_collapsed_renders_one_line_summary() {
    let entries = vec![entry(
        Role::User,
        vec![teammate_message("alice", "please review PR 42")],
    )];
    let lines = build(&entries, false);
    assert_eq!(
        non_blank_texts(&lines),
        vec!["\u{203a} Message from @alice (ctrl+o to expand)".to_string()]
    );
}

#[test]
fn teammate_message_collapsed_line_is_entirely_inactive() {
    let entries = vec![entry(
        Role::User,
        vec![teammate_message("alice", "please review PR 42")],
    )];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);
    for span in &line.spans {
        assert_eq!(span.style.fg, Some(palette::INACTIVE));
        assert_eq!(span.style.bg, None, "spec: no background block, unlike S3 user turns");
    }
}

#[test]
fn teammate_message_collapsed_ignores_the_body_entirely() {
    // Only the id renders in collapsed mode — the body text must not leak
    // into the summary line even if short.
    let entries = vec![entry(
        Role::User,
        vec![teammate_message("alice", "hi")],
    )];
    let lines = build(&entries, false);
    let texts = non_blank_texts(&lines);
    assert_eq!(texts.len(), 1);
    assert!(!texts[0].contains("hi"));
}

#[test]
fn teammate_message_expanded_shows_header_without_hint_then_body() {
    let entries = vec![entry(
        Role::User,
        vec![teammate_message("alice", "please review PR 42")],
    )];
    let lines = build(&entries, true);
    let texts = non_blank_texts(&lines);
    assert_eq!(texts[0], "\u{203a} Message from @alice");
    assert!(
        !texts[0].contains("ctrl+o to expand"),
        "expanded header must drop the collapsed-mode toggle hint: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("please review PR 42")),
        "expanded mode must render the full body: {texts:?}"
    );
}

// ── Expanded mode (conductor's own ctrl+o-equivalent toggle) ──────────────

#[test]
fn expanded_mode_shows_raw_tool_name_not_the_collapsed_alias() {
    // Collapsed mode renders `Edit` as `Update`; expanded mode must show the
    // tool's own raw name instead, since it draws every call individually.
    let entries = vec![entry(
        Role::Assistant,
        vec![tool_use("Edit", json!({"file_path": "/tmp/out.txt"}))],
    )];
    let lines = build(&entries, true);
    assert_eq!(
        non_blank_texts(&lines),
        vec![format!("{ASSISTANT_MARKER} Edit(/tmp/out.txt)")]
    );
}

// ── S3: user turns render as a full-width background block ────────────────

#[test]
fn user_text_renders_the_marker_glyph_not_the_assistant_bullet() {
    let entries = vec![entry(Role::User, vec![DisplayBlock::Text("hi".to_string())])];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);
    assert_eq!(line.spans[0].content, "\u{276f} ");
}

#[test]
fn user_text_marker_and_body_carry_the_background_fill_color() {
    let entries = vec![entry(Role::User, vec![DisplayBlock::Text("hi".to_string())])];
    let lines = build(&entries, false);
    let line = only_visible_line(&lines);
    for span in &line.spans {
        assert_eq!(span.style.bg, Some(palette::USER_BG), "span {span:?} missing background fill");
    }
    assert_eq!(line.spans[0].style.fg, Some(palette::USER_MARKER_FG));
    assert_eq!(line.spans[1].style.fg, Some(palette::USER_TEXT));
}

#[test]
fn user_text_bypasses_markdown_rendering() {
    // Markdown syntax in a user prompt must render as literal characters —
    // no bold/heading/etc. parsing — since user input is raw text, not prose.
    let entries = vec![entry(
        Role::User,
        vec![DisplayBlock::Text("**not bold** # not a heading".to_string())],
    )];
    let lines = build(&entries, false);
    let texts = non_blank_texts(&lines);
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].contains("**not bold** # not a heading"),
        "expected literal markdown syntax, got: {texts:?}"
    );
}

#[test]
fn user_text_preserves_source_newlines_as_separate_lines() {
    let entries = vec![entry(
        Role::User,
        vec![DisplayBlock::Text("first line\nsecond line".to_string())],
    )];
    let lines = build(&entries, false);
    let texts = non_blank_texts(&lines);
    assert_eq!(texts, vec!["\u{276f} first line", "second line"]);
}

// ── S2: no stray blank line for an entry with zero visible blocks ─────────

#[test]
fn entries_with_no_visible_blocks_produce_no_stray_blank_line() {
    // A `TodoWrite`-only entry (`Hidden` category) sits between two visible
    // text turns. It must contribute nothing — not even its own blank
    // separator — so there is exactly one blank line between "hello" and
    // "world", not two.
    let entries = vec![
        entry(Role::User, vec![DisplayBlock::Text("hello".to_string())]),
        entry(
            Role::Assistant,
            vec![tool_use("TodoWrite", json!({"todos": []}))],
        ),
        entry(Role::User, vec![DisplayBlock::Text("world".to_string())]),
    ];
    let lines = build(&entries, false);
    let texts: Vec<String> = lines.iter().map(line_text).collect();

    let hello_idx = texts
        .iter()
        .position(|t| t.contains("hello"))
        .expect("hello line present");
    let world_idx = texts
        .iter()
        .position(|t| t.contains("world"))
        .expect("world line present");
    assert_eq!(
        world_idx - hello_idx,
        2,
        "expected exactly one blank line between entries, got: {texts:?}"
    );
}

#[test]
fn counted_only_tool_use_entry_produces_no_stray_blank_line() {
    // A `Read` `tool_use` (Counted category) draws nothing at the
    // `tool_use` position — the aggregated summary draws at the paired
    // `tool_result`'s position instead (see the Counted-aggregation tests
    // above). An entry holding only such a call must not contribute a
    // blank separator either.
    let entries = vec![
        entry(Role::User, vec![DisplayBlock::Text("hello".to_string())]),
        entry(
            Role::Assistant,
            vec![tool_use("Read", json!({"file_path": "/a.txt"}))],
        ),
        entry(Role::User, vec![DisplayBlock::Text("world".to_string())]),
    ];
    let lines = build(&entries, false);
    let texts: Vec<String> = lines.iter().map(line_text).collect();

    let hello_idx = texts
        .iter()
        .position(|t| t.contains("hello"))
        .expect("hello line present");
    let world_idx = texts
        .iter()
        .position(|t| t.contains("world"))
        .expect("world line present");
    assert_eq!(
        world_idx - hello_idx,
        2,
        "expected exactly one blank line between entries, got: {texts:?}"
    );
}

#[test]
fn expanded_mode_shows_every_result_line_with_no_cap() {
    let raw_lines: Vec<String> = (0..12).map(|i| format!("line{i}")).collect();
    let raw_refs: Vec<&str> = raw_lines.iter().map(String::as_str).collect();
    let entries = vec![entry(
        Role::User,
        vec![tool_result(ResultKind::Counted { bucket: CountedBucket::Read, from_bash: false }, &raw_refs, false)],
    )];
    let lines = build(&entries, true);
    let texts = non_blank_texts(&lines);

    assert_eq!(texts.len(), 12, "no cap on expanded result lines: {texts:?}");
    for (i, text) in texts.iter().enumerate() {
        assert!(
            text.ends_with(&format!("line{i}")),
            "line {i} should end with its own content, got {text:?}"
        );
    }
}

// ── Aggregation rules measured against Claude Code (plan §4.9) ───────────

/// Build a user entry holding one result per `(kind, is_error)` pair.
fn results_entry(kinds: &[(ResultKind, bool)]) -> LogEntry {
    LogEntry {
        role: Role::User,
        model: None,
        blocks: kinds
            .iter()
            .map(|(k, e)| tool_result(*k, &["out"], *e))
            .collect(),
    }
}

fn counted(bucket: CountedBucket, from_bash: bool) -> ResultKind {
    ResultKind::Counted { bucket, from_bash }
}

#[test]
fn hidden_result_draws_nothing_even_when_it_errored() {
    // Measured: a `TodoWrite` whose result carried `is_error` produced not a
    // single line of native output. Hidden stays hidden on failure.
    let entries = vec![results_entry(&[(ResultKind::Hidden, true)])];
    assert!(non_blank_texts(&build(&entries, false)).is_empty());
}

#[test]
fn several_buckets_fold_into_one_comma_joined_line() {
    // Measured: ls x2 + Grep + Read renders as a single line, clauses ordered
    // search -> read -> list, only the first verb capitalised.
    let entries = vec![results_entry(&[
        (counted(CountedBucket::List, true), false),
        (counted(CountedBucket::List, true), false),
        (counted(CountedBucket::Search, false), false),
        (counted(CountedBucket::Read, false), false),
    ])];
    assert_eq!(
        non_blank_texts(&build(&entries, false)),
        vec!["Searched for 1 pattern, read 1 file, listed 2 directories (ctrl+o to expand)"]
    );
}

#[test]
fn two_buckets_keep_the_measured_order_and_casing() {
    // Measured: ls + Read renders "Read 1 file, listed 1 directory".
    let entries = vec![results_entry(&[
        (counted(CountedBucket::List, true), false),
        (counted(CountedBucket::Read, false), false),
    ])];
    assert_eq!(
        non_blank_texts(&build(&entries, false)),
        vec!["Read 1 file, listed 1 directory (ctrl+o to expand)"]
    );
}

#[test]
fn shell_cat_counts_only_when_the_read_tool_is_absent() {
    // The five measured combinations of `Bash(cat ...)` and `Read`.
    let cases: [(&[(ResultKind, bool)], &str); 5] = [
        (&[(counted(CountedBucket::Read, true), false)], "Read 1 file"),
        (
            &[
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, true), false),
            ],
            "Read 2 files",
        ),
        (
            &[
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, false), false),
            ],
            "Read 1 file",
        ),
        (
            &[
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, false), false),
                (counted(CountedBucket::Read, false), false),
            ],
            "Read 2 files",
        ),
        (
            &[
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, true), false),
                (counted(CountedBucket::Read, false), false),
            ],
            "Read 1 file",
        ),
    ];
    for (kinds, expected) in cases {
        let entries = vec![results_entry(kinds)];
        assert_eq!(
            non_blank_texts(&build(&entries, false)),
            vec![format!("{expected} (ctrl+o to expand)")],
            "for {kinds:?}"
        );
    }
}

#[test]
fn counted_result_ignores_is_error_entirely() {
    // Measured: a failed `Read` still folds into the plain summary.
    let entries = vec![results_entry(&[(counted(CountedBucket::Read, false), true)])];
    assert_eq!(
        non_blank_texts(&build(&entries, false)),
        vec!["Read 1 file (ctrl+o to expand)"]
    );
}

// ── Compact boundary / annotations (measured, see `claude_log::tests`) ────

fn annotation(text: &str) -> DisplayBlock {
    DisplayBlock::Annotation {
        lines: vec![text.to_string()],
    }
}

/// The whole `/compact` group, byte-for-byte as a resumed native transcript
/// draws it — including the absence of blank lines between the command and
/// its annotations, which is the reason for the separator-suppression rule.
#[test]
fn compact_group_matches_the_native_layout() {
    let entries = vec![
        entry(Role::Assistant, vec![DisplayBlock::CompactBoundary]),
        entry(Role::User, vec![DisplayBlock::Text("/compact".into())]),
        entry(
            Role::User,
            vec![annotation("Compacted (ctrl+o to see full summary)")],
        ),
        entry(Role::User, vec![annotation("Read alpha.rs (42 lines)")]),
        entry(Role::User, vec![annotation("Referenced file beta.yml")]),
        entry(
            Role::Assistant,
            vec![DisplayBlock::Text("done".into())],
        ),
    ];
    let rendered: Vec<String> = build(&entries, false)
        .iter()
        .map(|l| line_text(l).trim_end().to_string())
        .collect();

    assert_eq!(
        rendered,
        vec![
            format!("{THINKING_GLYPH} Conversation compacted (ctrl+o for history)"),
            String::new(),
            format!("{USER_MARKER} /compact"),
            format!("  {TOOL_RESULT_GLYPH}  Compacted (ctrl+o to see full summary)"),
            format!("  {TOOL_RESULT_GLYPH}  Read alpha.rs (42 lines)"),
            format!("  {TOOL_RESULT_GLYPH}  Referenced file beta.yml"),
            String::new(),
            format!("{ASSISTANT_MARKER} done"),
            String::new(),
        ]
    );
}

#[test]
fn an_annotation_never_starts_a_new_turn() {
    // The blank separator that normally follows an entry is suppressed when
    // the next entry is annotation-only, so `⏺ reply` and the `⎿` line the
    // CLI attached to it stay glued together.
    let entries = vec![
        entry(Role::Assistant, vec![DisplayBlock::Text("reply".into())]),
        entry(Role::User, vec![annotation("Read delta.rs (13 lines)")]),
    ];
    let rendered: Vec<String> = build(&entries, false)
        .iter()
        .map(|l| line_text(l).trim_end().to_string())
        .collect();
    assert_eq!(
        rendered,
        vec![
            format!("{ASSISTANT_MARKER} reply"),
            format!("  {TOOL_RESULT_GLYPH}  Read delta.rs (13 lines)"),
            String::new(),
        ]
    );
}

#[test]
fn a_notice_draws_an_assistant_bullet() {
    let entries = vec![entry(
        Role::User,
        vec![DisplayBlock::Notice(
            "Background command \"x\" completed (exit code 0)".into(),
        )],
    )];
    assert_eq!(
        line_text(only_visible_line(&build(&entries, false))).trim_end(),
        format!("{ASSISTANT_MARKER} Background command \"x\" completed (exit code 0)")
    );
}

#[test]
fn long_annotations_and_notices_stay_inside_the_panel() {
    // Both forms carry CLI-supplied text of unbounded length (a `../../..`
    // path outside the worktree runs long), so both must clip.
    let entries = vec![
        entry(Role::User, vec![annotation(&"p".repeat(300))]),
        entry(Role::User, vec![DisplayBlock::Notice("n".repeat(300))]),
        entry(Role::Assistant, vec![DisplayBlock::CompactBoundary]),
    ];
    for width in [10usize, 20, 40, 80] {
        let (theme, syntax_set, syntect_theme) = fixtures();
        let cache = MarkdownCache::new();
        let ctx = BuildCtx {
            entries: &entries,
            cache: &cache,
            theme: &theme,
            syntax_set: &syntax_set,
            syntect_theme: &syntect_theme,
            expanded: false,
        };
        for line in build_lines(&ctx, width).lines {
            let w = unicode_width::UnicodeWidthStr::width(line_text(&line).as_str());
            assert!(w <= width, "{w} cols at width {width}: {:?}", line_text(&line));
        }
    }
}

#[test]
fn a_long_annotation_wraps_instead_of_being_elided() {
    // Measured: a file carried across a compact from outside the worktree gets
    // a `../../../…` path too long for any panel, and Claude Code runs it onto
    // a continuation line aligned under the body — it does not truncate. The
    // break falls mid-path (a hard column split) and the `Read` verb keeps the
    // rest of its own line rather than sitting alone.
    let path = format!("../../../../private/tmp/{}/out.txt", "x".repeat(120));
    let entries = vec![entry(
        Role::User,
        vec![annotation(&format!("Read {path} (7 lines)"))],
    )];
    let lines = build(&entries, false);
    let texts: Vec<String> = lines
        .iter()
        .map(|l| line_text(l).trim_end().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    assert!(texts.len() > 1, "expected a wrap, got {texts:?}");
    assert!(
        texts[0].starts_with(&format!("  {TOOL_RESULT_GLYPH}  Read ../../")),
        "the verb must keep its line: {:?}",
        texts[0]
    );
    for t in texts.iter().skip(1) {
        assert!(
            t.starts_with("     ") && !t.starts_with("      "),
            "continuations align under the body at col5: {t:?}"
        );
    }
    // Nothing elided: every character of the path survives somewhere.
    let joined: String = texts.iter().map(|t| t.trim_start()).collect();
    assert!(joined.contains(&"x".repeat(120)), "path was cut: {joined}");
    assert!(!joined.contains('\u{2026}'), "unexpected ellipsis: {joined}");
}

#[test]
fn two_text_blocks_in_one_user_turn_are_separated() {
    // Measured: a user message holding two text blocks — a prompt plus an
    // appended `<system-reminder>`, say — is drawn as two `❯` turns with a
    // blank line between them, not as one packed pair. The entry-level
    // separator only fires between entries, so this covers the gap inside one.
    let entries = vec![entry(
        Role::User,
        vec![
            DisplayBlock::Text("my actual question".into()),
            DisplayBlock::Text("<system-reminder>note</system-reminder>".into()),
        ],
    )];
    let rendered: Vec<String> = build(&entries, false)
        .iter()
        .map(|l| line_text(l).trim_end().to_string())
        .collect();
    assert_eq!(
        rendered,
        vec![
            format!("{USER_MARKER} my actual question"),
            String::new(),
            format!("{USER_MARKER} <system-reminder>note</system-reminder>"),
            String::new(),
        ]
    );
}
