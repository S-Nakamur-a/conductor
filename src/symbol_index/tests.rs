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
