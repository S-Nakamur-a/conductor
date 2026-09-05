//! 定義位置を索引に持たない符号を、囲んでいる型に落とす。
//!
//! derive が作った impl がこれに当たる。`GitStatusMap::default()` の `default` は
//! 索引に定義位置を持たないが、`GitStatusMap` は持っている。
//!
//! 実索引を要求するものは `#[ignore]`。
//!   SHEAF_TEST_INDEX=<.scip> SHEAF_TEST_ROOT=<ツリー> cargo test --test enclosing -- --ignored

mod common;

use common::{doc, index, load_one, load_real_index, provenance, silent, workdir_with_src};
use sheaf_core::{Definition, Location, Store, definition_at};
use std::path::{Path, PathBuf};

const PKG: &str = "rust-analyzer cargo demo 0.1.0";
/// derive が作った impl。定義 occurrence を持たない。
const DERIVED: &str =
    "rust-analyzer cargo demo 0.1.0 app/focus/impl#[Focus][`PartialEq<Self>`]eq().";
/// その型。定義 occurrence を持つ。
const TYPE: &str = "rust-analyzer cargo demo 0.1.0 app/focus/Focus#";

const LIB: &str = "pub enum Focus {}\n";
const CALLER: &str = "fn a(x: Focus) { x.eq(); }\n";

/// `type_defined` が false なら、型のほうの定義 occurrence も置かない。
fn build(tag: &str, type_defined: bool, extra: &[(&str, i32)]) -> (PathBuf, PathBuf) {
    let root = workdir_with_src(tag);
    std::fs::write(root.join("src/lib.rs"), LIB).unwrap();
    std::fs::write(root.join("src/caller.rs"), CALLER).unwrap();

    let mut lib = doc("src/lib.rs");
    if type_defined {
        lib = lib.def([0, 9, 14], TYPE);
    }
    let mut caller = doc("src/caller.rs").reference([0, 19, 21], DERIVED);
    for (symbol, roles) in extra {
        caller = caller.occurrence([0, 19, 21], symbol, *roles);
    }

    let index_path = index()
        .utf8()
        .add(lib)
        .add(caller)
        .write(&root.join("index.scip"));
    (index_path, root)
}

fn load(index_path: &Path, root: &Path, lib_body: &str) -> Store {
    load_one(
        index_path,
        root,
        provenance(&[("src/lib.rs", lib_body), ("src/caller.rs", CALLER)]),
    )
    .unwrap()
}

#[test]
fn 定義位置を持たない符号は囲んでいる型に落ちる() {
    let (index_path, root) = build("hit", true, &[]);
    let store = load(&index_path, &root, LIB);

    let answer = definition_at(&store, &silent(), Path::new("src/caller.rs"), 0, 19);

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
fn 型に落とせない条件では構文層に回る() {
    // 座標を作る規則なので、作った先が無いことは普通に起きる (impl が型とは別の
    // モジュールにあるとき)。そこで無理に何かを返さない。
    let missing = build("miss", false, &[]);
    let stale = build("stale", true, &[]);

    for (why, store) in [
        (
            "組み立てた型が索引に無い",
            load(&missing.0, &missing.1, LIB),
        ),
        (
            "型が定義されているファイルが変わった",
            load(&stale.0, &stale.1, "中身が違う\n"),
        ),
    ] {
        let answer = definition_at(&store, &silent(), Path::new("src/caller.rs"), 0, 19);
        assert_eq!(answer, Definition::NotCode, "{why}");
    }
}

#[test]
fn 語そのものの定義があれば型には落とさない() {
    // 直接の定義とあとから足した弱い答えを混ぜない。混ぜると、強い主張しか
    // 無かったはずの答えに弱いものが紛れる。
    let direct = format!("{PKG} app/focus/impl#[Focus][`PartialEq<Self>`]ne().");
    let (index_path, root) = build("direct", true, &[(&direct, 0), (&direct, 1)]);
    let store = load(&index_path, &root, LIB);

    let answer = definition_at(&store, &silent(), Path::new("src/caller.rs"), 0, 19);

    let Definition::Exact(found) = answer else {
        panic!("Exact が返らなかった: {answer:?}");
    };
    assert_eq!(found.len(), 1);
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_derive_の_default_は型の定義に落ちる() {
    let (store, root) = load_real_index();

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

    let answer = definition_at(&store, &silent(), rel, 99, col);

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
