//! Invariant sweep over real Claude Code session logs.
//!
//! This writes down no expected output at all — it asserts properties that
//! must hold for *any* transcript, at every width. That is the point: real
//! logs contain inputs nobody would think to write by hand (broken UTF-8,
//! ZWJ emoji, nested code fences, megabyte tool results), and the properties
//! below are exactly the ones whose violation shows up as the panel visibly
//! corrupting itself.
//!
//! Opt-in: set `CONDUCTOR_TRANSCRIPT_CORPUS` to a directory of `.jsonl`
//! session logs. Skipped when unset. The logs are **not** in the repository
//! and must never be — they carry file paths, prompts and tool output.

use std::path::PathBuf;

use syntect::highlighting::ThemeSet;
use unicode_width::UnicodeWidthStr;

use crate::claude_log::{LogEntry, load_session};
use crate::ui::markdown::MarkdownCache;

use super::build::{BuildCtx, build_lines};

const WIDTHS: [usize; 6] = [20, 40, 60, 80, 120, 200];

/// Cap the sweep so a `cargo test` run stays in seconds. A corpus can hold
/// hundreds of logs; the invariants are about *shapes* of content, and the
/// biggest files repeat the same shapes rather than adding new ones.
const MAX_FILES: usize = 30;
const MAX_BYTES: u64 = 5 * 1024 * 1024;

fn corpus_files() -> Option<Vec<PathBuf>> {
    let dir = std::env::var_os("CONDUCTOR_TRANSCRIPT_CORPUS")?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .filter(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES)
        })
        .collect();
    // Sort so a failure is reproducible rather than depending on readdir order.
    files.sort();
    files.truncate(MAX_FILES);
    Some(files)
}

fn line_width(line: &ratatui::text::Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

fn texts(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

struct Harness {
    theme: crate::theme::Theme,
    syntax_set: syntect::parsing::SyntaxSet,
    syntect_theme: syntect::highlighting::Theme,
}

impl Harness {
    fn new() -> Self {
        Self {
            theme: crate::theme::Theme::default(),
            syntax_set: two_face::syntax::extra_newlines(),
            syntect_theme: ThemeSet::load_defaults()
                .themes
                .remove("base16-ocean.dark")
                .unwrap(),
        }
    }

    fn build(
        &self,
        cache: &MarkdownCache,
        entries: &[LogEntry],
        width: usize,
        expanded: bool,
    ) -> super::build::BuiltLines {
        let ctx = BuildCtx {
            entries,
            cache,
            theme: &self.theme,
            syntax_set: &self.syntax_set,
            syntect_theme: &self.syntect_theme,
            expanded,
        };
        build_lines(&ctx, width)
    }
}

#[test]
fn real_transcripts_hold_the_layout_invariants() {
    let Some(files) = corpus_files() else {
        eprintln!("CONDUCTOR_TRANSCRIPT_CORPUS unset — skipping corpus sweep");
        return;
    };
    assert!(!files.is_empty(), "corpus directory holds no .jsonl files");

    let h = Harness::new();
    let mut checked = 0usize;

    for path in &files {
        let entries = load_session(path);
        if entries.is_empty() {
            continue;
        }
        for expanded in [false, true] {
            for width in WIDTHS {
                let cache = MarkdownCache::new();
                let built = h.build(&cache, &entries, width, expanded);

                // (1) Nothing may exceed the panel. This is the direct
                // detector for the bleed class of bug, and the reason the
                // width-1 safety margin could be removed.
                for (i, line) in built.lines.iter().enumerate() {
                    let w = line_width(line);
                    assert!(
                        w <= width,
                        "{}: line {i} is {w} cols at width {width} (expanded={expanded}): {:?}",
                        path.display(),
                        texts(&built.lines[i..=i])[0],
                    );
                }

                // (2) Every line must carry metadata: the renderer zips the
                // two, and a short `meta` would silently truncate the view.
                assert_eq!(
                    built.lines.len(),
                    built.meta.len(),
                    "{}: line/meta length mismatch at width {width}",
                    path.display()
                );

                // (3) Building twice must agree — the Markdown cache sits in
                // the middle of this path and a stale hit would show up here.
                let again = h.build(&cache, &entries, width, expanded);
                assert_eq!(
                    texts(&built.lines),
                    texts(&again.lines),
                    "{}: rebuild at width {width} differed",
                    path.display()
                );

                checked += 1;
            }
        }
    }
    assert!(checked > 0, "corpus produced no displayable entries");
    eprintln!("corpus sweep: {} files x {} builds", files.len(), checked);
}

#[test]
fn rebuilding_at_a_previous_width_reproduces_it() {
    let Some(files) = corpus_files() else {
        eprintln!("CONDUCTOR_TRANSCRIPT_CORPUS unset — skipping corpus sweep");
        return;
    };
    let h = Harness::new();

    for path in &files {
        let entries = load_session(path);
        if entries.is_empty() {
            continue;
        }
        // One cache across all three builds, exactly as the real render path
        // uses it: a width round-trip must land back on the original layout
        // rather than on whatever the intermediate width left cached.
        let cache = MarkdownCache::new();
        let first = texts(&h.build(&cache, &entries, 80, false).lines);
        let _ = h.build(&cache, &entries, 40, false);
        let back = texts(&h.build(&cache, &entries, 80, false).lines);
        assert_eq!(first, back, "{}: 80 -> 40 -> 80 was not idempotent", path.display());
    }
}
