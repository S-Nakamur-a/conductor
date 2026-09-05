//! 1 つの Store が複数の索引を持つときの検査。
//!
//! 索引の中の相対パスは索引ルートからの相対なので、リポジトリルートからの相対に
//! 接ぎ木してから鍵にする。接ぎ木は「今まで捨てていたルート外の Document が
//! 答えの対象に入ってくる」という形で誤答の経路を作るので、そこを重点的に見る。

mod common;

use common::{doc, index, silent, source, workdir, write_and_hash};
use scip::types::SymbolRole;
use sheaf_core::{Definition, Location, Store, definition_at};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const DEF: i32 = SymbolRole::Definition as i32;

/// 出自の表の 1 行。`rel` は索引ルートからの相対で、リポジトリルートからの相対とは別物。
fn provenance(entries: &[(&str, &str)]) -> HashMap<PathBuf, String> {
    entries
        .iter()
        .map(|(rel, hash)| (PathBuf::from(*rel), hash.to_string()))
        .collect()
}

#[test]
fn 索引ルートが2つあっても同じstoreから両方引ける() {
    // api/ と api/nested/ が別々の索引ルート。入れ子は実在する形で、実測した対象では
    // go.mod が 8 個あり、うち 1 つが別の索引ルートの下にいた。
    let root = workdir("two-roots");
    let outer_hash = write_and_hash(&root, "api/main.go", "package main\n\nfunc Outer() {}\n");
    let inner_hash = write_and_hash(
        &root,
        "api/nested/lib.go",
        "package nested\n\nfunc Inner() {}\n",
    );

    let outer_index = index()
        .unnamed_tool()
        .add(doc("main.go").occurrence([2, 5, 10], "outer/Outer().", DEF))
        .write(&root.join("outer.scip"));
    let inner_index = index()
        .unnamed_tool()
        .add(doc("lib.go").occurrence([2, 5, 10], "nested/Inner().", DEF))
        .write(&root.join("inner.scip"));

    let store = Store::load(
        &[
            source(&outer_index, "api", provenance(&[("main.go", &outer_hash)])),
            source(
                &inner_index,
                "api/nested",
                provenance(&[("lib.go", &inner_hash)]),
            ),
        ],
        &root,
    )
    .unwrap();

    for (why, rel) in [
        ("外側の索引ルート", "api/main.go"),
        ("内側の索引ルート", "api/nested/lib.go"),
    ] {
        let answer = definition_at(&store, &silent(), Path::new(rel), 2, 5);
        assert_eq!(
            answer,
            Definition::Exact(vec![Location {
                path: PathBuf::from(rel),
                line: 2,
                col: 5,
            }]),
            "{why}の語が引けない"
        );
    }
}

#[test]
fn 同じ符号が別の索引にあっても混ざらない() {
    // scip-typescript は名前の無い package.json に `npm . .` という座標を振る。座標に
    // 索引ルートの情報が入らないので、別の索引ルートの同じ相対パスのファイルが同じ符号を
    // 持つ。またいで突き合わせると誤った定義を Exact で返す。
    let root = workdir("same-symbol");
    let a_hash = write_and_hash(
        &root,
        "appA/types/routes.d.ts",
        "export const marker = 1;\n",
    );
    let b_hash = write_and_hash(
        &root,
        "appB/types/routes.d.ts",
        "export const marker = 2;\n",
    );

    const SHARED: &str = "scip-typescript npm . . types/`routes.d.ts`/marker.";

    let a_index = index()
        .unnamed_tool()
        .add(doc("types/routes.d.ts").occurrence([0, 13, 19], SHARED, DEF))
        .write(&root.join("a.scip"));
    let b_index = index()
        .unnamed_tool()
        .add(doc("types/routes.d.ts").occurrence([0, 13, 19], SHARED, DEF))
        .write(&root.join("b.scip"));

    let store = Store::load(
        &[
            source(
                &a_index,
                "appA",
                provenance(&[("types/routes.d.ts", &a_hash)]),
            ),
            source(
                &b_index,
                "appB",
                provenance(&[("types/routes.d.ts", &b_hash)]),
            ),
        ],
        &root,
    )
    .unwrap();

    let answer = definition_at(
        &store,
        &silent(),
        Path::new("appA/types/routes.d.ts"),
        0,
        13,
    );
    assert_eq!(
        answer,
        Definition::Exact(vec![Location {
            path: PathBuf::from("appA/types/routes.d.ts"),
            line: 0,
            col: 13,
        }]),
        "別の索引の同じ符号が混ざった: {answer:?}"
    );
}

#[test]
fn 接ぎ木してもルートの外に出るdocumentは捨てる() {
    let root = workdir("escape");
    let hash = write_and_hash(&root, "api/main.go", "package main\n");

    let index_path = index()
        .unnamed_tool()
        .add(doc("main.go").occurrence([0, 8, 12], "m/Main().", DEF))
        // api/ から見て 2 つ上がるとリポジトリルートの外。
        .add(doc("../../outside.go").occurrence([0, 0, 3], "x/Out().", DEF))
        .write(&root.join("i.scip"));

    let store = Store::load(
        &[source(
            &index_path,
            "api",
            provenance(&[("main.go", &hash)]),
        )],
        &root,
    )
    .unwrap();
    assert_eq!(store.len(), 1, "ルート外の Document を投入した");
    assert_eq!(store.outside_root(), 1);
}

#[test]
fn 接ぎ木でルート内に入ったdocumentは出自が合わなければexactにしない() {
    // 索引ルートの外を指す Document は今まで問答無用で捨てていた。接ぎ木を入れると
    // リポジトリの中に入ってくるので、ここが唯一、誤答が起こり得る箇所になる。
    let root = workdir("graft-in");
    write_and_hash(&root, "api/main.go", "package main\n");
    // api/ から見た ../shared/util.go は shared/util.go になる。リポジトリ内。
    let shared_hash = write_and_hash(
        &root,
        "shared/util.go",
        "package shared\n\nfunc Util() {}\n",
    );

    let index_path = index()
        .unnamed_tool()
        .add(doc("../shared/util.go").occurrence([2, 5, 9], "shared/Util().", DEF))
        .write(&root.join("i.scip"));

    let without = Store::load(&[source(&index_path, "api", HashMap::new())], &root).unwrap();
    assert_eq!(without.len(), 1, "接ぎ木でルート内に入らなかった");
    assert_eq!(without.missing_provenance(), 1);
    let answer = definition_at(&without, &silent(), Path::new("shared/util.go"), 2, 5);
    assert_eq!(
        answer,
        Definition::NotCode,
        "出自の無い Document を Exact で返した: {answer:?}"
    );

    // 索引の鍵は索引ルート相対のままなので `../` 込みで渡す。
    let with = Store::load(
        &[source(
            &index_path,
            "api",
            provenance(&[("../shared/util.go", &shared_hash)]),
        )],
        &root,
    )
    .unwrap();
    let answer = definition_at(&with, &silent(), Path::new("shared/util.go"), 2, 5);
    assert_eq!(
        answer,
        Definition::Exact(vec![Location {
            path: PathBuf::from("shared/util.go"),
            line: 2,
            col: 5,
        }]),
        "接ぎ木でルート内に入った Document が引けない"
    );
}

#[test]
fn パスが衝突したらいちばん深い索引ルートが勝つ() {
    // front の索引が ../common/design_tokens/... を含む形が実在する。両方落とすと
    // そのファイルが両方から消える。索引ごと弾くと、呼び出し側が握り潰したときに
    // 全索引が黙って消える。
    let root = workdir("conflict");
    // 所有者側の索引だけが 2 行目の定義を持つ形にする。どちらが勝っても同じ位置が
    // 返る作りにすると、浅いほうが勝つ実装でも検査が通ってしまう。
    let own_hash = write_and_hash(
        &root,
        "common/tokens/src/mui.d.ts",
        "export const c = 1;\nexport const d = 2;\n",
    );
    let front_hash = write_and_hash(&root, "front/app.ts", "export const a = 2;\n");

    let front_index = index()
        .unnamed_tool()
        .add(doc("app.ts").occurrence([0, 13, 14], "front/a.", DEF))
        // front から見た ../common/tokens/src/mui.d.ts。所有者は common/tokens。
        .add(doc("../common/tokens/src/mui.d.ts").occurrence([0, 13, 14], "front-side/c.", DEF))
        .write(&root.join("front.scip"));
    let tokens_index = index()
        .unnamed_tool()
        .add(
            doc("src/mui.d.ts")
                .occurrence([0, 13, 14], "tokens-side/c.", DEF)
                .occurrence([1, 13, 14], "tokens-side/d.", DEF),
        )
        .write(&root.join("tokens.scip"));

    let store = Store::load(
        &[
            source(
                &front_index,
                "front",
                provenance(&[
                    ("app.ts", &front_hash),
                    ("../common/tokens/src/mui.d.ts", &own_hash),
                ]),
            ),
            source(
                &tokens_index,
                "common/tokens",
                provenance(&[("src/mui.d.ts", &own_hash)]),
            ),
        ],
        &root,
    )
    .unwrap();

    assert_eq!(store.path_conflicts(), 1, "衝突を数えていない");

    for (why, rel, line) in [
        (
            "所有する索引ルートの Document が残っていない",
            "common/tokens/src/mui.d.ts",
            0,
        ),
        // 勝ったのが所有者側であることを、所有者側の索引にしかない定義で確かめる。
        // 位置だけを見ると、浅いほうが勝つ実装でも上の行が通る。
        (
            "勝ったのが所有者側の索引ではない",
            "common/tokens/src/mui.d.ts",
            1,
        ),
        // 衝突していないパスは無傷。これが無いと、全部落とす実装でも上が通る。
        ("衝突していないパスまで落ちた", "front/app.ts", 0),
    ] {
        let answer = definition_at(&store, &silent(), Path::new(rel), line, 13);
        assert_eq!(
            answer,
            Definition::Exact(vec![Location {
                path: PathBuf::from(rel),
                line,
                col: 13,
            }]),
            "{why}"
        );
    }
}
