//! Symbol hover info — signature, doc comment, and reference count for the
//! symbol under the viewer cursor.
//!
//! Built on the existing tree-sitter [`SymbolIndex`](crate::symbol_index::SymbolIndex)
//! (no language server): the index locates the definition, then a bounded read
//! of the definition file extracts the declaration signature and the doc
//! comment block directly above it. Lookups that find nothing return `None`
//! so the caller can stay silent.

use crate::symbol_index::SymbolIndex;

/// Maximum number of signature lines collected before truncating with `…`.
const MAX_SIGNATURE_LINES: usize = 8;
/// Maximum number of doc-comment lines collected before truncating with `…`.
const MAX_DOC_LINES: usize = 12;

/// Hover information for a symbol, ready for rendering.
pub struct HoverInfo {
    /// The symbol name (popup title).
    pub symbol_name: String,
    /// Definition kind (e.g. "Function", "Struct"), from the symbol index.
    pub kind: String,
    /// Definition file path, relative to the repo root.
    pub file_path: String,
    /// 1-indexed definition line.
    pub line: usize,
    /// Doc-comment lines above the definition, comment markers stripped.
    pub doc_lines: Vec<String>,
    /// Declaration signature lines (dedented, body-opening brace stripped).
    pub signature_lines: Vec<String>,
    /// Number of definitions matching the name (>1 means the shown one is one of several).
    pub def_count: usize,
    /// Number of textual references across the repo.
    pub ref_count: usize,
}

/// Build hover info for `symbol`. Prefers a definition in `current_file` when
/// the name is defined in several places. Returns `None` (quietly) when the
/// index is not ready, the symbol has no definition, or the file can't be read.
pub fn build_hover_info(
    index: &SymbolIndex,
    symbol: &str,
    current_file: Option<&str>,
) -> Option<HoverInfo> {
    if !index.is_available() {
        return None;
    }
    let defs = index.find_definitions(symbol);
    let def = defs
        .iter()
        .find(|d| Some(d.file_path.as_str()) == current_file)
        .or_else(|| defs.first())?;

    let root = index.root();
    let source = std::fs::read_to_string(root.join(&def.file_path)).ok()?;
    let lines: Vec<&str> = source.lines().collect();
    let def_idx = def.line.checked_sub(1)?;
    if def_idx >= lines.len() {
        return None;
    }

    let ref_count = index.find_references(symbol, &root).len();

    Some(HoverInfo {
        symbol_name: symbol.to_string(),
        kind: format!("{:?}", def.kind),
        file_path: def.file_path.clone(),
        line: def.line,
        doc_lines: extract_doc_comment(&lines, def_idx),
        signature_lines: extract_signature(&lines, def_idx),
        def_count: defs.len(),
        ref_count,
    })
}

/// Extract the declaration signature starting at `def_idx` (0-indexed):
/// lines up to and including the first one that ends with `{` (brace stripped)
/// or `;`/`=`-terminated declarations, capped at [`MAX_SIGNATURE_LINES`].
/// Lines are dedented by the first line's indentation.
fn extract_signature(lines: &[&str], def_idx: usize) -> Vec<String> {
    let indent = lines[def_idx].len() - lines[def_idx].trim_start().len();
    let mut out = Vec::new();
    for raw in lines.iter().skip(def_idx).take(MAX_SIGNATURE_LINES) {
        let dedented = if raw.len() >= indent && raw[..indent.min(raw.len())].trim().is_empty() {
            &raw[indent..]
        } else {
            raw.trim_start()
        };
        let trimmed_end = dedented.trim_end();
        if let Some(stripped) = trimmed_end.strip_suffix('{') {
            let s = stripped.trim_end();
            if !s.is_empty() {
                out.push(s.to_string());
            }
            return out;
        }
        out.push(trimmed_end.to_string());
        if trimmed_end.ends_with(';') {
            return out;
        }
    }
    if lines.len() > def_idx + MAX_SIGNATURE_LINES {
        out.push("…".to_string());
    }
    out
}

/// Collect the comment block directly above `def_idx` (0-indexed), skipping
/// attribute/decorator lines (`#[...]`, `@...`). Handles `///`, `//!`, `//`
/// (Rust/Go) and `/** ... */` (TS/JS) styles; markers are stripped. Capped at
/// [`MAX_DOC_LINES`] (keeping the opening lines, which carry the summary).
fn extract_doc_comment(lines: &[&str], def_idx: usize) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut i = def_idx;
    let mut in_block = false; // inside a `/* ... */` scanned bottom-up
    while i > 0 {
        i -= 1;
        let t = lines[i].trim();
        if in_block {
            let body = t
                .trim_start_matches("/**")
                .trim_start_matches("/*")
                .trim_start_matches('*')
                .trim();
            collected.push(body.to_string());
            if t.starts_with("/*") {
                break;
            }
            continue;
        }
        // Skip attributes/decorators between the doc block and the item.
        if collected.is_empty() && (t.starts_with("#[") || t.starts_with('@') || t == "]") {
            continue;
        }
        if t.ends_with("*/") && !t.starts_with("//") {
            let body = t.trim_end_matches("*/").trim_end();
            // A `/*` opener on the same line means the block was one line.
            in_block = !body.starts_with("/*");
            let body = body
                .trim_start_matches("/**")
                .trim_start_matches("/*")
                .trim_start_matches('*')
                .trim();
            collected.push(body.to_string());
            if !in_block {
                break;
            }
        } else if let Some(rest) = t
            .strip_prefix("///")
            .or_else(|| t.strip_prefix("//!"))
            .or_else(|| t.strip_prefix("//"))
        {
            collected.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else {
            break;
        }
    }
    collected.reverse();
    // Drop leading/trailing empty lines left by block-comment delimiters.
    while collected.first().is_some_and(|l| l.is_empty()) {
        collected.remove(0);
    }
    while collected.last().is_some_and(|l| l.is_empty()) {
        collected.pop();
    }
    if collected.len() > MAX_DOC_LINES {
        collected.truncate(MAX_DOC_LINES);
        collected.push("…".to_string());
    }
    collected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(src: &str, def_line_1: usize) -> Vec<String> {
        let lines: Vec<&str> = src.lines().collect();
        extract_signature(&lines, def_line_1 - 1)
    }

    fn doc(src: &str, def_line_1: usize) -> Vec<String> {
        let lines: Vec<&str> = src.lines().collect();
        extract_doc_comment(&lines, def_line_1 - 1)
    }

    #[test]
    fn signature_single_line_fn() {
        let src = "pub fn foo(a: usize) -> bool {\n    true\n}\n";
        assert_eq!(sig(src, 1), vec!["pub fn foo(a: usize) -> bool"]);
    }

    #[test]
    fn signature_multi_line_fn() {
        let src = "fn foo(\n    a: usize,\n    b: &str,\n) -> bool {\n    true\n}\n";
        assert_eq!(sig(src, 1), vec!["fn foo(", "    a: usize,", "    b: &str,", ") -> bool"]);
    }

    #[test]
    fn signature_dedents_indented_method() {
        let src = "impl Foo {\n    pub fn bar(&self) -> usize {\n        1\n    }\n}\n";
        assert_eq!(sig(src, 2), vec!["pub fn bar(&self) -> usize"]);
    }

    #[test]
    fn signature_stops_at_semicolon() {
        let src = "type Alias = Vec<String>;\nfn next() {}\n";
        assert_eq!(sig(src, 1), vec!["type Alias = Vec<String>;"]);
    }

    #[test]
    fn doc_rust_triple_slash_with_attribute() {
        let src = "/// Does the thing.\n/// Second line.\n#[derive(Debug)]\npub struct Foo;\n";
        assert_eq!(doc(src, 4), vec!["Does the thing.", "Second line."]);
    }

    #[test]
    fn doc_go_double_slash() {
        let src = "// Foo does the thing.\nfunc Foo() {}\n";
        assert_eq!(doc(src, 2), vec!["Foo does the thing."]);
    }

    #[test]
    fn doc_ts_block_comment() {
        let src = "/**\n * Does the thing.\n * @param a input\n */\nfunction foo(a) {}\n";
        assert_eq!(doc(src, 5), vec!["Does the thing.", "@param a input"]);
    }

    #[test]
    fn doc_none_when_code_above() {
        let src = "let x = 1;\nfn foo() {}\n";
        assert!(doc(src, 2).is_empty());
    }

    #[test]
    fn doc_single_line_block_comment() {
        let src = "/** Does the thing. */\nfunction foo() {}\n";
        assert_eq!(doc(src, 2), vec!["Does the thing."]);
    }

    #[test]
    fn end_to_end_over_real_index() {
        // Build a real tree-sitter index over a temp repo and resolve hover
        // info through the full path (find_definitions → read → extract).
        let dir = std::env::temp_dir().join(format!("hover_e2e_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = "\
/// Adds two numbers together.
/// Returns their sum.
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

fn caller() {
    let _ = add(1, 2);
}
";
        std::fs::write(dir.join("lib.rs"), src).unwrap();

        let index = SymbolIndex::new(dir.clone());
        index.build().unwrap();

        let info = build_hover_info(&index, "add", Some("lib.rs")).expect("hover info");
        assert_eq!(info.symbol_name, "add");
        assert_eq!(info.kind, "Function");
        assert_eq!(info.file_path, "lib.rs");
        assert_eq!(info.line, 3);
        assert_eq!(
            info.doc_lines,
            vec!["Adds two numbers together.", "Returns their sum."]
        );
        assert_eq!(info.signature_lines, vec!["pub fn add(a: i64, b: i64) -> i64"]);
        // "add" appears at the definition and one call site.
        assert!(info.ref_count >= 2, "ref_count = {}", info.ref_count);

        // A name with no definition returns nothing (silent).
        assert!(build_hover_info(&index, "nonexistent_symbol", None).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
