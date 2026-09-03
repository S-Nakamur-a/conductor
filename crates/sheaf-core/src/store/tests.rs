//! Store の問い合わせの検査。鮮度を合成フィクスチャで見るものと、実索引を要るものがある。

#![allow(clippy::items_after_test_module)]

use super::fixture::{load_single, span};
use super::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// 定義 1 つと参照 2 つを別々のファイルに載せた索引。
///
/// 参照が 2 ファイルに散っていないと、片方を消したときに答えが空になってそのまま
/// None が返るので、「残りだけを Exact で返す」不具合を素通りする。
struct Fixture {
    root: PathBuf,
    index: PathBuf,
    /// 定義側 (src/lib.rs) の greet の範囲。
    definition: Span,
    /// 参照側 (src/caller.rs) の greet の範囲。
    reference: Span,
}

impl Fixture {
    fn new(root: &Path) -> Self {
        use protobuf::{EnumOrUnknown, Message, MessageField};
        use scip::types::{Document, Index, Metadata, Occurrence, TextEncoding};

        std::fs::create_dir_all(root.join("src")).unwrap();
        for (rel, body) in [
            ("src/lib.rs", "pub fn greet() {}\n"),
            ("src/caller.rs", "fn caller() { greet(); }\n"),
            ("src/other.rs", "fn other() { greet(); }\n"),
        ] {
            std::fs::write(root.join(rel), body).unwrap();
        }

        let symbol = "scip-test cargo demo 0.1.0 greet().";
        let doc = |rel: &str, range: Vec<i32>, roles: i32| Document {
            relative_path: rel.to_string(),
            language: "rust".to_string(),
            occurrences: vec![Occurrence {
                range,
                symbol: symbol.to_string(),
                symbol_roles: roles,
                ..Default::default()
            }],
            ..Default::default()
        };
        // position_encoding を宣言しない。scip-go と scip-typescript がこの形で、
        // 列の数え方を索引から決められないので、Store は occurrence ごとにソースを読む。
        let index = Index {
            metadata: MessageField::some(Metadata {
                project_root: format!("file://{}", root.display()),
                text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
                ..Default::default()
            }),
            documents: vec![
                doc("src/lib.rs", vec![0, 7, 12], 1),
                doc("src/caller.rs", vec![0, 14, 19], 0),
                doc("src/other.rs", vec![0, 13, 18], 0),
            ],
            ..Default::default()
        };
        let index_path = root.join("index.scip");
        std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();

        Fixture {
            root: root.to_path_buf(),
            index: index_path,
            definition: span(0, 7, 0, 12),
            reference: span(0, 14, 0, 19),
        }
    }

    fn load(&self) -> Store {
        let expected = hashes_of(&self.root, &["src/lib.rs", "src/caller.rs", "src/other.rs"]);
        load_single(&self.index, &self.root, expected)
    }
}

fn hashes_of(root: &Path, rels: &[&str]) -> HashMap<PathBuf, String> {
    rels.iter()
        .map(|rel| {
            let bytes = std::fs::read(root.join(rel)).unwrap();
            (PathBuf::from(rel), blob_hash(&bytes))
        })
        .collect()
}

/// 参照の答えが依拠しているのは、参照が載っているファイル全部。1 つでも索引生成時と
/// 違えば答えを丸ごと捨てる。飛び先 (定義) は依拠集合に入らない。
///
/// 消すだけでなく行を消す場合も見る。ファイルが読めても、列の数え方が確定していない
/// 索引では使えない occurrence が落ちて、その Document の参照が 0 件になる。0 件の
/// Document は鮮度の検査に載らない (検査は返ってきた位置ごとに回る) ので、欠けた答えが
/// Exact として通り抜ける。ブランチ切替でファイルが消えるのも編集も日常なので実際に踏める。
#[test]
fn 参照が載っているファイルが動いたら答えを丸ごと捨てる() {
    struct Case {
        how: &'static str,
        /// 参照側から聞くか。既定は定義側 (src/lib.rs) から聞く。
        ask_from_reference: bool,
        perturb: fn(&Path),
        still_found: bool,
    }

    let cases = [
        Case {
            how: "参照が載っているファイルが消えた",
            ask_from_reference: false,
            perturb: |root| std::fs::remove_file(root.join("src/caller.rs")).unwrap(),
            still_found: false,
        },
        Case {
            how: "参照が載っているファイルを書き換えた",
            ask_from_reference: false,
            perturb: |root| {
                std::fs::write(root.join("src/caller.rs"), "fn caller() { greet();  }\n").unwrap()
            },
            still_found: false,
        },
        Case {
            how: "参照が載っていた行だけが消えた",
            ask_from_reference: false,
            perturb: |root| std::fs::write(root.join("src/other.rs"), "\n").unwrap(),
            still_found: false,
        },
        Case {
            how: "聞いた位置のファイルを書き換えた",
            ask_from_reference: false,
            perturb: |root| {
                std::fs::write(root.join("src/lib.rs"), "// 変更\npub fn greet() {}\n").unwrap()
            },
            still_found: false,
        },
        Case {
            how: "飛び先の定義だけが動いた",
            ask_from_reference: true,
            perturb: |root| {
                std::fs::write(
                    root.join("src/lib.rs"),
                    "// 定義を動かした\npub fn greet() {}\n",
                )
                .unwrap()
            },
            still_found: true,
        },
    ];

    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture::new(dir.path());
        let store = fixture.load();
        let (asked, span) = if case.ask_from_reference {
            (Path::new("src/caller.rs"), fixture.reference)
        } else {
            (Path::new("src/lib.rs"), fixture.definition)
        };

        let before = store.references_in(asked, span);
        assert_eq!(
            before.as_ref().map(|f| f.direct.len()),
            Some(2),
            "{}: 動かす前から参照 2 件が揃っていない",
            case.how
        );

        (case.perturb)(dir.path());

        let after = store.references_in(asked, span);
        if case.still_found {
            assert_eq!(
                after, before,
                "{}: 依拠していないのに答えが変わった",
                case.how
            );
        } else {
            assert!(after.is_none(), "{}: 残りだけを Found で返した", case.how);
        }
    }
}

// ---- 参照クエリ：実索引が要る検査。SHEAF_TEST_INDEX と SHEAF_TEST_ROOT で場所を渡す。----
//   cargo test -p sheaf-core --lib -- --ignored

/// 索引と同じディレクトリの index.hashes を出自の表として使う。索引生成時に
/// 申告されたハッシュそのものなので、いまのツリーを丸ごと読み直して確かめるより
/// 索引の鮮度判定に忠実になる。
fn real_store() -> Store {
    let index = PathBuf::from(
        std::env::var("SHEAF_TEST_INDEX").expect("SHEAF_TEST_INDEX に .scip のパスを渡すこと"),
    );
    let root = PathBuf::from(
        std::env::var("SHEAF_TEST_ROOT").expect("SHEAF_TEST_ROOT にソースツリーのルートを渡すこと"),
    );
    let hashes_path = index.with_file_name("index.hashes");
    // 手で読まない。出自の表は道具の指紋を見出しに持つので、書式を2箇所に持つと
    // 見出しが本文として読まれて、存在しないパスが出自として入る。
    let expected =
        crate::read_provenance(&hashes_path, &crate::RustAnalyzer).unwrap_or_else(|| {
            panic!(
                "{} を rust-analyzer が作った出自の表として読めない",
                hashes_path.display()
            )
        });
    let drifted = expected
        .iter()
        .filter(|(rel, hash)| !std::fs::read(root.join(rel)).is_ok_and(|b| blob_hash(&b) == **hash))
        .count();
    // 参照クエリは参照先が1つでも動いていれば答えを返さない。ここで数えておかないと、
    // ツリーが索引より進んだだけの失敗が「参照が引けない」に見える。
    assert_eq!(
        drifted, 0,
        "索引が {} ファイル分だけツリーより古い。索引を作り直してから走らせること",
        drifted
    );
    load_single(&index, &root, expected)
}

fn range_to_span(range: &[i32]) -> Span {
    match *range {
        [sl, sc, ec] => span(sl as u32, sc as u32, sl as u32, ec as u32),
        [sl, sc, el, ec] => span(sl as u32, sc as u32, el as u32, ec as u32),
        _ => panic!("unexpected range shape: {range:?}"),
    }
}

/// 転置索引が指す Document の1つから、そのシンボルへの参照 occurrence を探して
/// span にする。そこから references_in を呼べば、必ずそのシンボルへのクエリになる。
fn first_reference_span(store: &Store, symbol: &str, path: &Path) -> Span {
    let entry = store.docs.get(path).expect("path が docs に無い");
    let doc = parse_document(&store.bytes[entry.index][entry.span.clone()]).unwrap();
    let content = (entry.column_encoding != scip_split::ColumnEncoding::Utf8)
        .then(|| std::fs::read(store.root.join(path)).ok())
        .flatten();
    let lines = content.as_deref().map(Lines::of);
    doc.occurrences
        .iter()
        .find(|o| o.symbol == symbol && !is_definition(o.symbol_roles))
        .and_then(|o| usable_range(&o.range, entry.column_encoding, lines.as_ref()))
        .map(|r| range_to_span(&r))
        .unwrap_or_else(|| panic!("{symbol} の参照 occurrence が {path:?} に見つからない"))
}

// 符号にはクレートの版が埋まっている（`... <クレート> 0.104.0 app/App#viewer.`）ので、
// 完全一致で固定すると版を上げるだけで落ちる。末尾で引いて、対象そのものが
// 消えたときだけ落ちるようにする。
const VIEWER_STATE: &str = "app/App#viewer.";
const OPTION_SOME: &str = "option/Option#Some#";

/// 末尾が一致する符号を索引から 1 つだけ選ぶ。複数あれば対象が曖昧なので落とす。
fn symbol_ending_with(store: &Store, suffix: &str) -> String {
    let mut found: Vec<&str> = store
        .references
        .iter()
        .flatten()
        .map(|(k, _)| &**k)
        .filter(|k| k.ends_with(suffix))
        .collect();
    found.sort_unstable();
    found.dedup();
    match found.as_slice() {
        [one] => (*one).to_string(),
        [] => panic!("{suffix} で終わる符号が索引に無い。対象が変わっている"),
        many => panic!("{suffix} で終わる符号が複数ある: {many:?}"),
    }
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_転置から引いた参照が全走査と一致する() {
    let store = real_store();
    for suffix in [VIEWER_STATE, OPTION_SOME] {
        let symbol = symbol_ending_with(&store, suffix);
        let doc_ids = &store.references[0][symbol.as_str()];

        let path = store.doc_paths[doc_ids[0] as usize].clone();
        let span = first_reference_span(&store, &symbol, &path);
        let found = store
            .references_in(&path, span)
            .expect("Found を返さなかった");

        assert!(found.via_interface.is_empty(), "{suffix}");
        assert_eq!(found.direct, scan_references(&store, &symbol), "{suffix}");
    }
}

/// 転置を使わずに、索引を頭から歩いてその語への参照を集める。
fn scan_references(store: &Store, symbol: &str) -> Vec<Location> {
    let mut out = Vec::new();
    for path in &store.doc_paths {
        let entry = &store.docs[path];
        let doc = parse_document(&store.bytes[entry.index][entry.span.clone()]).unwrap();
        let rel = path.to_string_lossy();
        for occ in &doc.occurrences {
            if occ.symbol != symbol || is_definition(occ.symbol_roles) {
                continue;
            }
            let Some(range) = usable_range(&occ.range, entry.column_encoding, None) else {
                continue;
            };
            if let Some(loc) = location_of(&range, rel.as_ref()) {
                out.push(loc);
            }
        }
    }
    // doc_paths の順は投入時の HashMap の走査順なので実行ごとに変わる。
    // 突き合わせる相手と同じ規則で並べる。
    out.sort_by(|a, b| position_key(a).cmp(&position_key(b)));
    out
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_転置は全走査と同じものを持っている() {
    // 件数を定数で固定すると、索引の中身が少し動いただけで落ちる。
    // 検査したいのは「転置が落としていないこと」なので、同じ索引から
    // 全走査で作った集合と突き合わせる。
    let store = real_store();
    let mut scanned: HashSet<(String, u32)> = HashSet::new();
    for (doc_id, path) in store.doc_paths.iter().enumerate() {
        let entry = &store.docs[path];
        let doc = parse_document(&store.bytes[entry.index][entry.span.clone()]).unwrap();
        for occ in &doc.occurrences {
            if !is_definition(occ.symbol_roles) && !occ.symbol.starts_with("local ") {
                scanned.insert((occ.symbol.clone(), doc_id as u32));
            }
        }
    }
    let held: HashSet<(String, u32)> = store
        .references
        .iter()
        .flatten()
        .flat_map(|(s, ids)| ids.iter().map(|d| (s.to_string(), *d)))
        .collect();
    assert_eq!(held, scanned, "転置と全走査が食い違っている");
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_localは_document内だけで解決し転置のキーを増やさない() {
    let store = real_store();
    assert!(
        store
            .references
            .iter()
            .flatten()
            .all(|(k, _)| !k.starts_with("local ")),
        "local シンボルが転置索引に入っている"
    );

    // local への参照を持つ Document を1つ探し、その参照が同じファイル内だけで
    // 解決できることを確かめる（別ファイルの同名 local と混ざっていないか）。
    let (path, span) = store
        .doc_paths
        .iter()
        .find_map(|path| {
            let entry = store.docs.get(path)?;
            let doc = parse_document(&store.bytes[entry.index][entry.span.clone()]).ok()?;
            let content = (entry.column_encoding != scip_split::ColumnEncoding::Utf8)
                .then(|| std::fs::read(store.root.join(path)).ok())
                .flatten();
            let lines = content.as_deref().map(Lines::of);
            let occ = doc
                .occurrences
                .iter()
                .find(|o| o.symbol.starts_with("local ") && !is_definition(o.symbol_roles))?;
            let range = usable_range(&occ.range, entry.column_encoding, lines.as_ref())?;
            Some((path.clone(), range_to_span(&range)))
        })
        .expect("local への参照を持つ Document が見つからない");

    let found = store
        .references_in(&path, span)
        .expect("local への参照が解決できなかった");
    assert!(
        found.direct.iter().all(|l| l.path == path),
        "local への参照が別ファイルを指した: {:?}",
        found.direct
    );
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_参照クエリのレイテンシ() {
    let store = real_store();
    let queries: Vec<(PathBuf, Span)> = store
        .references
        .iter()
        .flatten()
        .map(|(symbol, doc_ids)| {
            let path = store.doc_paths[doc_ids[0] as usize].clone();
            let span = first_reference_span(&store, symbol, &path);
            (path, span)
        })
        .collect();
    assert!(queries.len() > 1000, "対象が少なすぎる: {}", queries.len());

    // 閾値は release で測った値。debug だと p50 が 2.4ms 出て落ちる。
    let mut elapsed: Vec<std::time::Duration> = Vec::with_capacity(queries.len());
    for (path, span) in &queries {
        let start = std::time::Instant::now();
        let _ = store.references_in(path, *span);
        elapsed.push(start.elapsed());
    }
    elapsed.sort();
    let p50 = elapsed[elapsed.len() / 2];
    let p99 = elapsed[elapsed.len() * 99 / 100];
    let max = *elapsed.last().unwrap();
    println!("p50={p50:?} p99={p99:?} max={max:?}");
    assert!(p50 < std::time::Duration::from_millis(1), "p50={p50:?}");
    assert!(p99 < std::time::Duration::from_millis(10), "p99={p99:?}");
    assert!(max < std::time::Duration::from_millis(30), "max={max:?}");
}

// 囲みクエリ

/// 入れ子の 2 つを載せた索引。行 1 は両方に囲まれている。
fn build_enclosure_fixture(root: &Path) -> PathBuf {
    use protobuf::{EnumOrUnknown, Message, MessageField};
    use scip::types::{Document, Index, Metadata, Occurrence, TextEncoding};

    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "mod outer {\n    pub fn inner() {}\n}\n",
    )
    .unwrap();

    let with_enclosing = |range: Vec<i32>, symbol: &str, enclosing: Vec<i32>| Occurrence {
        range,
        symbol: symbol.to_string(),
        symbol_roles: 1,
        enclosing_range: enclosing,
        ..Default::default()
    };
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
            ..Default::default()
        }),
        documents: vec![Document {
            relative_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            occurrences: vec![
                with_enclosing(
                    vec![0, 4, 9],
                    "scip-test cargo demo 0.1.0 outer/",
                    vec![0, 0, 2, 1],
                ),
                with_enclosing(
                    vec![1, 11, 16],
                    "scip-test cargo demo 0.1.0 outer/inner().",
                    vec![1, 4, 1, 21],
                ),
            ],
            ..Default::default()
        }],
        ..Default::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
    index_path
}

/// 内側が先。いちばん内側だけを出す呼び出し側が、並べ替えずに先頭を取れる。
#[test]
fn 囲みは内側が先に来る() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let index_path = build_enclosure_fixture(root);
    let store = load_single(&index_path, root, hashes_of(root, &["src/lib.rs"]));

    let crate::Enclosures::Exact(found) = crate::enclosures_at(&store, Path::new("src/lib.rs"), 1)
    else {
        panic!("索引が囲みを答えなかった");
    };
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].declaration.line, 1);
    assert_eq!((found[0].first_line, found[0].last_line), (1, 1));
    assert_eq!(found[1].declaration.line, 0);
    assert_eq!((found[1].first_line, found[1].last_line), (0, 2));
}

#[test]
fn 囲むものが無いことと答えられないことは違う() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let index_path = build_enclosure_fixture(root);
    let store = load_single(&index_path, root, hashes_of(root, &["src/lib.rs"]));

    assert_eq!(
        crate::enclosures_at(&store, Path::new("src/other.rs"), 0),
        crate::Enclosures::Unknown
    );

    std::fs::write(root.join("src/lib.rs"), "mod outer {}\n").unwrap();
    assert_eq!(
        crate::enclosures_at(&store, Path::new("src/lib.rs"), 1),
        crate::Enclosures::Unknown,
        "索引生成時と違うファイルに、索引の行番号を答えている"
    );
}
