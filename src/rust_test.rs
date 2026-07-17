//! Detection of runnable Rust tests within an open `*.rs` file.
//!
//! The Rust counterpart to [`crate::go_test`]. Where Go has a flat convention
//! (`func TestXxx`), Rust nests `#[test]` / `#[tokio::test]` functions inside
//! `#[cfg(test)] mod` blocks, and the test harness names each test by its full
//! module path (`<file-module>::<inner-mods>::<fn>`, e.g.
//! `ai_caller::tests::command::echoes_prompt_via_stdin`). To filter precisely we
//! need both the file's module path (derived from where the file sits in the
//! source tree) and the inline `mod` nesting inside it — so this scanner parses
//! the file with tree-sitter-rust rather than line-regex.
//!
//! It produces a map from 1-indexed line number to a [`TestRun`] describing the
//! `cargo test` command that runs that scope. Three button kinds are emitted:
//!
//! - **File**: on line 1 — every test in the file (`cargo test '<mod>::'`).
//! - **Func**: on each test `fn` line — that one test, exactly
//!   (`cargo test '<full path>' -- --exact`).
//! - **Module**: on each `mod` line that (transitively) contains a test — all
//!   tests under it (`cargo test '<mod path>::'`).
//!
//! Commands assume the Shell PTY's working directory is the crate root (the
//! worktree root), the same assumption `go_test` makes for `go test`.

use std::collections::HashMap;

use crate::test_run::{TestRun, TestRunKind, shell_single_quote};

/// How a file maps onto a `cargo test` target + module-path prefix.
enum FileTarget {
    /// Unit tests compiled into the default (bin/lib) target. `prefix` is the
    /// file's module path (empty at the crate root, i.e. `main.rs`/`lib.rs`).
    Unit { prefix: Vec<String> },
    /// A top-level `tests/<name>.rs` integration binary, selected with
    /// `--test <name>`; test paths inside it carry no file-module prefix.
    Integration { name: String },
}

/// Scan an open file's content for runnable Rust tests.
///
/// Returns an empty map when `relative_path` is not a supported `.rs` file
/// (outside `src/` and top-level `tests/`) or the file contains no tests.
pub fn scan_rust_test_runs(file_content: &[String], relative_path: &str) -> HashMap<usize, TestRun> {
    let mut runs = HashMap::new();

    let Some(target) = file_target(relative_path) else {
        return runs;
    };

    // Rebuild source text for the parser. `file_content` is the file's logical
    // lines (tabs already expanded to spaces), so joining with '\n' reproduces
    // an equivalent line/column structure — tree-sitter rows map 1:1 onto
    // `file_content` indices, and identifiers are unaffected by tab expansion.
    let source = file_content.join("\n");

    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .is_err()
    {
        return runs;
    }
    let Some(tree) = parser.parse(&source, None) else {
        return runs;
    };

    let ctx = Ctx { target };
    let mut mod_stack: Vec<String> = Vec::new();
    let found = scan_node(tree.root_node(), &source, &mut mod_stack, &ctx, &mut runs);

    // File-level button on line 1, but never clobber a real func/module button
    // that happens to sit on line 1 (`or_insert`).
    if found {
        runs.entry(1).or_insert_with(|| TestRun {
            kind: TestRunKind::File,
            label: file_label(relative_path),
            command: ctx.file_command(),
        });
    }

    runs
}

/// Per-file command-building context.
struct Ctx {
    target: FileTarget,
}

impl Ctx {
    /// `--test <name>` selector for integration binaries, or empty for the
    /// default target. Leading space included so it slots into `cargo test{...}`.
    fn test_flag(&self) -> String {
        match &self.target {
            FileTarget::Integration { name } => format!(" --test {}", shell_single_quote(name)),
            FileTarget::Unit { .. } => String::new(),
        }
    }

    /// The file's module-path segments that prefix every in-file test path
    /// (empty for the crate root and for integration binaries).
    fn file_prefix(&self) -> &[String] {
        match &self.target {
            FileTarget::Unit { prefix } => prefix,
            FileTarget::Integration { .. } => &[],
        }
    }

    /// Full harness path for a single test: `<file prefix>::<inner mods>::<fn>`.
    fn full_path(&self, mod_stack: &[String], name: &str) -> String {
        let mut segs: Vec<&str> = self.file_prefix().iter().map(String::as_str).collect();
        segs.extend(mod_stack.iter().map(String::as_str));
        segs.push(name);
        segs.join("::")
    }

    /// Module-scoping filter for `mod_stack` (which already includes the module
    /// itself): the joined path with a trailing `::` so the substring match is
    /// confined to that module's descendants.
    fn module_prefix(&self, mod_stack: &[String]) -> String {
        let mut segs: Vec<&str> = self.file_prefix().iter().map(String::as_str).collect();
        segs.extend(mod_stack.iter().map(String::as_str));
        if segs.is_empty() {
            String::new()
        } else {
            format!("{}::", segs.join("::"))
        }
    }

    /// `cargo test '<full path>' -- --exact` — run exactly this one test.
    fn func_command(&self, full_path: &str) -> String {
        format!(
            "cargo test{} {} -- --exact",
            self.test_flag(),
            shell_single_quote(full_path)
        )
    }

    /// `cargo test '<prefix>::'` — run every test under this module.
    fn module_command(&self, module_prefix: &str) -> String {
        format!(
            "cargo test{} {}",
            self.test_flag(),
            shell_single_quote(module_prefix)
        )
    }

    /// Run every test in the file: filtered by the file's module prefix, or
    /// `--test <name>` for an integration binary, or unfiltered at the crate
    /// root (`main.rs`/`lib.rs`).
    fn file_command(&self) -> String {
        let prefix = self.module_prefix(&[]);
        if prefix.is_empty() {
            format!("cargo test{}", self.test_flag())
        } else {
            format!(
                "cargo test{} {}",
                self.test_flag(),
                shell_single_quote(&prefix)
            )
        }
    }
}

/// Walk the direct children of `node`, emitting func/module buttons and
/// recursing into `mod` bodies. Returns whether any test was found in this
/// subtree (so a `mod` can decide whether to draw its Module button and the
/// caller whether to draw the File button).
fn scan_node(
    node: tree_sitter::Node,
    source: &str,
    mod_stack: &mut Vec<String>,
    ctx: &Ctx,
    runs: &mut HashMap<usize, TestRun>,
) -> bool {
    let mut found = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                if is_test_fn(child, source)
                    && let Some(name_node) = child.child_by_field_name("name")
                {
                    found = true;
                    let name = node_text(name_node, source).to_string();
                    let line = child.start_position().row + 1;
                    let command = ctx.func_command(&ctx.full_path(mod_stack, &name));
                    runs.entry(line).or_insert(TestRun {
                        kind: TestRunKind::Func,
                        label: name,
                        command,
                    });
                }
            }
            "mod_item" => {
                let Some(name_node) = child.child_by_field_name("name") else {
                    continue;
                };
                let name = node_text(name_node, source).to_string();
                mod_stack.push(name.clone());
                let sub_found = match child.child_by_field_name("body") {
                    Some(body) => scan_node(body, source, mod_stack, ctx, runs),
                    None => false, // `mod foo;` (external file) has no body here.
                };
                if sub_found {
                    found = true;
                    let line = child.start_position().row + 1;
                    let command = ctx.module_command(&ctx.module_prefix(mod_stack));
                    runs.entry(line).or_insert(TestRun {
                        kind: TestRunKind::Module,
                        label: name,
                        command,
                    });
                }
                mod_stack.pop();
            }
            _ => {}
        }
    }
    found
}

/// Whether a `function_item` carries a test attribute. Attributes in
/// tree-sitter-rust are preceding-sibling `attribute_item` nodes; a few grammar
/// versions also nest them as children, so check both.
fn is_test_fn(fn_node: tree_sitter::Node, source: &str) -> bool {
    // Preceding siblings: `#[…]` lines directly above the `fn`, possibly with
    // comments interleaved.
    let mut sib = fn_node.prev_sibling();
    while let Some(s) = sib {
        match s.kind() {
            "attribute_item" => {
                if attr_is_test(s, source) {
                    return true;
                }
            }
            "line_comment" | "block_comment" => {}
            _ => break,
        }
        sib = s.prev_sibling();
    }

    // Insurance: any attribute_item nested inside the function_item.
    let mut cursor = fn_node.walk();
    for child in fn_node.children(&mut cursor) {
        if child.kind() == "attribute_item" && attr_is_test(child, source) {
            return true;
        }
    }
    false
}

/// Whether an `attribute_item`'s path marks a test: its last `::` segment is
/// `test` (`#[test]`, `#[tokio::test]`, `#[async_std::test]`,
/// `#[actix_web::test]`, …), or it is one of a small allowlist of well-known
/// test macros that don't end in `test`. `#[cfg(test)]` is correctly excluded
/// (its path segment is `cfg`).
fn attr_is_test(attr_item: tree_sitter::Node, source: &str) -> bool {
    let text = node_text(attr_item, source).trim();
    // Strip the `#[ … ]` (outer) wrapper; ignore `#![ … ]` inner attributes.
    let inner = match text.strip_prefix("#[").and_then(|s| s.strip_suffix(']')) {
        Some(inner) => inner.trim(),
        None => return false,
    };
    // Drop any argument list: `cfg(test)` → `cfg`, `test` → `test`.
    let path = inner.split('(').next().unwrap_or("").trim();
    let last = path.rsplit("::").next().unwrap_or("").trim();
    last == "test" || matches!(path, "rstest")
}

/// Derive the `cargo test` target + module prefix from a file's repo-relative
/// path. `None` for unsupported locations (no run buttons).
fn file_target(relative_path: &str) -> Option<FileTarget> {
    let stem = relative_path.strip_suffix(".rs")?;

    if let Some(rest) = stem.strip_prefix("src/") {
        // Crate root: unit-test paths have no module prefix.
        if rest == "main" || rest == "lib" {
            return Some(FileTarget::Unit { prefix: Vec::new() });
        }
        let mut prefix: Vec<String> = rest.split('/').map(str::to_string).collect();
        // `foo/mod.rs` is module `foo`, not `foo::mod`.
        if prefix.last().map(String::as_str) == Some("mod") {
            prefix.pop();
        }
        return Some(FileTarget::Unit { prefix });
    }

    if let Some(rest) = stem.strip_prefix("tests/") {
        // Only a top-level `tests/<name>.rs` is its own binary; files nested
        // under it are shared submodules and aren't independently runnable.
        if rest.contains('/') {
            return None;
        }
        return Some(FileTarget::Integration {
            name: rest.to_string(),
        });
    }

    None
}

fn file_label(relative_path: &str) -> String {
    relative_path
        .rsplit_once('/')
        .map(|(_, f)| f)
        .unwrap_or(relative_path)
        .to_string()
}

fn node_text<'a>(node: tree_sitter::Node, source: &'a str) -> &'a str {
    &source[node.start_byte()..node.end_byte()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &str) -> Vec<String> {
        src.lines().map(str::to_string).collect()
    }

    #[test]
    fn non_rust_file_yields_nothing() {
        let src = lines("fn main() {}\n#[test]\nfn t() {}");
        assert!(scan_rust_test_runs(&src, "README.md").is_empty());
    }

    #[test]
    fn file_outside_src_or_tests_yields_nothing() {
        let src = lines("#[test]\nfn t() {}");
        assert!(scan_rust_test_runs(&src, "benches/bench.rs").is_empty());
    }

    #[test]
    fn detects_file_module_and_func() {
        let src = lines(
            "pub fn foo() {}\n\
             \n\
             #[cfg(test)]\n\
             mod tests {\n\
             \x20   #[test]\n\
             \x20   fn it_works() {}\n\
             }\n",
        );
        let runs = scan_rust_test_runs(&src, "src/ai_caller.rs");

        // File button on line 1 runs everything in the file.
        let file = &runs[&1];
        assert_eq!(file.kind, TestRunKind::File);
        assert_eq!(file.label, "ai_caller.rs");
        assert_eq!(file.command, "cargo test 'ai_caller::'");

        // Module button on the `mod tests` line (line 4).
        let module = &runs[&4];
        assert_eq!(module.kind, TestRunKind::Module);
        assert_eq!(module.label, "tests");
        assert_eq!(module.command, "cargo test 'ai_caller::tests::'");

        // Func button on the `fn it_works` line (line 6).
        let func = &runs[&6];
        assert_eq!(func.kind, TestRunKind::Func);
        assert_eq!(func.label, "it_works");
        assert_eq!(
            func.command,
            "cargo test 'ai_caller::tests::it_works' -- --exact"
        );
    }

    #[test]
    fn tokio_test_async_fn_is_detected() {
        let src = lines(
            "#[cfg(test)]\n\
             mod tests {\n\
             \x20   #[tokio::test]\n\
             \x20   async fn talks() {}\n\
             }\n",
        );
        let runs = scan_rust_test_runs(&src, "src/net.rs");
        let func = &runs[&4];
        assert_eq!(func.kind, TestRunKind::Func);
        assert_eq!(func.command, "cargo test 'net::tests::talks' -- --exact");
    }

    #[test]
    fn nested_modules_build_full_paths() {
        let src = lines(
            "#[cfg(test)]\n\
             mod tests {\n\
             \x20   mod command {\n\
             \x20       #[test]\n\
             \x20       fn echoes() {}\n\
             \x20   }\n\
             }\n",
        );
        let runs = scan_rust_test_runs(&src, "src/ai_caller.rs");

        // Inner module button (line 3) scoped to the nested module.
        assert_eq!(
            runs[&3].command,
            "cargo test 'ai_caller::tests::command::'"
        );
        // Outer module button (line 2).
        assert_eq!(runs[&2].command, "cargo test 'ai_caller::tests::'");
        // Func carries the full nested path (line 5).
        assert_eq!(
            runs[&5].command,
            "cargo test 'ai_caller::tests::command::echoes' -- --exact"
        );
    }

    #[test]
    fn cfg_test_module_without_tests_yields_nothing() {
        // A `#[cfg(test)]` module with only helpers is not a test scope.
        let src = lines(
            "#[cfg(test)]\n\
             mod tests {\n\
             \x20   fn helper() {}\n\
             }\n",
        );
        assert!(scan_rust_test_runs(&src, "src/foo.rs").is_empty());
    }

    #[test]
    fn mod_rs_maps_to_directory_module() {
        let src = lines("#[test]\nfn t() {}\n");
        let runs = scan_rust_test_runs(&src, "src/app/mod.rs");
        // `app/mod.rs` is module `app`; a top-level `#[test]` sits directly
        // under it.
        assert_eq!(runs[&2].command, "cargo test 'app::t' -- --exact");
        assert_eq!(runs[&1].command, "cargo test 'app::'");
    }

    #[test]
    fn crate_root_runs_all_for_file_button() {
        let src = lines("#[test]\nfn t() {}\n");
        let runs = scan_rust_test_runs(&src, "src/main.rs");
        // No module prefix at the crate root — file button runs everything.
        assert_eq!(runs[&1].command, "cargo test");
        assert_eq!(runs[&2].command, "cargo test 't' -- --exact");
    }

    #[test]
    fn integration_file_uses_test_flag() {
        let src = lines("#[test]\nfn smoke() {}\n");
        let runs = scan_rust_test_runs(&src, "tests/e2e.rs");
        assert_eq!(runs[&1].command, "cargo test --test 'e2e'");
        assert_eq!(
            runs[&2].command,
            "cargo test --test 'e2e' 'smoke' -- --exact"
        );
    }

    #[test]
    fn hostile_file_name_is_shell_quoted() {
        // A path with a single quote (as an untrusted repo could contain) must
        // be neutralized by the `'\''` escaping in the module prefix.
        let src = lines("#[test]\nfn t() {}\n");
        let runs = scan_rust_test_runs(&src, "src/o'clock.rs");
        assert_eq!(runs[&1].command, "cargo test 'o'\\''clock::'");
    }
}
