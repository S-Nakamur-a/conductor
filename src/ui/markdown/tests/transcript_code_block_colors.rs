//! Acceptance tests for Transcript-flavor fenced-code-block colouring
//! (`code_colors.rs`): exact source fixtures and expected 8-basic-ANSI-colour
//! tokens, matched against a native Claude Code capture. Kept as its own
//! file (rather than appended to `rendering.rs`, already at the project's
//! size guideline) since these are a distinct, self-contained fixture set.

use super::*;
use ratatui::style::Color;

/// Render one fenced code block in Transcript flavor.
fn code_lines(lang: &str, source: &str) -> Vec<Line<'static>> {
    let text = format!("```{lang}\n{source}\n```");
    render_transcript(&text, 100)
}

/// Find the single span whose content is exactly `text` — not a substring
/// match, since same-style adjacent syntect tokens coalesce into one span
/// (`wrap.rs`'s `cells_to_line`). Callers ask for the merged run they expect
/// (e.g. `"hi"` for a whole string literal, not just `hi`).
fn span<'a>(lines: &'a [Line<'static>], text: &str) -> &'a Span<'static> {
    lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == text)
        .unwrap_or_else(|| panic!("no span with exact content {text:?}"))
}

fn assert_fg(lines: &[Line<'static>], text: &str, color: Color) {
    let s = span(lines, text);
    assert_eq!(s.style.fg, Some(color), "{text:?} expected fg {color:?}, got {:?}", s.style.fg);
}

fn assert_fg_dim(lines: &[Line<'static>], text: &str, color: Color) {
    let s = span(lines, text);
    assert_eq!(s.style.fg, Some(color), "{text:?} expected fg {color:?}, got {:?}", s.style.fg);
    assert!(s.style.add_modifier.contains(Modifier::DIM), "{text:?} expected DIM modifier");
}

/// Find the colour of `word` as a standalone identifier, even where it was
/// coalesced into a wider same-style span together with neighbouring
/// punctuation/whitespace (`wrap.rs` merges adjacent same-style spans, so an
/// unstyled identifier like a struct name commonly ends up glued to the
/// braces/colons/spaces around it). Tokenizes each span's content on
/// non-word characters and looks for an exact-word occurrence.
///
/// Also asserts DIM is absent — checking `fg` alone would silently accept a
/// span that's actually `Type` (Cyan + DIM) where the fixture expects
/// `Builtin` (Cyan, no DIM), since both share the same foreground colour.
/// All current callers expect a non-dim colour; if a future fixture needs a
/// dim word-based check, extend this rather than adding an unchecked twin.
fn assert_word_fg(lines: &[Line<'static>], word: &str, color: Color) {
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    for line in lines {
        for s in &line.spans {
            if s.content.split(|c: char| !is_word_char(c)).any(|tok| tok == word) {
                assert_eq!(s.style.fg, Some(color), "{word:?} expected fg {color:?}, got {:?}", s.style.fg);
                assert!(
                    !s.style.add_modifier.contains(Modifier::DIM),
                    "{word:?} unexpectedly carries DIM (Type vs Builtin mix-up?)"
                );
                return;
            }
        }
    }
    panic!("no occurrence of word {word:?}");
}

#[test]
fn transcript_code_block_has_no_card_chrome() {
    // No background colour, no inset, no blank padding rows — content starts
    // at column 0 and the block is exactly as many rows as source lines.
    let lines = code_lines("rust", "let x = 1;");
    assert_eq!(lines.len(), 1, "no top/bottom padding rows");
    for span in &lines[0].spans {
        assert_eq!(span.style.bg, None, "no background colour");
    }
    assert_eq!(lines[0].spans[0].content.as_ref(), "let", "content starts at column 0, no left inset");
}

#[test]
fn transcript_code_block_rust_colours() {
    let src = "// a line comment\n\
/// a doc comment\n\
fn f(a: &str, b: bool) -> Option<u32> {\n\
    let s = String::from(\"hi\");   // trailing\n\
    const N: usize = 42;\n\
    if b { Some(N as u32) } else { None }\n\
}\n\
struct S { field: Vec<u8> }";
    let lines = code_lines("rust", src);

    assert_fg(&lines, "// a line comment", Color::Green);
    assert_fg(&lines, "/// a doc comment", Color::Green);
    assert_fg(&lines, "// trailing", Color::Green);
    assert_fg(&lines, "42", Color::Green);

    for kw in ["fn", "let", "const", "if", "as", "else", "struct", "None"] {
        assert_fg(&lines, kw, Color::Blue);
    }

    assert_fg(&lines, "f", Color::Yellow);
    assert_fg(&lines, "from", Color::Yellow);
    assert_fg(&lines, "Some", Color::Yellow);

    for ty in ["str", "bool", "Option", "u32", "String", "usize", "Vec", "u8"] {
        assert_fg_dim(&lines, ty, Color::Cyan);
    }

    assert_fg(&lines, "\"hi\"", Color::Red);

    // Struct name reads unstyled, not as a function/type name. These plain
    // identifiers get coalesced into a wider Reset span alongside the
    // punctuation/whitespace around them, so look them up by word rather
    // than by exact span content.
    assert_word_fg(&lines, "S", Color::Reset);
    for ident in ["s", "N", "b", "field"] {
        assert_word_fg(&lines, ident, Color::Reset);
    }
}

#[test]
fn transcript_code_block_python_colours() {
    let src = "# python comment\n\
import os\n\
def g(x=1, *args):\n\
    s = f\"val {x}\"\n\
    return [i for i in range(10) if i % 2 == 0]\n\
class C(object):\n\
    pass";
    let lines = code_lines("python", src);

    assert_fg(&lines, "# python comment", Color::Green);

    for kw in ["import", "def", "return", "for", "in", "if", "class", "object", "pass"] {
        assert_fg(&lines, kw, Color::Blue);
    }

    assert_fg(&lines, "g", Color::Yellow);
    assert_fg(&lines, "range", Color::Cyan);

    for num in ["1", "10", "2", "0"] {
        assert_fg(&lines, num, Color::Green);
    }

    // Interpolated f-string: literal run before the embedded expression is
    // red; from the first embedded-expression token onward — including the
    // closing quote — it reverts to the default colour.
    assert_fg(&lines, "f\"val ", Color::Red);
    assert_fg(&lines, "{x}\"", Color::Reset);

    for ident in ["os", "C", "s", "i", "args"] {
        assert_word_fg(&lines, ident, Color::Reset);
    }
}

#[test]
fn transcript_code_block_bash_colours() {
    let src = "# shell comment\n\
export FOO=bar\n\
if [ -f \"$HOME/.zshrc\" ]; then\n\
  echo \"yes\" | grep -q y && ls -la\n\
fi";
    let lines = code_lines("bash", src);

    assert_fg(&lines, "# shell comment", Color::Green);

    for builtin in ["export", "echo", "ls"] {
        assert_fg(&lines, builtin, Color::Cyan);
    }
    for kw in ["if", "then", "fi"] {
        assert_fg(&lines, kw, Color::Blue);
    }

    assert_fg(&lines, "\"yes\"", Color::Red);
    // Opening quote of the second string: red up to (not including) the
    // interruption by `$HOME` — the rest of that string, including its
    // closing quote, reverts to the default colour (coalesced with the
    // trailing `];` into one Reset span, so check it by word).
    assert_fg(&lines, "\"", Color::Red);
    assert_word_fg(&lines, "zshrc", Color::Reset);

    // `grep` isn't a recognised builtin, unlike `ls`/`echo`/`export`.
    assert_word_fg(&lines, "grep", Color::Reset);
}

#[test]
fn transcript_code_block_json_colours() {
    let src = "{\"a\": 1, \"b\": true, \"c\": null, \"d\": [\"x\", 2.5]}";
    let lines = code_lines("json", src);

    for key in ["\"a\"", "\"b\"", "\"c\"", "\"d\""] {
        assert_fg(&lines, key, Color::Cyan);
    }
    for num in ["1", "2.5"] {
        assert_fg(&lines, num, Color::Green);
    }
    for lit in ["true", "null"] {
        assert_fg(&lines, lit, Color::Blue);
    }
    assert_fg(&lines, "\"x\"", Color::Red);
}
