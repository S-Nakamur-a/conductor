//! Tool-name classification shared by the log parser (which must resolve, at
//! parse time, which result-side aggregation bucket a `tool_use` belongs to)
//! and the reflow transcript renderer (which uses [`classify`] directly to
//! lay out each `tool_use` line).
//!
//! The table implemented here is not a guess: it was reconstructed from a
//! raw-byte capture of Claude Code's own default (non-`ctrl+o`) transcript
//! output, one tool at a time. See `docs/plans/2026-07-31-native-render-parity.md`
//! §2.1 for the source table.

use serde_json::Value;

/// How Claude Code's default transcript draws one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCategory {
    /// Folds into a single "{verb} {N} {noun} (ctrl+o to expand)" summary at
    /// the `tool_result`'s position; the `tool_use` itself draws nothing.
    Counted(CountedBucket),
    /// Draws as its own `⏺ {display_name}({arg})` line at the `tool_use`'s
    /// position; the `tool_result` draws nothing on success. On error it is
    /// the *only* category that draws a result line at all — `Counted`
    /// ignores `is_error` completely (measured: a failed `Read` still folds
    /// into the plain "Read 1 file" summary) — see the reflow renderer.
    Inline {
        display_name: String,
        arg: Option<String>,
    },
    /// Draws nothing, in either position (e.g. `TodoWrite`), even on error —
    /// unmeasured for this category specifically, but Claude Code's own UI
    /// never surfaces a `TodoWrite` failure as visible transcript text.
    Hidden,
}

/// The result-side aggregation bucket a [`ToolCategory::Counted`] call falls
/// into. Two tool calls resolving to the same bucket within one entry are
/// summed into a single collapsed line — this is how, per the source table,
/// a `cat` shell invocation merges with a `Read` tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CountedBucket {
    Read,
    Search,
    List,
}

impl CountedBucket {
    /// `(verb, noun_singular, noun_plural)` for the collapsed summary line,
    /// e.g. `("Read", "file", "files")` → "Read 3 files (ctrl+o to expand)".
    pub fn labels(self) -> (&'static str, &'static str, &'static str) {
        match self {
            CountedBucket::Read => ("Read", "file", "files"),
            CountedBucket::Search => ("Searched for", "pattern", "patterns"),
            CountedBucket::List => ("Listed", "directory", "directories"),
        }
    }
}

/// The order buckets appear in when one entry produces several clauses.
/// Measured: `ls`×2 + `Grep` + `Read` renders as
/// "Searched for 1 pattern, read 1 file, listed 2 directories".
pub const BUCKET_ORDER: [CountedBucket; 3] = [
    CountedBucket::Search,
    CountedBucket::Read,
    CountedBucket::List,
];

/// What the *result* side of a tool call draws — the half of [`ToolCategory`]
/// that survives into [`crate::claude_log::DisplayBlock::ToolResult`].
///
/// `Inline` and `Hidden` must stay distinguishable here: a failed `Inline`
/// call draws an `⎿ Error:` block, while a failed `Hidden` call draws
/// **nothing** (measured: a `TodoWrite` with `is_error` produced not one line
/// of output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultKind {
    Counted {
        bucket: CountedBucket,
        /// True when a shell invocation (`ls`/`cat`) was classified into this
        /// bucket rather than the bucket's own tool being called. Native only
        /// counts these as a *fallback* — see [`BUCKET_ORDER`] and the
        /// renderer's aggregation.
        from_bash: bool,
    },
    Inline,
    Hidden,
}

/// The result-side projection of [`classify`].
pub fn result_kind(name: &str, input: &Value) -> ResultKind {
    let from_bash = name == "Bash";
    match classify(name, input) {
        ToolCategory::Counted(bucket) => ResultKind::Counted { bucket, from_bash },
        ToolCategory::Inline { .. } => ResultKind::Inline,
        ToolCategory::Hidden => ResultKind::Hidden,
    }
}

/// Classify a `tool_use` call by its raw API name and input JSON.
///
/// `Bash` is the only tool whose category depends on its argument: the
/// command's first whitespace-separated word selects `ls` → [`CountedBucket::List`],
/// `cat` → [`CountedBucket::Read`] (merging with the `Read` tool's own
/// bucket), anything else → a plain `Bash(command)` inline line.
pub fn classify(name: &str, input: &Value) -> ToolCategory {
    match name {
        "Read" => ToolCategory::Counted(CountedBucket::Read),
        "Grep" | "Glob" => ToolCategory::Counted(CountedBucket::Search),
        "Bash" => classify_bash(input),
        "Write" => inline("Write", input, "file_path"),
        "Edit" => inline("Update", input, "file_path"),
        "Task" => inline("Agent", input, "description"),
        "WebFetch" => inline("Fetch", input, "url"),
        "TodoWrite" => ToolCategory::Hidden,
        other => ToolCategory::Inline {
            display_name: other.to_string(),
            arg: unknown_tool_arg(input),
        },
    }
}

fn classify_bash(input: &Value) -> ToolCategory {
    let command = input.get("command").and_then(Value::as_str).unwrap_or("");
    match command.split_whitespace().next() {
        Some("ls") => ToolCategory::Counted(CountedBucket::List),
        Some("cat") => ToolCategory::Counted(CountedBucket::Read),
        _ => inline("Bash", input, "command"),
    }
}

/// Build an `Inline` category, reading a single named argument key. Empty or
/// absent values become `arg: None` so the renderer omits the parens
/// entirely (`⏺ Name` rather than `⏺ Name()`).
///
/// The value is sanitized like tool *output* already is: an argument is raw
/// JSON — a multi-line `Bash` command or a pattern with a tab in it — and a
/// newline or tab inside a span desyncs the terminal from ratatui's own
/// column arithmetic, which shows up as an over-wide line.
fn inline(display_name: &str, input: &Value, key: &str) -> ToolCategory {
    let arg = input
        .get(key)
        .and_then(Value::as_str)
        .map(super::convert::sanitize_preview_line)
        .filter(|s| !s.is_empty());
    ToolCategory::Inline {
        display_name: display_name.to_string(),
        arg,
    }
}

/// Argument-key search order for a tool name not in the known table, tried
/// in this fixed priority order (the source table's "未知ツールの引数キー探索順").
const UNKNOWN_ARG_KEYS: &[&str] = &[
    "command",
    "file_path",
    "path",
    "pattern",
    "url",
    "query",
    "description",
];

/// Find the first present, non-empty string argument for an unrecognised
/// tool, trying [`UNKNOWN_ARG_KEYS`] in order. Also reused by the reflow
/// renderer's expanded-mode display, where every tool (including `Counted`
/// ones like `Read`/`Grep`) needs *some* argument shown regardless of its
/// collapsed-mode category.
pub fn unknown_tool_arg(input: &Value) -> Option<String> {
    let obj = input.as_object()?;
    for key in UNKNOWN_ARG_KEYS {
        if let Some(s) = obj.get(*key).and_then(Value::as_str) {
            let cleaned = super::convert::sanitize_preview_line(s);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn read_is_counted_bucket_read() {
        let input = json!({"file_path": "/a.txt"});
        assert_eq!(classify("Read", &input), ToolCategory::Counted(CountedBucket::Read));
    }

    #[test]
    fn grep_and_glob_are_counted_bucket_search() {
        let input = json!({"pattern": "foo"});
        assert_eq!(classify("Grep", &input), ToolCategory::Counted(CountedBucket::Search));
        assert_eq!(classify("Glob", &input), ToolCategory::Counted(CountedBucket::Search));
    }

    #[test]
    fn bash_ls_is_counted_bucket_list() {
        let input = json!({"command": "ls -la /tmp"});
        assert_eq!(classify("Bash", &input), ToolCategory::Counted(CountedBucket::List));
    }

    #[test]
    fn bash_cat_merges_into_counted_bucket_read() {
        // The source table merges a `cat` shell invocation with the `Read`
        // tool's own bucket — both count toward one "Read N files" line.
        let input = json!({"command": "cat foo.txt"});
        assert_eq!(classify("Bash", &input), ToolCategory::Counted(CountedBucket::Read));
    }

    #[test]
    fn bash_other_command_is_inline() {
        let input = json!({"command": "cargo build"});
        assert_eq!(
            classify("Bash", &input),
            ToolCategory::Inline {
                display_name: "Bash".to_string(),
                arg: Some("cargo build".to_string()),
            }
        );
    }

    #[test]
    fn bash_leading_whitespace_still_dispatches_on_first_word() {
        let input = json!({"command": "   ls /tmp"});
        assert_eq!(classify("Bash", &input), ToolCategory::Counted(CountedBucket::List));
    }

    #[test]
    fn write_is_inline_with_file_path_arg() {
        let input = json!({"file_path": "/tmp/out.txt", "content": "..."});
        assert_eq!(
            classify("Write", &input),
            ToolCategory::Inline {
                display_name: "Write".to_string(),
                arg: Some("/tmp/out.txt".to_string()),
            }
        );
    }

    #[test]
    fn edit_displays_as_update() {
        let input = json!({"file_path": "/tmp/out.txt"});
        assert_eq!(
            classify("Edit", &input),
            ToolCategory::Inline {
                display_name: "Update".to_string(),
                arg: Some("/tmp/out.txt".to_string()),
            }
        );
    }

    #[test]
    fn task_displays_as_agent_with_description_arg() {
        let input = json!({"description": "investigate bug", "prompt": "..."});
        assert_eq!(
            classify("Task", &input),
            ToolCategory::Inline {
                display_name: "Agent".to_string(),
                arg: Some("investigate bug".to_string()),
            }
        );
    }

    #[test]
    fn webfetch_displays_as_fetch_with_url_arg() {
        let input = json!({"url": "https://example.com"});
        assert_eq!(
            classify("WebFetch", &input),
            ToolCategory::Inline {
                display_name: "Fetch".to_string(),
                arg: Some("https://example.com".to_string()),
            }
        );
    }

    #[test]
    fn todowrite_is_hidden() {
        let input = json!({"todos": []});
        assert_eq!(classify("TodoWrite", &input), ToolCategory::Hidden);
    }

    #[test]
    fn unknown_tool_falls_back_to_generic_arg_key_search() {
        let input = json!({"query": "some search term"});
        assert_eq!(
            classify("WebSearch", &input),
            ToolCategory::Inline {
                display_name: "WebSearch".to_string(),
                arg: Some("some search term".to_string()),
            }
        );
    }

    #[test]
    fn inline_arg_absent_key_becomes_none() {
        let input = json!({"content": "..."}); // no `file_path` key at all
        assert_eq!(
            classify("Write", &input),
            ToolCategory::Inline {
                display_name: "Write".to_string(),
                arg: None,
            }
        );
    }

    #[test]
    fn inline_arg_empty_string_becomes_none() {
        let input = json!({"file_path": ""});
        assert_eq!(
            classify("Write", &input),
            ToolCategory::Inline {
                display_name: "Write".to_string(),
                arg: None,
            }
        );
    }

    #[test]
    fn unknown_tool_arg_tries_keys_in_priority_order() {
        // `command` outranks `file_path` in UNKNOWN_ARG_KEYS.
        let input = json!({"file_path": "/a", "command": "run me"});
        assert_eq!(unknown_tool_arg(&input), Some("run me".to_string()));
    }

    #[test]
    fn unknown_tool_arg_none_when_no_known_key_present() {
        let input = json!({"unrelated": "x"});
        assert_eq!(unknown_tool_arg(&input), None);
    }

}
