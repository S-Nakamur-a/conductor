//! 参照検索の公開入口の検査。
//!
//! 定義ジャンプと同じ形で、確信度を判定しないと位置を取り出せないことを見る。

use protobuf::{EnumOrUnknown, Message, MessageField};
use scip::types::{Document, Index, Metadata, Occurrence, TextEncoding};
use sheaf_core::{
    IndexSource, Location, References, Span, Store, SyntacticAnswer, SyntacticLayer, Token,
    references_at,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SYMBOL: &str = "scip-test cargo demo 0.1.0 greet().";
const LIB: &str = "pub fn greet() {}\n";
const CALLER: &str = "fn a() { greet(); }\nfn b() { greet(); }\n";

/// 語の切り出しを素朴にした構文層。フォールバックの答えは呼び出し側が決める。
struct Rough(SyntacticAnswer);

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
        self.0.clone()
    }
}

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sheaf-test-refs-{}-{}-{:?}",
        tag,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    dir
}

fn occurrence(range: Vec<i32>, roles: i32) -> Occurrence {
    Occurrence {
        range,
        symbol: SYMBOL.to_string(),
        symbol_roles: roles,
        ..Default::default()
    }
}

/// 定義が `src/lib.rs`、参照 2 件が `src/caller.rs` にある索引を書いてルートを返す。
fn build(tag: &str) -> (PathBuf, PathBuf) {
    let root = workdir(tag);
    std::fs::write(root.join("src/lib.rs"), LIB).unwrap();
    std::fs::write(root.join("src/caller.rs"), CALLER).unwrap();

    let index = Index {
        metadata: MessageField::some(Metadata {
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![
            Document {
                relative_path: "src/lib.rs".to_string(),
                occurrences: vec![occurrence(vec![0, 7, 12], 1)],
                ..Default::default()
            },
            Document {
                relative_path: "src/caller.rs".to_string(),
                occurrences: vec![occurrence(vec![0, 9, 14], 0), occurrence(vec![1, 9, 14], 0)],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
    (index_path, root)
}

fn load(index_path: &Path, root: &Path, expected: Vec<(&str, &str)>) -> Store {
    let expected: HashMap<PathBuf, String> = expected
        .into_iter()
        .map(|(rel, body)| (PathBuf::from(rel), sheaf_core::blob_hash(body.as_bytes())))
        .collect();
    Store::load(
        &[IndexSource {
            index: index_path.to_path_buf(),
            subroot: PathBuf::new(),
            expected,
        }],
        root,
    )
    .unwrap()
}

#[test]
fn 索引が答えられる位置は参照をexactで返す() {
    let (index_path, root) = build("exact");
    let store = load(
        &index_path,
        &root,
        vec![("src/lib.rs", LIB), ("src/caller.rs", CALLER)],
    );

    let answer = references_at(
        &store,
        &Rough(SyntacticAnswer::NotCode),
        Path::new("src/lib.rs"),
        0,
        7,
    );

    let References::Exact(found) = answer else {
        panic!("Exact が返らなかった: {answer:?}");
    };
    assert_eq!(
        found.direct,
        vec![
            Location {
                path: PathBuf::from("src/caller.rs"),
                line: 0,
                col: 9,
            },
            Location {
                path: PathBuf::from("src/caller.rs"),
                line: 1,
                col: 9,
            },
        ]
    );
    // インタフェース経由は Go の producer と一緒に埋める。いまは空。
    assert!(found.via_interface.is_empty());
}

#[test]
fn 参照側のファイルが変わったら構文層に回る() {
    // 参照は「そのファイルがそう書いてある」という主張なので、参照先が 1 つでも
    // 索引生成時と違えば、件数そのものが信用できない。
    let (index_path, root) = build("stale");
    let store = load(
        &index_path,
        &root,
        vec![("src/lib.rs", LIB), ("src/caller.rs", "中身が違う\n")],
    );

    let fallback = vec![Location {
        path: PathBuf::from("src/caller.rs"),
        line: 0,
        col: 9,
    }];
    let answer = references_at(
        &store,
        &Rough(SyntacticAnswer::Found(fallback.clone())),
        Path::new("src/lib.rs"),
        0,
        7,
    );

    assert_eq!(answer, References::Syntactic(fallback));
}

#[test]
fn 識別子でない位置は索引を引かずに_notcode_を返す() {
    let (index_path, root) = build("notcode");
    let store = load(
        &index_path,
        &root,
        vec![("src/lib.rs", LIB), ("src/caller.rs", CALLER)],
    );

    // `pub fn greet() {}` の空白。
    let answer = references_at(
        &store,
        &Rough(SyntacticAnswer::NotCode),
        Path::new("src/lib.rs"),
        0,
        3,
    );

    assert_eq!(answer, References::NotCode);
}
