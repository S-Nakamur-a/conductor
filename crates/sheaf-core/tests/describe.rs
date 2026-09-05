//! 索引が書いている説明 (種別・宣言・doc・所属) と、実装先の検査。
//!
//! producer によって書きぶりが違うので 2 通りの索引を組み立てる。rust-analyzer の形
//! (種別の番号と `signature_documentation` を書き、`relationships` を書かない) と、
//! scip-typescript の形 (種別も `signature_documentation` も書かず、宣言を
//! `documentation` のコードブロックに入れ、`relationships` を書く)。
//!
//!   SHEAF_TEST_INDEX=<.scip> SHEAF_TEST_ROOT=<ツリー> cargo test --test describe -- --ignored

mod common;

use common::{doc, hashes_of, index, load_one, load_real_index, silent, workdir_with_src};
use protobuf::SpecialFields;
use protobuf::{EnumOrUnknown, MessageField};
use scip::types::{Relationship, Signature, SymbolInformation};
use sheaf_core::{Implementations, Store, SymbolKind, describe_at, implementations_at};
use std::path::{Path, PathBuf};

const SOURCE: &str = "\
pub fn greet(name: &str) {}
struct Rough;
impl SyntacticLayer for Rough {}
";

const COORDINATE: &str = "rust-analyzer cargo demo 0.1.0 ";

/// ファイルを名前空間として符号に含める綴りなので、所属の組み立てもここで検査できる。
const TS_SOURCE: &str = "\
interface Greeter {}
class Loud implements Greeter {}
";

const TS_COORDINATE: &str = "scip-typescript npm demo 1.0.0 src/`lib.tsx`/";

fn ts_symbol(descriptors: &str) -> String {
    format!("{TS_COORDINATE}{descriptors}")
}

fn symbol(descriptors: &str) -> String {
    format!("{COORDINATE}{descriptors}")
}

/// rust-analyzer が書く形の宣言。
///
/// scip crate が生成する `Signature` は本文を 2 番に置くが、rust-analyzer が書くのは
/// 旧仕様の `Document` で本文は 5 番にある。型付きの綴りで作ると、読めなくなっていることに
/// 検査が気づけない。
fn document_signature(text: &str) -> Signature {
    let mut fields = SpecialFields::new();
    fields
        .mut_unknown_fields()
        .add_length_delimited(5, text.as_bytes().to_vec());
    Signature {
        special_fields: fields,
        ..Default::default()
    }
}

/// 種別の番号は rust-analyzer が実際に書くもの。
fn information(symbol: &str, kind: i32, signature: &str, doc: &[&str]) -> SymbolInformation {
    SymbolInformation {
        symbol: symbol.to_string(),
        kind: EnumOrUnknown::from_i32(kind),
        signature_documentation: MessageField::some(document_signature(signature)),
        documentation: doc.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

/// 種別も宣言も書かず、宣言を doc のコードブロックに入れる形。scip-typescript がこれ。
fn fenced(symbol: &str, declaration: &str, doc: &[&str]) -> SymbolInformation {
    let mut documentation = vec![format!("```ts\n{declaration}\n```")];
    documentation.extend(doc.iter().map(|s| s.to_string()));
    SymbolInformation {
        symbol: symbol.to_string(),
        documentation,
        ..Default::default()
    }
}

fn implements(mut info: SymbolInformation, interface: &str) -> SymbolInformation {
    info.relationships.push(Relationship {
        symbol: interface.to_string(),
        is_implementation: true,
        ..Default::default()
    });
    info
}

/// `tool` が None ならツール名を名乗らない索引になる。
fn build(tag: &str, tool: Option<&str>) -> (PathBuf, PathBuf) {
    let root = workdir_with_src(tag);
    std::fs::write(root.join("src/lib.rs"), SOURCE).unwrap();

    let greet = symbol("greet().");
    let rough = symbol("Rough#");
    let impl_block = symbol("impl#[Rough][SyntacticLayer]");

    let mut builder = index().rooted_at(&root).utf8();
    if let Some(name) = tool {
        builder = builder.tool(name);
    }
    let index_path = builder
        .add(
            doc("src/lib.rs")
                .lang("rust")
                .def([0, 7, 12], &greet)
                .def([0, 13, 17], "local 0")
                .def([1, 7, 12], &rough)
                .def([2, 0, 4], &impl_block)
                .reference([2, 5, 19], &symbol("SyntacticLayer#"))
                .info(information(
                    &greet,
                    17,
                    "pub fn greet(name: &str)",
                    &["名前を呼ぶ。", "2 行目。"],
                ))
                .info(SymbolInformation {
                    enclosing_symbol: greet.clone(),
                    ..information("local 0", 37, "name: &str", &[])
                })
                .info(information(&rough, 49, "struct Rough", &[]))
                .info(information(&impl_block, 55, "struct Rough", &[])),
        )
        .write(&root.join("index.scip"));
    (index_path, root)
}

fn load(index_path: &Path, root: &Path, rel: &str) -> Store {
    load_one(index_path, root, hashes_of(root, &[rel])).unwrap()
}

#[test]
fn 索引が書いた説明を位置ごとに答える() {
    let (index_path, root) = build("signature", Some("rust-analyzer"));
    let store = load(&index_path, &root, "src/lib.rs");

    for (why, line, col, kind, signature, container, documentation) in [
        (
            "自由関数",
            0,
            8,
            SymbolKind::Function,
            Some("pub fn greet(name: &str)"),
            None,
            vec!["名前を呼ぶ。", "2 行目。"],
        ),
        // ローカルの符号は `local 0` で綴りを持たないので、囲んでいる符号を見ないと
        // どの関数の引数なのかがホバーから消える。
        (
            "ローカル束縛",
            0,
            14,
            SymbolKind::Parameter,
            Some("name: &str"),
            Some("greet"),
            vec![],
        ),
        (
            "impl ブロック",
            2,
            1,
            SymbolKind::ImplBlock,
            Some("struct Rough"),
            None,
            vec![],
        ),
        // SyntacticLayer 自身の定義は別クレートにあり SymbolInformation を持たないが、
        // 何を指しているかは答えられる。所属の綴りを組み立てるのに要る。
        (
            "説明を持たない符号",
            2,
            8,
            SymbolKind::Unknown,
            None,
            None,
            vec![],
        ),
    ] {
        let found = describe_at(&store, &silent(), Path::new("src/lib.rs"), line, col);
        assert_eq!(found.len(), 1, "{why}: {found:?}");
        assert_eq!(found[0].kind, kind, "{why}");
        assert_eq!(found[0].signature.as_deref(), signature, "{why}");
        assert_eq!(found[0].container.as_deref(), container, "{why}");
        assert_eq!(found[0].documentation, documentation, "{why}");
    }

    let bare = describe_at(&store, &silent(), Path::new("src/lib.rs"), 2, 8);
    assert_eq!(bare[0].symbol.as_str(), symbol("SyntacticLayer#"));
}

#[test]
fn 番号の意味はツール名に依らない() {
    // 番号の体系は producer をまたいで同じ (rust-analyzer と scip-go の実索引で確認済み)。
    // ツール名で表を切り替えると、名乗らない索引で全部が種別なしになる。
    let (index_path, root) = build("no-tool", None);
    let store = load(&index_path, &root, "src/lib.rs");

    let found = describe_at(&store, &silent(), Path::new("src/lib.rs"), 0, 8);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, SymbolKind::Function);
}

#[test]
fn 索引が古ければ説明を出さない() {
    let (index_path, root) = build("stale", Some("rust-analyzer"));
    let store = load(&index_path, &root, "src/lib.rs");
    std::fs::write(root.join("src/lib.rs"), format!("// 追記\n{SOURCE}")).unwrap();

    let found = describe_at(&store, &silent(), Path::new("src/lib.rs"), 0, 8);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn 新しい綴りの宣言も読める() {
    // scip の現行仕様どおり Signature.text に本文を置く producer もあり得る。
    // 旧仕様への対応が新仕様を潰していないことを見る。
    let root = workdir_with_src("new-signature");
    std::fs::write(root.join("src/lib.rs"), SOURCE).unwrap();
    let greet = symbol("greet().");
    let index_path = index()
        .rooted_at(&root)
        .utf8()
        .tool("rust-analyzer")
        .add(
            doc("src/lib.rs")
                .lang("rust")
                .def([0, 7, 12], &greet)
                .info(SymbolInformation {
                    symbol: greet.clone(),
                    kind: EnumOrUnknown::from_i32(17),
                    signature_documentation: MessageField::some(Signature {
                        text: "pub fn greet(name: &str)".to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
        )
        .write(&root.join("index.scip"));
    let store = load(&index_path, &root, "src/lib.rs");

    let found = describe_at(&store, &silent(), Path::new("src/lib.rs"), 0, 8);
    assert_eq!(
        found[0].signature.as_deref(),
        Some("pub fn greet(name: &str)")
    );
}

#[test]
fn 実装先の答えは確信度で分かれる() {
    // rust-analyzer は relationships を 1 件も書かないので、符号の綴りから導出するしかない。
    // 「索引が答えられない」(Unknown) と「探したが無い」(Derived が空) は別物。
    let (index_path, root) = build("impls", Some("rust-analyzer"));
    let store = load(&index_path, &root, "src/lib.rs");

    let at = |line, col| implementations_at(&store, &silent(), Path::new("src/lib.rs"), line, col);

    assert_eq!(at(0, 3), Implementations::NotCode, "識別子でない位置");
    assert_eq!(
        at(0, 8),
        Implementations::Derived(Vec::new()),
        "実装を持たない語"
    );

    let answer = at(2, 8);
    let Implementations::Derived(found) = answer else {
        panic!("trait の位置から実装先に届かない: {answer:?}");
    };
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].ty, "Rough");
    assert_eq!((found[0].site.line, found[0].site.col), (2, 0));

    std::fs::write(root.join("src/lib.rs"), format!("// 追記\n{SOURCE}")).unwrap();
    assert_eq!(at(2, 8), Implementations::Unknown, "索引が古い位置");
}

fn build_fenced() -> (PathBuf, PathBuf) {
    let root = workdir_with_src("fenced");
    std::fs::write(root.join("src/lib.tsx"), TS_SOURCE).unwrap();

    let greeter = ts_symbol("Greeter#");
    let loud = ts_symbol("Loud#");

    let index_path = index()
        .rooted_at(&root)
        .utf8()
        .add(
            doc("src/lib.tsx")
                .def([0, 10, 17], &greeter)
                .def([1, 6, 10], &loud)
                .reference([1, 22, 29], &greeter)
                .info(fenced(&greeter, "interface Greeter", &["挨拶できるもの。"]))
                .info(implements(fenced(&loud, "class Loud", &[]), &greeter)),
        )
        .write(&root.join("index.scip"));
    (index_path, root)
}

#[test]
fn 宣言をコードブロックに入れる索引からも種別と宣言と所属を出す() {
    let (index_path, root) = build_fenced();
    let store = load(&index_path, &root, "src/lib.tsx");

    let interface = describe_at(&store, &silent(), Path::new("src/lib.tsx"), 0, 12);
    assert_eq!(interface.len(), 1, "{interface:?}");
    assert_eq!(interface[0].signature.as_deref(), Some("interface Greeter"));
    // 番号が無いので、種別は宣言の綴りから読む。
    assert_eq!(interface[0].kind, SymbolKind::Interface);
    // 宣言に使ったぶんは doc から抜く。
    assert_eq!(interface[0].documentation, vec!["挨拶できるもの。"]);

    let class = describe_at(&store, &silent(), Path::new("src/lib.tsx"), 1, 7);
    assert_eq!(class.len(), 1, "{class:?}");
    assert_eq!(
        class[0].container.as_deref(),
        Some("src"),
        "ファイルの名前空間が所属の綴りに入っている"
    );
}

#[test]
fn 索引が関係を書いていれば実装先はそのまま答えになる() {
    let (index_path, root) = build_fenced();
    let store = load(&index_path, &root, "src/lib.tsx");

    let found = implementations_at(&store, &silent(), Path::new("src/lib.tsx"), 0, 12);
    let Implementations::Exact(found) = found else {
        panic!("索引の relationships から答えていない: {found:?}");
    };
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].ty, "Loud");
    assert_eq!((found[0].site.line, found[0].site.col), (1, 6));
}

/// `line_1` は 1 始まり。行がずれていたら失敗させる。
fn at<T>(
    root: &Path,
    rel: &str,
    line_1: usize,
    needle: &str,
    query: impl Fn(&Path, u32, u32) -> T,
) -> T {
    let src = std::fs::read_to_string(root.join(rel)).unwrap();
    let line = src.lines().nth(line_1 - 1).expect("行が無い");
    let col = line
        .find(needle)
        .unwrap_or_else(|| panic!("{rel}:{line_1} に {needle} が無い: {line}"));
    query(Path::new(rel), line_1 as u32 - 1, col as u32)
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_宣言と種別が読める() {
    let (store, root) = load_real_index();

    let found = at(
        &root,
        "src/hover_info.rs",
        13,
        "MAX_SIGNATURE_LINES",
        |p, l, c| describe_at(&store, &silent(), p, l, c),
    );
    assert!(!found.is_empty(), "索引が答えていない (索引が古い?)");
    assert_eq!(found[0].kind, SymbolKind::Constant, "{found:?}");
    assert_eq!(
        found[0].signature.as_deref(),
        Some("const MAX_SIGNATURE_LINES: usize"),
        "宣言が読めていない。Signature の綴り違いを疑うこと"
    );
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_ローカル束縛の推論後の型が出る() {
    // tree-sitter は名前でも引けず、字面には型が書かれていない位置。索引に推論結果が
    // あることがこの機能の一番の効きどころなので、実索引で押さえる。
    let (store, root) = load_real_index();
    let found = at(&root, "src/hover_info.rs", 89, "source", |p, l, c| {
        describe_at(&store, &silent(), p, l, c)
    });
    let signature = found
        .iter()
        .find_map(|d| d.signature.as_deref())
        .expect("宣言が引けない");
    assert!(
        signature.starts_with("let source: ") && signature.contains("String"),
        "推論後の型が出ていない: {signature}"
    );
}

#[test]
#[ignore = "実索引が要る"]
fn 実索引_trait_から実装先が引ける() {
    let (store, root) = load_real_index();
    let answer = at(
        &root,
        "src/semantic_index/bridge.rs",
        65,
        "SyntacticLayer",
        |p, l, c| implementations_at(&store, &silent(), p, l, c),
    );
    let Implementations::Derived(found) = &answer else {
        panic!("{answer:?}");
    };
    // 綴りはバッククォートを外した実物。型引数は落とさない (どの実装かの区別に要る)。
    let bridge = found
        .iter()
        .find(|i| i.ty == "Bridge<'a>")
        .unwrap_or_else(|| panic!("Bridge への実装が出ない: {found:?}"));
    assert_eq!(bridge.site.path, Path::new("src/semantic_index/bridge.rs"));

    // ジェネリックな impl は索引にブロックの符号が無いので、着地はその中の最初のメソッド。
    let src = std::fs::read_to_string(root.join(&bridge.site.path)).unwrap();
    let landed = src.lines().nth(bridge.site.line as usize).unwrap();
    assert!(landed.contains("fn token_at"), "着地点が動いた: {landed}");

    // 同名の trait を 1 つも持たないので、実装の数は手書きのものと一致する。
    assert!(found.len() >= 2, "テスト側の実装も出るはず: {found:?}");
}
