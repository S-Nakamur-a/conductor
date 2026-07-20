//! Line-oriented block parsing: splits raw Markdown text into [`MdBlock`]s
//! (headings, list items, block quotes, fenced code, GFM tables, rules, blank
//! lines, and plain paragraphs) ahead of rendering.

/// A block of the parsed summary. The parser is line-oriented, so most blocks
/// map to a single source line; only `CodeBlock` spans multiple lines.
#[derive(Debug, PartialEq)]
pub(crate) enum MdBlock {
    /// `# heading` .. `###### heading` (level 1–6).
    Heading { level: u8, text: String },
    /// A normal text line. Author line breaks are preserved (one block each).
    Paragraph(String),
    /// `- item` / `* item` / `+ item` or `1. item` / `1) item`.
    ListItem {
        /// `Some("1")` for an ordered item (keeps the author's number), `None`
        /// for a bullet.
        ordered: Option<String>,
        /// GFM task marker: `None` = plain item, `Some(false)` = `[ ]` (open),
        /// `Some(true)` = `[x]` (done).
        checked: Option<bool>,
        text: String,
        /// Leading-whitespace columns before the marker (nesting indent).
        indent: usize,
    },
    /// `> quoted text`.
    Quote(String),
    /// A fenced code block. `lang` is the info-string's first token (if any).
    CodeBlock {
        lang: Option<String>,
        lines: Vec<String>,
    },
    /// A GFM pipe table: a header row, an alignment row, and zero or more body
    /// rows. `aligns` carries one entry per header column.
    Table {
        headers: Vec<String>,
        aligns: Vec<Align>,
        rows: Vec<Vec<String>>,
    },
    /// `---` / `***` / `___` (3+ of the same marker).
    Rule,
    /// A blank source line (preserved as paragraph spacing).
    Blank,
}

/// Per-column text alignment for a [`MdBlock::Table`], from the delimiter row's
/// colons (`:--` left, `--:` right, `:-:` center).
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

/// Split `text` into blocks. Lines are split on `\n`; a trailing `\r` (CRLF
/// input) is stripped so fence detection and code bodies stay clean.
pub(crate) fn parse_blocks(text: &str) -> Vec<MdBlock> {
    let lines: Vec<&str> = text.split('\n').map(strip_cr).collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // Fenced code block — consumes lines until a matching close fence (or EOF).
        if let Some((fence_char, fence_len, info)) = fence_open(trimmed) {
            let lang = info
                .split_whitespace()
                .next()
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() {
                if is_fence_close(lines[i].trim_start(), fence_char, fence_len) {
                    i += 1;
                    break;
                }
                body.push(lines[i].to_string());
                i += 1;
            }
            blocks.push(MdBlock::CodeBlock { lang, lines: body });
            continue;
        }

        // GFM table — a `|`-bearing line immediately followed by a valid
        // delimiter row. The lookahead helper consumes the whole table (and
        // returns `None`, eating nothing, when it isn't really a table).
        if let Some((table, consumed)) = parse_table_at(&lines, i) {
            blocks.push(table);
            i += consumed;
            continue;
        }

        if trimmed.is_empty() {
            blocks.push(MdBlock::Blank);
        } else if is_hr(trimmed) {
            blocks.push(MdBlock::Rule);
        } else if let Some((level, htext)) = parse_heading(trimmed) {
            blocks.push(MdBlock::Heading {
                level,
                text: htext,
            });
        } else if let Some(rest) = trimmed.strip_prefix('>') {
            blocks.push(MdBlock::Quote(
                rest.strip_prefix(' ').unwrap_or(rest).to_string(),
            ));
        } else if let Some(item) = parse_list_item(line) {
            blocks.push(item);
        } else {
            blocks.push(MdBlock::Paragraph(trimmed.to_string()));
        }
        i += 1;
    }
    blocks
}

fn strip_cr(s: &str) -> &str {
    s.strip_suffix('\r').unwrap_or(s)
}

/// If `s` opens a code fence, return `(fence_char, fence_len, info_string)`.
/// A fence is 3+ backticks or 3+ tildes at the start of the (trimmed) line.
fn fence_open(s: &str) -> Option<(char, usize, &str)> {
    let first = s.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let len = s.chars().take_while(|&c| c == first).count();
    if len < 3 {
        return None;
    }
    // `len` equals the byte offset because both fence chars are ASCII.
    Some((first, len, s[len..].trim()))
}

/// A close fence is 3+ (>= open length) of the same char, then only whitespace.
fn is_fence_close(s: &str, fence_char: char, fence_len: usize) -> bool {
    let len = s.chars().take_while(|&c| c == fence_char).count();
    len >= fence_len && s.chars().skip(len).all(char::is_whitespace)
}

/// `---`, `***`, `___` (>= 3 of one marker, spaces allowed between).
fn is_hr(s: &str) -> bool {
    let marks: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    if marks.len() < 3 {
        return false;
    }
    let first = marks[0];
    matches!(first, '-' | '*' | '_') && marks.iter().all(|&c| c == first)
}

/// `# ` .. `###### ` → `(level, heading_text)`. A space after the hashes is
/// required (so `#nofilter`, `C#`, issue refs like `#242` stay paragraphs).
fn parse_heading(s: &str) -> Option<(u8, String)> {
    let hashes = s.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &s[hashes..];
    // Require a separating space, except for an otherwise-empty heading ("# ").
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((hashes as u8, rest.trim_start().to_string()))
}

/// `- `/`* `/`+ ` (bullet) or `N. `/`N) ` (ordered) → a `ListItem`. A leading
/// GFM task marker (`[ ] `/`[x] `) on the item text is split off into `checked`.
fn parse_list_item(line: &str) -> Option<MdBlock> {
    let indent = line.len() - line.trim_start().len();
    let s = line.trim_start();

    if let Some(rest) = s
        .strip_prefix("- ")
        .or_else(|| s.strip_prefix("* "))
        .or_else(|| s.strip_prefix("+ "))
    {
        let (checked, text) = split_task_marker(rest);
        return Some(MdBlock::ListItem {
            ordered: None,
            checked,
            text: text.to_string(),
            indent,
        });
    }

    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        let after = &s[digits.len()..];
        if let Some(rest) = after.strip_prefix(". ").or_else(|| after.strip_prefix(") ")) {
            let (checked, text) = split_task_marker(rest);
            return Some(MdBlock::ListItem {
                ordered: Some(digits),
                checked,
                text: text.to_string(),
                indent,
            });
        }
    }
    None
}

/// Split a leading GFM task marker off list-item text. `"[ ] foo"` →
/// `(Some(false), "foo")`; `"[x] foo"`/`"[X] foo"` → `(Some(true), "foo")`; an
/// empty task `"[ ]"` → `(Some(_), "")`. A marker must be followed by a space or
/// end-of-string, so `"[ ]x"` and `"[y]"` stay literal `(None, original)`.
fn split_task_marker(text: &str) -> (Option<bool>, &str) {
    for (pat, val) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
        if let Some(rest) = text.strip_prefix(pat) {
            if rest.is_empty() {
                return (Some(val), "");
            }
            if let Some(after) = rest.strip_prefix(' ') {
                return (Some(val), after);
            }
        }
    }
    (None, text)
}

/// If a GFM pipe table starts at `lines[i]` — a `|`-bearing line immediately
/// followed by a valid delimiter row — parse it and return the block plus the
/// number of source lines consumed. Returns `None` (consuming nothing) when it
/// isn't a real table, so a paragraph like `a | b` is never misread.
///
/// The delimiter row is the gate: if it isn't all valid `:?-+:?` cells the whole
/// candidate is rejected before any line is consumed.
fn parse_table_at(lines: &[&str], i: usize) -> Option<(MdBlock, usize)> {
    let header_line = lines.get(i)?;
    if !header_line.contains('|') {
        return None;
    }
    let delim_line = lines.get(i + 1)?;
    let aligns = parse_alignments(&split_table_row(delim_line))?;
    let headers = split_table_row(header_line);
    if headers.is_empty() {
        return None;
    }

    // Body rows: subsequent non-blank `|`-bearing lines.
    let mut rows = Vec::new();
    let mut j = i + 2;
    while let Some(l) = lines.get(j) {
        if l.trim().is_empty() || !l.contains('|') {
            break;
        }
        rows.push(split_table_row(l));
        j += 1;
    }

    Some((
        MdBlock::Table {
            headers,
            aligns,
            rows,
        },
        j - i,
    ))
}

/// Split one table row into trimmed cells, dropping the empty cells created by
/// the surrounding `|`. `"| a | b |"` and `"a | b"` both yield `["a", "b"]`.
/// (Escaped `\|` and pipes inside `code` are out of scope.)
pub(crate) fn split_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Parse a delimiter row's cells into alignments, or `None` if any cell isn't a
/// valid `:?-+:?` separator (≥1 dash). Doubles as the "is this a table?" gate.
fn parse_alignments(cells: &[String]) -> Option<Vec<Align>> {
    if cells.is_empty() {
        return None;
    }
    cells
        .iter()
        .map(|c| {
            let c = c.trim();
            let left = c.starts_with(':');
            let right = c.ends_with(':');
            let core = c.trim_start_matches(':').trim_end_matches(':');
            if core.is_empty() || !core.chars().all(|ch| ch == '-') {
                return None;
            }
            Some(match (left, right) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}
