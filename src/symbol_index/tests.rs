//! Tests for symbol index construction and querying.

use std::path::PathBuf;

use super::extract_rust::extract_rust_symbols;
use super::index::SymbolIndex;
use super::model::{Symbol, SymbolKind};

#[test]
fn test_symbol_index_new() {
    let idx = SymbolIndex::new(PathBuf::from("/tmp"));
    assert!(!idx.is_available());
    assert_eq!(idx.root(), PathBuf::from("/tmp"));
}

#[test]
fn test_find_definitions_empty() {
    let idx = SymbolIndex::new(PathBuf::from("/tmp"));
    let results = idx.find_definitions("foo");
    assert!(results.is_empty());
}

#[test]
fn test_extract_symbols_from_rust_source() {
    let source = r#"
pub fn hello_world() {
    println!("hello");
}

struct MyStruct {
    field_a: u32,
}

enum Color {
    Red,
    Blue,
}

trait Drawable {
    fn draw(&self);
}

impl Drawable for MyStruct {
    fn draw(&self) {}
}

type Alias = Vec<u32>;

const MAX_SIZE: usize = 100;

static GLOBAL: &str = "test";

mod submodule;

macro_rules! my_macro {
    () => {};
}
"#;

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();

    let mut symbols = Vec::new();
    extract_rust_symbols(tree.root_node(), source, "test.rs", &mut symbols);

    let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"hello_world"));
    assert!(names.contains(&"MyStruct"));
    assert!(names.contains(&"Color"));
    assert!(names.contains(&"Drawable"));
    assert!(names.contains(&"Alias"));
    assert!(names.contains(&"MAX_SIZE"));
    assert!(names.contains(&"GLOBAL"));
    assert!(names.contains(&"submodule"));
    assert!(names.contains(&"my_macro"));

    // Check enum variants.
    assert!(names.contains(&"Red"));
    assert!(names.contains(&"Blue"));

    // Check field.
    assert!(names.contains(&"field_a"));

    // Check impl — should have scope "MyStruct".
    let impl_sym = symbols.iter().find(|s| s.kind == SymbolKind::Impl).unwrap();
    assert_eq!(impl_sym.scope.as_deref(), Some("MyStruct"));

    // Check function inside impl.
    let draw_fns: Vec<_> = symbols.iter().filter(|s| s.name == "draw").collect();
    assert!(!draw_fns.is_empty());

    // Verify line numbers are 1-indexed and reasonable.
    let hello = symbols.iter().find(|s| s.name == "hello_world").unwrap();
    assert!(hello.line >= 1);
    assert_eq!(hello.kind, SymbolKind::Function);
}

#[test]
fn test_find_definitions_filters_fields() {
    let idx = SymbolIndex::new(PathBuf::from("/tmp"));
    {
        let mut data = idx.data.lock().unwrap();
        data.symbols = vec![
            Symbol {
                name: "Foo".to_string(),
                kind: SymbolKind::Struct,
                file_path: "lib.rs".to_string(),
                line: 1,
                column: 0,
                scope: None,
            },
            Symbol {
                name: "Foo".to_string(),
                kind: SymbolKind::Field,
                file_path: "lib.rs".to_string(),
                line: 5,
                column: 0,
                scope: None,
            },
        ];
        data.available = true;
    }
    let defs = idx.find_definitions("Foo");
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].kind, SymbolKind::Struct);
}

// ── S3: find_references excludes non-code extensions and non-code hits ──

/// Write `name` under `dir` with `contents`, creating parent directories.
fn write_fixture(dir: &std::path::Path, name: &str, contents: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

#[test]
fn find_references_skips_non_code_extensions() {
    let dir = std::env::temp_dir().join(format!("refs_ext_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    write_fixture(&dir, "notes.md", "widget appears here\n");
    write_fixture(&dir, "Cargo.toml", "widget = \"1.0\"\n");
    write_fixture(&dir, "config.yaml", "widget: true\n");
    write_fixture(&dir, "config.yml", "widget: true\n");
    write_fixture(&dir, "data.json", "{\"widget\": 1}\n");
    write_fixture(&dir, "lib.rs", "fn widget() {}\n");

    let idx = SymbolIndex::new(dir.clone());
    let refs = idx.find_references("widget", &dir);

    assert_eq!(refs.len(), 1, "only the .rs hit should survive: {refs:?}");
    assert_eq!(refs[0].file_path, "lib.rs");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn find_references_skips_comment_and_string_hits() {
    let dir = std::env::temp_dir().join(format!("refs_mask_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    write_fixture(
        &dir,
        "lib.rs",
        "// widget does things\nfn real() {\n    let s = \"widget\";\n    widget();\n}\n",
    );

    let idx = SymbolIndex::new(dir.clone());
    let refs = idx.find_references("widget", &dir);

    // Only line 4's call is a real, code-position reference — the comment on
    // line 1 and the string literal on line 3 must not come back.
    assert_eq!(refs.len(), 1, "expected exactly one code-position hit: {refs:?}");
    assert_eq!(refs[0].line, 4);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Frame-budget gate for the hover path.
///
/// Deliberately uses `new` — the worst case in this repository, mentioned in
/// close to 200 files. An earlier version of this test measured
/// `find_references` itself, which occurs in six files and so never exercised
/// the cost that scales with hit count: it passed while hovering a common name
/// took ~157ms and dropped ten frames. The cap is what bounds the work, so the
/// capped call is what has to be measured, and with the name that hurts most.
#[test]
fn hover_reference_count_stays_within_a_frame() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let idx = SymbolIndex::new(root.clone());

    // Untimed run first, so the measurement isn't dominated by cold-page-cache
    // disk reads that have nothing to do with this code.
    idx.count_references_upto("new", &root, 50);

    let start = std::time::Instant::now();
    let (count, capped) = idx.count_references_upto("new", &root, 50);
    let elapsed = start.elapsed();

    assert!(count > 0, "sanity: `new` should be found at all");
    assert!(capped, "sanity: `new` should exceed a cap of 50 in this repo");
    assert!(
        elapsed < std::time::Duration::from_millis(30),
        "hover reference count took {elapsed:?}; uncapped this measured ~157ms \
         for `new`, which is ten dropped frames at 16ms"
    );
}

/// The uncapped search is user-initiated (`gr`, the references overlay), so it
/// may take longer than a frame — but it must still not degenerate into
/// parsing the whole tree. A distinctive name touches few files and should
/// stay close to the pre-mask baseline of 8-10ms.
#[test]
fn find_references_defers_parsing_to_files_that_match() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let idx = SymbolIndex::new(root.clone());
    idx.find_references("count_references_upto", &root);

    let start = std::time::Instant::now();
    let refs = idx.find_references("count_references_upto", &root);
    let elapsed = start.elapsed();

    assert!(!refs.is_empty(), "sanity: the symbol should be found at all");
    assert!(
        elapsed < std::time::Duration::from_millis(80),
        "took {elapsed:?}; parsing every visited file instead of only the \
         matching ones measures ~121ms here"
    );
}

#[test]
fn test_find_implementations() {
    let idx = SymbolIndex::new(PathBuf::from("/tmp"));
    {
        let mut data = idx.data.lock().unwrap();
        data.symbols = vec![Symbol {
            name: "impl MyStruct".to_string(),
            kind: SymbolKind::Impl,
            file_path: "lib.rs".to_string(),
            line: 10,
            column: 0,
            scope: Some("MyStruct".to_string()),
        }];
        data.available = true;
    }
    let impls = idx.find_implementations("MyStruct");
    assert_eq!(impls.len(), 1);
}

// ── Re-rooting across worktrees ───────────────────────────────────────

/// Write `files` (relative path -> contents) under a fresh temp directory.
fn scratch_tree(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "symidx_{tag}_{}_{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, body) in files {
        std::fs::write(dir.join(name), body).unwrap();
    }
    dir
}

/// After re-rooting, the index must answer for the new tree only. Answering
/// for the old one is the failure that shows up as a jump to a plausible file
/// at a line number from another branch.
#[test]
fn rerooting_replaces_what_the_index_answers_for() {
    let a = scratch_tree("root_a", &[("a.rs", "pub fn only_in_a() {}\n")]);
    let b = scratch_tree("root_b", &[("b.rs", "pub fn only_in_b() {}\n")]);

    let idx = SymbolIndex::new(a.clone());
    idx.build().unwrap();
    assert!(!idx.find_definitions("only_in_a").is_empty());
    assert!(idx.find_definitions("only_in_b").is_empty());

    idx.set_root(b.clone());
    // Before the rebuild lands the index must admit it knows nothing rather
    // than keep answering from the tree we just left.
    assert!(
        !idx.is_available(),
        "re-rooting must invalidate until the rebuild lands"
    );
    assert!(idx.find_definitions("only_in_a").is_empty());

    idx.build().unwrap();
    assert!(!idx.find_definitions("only_in_b").is_empty());
    assert!(
        idx.find_definitions("only_in_a").is_empty(),
        "symbols from the previous root must not survive"
    );

    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

/// Re-rooting to the same path must not blow the index away — the
/// filesystem-change path rebuilds in place and would otherwise flap to
/// "not ready" on every save.
#[test]
fn rerooting_to_the_same_path_is_a_no_op() {
    let a = scratch_tree("root_same", &[("a.rs", "pub fn keep() {}\n")]);
    let idx = SymbolIndex::new(a.clone());
    idx.build().unwrap();

    idx.set_root(a.clone());
    assert!(idx.is_available(), "same root must not invalidate");
    assert!(!idx.find_definitions("keep").is_empty());

    let _ = std::fs::remove_dir_all(&a);
}

/// A build that started before a re-root must not publish its result.
///
/// `BackgroundOp` cannot cancel the worker it spawned — it drops the join
/// handle, and the worker writes into the shared index whether or not anyone
/// is still listening — so the only defence is refusing the stale result when
/// it arrives.
///
/// Played out as three ordered calls against the publish guard rather than by
/// racing a slow build against a re-root: the interleaving under test is
/// exactly "stamp, then move the root, then finish", and a threaded version
/// would only reach it when the scheduler happened to agree.
#[test]
fn a_build_that_started_before_a_reroot_is_discarded() {
    let old = scratch_tree("stale_old", &[("old.rs", "pub fn from_old_tree() {}\n")]);
    let new = scratch_tree("stale_new", &[("new.rs", "pub fn from_new_tree() {}\n")]);

    let idx = SymbolIndex::new(old.clone());

    // A build starts over `old` and stamps itself.
    let stamped = idx.generation();
    // The user switches worktrees while it is still walking.
    idx.set_root(new.clone());
    // It finishes and offers what it found.
    let stale = vec![Symbol {
        name: "from_old_tree".to_string(),
        kind: SymbolKind::Function,
        file_path: "old.rs".to_string(),
        line: 1,
        column: 0,
        scope: None,
    }];
    let published = idx.publish(stale, stamped);

    assert_eq!(
        published, 0,
        "a build stamped with the previous generation must publish nothing"
    );
    assert!(
        idx.find_definitions("from_old_tree").is_empty(),
        "stale symbols leaked into the re-rooted index"
    );
    assert!(
        !idx.is_available(),
        "a discarded build must not mark the index ready"
    );

    // A build that starts *after* the re-root publishes normally.
    idx.build().unwrap();
    assert!(!idx.find_definitions("from_new_tree").is_empty());

    let _ = std::fs::remove_dir_all(&old);
    let _ = std::fs::remove_dir_all(&new);
}

/// A language we have no grammar for must not come back empty from a reference
/// search. Dropping every hit would answer "there are no references" when the
/// truth is "we could not tell" — the same silent-wrong-answer this work
/// exists to remove, just pointed at a different surface.
#[test]
fn find_references_keeps_hits_in_unparseable_languages() {
    let dir = scratch_tree(
        "refs_unsupported",
        &[
            ("used.py", "def wrapper():\n    return target_name()\n"),
            ("used.rs", "pub fn caller() { target_name(); }\n"),
        ],
    );

    let idx = SymbolIndex::new(dir.clone());
    let refs = idx.find_references("target_name", &dir);

    let files: Vec<&str> = refs.iter().map(|r| r.file_path.as_str()).collect();
    assert!(
        files.contains(&"used.py"),
        "Python hits must survive rather than be silently dropped; got {files:?}"
    );
    assert!(
        files.contains(&"used.rs"),
        "Rust hits must still resolve; got {files:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
