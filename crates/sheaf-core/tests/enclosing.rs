//! 定義位置を索引に持たない符号を、囲んでいる型に落とす。
//!
//! derive が作った impl がこれに当たる。`GitStatusMap::default()` の `default` は
//! 索引に定義位置を持たないが、`GitStatusMap` は持っている。
//!
//! 実索引を要求するものだけ `#[ignore]`。
//!   SHEAF_TEST_INDEX=<.scip> SHEAF_TEST_ROOT=<ツリー> cargo test --test enclosing -- --ignored

use protobuf::{EnumOrUnknown, Message, MessageField};
use scip::types::{Document, Index, Metadata, Occurrence, TextEncoding};
use sheaf_core::{
    Definition, IndexSource, Location, Span, Store, SyntacticAnswer, SyntacticLayer, Token,
    definition_at,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const PKG: &str = "rust-analyzer cargo demo 0.1.0";
/// derive が作った impl。定義 occurrence を持たない。
const DERIVED: &str =
    "rust-analyzer cargo demo 0.1.0 app/focus/impl#[Focus][`PartialEq<Self>`]eq().";
/// その型。定義 occurrence を持つ。
const TYPE: &str = "rust-analyzer cargo demo 0.1.0 app/focus/Focus#";

const LIB: &str = "pub enum Focus {}\n";
const CALLER: &str = "fn a(x: Focus) { x.eq(); }\n";

struct Rough;

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

impl SyntacticLayer for Rough {
    fn token_at(&self, path: &Path, line: u32, col: u32) -> Token {
        let Ok(src) = std::fs::read(path) else {
            return Token::Unknown;
        };
        let Some(text) = src.split(|b| *b == b'\n').nth(line as usize) else {
            return Token::NotWord;
        };
        let col = col as usize;
        if col >= text.len() || !is_word_byte(text[col]) {
            return Token::NotWord;
        }
        let mut start = col;
        while start > 0 && is_word_byte(text[start - 1]) {
            start -= 1;
        }
        let mut end = col + 1;
        while end < text.len() && is_word_byte(text[end]) {
            end += 1;
        }
        Token::Word(Span {
            start_line: line,
            start_col: start as u32,
            end_line: line,
            end_col: end as u32,
        })
    }

    fn definition_at(&self, _path: &Path, _line: u32, _col: u32) -> SyntacticAnswer {
        SyntacticAnswer::NotCode
    }

    fn references_at(&self, _path: &Path, _line: u32, _col: u32) -> SyntacticAnswer {
        SyntacticAnswer::NotCode
    }
}

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sheaf-test-enclosing-{}-{}-{:?}",
        tag,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

fn occurrence(range: Vec<i32>, symbol: &str, roles: i32) -> Occurrence {
    Occurrence {
        range,
        symbol: symbol.to_string(),
        symbol_roles: roles,
        ..Default::default()
    }
}

/// `src/caller.rs` の `eq` に derive 由来の符号を、`src/lib.rs` の `Focus` に
/// その型の定義を置く索引を書く。`type_defined` が false なら型の定義も置かない。
fn build(tag: &str, type_defined: bool, extra: Vec<Occurrence>) -> (PathBuf, PathBuf) {
    let root = workdir(tag);
    std::fs::write(root.join("src/lib.rs"), LIB).unwrap();
    std::fs::write(root.join("src/caller.rs"), CALLER).unwrap();

    let mut lib_occ = Vec::new();
    if type_defined {
        // "pub enum Focus {}" の Focus。
        lib_occ.push(occurrence(vec![0, 9, 14], TYPE, 1));
    }
    let mut caller_occ = vec![
        // "fn a(x: Focus) { x.eq(); }" の eq。
        occurrence(vec![0, 19, 21], DERIVED, 0),
    ];
    caller_occ.extend(extra);

    let index = Index {
        metadata: MessageField::some(Metadata {
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![
            Document {
                relative_path: "src/lib.rs".to_string(),
                occurrences: lib_occ,
                ..Default::default()
            },
            Document {
                relative_path: "src/caller.rs".to_string(),
                occurrences: caller_occ,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let path = root.join("index.scip");
    std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();
    (path, root)
}

fn load(index: PathBuf, root: &Path, expected: &[(&str, &str)]) -> Store {
    let expected: HashMap<PathBuf, String> = expected
        .iter()
        .map(|(rel, body)| (PathBuf::from(rel), sheaf_core::blob_hash(body.as_bytes())))
        .collect();
    Store::load(
        &[IndexSource {
            index,
            subroot: PathBuf::new(),
            expected,
        }],
        root,
    )
    .unwrap()
}

const FRESH: [(&str, &str); 2] = [("src/lib.rs", LIB), ("src/caller.rs", CALLER)];

#[test]
fn 定義位置を持たない符号は囲んでいる型に落ちる() {
    let (index, root) = build("hit", true, Vec::new());
    let store = load(index, &root, &FRESH);

    let answer = definition_at(&store, &Rough, Path::new("src/caller.rs"), 0, 19);

    let Definition::Enclosing(found) = answer else {
        panic!("Enclosing が返らなかった: {answer:?}");
    };
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].definition,
        Location {
            path: PathBuf::from("src/lib.rs"),
            line: 0,
            col: 9,
        }
    );
    assert_eq!(found[0].ty.as_str(), TYPE);
}

#[test]
fn 組み立てた型が索引に無ければ構文層に回る() {
    // 座標を作る規則なので、作った先が無いことは普通に起きる（impl が型とは別の
    // モジュールにあるとき）。そこで無理に何かを返さない。
    let (index, root) = build("miss", false, Vec::new());
    let store = load(index, &root, &FRESH);

    let answer = definition_at(&store, &Rough, Path::new("src/caller.rs"), 0, 19);

    assert_eq!(answer, Definition::NotCode, "{answer:?}");
}

#[test]
fn 語そのものの定義があれば型には落とさない() {
    // 直接の定義とあとから足した弱い答えを混ぜない。混ぜると、強い主張しか
    // 無かったはずの答えに弱いものが紛れる。
    let direct = format!("{PKG} app/focus/impl#[Focus][`PartialEq<Self>`]ne().");
    let (index, root) = build(
        "direct",
        true,
        vec![
            // 同じ範囲にもう 1 つ、定義を持つ符号を乗せる。
            occurrence(vec![0, 19, 21], &direct, 0),
            occurrence(vec![0, 19, 21], &direct, 1),
        ],
    );
    let store = load(index, &root, &FRESH);

    let answer = definition_at(&store, &Rough, Path::new("src/caller.rs"), 0, 19);

    let Definition::Exact(found) = answer else {
        panic!("Exact が返らなかった: {answer:?}");
    };
    assert_eq!(found.len(), 1);
}

#[test]
fn 型が定義されているファイルが変わったら落とさない() {
    let (index, root) = build("stale", true, Vec::new());
    let store = load(
        index,
        &root,
        &[("src/lib.rs", "中身が違う\n"), ("src/caller.rs", CALLER)],
    );

    let answer = definition_at(&store, &Rough, Path::new("src/caller.rs"), 0, 19);

    assert_eq!(answer, Definition::NotCode, "{answer:?}");
}

// ここから先は実索引が要る。

#[test]
#[ignore = "実索引が要る"]
fn 実索引_derive_の_default_は型の定義に落ちる() {
    let index = PathBuf::from(
        std::env::var("SHEAF_TEST_INDEX").expect("SHEAF_TEST_INDEX に .scip のパスを渡すこと"),
    );
    let root = PathBuf::from(
        std::env::var("SHEAF_TEST_ROOT").expect("SHEAF_TEST_ROOT にソースツリーのルートを渡すこと"),
    );
    let expected = sheaf_core::read_provenance(
        &index.with_file_name("index.hashes"),
        &sheaf_core::RustAnalyzer,
    )
    .expect("SHEAF_TEST_INDEX には rust-analyzer が作った索引を渡すこと");
    let store = Store::load(
        &[IndexSource {
            index,
            subroot: PathBuf::new(),
            expected,
        }],
        &root,
    )
    .unwrap();

    // `GitStatusMap::default()` の default。行・列は 0 始まり。
    let rel = Path::new("src/viewer/tree.rs");
    let src = std::fs::read_to_string(root.join(rel)).unwrap();
    let line = src
        .lines()
        .nth(99)
        .expect("対象の行が無い。索引を作り直すこと");
    assert!(
        line.contains("GitStatusMap::default()"),
        "対象の行が変わっている: {line}"
    );
    let col = line.rfind("default").unwrap() as u32;

    let answer = definition_at(&store, &Rough, rel, 99, col);

    let Definition::Enclosing(found) = answer else {
        panic!("Enclosing が返らなかった: {answer:?}");
    };
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].definition,
        Location {
            path: PathBuf::from("src/git_engine/status_map.rs"),
            line: 25,
            col: 11,
        }
    );
    assert!(
        found[0]
            .ty
            .as_str()
            .ends_with("git_engine/status_map/GitStatusMap#"),
        "{}",
        found[0].ty.as_str()
    );
}
