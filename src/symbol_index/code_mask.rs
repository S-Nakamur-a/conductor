//! Which identifier occurrences on a line sit in *code* position — as opposed
//! to inside a comment or a string literal.
//!
//! The symbol index knows where definitions are, but every query that starts
//! from something on screen (`gd`, the hover popup, Cmd+Click, the `g`-prefix
//! symbol hints) has to answer a different question first: *is the word the
//! user is pointing at actually code?* Without that, an English word in a doc
//! comment resolves to a same-named symbol and the UI offers a jump that means
//! nothing.
//!
//! # Why occurrence indices instead of byte ranges
//!
//! The viewer stores tab-expanded lines (`ViewerState::open_file` runs every
//! line through `expand_tabs`), so a byte offset into the source does not line
//! up with a column on screen — and Go, one of the indexed languages, is
//! tab-indented by convention. Expanding tabs replaces `\t` with spaces, which
//! changes columns but never reorders or rewrites identifiers. So "the k-th
//! identifier on this line" survives expansion, and that is what this mask is
//! keyed by. Both sides must agree on what counts as an identifier, which is
//! why [`identifier_occurrences`] is the single definition of that and is used
//! to build the mask *and* to query it.
//!
//! # Why an allowlist
//!
//! The mask records the occurrences that *are* code, not the ones that are
//! masked out. The two are equivalent when everything works; they differ when
//! something goes wrong. An empty allowlist means nothing is jumpable — the
//! feature goes quiet. An empty blocklist would mean everything is jumpable,
//! which is exactly the bug this module exists to fix, and it would come back
//! silently. Failing toward "no jump offered" is the direction that costs the
//! user least: a jump that does not happen is a non-event, while a jump to the
//! wrong place displaces what they were reading.
//!
//! That choice also settles files in languages we have no grammar for: they get
//! no mask, so nothing in them is jumpable.

use std::sync::OnceLock;

use regex::Regex;

/// Identifier occurrences beyond this index on a single line are treated as
/// non-code. Real code does not come close — the widest line in this
/// repository holds 76 identifiers — and the alternative to a fixed cap is a
/// spill representation whose only purpose is to be exercised by nothing.
const MAX_TRACKED_PER_LINE: usize = 128;

/// The one definition of "an identifier occurrence", shared by mask
/// construction and mask lookup.
///
/// Keeping a single implementation is not a tidiness preference: the mask is
/// keyed by position in this sequence, so if the two sides ever disagreed
/// about what counts as an identifier the indices would silently shift and the
/// mask would answer for the wrong word.
///
/// Yields `(start_byte, end_byte, text)` for each match, in order.
pub fn identifier_occurrences(line: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").unwrap());
    re.find_iter(line).map(|m| (m.start(), m.end(), m.as_str()))
}

/// Per-line record of which identifier occurrences are in code position.
///
/// Dense in the line number (one entry per line of the file) because it
/// describes exactly one open file, so the wasted words on comment-only lines
/// are cheaper than the indirection of a map.
#[derive(Debug, Default, Clone)]
pub struct CodeMask {
    /// `lines[i]` has bit `k` set when the k-th identifier on line `i + 1` is
    /// code. Lines past the end of the vector hold nothing.
    lines: Vec<u128>,
    /// Whether this mask is an answer at all, as opposed to the absence of one.
    /// False for a language we have no grammar for.
    supported: bool,
}

impl CodeMask {
    /// Whether we could analyse this file's language at all.
    ///
    /// Worth distinguishing from "no identifiers are code", because the two
    /// call for opposite handling and the bits alone cannot tell them apart.
    /// Offering a jump is a claim about one word, so silence is the safe
    /// answer and an unanalysable file should offer nothing. Listing
    /// references is a claim about the whole repository, and there "no
    /// results" is not silence — it asserts that none exist. A caller
    /// answering a search should fall back to unfiltered matches here rather
    /// than report an empty result it cannot stand behind.
    pub fn is_supported(&self) -> bool {
        self.supported
    }

    /// Whether the `occurrence`-th identifier (0-based) on `line_1` (1-based)
    /// is in code position.
    ///
    /// Anything this mask does not know about answers `false`, which is what
    /// makes the allowlist fail quiet: an out-of-range line, an occurrence past
    /// [`MAX_TRACKED_PER_LINE`], and a file we could not parse all land here.
    pub fn is_code(&self, line_1: usize, occurrence: usize) -> bool {
        if line_1 == 0 || occurrence >= MAX_TRACKED_PER_LINE {
            return false;
        }
        match self.lines.get(line_1 - 1) {
            Some(bits) => bits & (1u128 << occurrence) != 0,
            None => false,
        }
    }

    /// Whether the identifier covering `col` on `line_1` is in code position,
    /// where `col` is a byte offset into the *rendered* line.
    ///
    /// Resolves the column to an occurrence index using the same scan that
    /// built the mask, so tab expansion — which shifts columns but not
    /// ordering — does not matter.
    pub fn is_code_at_column(&self, rendered_line: &str, line_1: usize, col: usize) -> bool {
        for (k, (start, end, _)) in identifier_occurrences(rendered_line).enumerate() {
            if col >= start && col < end {
                return self.is_code(line_1, k);
            }
        }
        false
    }

    /// Build a mask for `source`, dispatching on `path`'s extension.
    ///
    /// Returns an empty mask (nothing is code) for a language we have no
    /// grammar for, and for a file tree-sitter declines to parse.
    pub fn compute(source: &str, path: &str) -> Self {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let Some(grammar) = grammar_for(ext) else {
            return Self::default();
        };

        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&grammar.language).is_err() {
            return Self::default();
        }
        let Some(tree) = parser.parse(source, None) else {
            return Self::default();
        };

        Self::from_masked_ranges(source, &collect_masked_ranges(&tree, source, &grammar))
    }

    /// Set a bit for every identifier occurrence that falls outside `masked`.
    ///
    /// `masked` must be sorted by start offset and non-overlapping, which is
    /// what a pre-order walk that stops descending at a masked node produces.
    fn from_masked_ranges(source: &str, masked: &[(usize, usize)]) -> Self {
        let mut lines = Vec::new();
        let mut line_start = 0usize;
        // Index of the first range that could still cover the current line.
        // Ranges and lines both advance monotonically, so this never rewinds.
        let mut cursor = 0usize;

        for line in source.split_inclusive('\n') {
            let mut bits: u128 = 0;
            let trimmed = line.trim_end_matches(['\n', '\r']);

            while cursor < masked.len() && masked[cursor].1 <= line_start {
                cursor += 1;
            }

            for (k, (start, _, _)) in identifier_occurrences(trimmed).enumerate() {
                if k >= MAX_TRACKED_PER_LINE {
                    break;
                }
                let abs = line_start + start;
                // A block comment or multi-line string can span many lines, so
                // scan forward from `cursor` without consuming it — a later
                // identifier on this same line may fall before the same range.
                let inside = masked[cursor..]
                    .iter()
                    .take_while(|(s, _)| *s <= abs)
                    .any(|(s, e)| abs >= *s && abs < *e);
                if !inside {
                    bits |= 1u128 << k;
                }
            }

            lines.push(bits);
            line_start += line.len();
        }

        Self {
            lines,
            supported: true,
        }
    }
}

/// Carve the captured identifiers of Rust's inline format arguments back out
/// of a masked range.
///
/// `format!("{widget:?}")` names a real binding, and the equivalent in every
/// other language here stays navigable — TypeScript's `${...}` falls out of
/// the grammar as its own node. tree-sitter-rust does not split the format
/// string, so the whole literal arrives as one `string_content` and the
/// identifiers inside it would be masked along with the prose. On a 2021-era
/// codebase that is not an edge case: this repository has 945 such references
/// across 159 files.
///
/// Given one masked range, returns the sub-ranges that remain masked once each
/// `{ident}` / `{ident:spec}` identifier is excluded. `{}` and `{0}` hold no
/// identifier and are left alone, as is `{{`, which is an escaped brace.
fn subtract_format_args(source: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let text = &source[start..end];
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut cut = 0usize; // start of the masked stretch being accumulated
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        // `{{` escapes a literal brace — no argument here.
        if bytes.get(i + 1) == Some(&b'{') {
            i += 2;
            continue;
        }
        let name_start = i + 1;
        let mut j = name_start;
        if bytes.get(j).is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_') {
            j += 1;
            while bytes
                .get(j)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                j += 1;
            }
            // Only a name terminated by `}` or a `:` format spec is a capture;
            // anything else is prose that happens to follow a brace.
            if matches!(bytes.get(j), Some(b'}') | Some(b':')) {
                if name_start > cut {
                    out.push((start + cut, start + name_start));
                }
                cut = j;
                i = j;
                continue;
            }
        }
        i = name_start;
    }

    if cut < bytes.len() {
        out.push((start + cut, end));
    }
    out
}

/// Node kinds whose contents are prose or literal text rather than code, per
/// grammar. Verified against each grammar's actual output rather than guessed
/// from the names — see the note on TypeScript below.
/// A grammar's masked node kinds, plus the subset whose text may hold inline
/// format captures that must stay navigable (see [`subtract_format_args`]).
struct Grammar {
    language: tree_sitter::Language,
    masked: &'static [&'static str],
    format_capable: &'static [&'static str],
}

fn grammar_for(ext: &str) -> Option<Grammar> {
    const RUST: &[&str] = &[
        "line_comment",
        "block_comment",
        "doc_comment",
        "string_content",
        "raw_string_literal",
        "char_literal",
    ];
    // Both string forms reach `format!`, so `format!(r#"{x}"#)` counts too.
    // Comments do not: `{x}` in prose names nothing.
    const RUST_FORMAT: &[&str] = &["string_content", "raw_string_literal"];
    const GO: &[&str] = &[
        "comment",
        "interpreted_string_literal_content",
        "raw_string_literal_content",
    ];
    // Go's `%v` verbs carry no identifier, and TypeScript's interpolations are
    // already separate nodes, so neither needs the Rust carve-out.
    const NONE: &[&str] = &[];
    // `string_fragment` — deliberately not `template_string`. A template
    // literal parses as ['`', string_fragment, template_substitution,
    // string_fragment, '`'], so masking the whole node would swallow the
    // interpolated expressions, which are ordinary code and should stay
    // jumpable. Masking the fragments covers plain strings and the literal
    // stretches of templates while leaving `${...}` alone.
    const TS: &[&str] = &["comment", "string_fragment"];

    let (language, masked, format_capable) = match ext {
        "rs" => (tree_sitter_rust::LANGUAGE.into(), RUST, RUST_FORMAT),
        "go" => (tree_sitter_go::LANGUAGE.into(), GO, NONE),
        "ts" | "js" => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            TS,
            NONE,
        ),
        "tsx" | "jsx" => (tree_sitter_typescript::LANGUAGE_TSX.into(), TS, NONE),
        _ => return None,
    };
    Some(Grammar {
        language,
        masked,
        format_capable,
    })
}

/// Pre-order walk collecting the byte ranges of masked nodes, not descending
/// into one once matched. Ranges come out sorted and non-overlapping.
fn collect_masked_ranges(
    tree: &tree_sitter::Tree,
    source: &str,
    grammar: &Grammar,
) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut cursor = tree.walk();

    loop {
        let node = cursor.node();
        if grammar.masked.contains(&node.kind()) {
            let (start, stop) = (node.start_byte(), node.end_byte());
            if grammar.format_capable.contains(&node.kind()) {
                // Emitted in ascending, non-overlapping order, which is what
                // `from_masked_ranges` relies on.
                ranges.extend(subtract_format_args(source, start, stop));
            } else {
                ranges.push((start, stop));
            }
            // Masked: skip the subtree entirely.
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    return ranges;
                }
            }
            continue;
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return ranges;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect `(occurrence_index, text, is_code)` for one 1-based line.
    fn row(mask: &CodeMask, source: &str, line_1: usize) -> Vec<(usize, String, bool)> {
        let line = source.lines().nth(line_1 - 1).unwrap();
        identifier_occurrences(line)
            .enumerate()
            .map(|(k, (_, _, text))| (k, text.to_string(), mask.is_code(line_1, k)))
            .collect()
    }

    /// Expected values below are counted by hand from the fixture, not derived
    /// from the implementation — a mask checked against its own construction
    /// would pass no matter what it did.
    #[test]
    fn rust_masks_comments_strings_and_chars() {
        let src = "\
// comment mentions Foo
fn real(x: i32) -> Foo {
    let s = \"Foo in string\";
    let c = 'x';
    bar(Foo)
}
";
        let mask = CodeMask::compute(src, "lib.rs");

        // Every word in a line comment is prose.
        assert_eq!(
            row(&mask, src, 1),
            vec![
                (0, "comment".into(), false),
                (1, "mentions".into(), false),
                (2, "Foo".into(), false),
            ]
        );
        // A declaration is entirely code, keyword included — filtering
        // keywords is the caller's job, not the mask's.
        assert_eq!(
            row(&mask, src, 2),
            vec![
                (0, "fn".into(), true),
                (1, "real".into(), true),
                (2, "x".into(), true),
                (3, "i32".into(), true),
                (4, "Foo".into(), true),
            ]
        );
        // `let`/`s` are code; the three words inside the literal are not.
        assert_eq!(
            row(&mask, src, 3),
            vec![
                (0, "let".into(), true),
                (1, "s".into(), true),
                (2, "Foo".into(), false),
                (3, "in".into(), false),
                (4, "string".into(), false),
            ]
        );
        // Char literals hide identifiers too.
        assert_eq!(
            row(&mask, src, 4),
            vec![
                (0, "let".into(), true),
                (1, "c".into(), true),
                (2, "x".into(), false),
            ]
        );
        // Same name as the masked ones on line 3, but in code position.
        assert_eq!(
            row(&mask, src, 5),
            vec![(0, "bar".into(), true), (1, "Foo".into(), true)]
        );
    }

    #[test]
    fn go_masks_comments_and_both_string_forms() {
        let src = "package main\n// Foo does things\nfunc Bar() {\n\ts := \"Foo\"\n\tr := `Foo raw`\n}\n";
        let mask = CodeMask::compute(src, "main.go");

        assert_eq!(
            row(&mask, src, 2),
            vec![
                (0, "Foo".into(), false),
                (1, "does".into(), false),
                (2, "things".into(), false),
            ]
        );
        assert_eq!(
            row(&mask, src, 3),
            vec![(0, "func".into(), true), (1, "Bar".into(), true)]
        );
        // Interpreted string.
        assert_eq!(
            row(&mask, src, 4),
            vec![(0, "s".into(), true), (1, "Foo".into(), false)]
        );
        // Raw string — a different node kind in the grammar, same treatment.
        assert_eq!(
            row(&mask, src, 5),
            vec![
                (0, "r".into(), true),
                (1, "Foo".into(), false),
                (2, "raw".into(), false),
            ]
        );
    }

    /// The case a substring test on the node kind would get wrong: a template
    /// literal's interpolations are code and must stay jumpable, even though
    /// they sit inside a node whose name contains "string".
    #[test]
    fn typescript_keeps_template_interpolations_jumpable() {
        let src = "// Foo comment\nconst t = `text ${realCode} more`;\nconst s = \"Foo\";\n";
        let mask = CodeMask::compute(src, "a.ts");

        assert_eq!(
            row(&mask, src, 1),
            vec![(0, "Foo".into(), false), (1, "comment".into(), false)]
        );
        assert_eq!(
            row(&mask, src, 2),
            vec![
                (0, "const".into(), true),
                (1, "t".into(), true),
                (2, "text".into(), false),
                (3, "realCode".into(), true), // ← inside ${ }, still code
                (4, "more".into(), false),
            ]
        );
        assert_eq!(
            row(&mask, src, 3),
            vec![
                (0, "const".into(), true),
                (1, "s".into(), true),
                (2, "Foo".into(), false),
            ]
        );
    }

    /// Rust's counterpart to the TypeScript template case: an identifier
    /// captured by a format string names a real binding and must stay
    /// navigable, even though the grammar hands the whole literal over as one
    /// `string_content` with no structure inside it.
    #[test]
    fn rust_keeps_inline_format_captures_jumpable() {
        let src = "\
fn f(widget: u32) {
    let s = format!(\"{widget} and {}\", widget);
    println!(\"{widget:?} plus {count:>3} prose\");
    let raw = format!(r#\"{widget}\"#);
    let escaped = format!(\"{{widget}} literal\");
    let positional = format!(\"{0} {} text\", widget);
}
";
        let mask = CodeMask::compute(src, "lib.rs");

        assert_eq!(
            row(&mask, src, 2),
            vec![
                (0, "let".into(), true),
                (1, "s".into(), true),
                (2, "format".into(), true),
                (3, "widget".into(), true), // captured by `{widget}`
                (4, "and".into(), false),   // prose between the braces
                (5, "widget".into(), true), // ordinary trailing argument
            ]
        );
        // A format spec after `:` still leaves the name captured.
        assert_eq!(
            row(&mask, src, 3),
            vec![
                (0, "println".into(), true),
                (1, "widget".into(), true),
                (2, "plus".into(), false),
                (3, "count".into(), true),
                (4, "prose".into(), false),
            ]
        );
        // Raw strings reach `format!` too. The `r` prefix stays masked: it is
        // part of the literal's own syntax, not a reference to anything.
        assert_eq!(
            row(&mask, src, 4),
            vec![
                (0, "let".into(), true),
                (1, "raw".into(), true),
                (2, "format".into(), true),
                (3, "r".into(), false),
                (4, "widget".into(), true),
            ]
        );
        // `{{` is an escaped brace, so this names nothing.
        assert_eq!(
            row(&mask, src, 5),
            vec![
                (0, "let".into(), true),
                (1, "escaped".into(), true),
                (2, "format".into(), true),
                (3, "widget".into(), false),
                (4, "literal".into(), false),
            ]
        );
        // `{0}` and `{}` carry no identifier; the real argument does.
        assert_eq!(
            row(&mask, src, 6),
            vec![
                (0, "let".into(), true),
                (1, "positional".into(), true),
                (2, "format".into(), true),
                (3, "text".into(), false),
                (4, "widget".into(), true),
            ]
        );
    }

    #[test]
    fn block_comment_spanning_lines_masks_all_of_them() {
        let src = "fn a() {}\n/* Foo\n   Bar\n   Baz */\nfn b() {}\n";
        let mask = CodeMask::compute(src, "lib.rs");

        assert!(mask.is_code(1, 0)); // fn
        for line in 2..=4 {
            assert!(
                row(&mask, src, line).iter().all(|(_, _, code)| !*code),
                "line {line} should be entirely masked"
            );
        }
        assert!(mask.is_code(5, 0)); // fn
    }

    /// Tab expansion shifts columns but not ordering, which is the whole reason
    /// the mask is keyed by occurrence index. Go is tab-indented by convention,
    /// so this is the common case, not a corner.
    #[test]
    fn occurrence_indices_survive_tab_expansion() {
        let src = "package main\nfunc f() {\n\tx := \"Foo\"\n}\n";
        let mask = CodeMask::compute(src, "main.go");

        let raw = src.lines().nth(2).unwrap();
        let expanded = raw.replace('\t', "    ");
        assert_ne!(raw, expanded, "fixture must actually contain a tab");

        // Same verdicts whether the caller hands us the raw or rendered line.
        assert!(mask.is_code_at_column(&expanded, 3, expanded.find('x').unwrap()));
        assert!(!mask.is_code_at_column(&expanded, 3, expanded.find("Foo").unwrap()));
    }

    #[test]
    fn unsupported_language_offers_nothing() {
        let src = "def build(x):\n    return x\n";
        let mask = CodeMask::compute(src, "script.py");
        assert!(!mask.is_code(1, 0));
        assert!(!mask.is_code(2, 0));
    }

    #[test]
    fn out_of_range_lookups_are_not_code() {
        let mask = CodeMask::compute("fn a() {}\n", "lib.rs");
        assert!(!mask.is_code(0, 0), "line numbers are 1-based");
        assert!(!mask.is_code(99, 0), "past end of file");
        assert!(!mask.is_code(1, MAX_TRACKED_PER_LINE), "past the cap");
    }
}
