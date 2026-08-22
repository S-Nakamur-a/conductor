//! 1 つの Store が複数の索引を持つときの検査。
//!
//! 索引の中の相対パスは索引ルートからの相対なので、リポジトリルートからの相対に
//! 接ぎ木してから鍵にする。接ぎ木は「今まで捨てていたルート外の Document が
//! 答えの対象に入ってくる」という形で誤答の経路を作るので、そこを重点的に見る。

use scip::types::{Document, Index, Metadata, Occurrence, SymbolRole, ToolInfo};
use sheaf_core::{Definition, IndexSource, Location, Span, Store, SyntacticAnswer, SyntacticLayer, Token, blob_hash, definition_at};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 語の切り出しを素朴にした構文層。索引が答えられるかだけを見たいので、
/// フォールバックは常に NotCode を返す。
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
        "sheaf-test-layout-{}-{}-{:?}",
        tag,
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
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

/// 索引ルート相対のパスを持つ Document を並べた索引を書く。
fn write_index(at: &Path, docs: Vec<Document>) {
    let index = Index {
        metadata: protobuf::MessageField::some(Metadata {
            tool_info: protobuf::MessageField::some(ToolInfo::default()),
            ..Default::default()
        }),
        documents: docs,
        ..Default::default()
    };
    std::fs::write(at, protobuf::Message::write_to_bytes(&index).unwrap()).unwrap();
}

fn doc(rel: &str, occurrences: Vec<Occurrence>) -> Document {
    Document {
        relative_path: rel.to_string(),
        occurrences,
        ..Default::default()
    }
}

/// ツリーにファイルを書いて内容ハッシュを返す。
///
/// `rel` はリポジトリルートからの相対。出自の表の鍵は**索引ルートからの相対**で
/// 別物なので、鍵は呼ぶ側が明示的に組む。
fn put(root: &Path, rel: &str, body: &str) -> String {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, body).unwrap();
    blob_hash(body.as_bytes())
}

/// 出自の表の 1 行。`rel` は索引ルートからの相対。
fn provenance(rel: &str, hash: String) -> (PathBuf, String) {
    (PathBuf::from(rel), hash)
}

fn source(index: &Path, subroot: &str, expected: Vec<(PathBuf, String)>) -> IndexSource {
    IndexSource {
        index: index.to_path_buf(),
        subroot: PathBuf::from(subroot),
        expected: expected.into_iter().collect::<HashMap<_, _>>(),
    }
}

const DEF: i32 = SymbolRole::Definition as i32;

// ---- (a) 索引ルートが 2 つあるツリーで、どちらの語も同じ Store から引ける ----

#[test]
fn 索引ルートが2つあっても同じstoreから両方引ける() {
    let root = workdir("two-roots");
    // api/ と api/nested/ が別々の索引ルート。入れ子は実在する形で、
    // 実測した対象では go.mod が 8 個あり、うち 1 つが別の索引ルートの下にいた。
    let outer_hash = put(&root, "api/main.go", "package main\n\nfunc Outer() {}\n");
    let inner_hash = put(&root, "api/nested/lib.go", "package nested\n\nfunc Inner() {}\n");

    let outer_index = root.join("outer.scip");
    write_index(
        &outer_index,
        vec![doc(
            "main.go",
            vec![occurrence(vec![2, 5, 10], "outer/Outer().", DEF)],
        )],
    );
    let inner_index = root.join("inner.scip");
    write_index(
        &inner_index,
        vec![doc(
            "lib.go",
            vec![occurrence(vec![2, 5, 10], "nested/Inner().", DEF)],
        )],
    );

    let store = Store::load(
        &[
            source(&outer_index, "api", vec![provenance("main.go", outer_hash)]),
            source(&inner_index, "api/nested", vec![provenance("lib.go", inner_hash)]),
        ],
        &root,
    )
    .unwrap();

    let outer = definition_at(&store, &Rough, Path::new("api/main.go"), 2, 5);
    assert_eq!(
        outer,
        Definition::Exact(vec![Location {
            path: PathBuf::from("api/main.go"),
            line: 2,
            col: 5,
        }]),
        "外側の索引ルートの語が引けない"
    );

    let inner = definition_at(&store, &Rough, Path::new("api/nested/lib.go"), 2, 5);
    assert_eq!(
        inner,
        Definition::Exact(vec![Location {
            path: PathBuf::from("api/nested/lib.go"),
            line: 2,
            col: 5,
        }]),
        "内側の索引ルートの語が引けない"
    );
}

// ---- (b) 索引をまたいで符号を突き合わせない ----

#[test]
fn 同じ符号が別の索引にあっても混ざらない() {
    // scip-typescript は名前の無い package.json に `npm . .` という座標を振る。
    // 座標に索引ルートの情報が入らないので、別の索引ルートの同じ相対パスの
    // ファイルが同じ符号を持つ。またいで突き合わせると誤った定義を Exact で返す。
    let root = workdir("same-symbol");
    let a_hash = put(&root, "appA/types/routes.d.ts", "export const marker = 1;\n");
    let b_hash = put(&root, "appB/types/routes.d.ts", "export const marker = 2;\n");

    const SHARED: &str = "scip-typescript npm . . types/`routes.d.ts`/marker.";

    let a_index = root.join("a.scip");
    write_index(
        &a_index,
        vec![doc(
            "types/routes.d.ts",
            vec![occurrence(vec![0, 13, 19], SHARED, DEF)],
        )],
    );
    let b_index = root.join("b.scip");
    write_index(
        &b_index,
        vec![doc(
            "types/routes.d.ts",
            vec![occurrence(vec![0, 13, 19], SHARED, DEF)],
        )],
    );

    let store = Store::load(
        &[
            source(&a_index, "appA", vec![provenance("types/routes.d.ts", a_hash)]),
            source(&b_index, "appB", vec![provenance("types/routes.d.ts", b_hash)]),
        ],
        &root,
    )
    .unwrap();

    let answer = definition_at(&store, &Rough, Path::new("appA/types/routes.d.ts"), 0, 13);
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

// ---- (c) 接ぎ木してもリポジトリルートの外に出るものは捨てる ----

#[test]
fn 接ぎ木してもルートの外に出るdocumentは捨てる() {
    let root = workdir("escape");
    let hash = put(&root, "api/main.go", "package main\n");

    let index = root.join("i.scip");
    write_index(
        &index,
        vec![
            doc("main.go", vec![occurrence(vec![0, 8, 12], "m/Main().", DEF)]),
            // api/ から見て 2 つ上がるとリポジトリルートの外。
            doc(
                "../../outside.go",
                vec![occurrence(vec![0, 0, 3], "x/Out().", DEF)],
            ),
        ],
    );

    let store = Store::load(
        &[source(&index, "api", vec![provenance("main.go", hash)])],
        &root,
    )
    .unwrap();
    assert_eq!(store.len(), 1, "ルート外の Document を投入した");
    assert_eq!(store.outside_root(), 1);
}

// ---- (d) 接ぎ木で新たにルート内に入った Document は、出自が合ったときだけ Exact ----

#[test]
fn 接ぎ木でルート内に入ったdocumentは出自が合わなければexactにしない() {
    // 索引ルートの外を指す Document は今まで問答無用で捨てていた。接ぎ木を入れると
    // リポジトリの中に入ってくるので、ここが S1 で唯一、誤答が起こり得る箇所になる。
    let root = workdir("graft-in");
    put(&root, "api/main.go", "package main\n");
    // api/ から見た ../shared/util.go は shared/util.go になる。リポジトリ内。
    let shared = root.join("shared/util.go");
    std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
    std::fs::write(&shared, "package shared\n\nfunc Util() {}\n").unwrap();

    let index = root.join("i.scip");
    write_index(
        &index,
        vec![doc(
            "../shared/util.go",
            vec![occurrence(vec![2, 5, 9], "shared/Util().", DEF)],
        )],
    );

    // 出自を申告しないまま投入すると Exact にならない。
    let without = Store::load(&[source(&index, "api", vec![])], &root).unwrap();
    assert_eq!(without.len(), 1, "接ぎ木でルート内に入らなかった");
    assert_eq!(without.missing_provenance(), 1);
    let answer = definition_at(&without, &Rough, Path::new("shared/util.go"), 2, 5);
    assert_eq!(
        answer,
        Definition::NotCode,
        "出自の無い Document を Exact で返した: {answer:?}"
    );

    // 出自が合っていれば Exact。索引の鍵は索引ルート相対のままなので `../` 込みで渡す。
    let hash = blob_hash(&std::fs::read(&shared).unwrap());
    let with = Store::load(
        &[source(
            &index,
            "api",
            vec![provenance("../shared/util.go", hash)],
        )],
        &root,
    )
    .unwrap();
    let answer = definition_at(&with, &Rough, Path::new("shared/util.go"), 2, 5);
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

// ---- (e)(f) パスの衝突はいちばん深い索引ルートが勝ち、衝突していないパスは無傷 ----

#[test]
fn パスが衝突したらいちばん深い索引ルートが勝つ() {
    // front の索引が ../common/design_tokens/... を含む形が実在する。
    // 両方落とすとそのファイルが両方から消える。索引ごと弾くと、呼び出し側が
    // 握り潰したときに全索引が黙って消える。
    let root = workdir("conflict");
    // 2 行にして、所有者側の索引だけが 2 行目の定義を持つ形にする。どちらが勝っても
    // 同じ位置が返る作りにすると、浅いほうが勝つ実装でも検査が通ってしまう。
    let own_hash = put(
        &root,
        "common/tokens/src/mui.d.ts",
        "export const c = 1;\nexport const d = 2;\n",
    );
    let front_hash = put(&root, "front/app.ts", "export const a = 2;\n");

    let front_index = root.join("front.scip");
    write_index(
        &front_index,
        vec![
            doc(
                "app.ts",
                vec![occurrence(vec![0, 13, 14], "front/a.", DEF)],
            ),
            // front から見た ../common/tokens/src/mui.d.ts。所有者は common/tokens。
            doc(
                "../common/tokens/src/mui.d.ts",
                vec![occurrence(vec![0, 13, 14], "front-side/c.", DEF)],
            ),
        ],
    );
    let tokens_index = root.join("tokens.scip");
    write_index(
        &tokens_index,
        vec![doc(
            "src/mui.d.ts",
            vec![
                occurrence(vec![0, 13, 14], "tokens-side/c.", DEF),
                occurrence(vec![1, 13, 14], "tokens-side/d.", DEF),
            ],
        )],
    );

    let store = Store::load(
        &[
            source(
                &front_index,
                "front",
                vec![
                    provenance("app.ts", front_hash),
                    provenance("../common/tokens/src/mui.d.ts", own_hash.clone()),
                ],
            ),
            source(&tokens_index, "common/tokens", vec![provenance("src/mui.d.ts", own_hash)]),
        ],
        &root,
    )
    .unwrap();

    assert_eq!(store.path_conflicts(), 1, "衝突を数えていない");

    // 衝突したパスは、所有する（いちばん深い）索引ルートのものが残る。
    let owned = definition_at(
        &store,
        &Rough,
        Path::new("common/tokens/src/mui.d.ts"),
        0,
        13,
    );
    assert_eq!(
        owned,
        Definition::Exact(vec![Location {
            path: PathBuf::from("common/tokens/src/mui.d.ts"),
            line: 0,
            col: 13,
        }]),
        "所有する索引ルートの Document が残っていない"
    );

    // 勝ったのが所有者側であることを、所有者側の索引にしかない定義で確かめる。
    // 位置だけを見ると、浅いほうが勝つ実装でも上の検査が通る。
    let only_owner_knows = definition_at(
        &store,
        &Rough,
        Path::new("common/tokens/src/mui.d.ts"),
        1,
        13,
    );
    assert_eq!(
        only_owner_knows,
        Definition::Exact(vec![Location {
            path: PathBuf::from("common/tokens/src/mui.d.ts"),
            line: 1,
            col: 13,
        }]),
        "勝ったのが所有者側の索引ではない"
    );

    // 衝突していないパスは無傷。これが無いと、全部落とす実装でも上が通る。
    let untouched = definition_at(&store, &Rough, Path::new("front/app.ts"), 0, 13);
    assert_eq!(
        untouched,
        Definition::Exact(vec![Location {
            path: PathBuf::from("front/app.ts"),
            line: 0,
            col: 13,
        }]),
        "衝突していないパスまで落ちた"
    );
}
