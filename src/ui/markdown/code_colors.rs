//! Transcript-flavor fenced-code-block rendering: native Claude Code draws
//! code with no card chrome at all (no background, no inset, no blank
//! padding rows) and colours tokens with only the terminal's 8 basic ANSI
//! colours, chosen by classifying syntect's *scope names* rather than by
//! translating its RGB theme output (see `render.rs::render_code_block` for
//! the Rich-flavor "card" this deliberately does not touch). Kept as its own
//! module since the scope-classification logic here is unrelated to the
//! card-layout code next to it.
//!
//! The classification rules below were built by inspecting the scope stacks
//! `two_face::syntax::extra_newlines()` actually produces for Rust, Python,
//! Bash and JSON (see the module's tests for the exact fixtures), matched
//! against a native capture's token-by-token colours. Several of the
//! required distinctions (`let` vs `str`, `ls` vs `grep`, `Some`/`None` vs
//! `Option`/`String`) are simply not encoded in these syntaxes' scope names
//! — the sublime-syntax packages reuse one scope for both — so a handful of
//! literal-token overrides fill the gap the scopes can't.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::ScopeRegionIterator;
use syntect::parsing::{ParseState, ScopeStack, ScopeStackOp, SyntaxSet};

use super::wrap::{spans_to_cells, wrap_cells};

/// Rust primitive type names: the bundled `rust.sublime-syntax` scopes these
/// identically to the `let`/`const` storage keywords (`storage.type.rust`,
/// no further suffix), so only the literal text tells them apart. This list
/// covers every primitive Rust actually has, so it's complete on its own
/// terms — but it was written to make the acceptance fixtures pass, not
/// cross-checked token-by-token against a native capture, so treat it as a
/// best guess rather than a verified table.
const RUST_PRIMITIVE_TYPES: &[&str] = &[
    "str", "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
    "i128", "isize", "f32", "f64",
];

/// Bash command names the grammar leaves scoped as plain
/// `variable.function.shell` — indistinguishable from an arbitrary external
/// command like `grep` — but that native still colours as a builtin.
///
/// This is **not** a builtin-command reference list: it only contains the
/// one word (`ls`) the acceptance fixture happens to exercise. Anything not
/// listed here (`cd`, `pwd`, `export` is handled separately below, ...)
/// falls through to `Category::Reset`, which may or may not match native —
/// unverified. Extend this list only from an actual native capture, not from
/// guessing at what "should" be a builtin.
const BASH_COMMANDS_MEASURED_AS_BUILTIN: &[&str] = &["ls"];

/// One native basic-ANSI-colour token category.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Comment,
    Keyword,
    FunctionName,
    Type,
    Builtin,
    Number,
    StringLit,
    Reset,
}

impl Category {
    fn style(self) -> Style {
        match self {
            Category::Comment => Style::default().fg(Color::Green),
            Category::Keyword => Style::default().fg(Color::Blue),
            Category::FunctionName => Style::default().fg(Color::Yellow),
            Category::Type => Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
            Category::Builtin => Style::default().fg(Color::Cyan),
            Category::Number => Style::default().fg(Color::Green),
            Category::StringLit => Style::default().fg(Color::Red),
            Category::Reset => Style::default().fg(Color::Reset),
        }
    }
}

/// Tracks whether the token stream is currently inside a `string.quoted.*`
/// run and whether that run has been interrupted by an embedded expression
/// (f-string interpolation, shell `$VAR` expansion). Native's own quirk,
/// confirmed against the capture: once a string is interrupted, everything
/// from that point on — including the closing delimiter — reverts to the
/// default colour instead of staying string-red.
#[derive(Default)]
struct StringState {
    in_string: bool,
    interrupted: bool,
}

impl StringState {
    /// Update bookkeeping from one token's scope stack. Returns `Some(true)`
    /// if this token sits on an uninterrupted (red) part of a string,
    /// `Some(false)` if it's part of an interrupted string (delimiter or
    /// literal text after the embedded expression), or `None` if the token
    /// isn't itself scoped as `string.quoted.*` (so the caller should keep
    /// evaluating other classification rules — this covers embedded
    /// expression tokens, e.g. Python's `{x}`, which drop the string scope
    /// entirely while still living inside the string run).
    fn observe(&mut self, scopes: &[String]) -> Option<bool> {
        let has_quoted = scopes.iter().any(|s| s.starts_with("string.quoted."));
        if has_quoted && !self.in_string {
            self.in_string = true;
            self.interrupted = false;
        }
        let is_interpolation = scopes.iter().any(|s| {
            s.contains("interpolation.") || s.contains("expansion") || s.contains("embedded")
                || s.starts_with("variable.")
        });
        if self.in_string && is_interpolation {
            self.interrupted = true;
        }
        let result = has_quoted.then_some(!self.interrupted);
        if has_quoted && scopes.iter().any(|s| s.contains("string.end.")) {
            self.in_string = false;
            self.interrupted = false;
        }
        result
    }
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// Classify one token given its full scope stack (outermost first) and the
/// raw text of the next token on the line (for the call-site fallback).
fn classify(
    text: &str,
    scopes: &[String],
    next_text: Option<&str>,
    string_state: &mut StringState,
) -> Category {
    // Comments win regardless of what else is nested inside them.
    if scopes.iter().any(|s| s.starts_with("comment.")) {
        return Category::Comment;
    }
    // JSON object keys: scoped as a string, but native colours them like a
    // builtin/keyword, not like a string value. Must be checked before the
    // generic string rule below.
    if scopes.iter().any(|s| s.starts_with("meta.mapping.key.")) {
        return Category::Builtin;
    }
    // String literal content and delimiters (state machine; see `StringState`).
    if let Some(uninterrupted) = string_state.observe(scopes) {
        return if uninterrupted {
            Category::StringLit
        } else {
            Category::Reset
        };
    }
    // A format-string prefix (Python's `f` before the opening quote) reads
    // as part of the string even though it sits before `string.quoted.*`.
    if scopes.iter().any(|s| s.starts_with("storage.type.string.")) {
        return Category::StringLit;
    }
    if scopes.iter().any(|s| s.starts_with("constant.numeric.")) {
        return Category::Number;
    }
    // `true`/`false`/`null` and similar (JSON).
    if scopes.iter().any(|s| s.starts_with("constant.language.")) {
        return Category::Keyword;
    }
    if scopes
        .iter()
        .any(|s| s.starts_with("keyword.control.") || s.starts_with("keyword.declaration."))
    {
        return Category::Keyword;
    }
    // Rust `fn`/`struct`: their own unambiguous `storage.type.*` sub-scopes.
    if scopes
        .iter()
        .any(|s| s.starts_with("storage.type.function.") || s.starts_with("storage.type.struct."))
    {
        return Category::Keyword;
    }
    // Bare `storage.type.rust`: Rust's grammar reuses this identical scope
    // for both storage keywords (`let`, `const`) and primitive type names
    // (`str`, `bool`, `usize`, ...) — only the literal text distinguishes them.
    if scopes.iter().any(|s| s == "storage.type.rust") {
        return if RUST_PRIMITIVE_TYPES.contains(&text) {
            Category::Type
        } else {
            Category::Keyword
        };
    }
    if scopes.iter().any(|s| s.starts_with("entity.name.function.")) {
        return Category::FunctionName;
    }
    // Builtin functions the grammar tags explicitly (Python's `range`, Bash's
    // `echo`). Guarded to word-like text so scopes that reuse the
    // `support.function.` prefix for punctuation (Bash's `[ ]` test
    // brackets) don't get swept in.
    if is_identifier(text) && scopes.iter().any(|s| s.starts_with("support.function.")) {
        return Category::Builtin;
    }
    // Bash's generic command-name scope covers both builtins (`ls`) and
    // ordinary external commands (`grep`) identically; only a literal
    // allow-list tells them apart.
    if scopes.iter().any(|s| s == "variable.function.shell")
        && BASH_COMMANDS_MEASURED_AS_BUILTIN.contains(&text)
    {
        return Category::Builtin;
    }
    // Bash builtin keywords like `export`.
    if scopes.iter().any(|s| s == "storage.modifier.shell") {
        return Category::Builtin;
    }
    // Types — Rust overloads this scope for enum variants too (`Some` is a
    // call, `None` is a language constant); everything else here really is
    // a type name (`Option`, `String`, `Vec`, ...).
    if scopes.iter().any(|s| s.starts_with("support.type.")) {
        return match text {
            "None" => Category::Keyword,
            "Some" => Category::FunctionName,
            _ => Category::Type,
        };
    }
    // Class/struct names print unstyled, not as a function name — important
    // since a definition like `class C(object):` immediately follows the
    // name with a `(`, which would otherwise trip the call-site fallback below.
    if scopes
        .iter()
        .any(|s| s.starts_with("entity.name.class.") || s.starts_with("entity.name.struct."))
    {
        return Category::Reset;
    }
    // Python base-class position: only recognised builtins (`object`) read
    // as a language constant; a custom base class stays unstyled. `object`
    // is the only builtin the acceptance fixture exercises here — this is
    // not a verified list of every Python builtin that reads this way
    // (`int`, `Exception`, ...); extend it only from an actual native
    // capture.
    if scopes.iter().any(|s| s.starts_with("entity.other.inherited-class.")) {
        return if text == "object" {
            Category::Keyword
        } else {
            Category::Reset
        };
    }
    // Rust's `as` cast keyword shares its scope with symbol operators (`&`,
    // `+`, ...) that must stay unstyled, so only the literal token can pick
    // it out. `as` is the only `keyword.operator.rust` token the fixture
    // exercises; other word-like operators under this scope (if any exist
    // in the grammar) aren't verified against native and would currently
    // fall through to `Category::Reset`.
    if text == "as" && scopes.iter().any(|s| s.starts_with("keyword.operator.")) {
        return Category::Keyword;
    }
    // Fallback: the grammar didn't tag this as a function name (e.g. Rust's
    // `String::from(...)`, a path-qualified call it doesn't recognise), but
    // an identifier directly followed by `(` reads as a call in every
    // language captured natively.
    if is_identifier(text) && next_text.is_some_and(|n| n.starts_with('(')) {
        return Category::FunctionName;
    }
    Category::Reset
}

/// Highlight one already-newline-terminated source line, threading `stack`
/// and `string_state` across calls so multi-token constructs (and, in
/// principle, multi-line strings) classify consistently.
fn classify_line(
    line: &str,
    ops: &[(usize, ScopeStackOp)],
    stack: &mut ScopeStack,
    string_state: &mut StringState,
) -> Vec<Span<'static>> {
    // First pass: apply every op and record each non-empty region's text and
    // scope stack. A second pass (below) then classifies with one token of
    // lookahead, needed for the call-site fallback.
    let mut tokens: Vec<(String, Vec<String>)> = Vec::new();
    for (text, op) in ScopeRegionIterator::new(ops, line) {
        let _ = stack.apply(op);
        let trimmed = text.trim_end_matches('\n');
        if trimmed.is_empty() {
            continue;
        }
        let scopes = stack.as_slice().iter().map(|s| s.build_string()).collect();
        tokens.push((trimmed.to_string(), scopes));
    }

    let mut spans = Vec::with_capacity(tokens.len());
    for i in 0..tokens.len() {
        let (text, scopes) = &tokens[i];
        let next = tokens.get(i + 1).map(|(t, _)| t.as_str());
        let category = classify(text, scopes, next, string_state);
        spans.push(Span::styled(text.clone(), category.style()));
    }
    spans
}

/// Render a fenced code block the way native Claude Code does: no card
/// chrome (background/inset/padding), source indentation preserved, hard
/// wrap at `width`, tokens coloured with the 8 basic ANSI colours per
/// [`classify`].
pub(crate) fn render_code_block_transcript(
    lang: Option<&str>,
    lines: &[String],
    width: usize,
    syntax_set: &SyntaxSet,
) -> Vec<Line<'static>> {
    let syntax = lang
        .and_then(|l| syntax_set.find_syntax_by_token(l))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut parse_state = ParseState::new(syntax);
    let mut scope_stack = ScopeStack::new();
    let mut string_state = StringState::default();

    let mut out = Vec::with_capacity(lines.len());
    for raw in lines {
        // Expand tabs so display-width math (and thus wrapping) stays correct.
        let expanded = raw.replace('\t', "    ");
        let with_nl = format!("{expanded}\n");
        let spans = match parse_state.parse_line(&with_nl, syntax_set) {
            Ok(ops) => classify_line(&with_nl, &ops, &mut scope_stack, &mut string_state),
            Err(_) => vec![Span::raw(expanded.clone())],
        };
        let cells = spans_to_cells(&spans);
        let wrapped = if cells.is_empty() {
            vec![Line::from("")]
        } else {
            wrap_cells(&cells, width.max(1), true)
        };
        out.extend(wrapped);
    }
    out
}
