//! 参照検索の公開入口の検査。
//!
//! 定義ジャンプと同じ形で、確信度を判定しないと位置を取り出せないことを見る。

mod common;

use common::{Rough, doc, index, load_one, provenance, silent, workdir_with_src};
use sheaf_core::{Found, Location, References, SyntacticAnswer, references_at};
use std::path::{Path, PathBuf};

const SYMBOL: &str = "scip-test cargo demo 0.1.0 greet().";
const LIB: &str = "pub fn greet() {}\n";
const CALLER: &str = "fn a() { greet(); }\nfn b() { greet(); }\n";

fn at(rel: &str, line: u32, col: u32) -> Location {
    Location {
        path: PathBuf::from(rel),
        line,
        col,
    }
}

#[test]
fn 参照の答えは確信度で分かれる() {
    // 参照は「そのファイルがそう書いてある」という主張なので、参照先が 1 つでも
    // 索引生成時と違えば件数そのものが信用できない。
    let root = workdir_with_src("refs");
    std::fs::write(root.join("src/lib.rs"), LIB).unwrap();
    std::fs::write(root.join("src/caller.rs"), CALLER).unwrap();

    let index_path = index()
        .utf8()
        .add(doc("src/lib.rs").def([0, 7, 12], SYMBOL))
        .add(
            doc("src/caller.rs")
                .reference([0, 9, 14], SYMBOL)
                .reference([1, 9, 14], SYMBOL),
        )
        .write(&root.join("index.scip"));

    let fresh = load_one(
        &index_path,
        &root,
        provenance(&[("src/lib.rs", LIB), ("src/caller.rs", CALLER)]),
    )
    .unwrap();
    let stale = load_one(
        &index_path,
        &root,
        provenance(&[("src/lib.rs", LIB), ("src/caller.rs", "中身が違う\n")]),
    )
    .unwrap();

    let fallback = vec![at("src/caller.rs", 0, 9)];

    for (why, store, syntactic, col, want) in [
        (
            "索引が答えられる位置",
            &fresh,
            silent(),
            7,
            References::Exact(Found {
                direct: vec![at("src/caller.rs", 0, 9), at("src/caller.rs", 1, 9)],
                via_interface: Vec::new(),
            }),
        ),
        (
            "参照側のファイルが変わった",
            &stale,
            Rough::new(SyntacticAnswer::Found(fallback.clone())),
            7,
            References::Syntactic(fallback.clone()),
        ),
        // `pub fn greet() {}` の空白。
        ("識別子でない位置", &fresh, silent(), 3, References::NotCode),
    ] {
        let answer = references_at(store, &syntactic, Path::new("src/lib.rs"), 0, col);
        assert_eq!(answer, want, "{why}");
    }
}
