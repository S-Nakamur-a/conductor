//! 索引が書いている説明 (種別・宣言・doc・所属) と、実装先の検査。
//!
//! producer によって書きぶりが違うので、2 通りの索引を組み立てる。
//! rust-analyzer の形 (種別の番号と `signature_documentation` を書き、
//! `relationships` を書かない) と、scip-typescript の形 (種別も
//! `signature_documentation` も書かず、宣言を `documentation` のコードブロックに
//! 入れ、`relationships` を書く)。

use protobuf::SpecialFields;
use protobuf::{EnumOrUnknown, Message, MessageField};
use scip::types::{
    Document, Index, Metadata, Occurrence, Relationship, Signature, SymbolInformation,
    TextEncoding, ToolInfo,
};
use sheaf_core::{
    Implementations, IndexSource, Span, Store, SymbolKind, SyntacticAnswer, SyntacticLayer, Token,
    describe_at, implementations_at,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `impl Rough for SyntacticLayer` の形と、自由関数を 1 つ持つソース。
const SOURCE: &str = "\
pub fn greet(name: &str) {}
struct Rough;
impl SyntacticLayer for Rough {}
";

const COORDINATE: &str = "rust-analyzer cargo demo 0.1.0 ";

/// scip-typescript の形の索引を組み立てるソース。ファイルを名前空間として符号に
/// 含める綴りなので、所属の組み立てもここで検査できる。
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

/// 語の切り出しだけを持つ構文層。定義も参照も答えない。
struct Words;

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

impl SyntacticLayer for Words {
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

    fn definition_at(&self, _: &Path, _: u32, _: u32) -> SyntacticAnswer {
        SyntacticAnswer::NotCode
    }

    fn references_at(&self, _: &Path, _: u32, _: u32) -> SyntacticAnswer {
        SyntacticAnswer::NotCode
    }
}

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sheaf-describe-{}-{}-{:?}",
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

/// rust-analyzer が書く形の宣言。
///
/// scip crate が生成する `Signature` は本文を 2 番に置くが、rust-analyzer が書くのは
/// 旧仕様の `Document` で本文は 5 番にある。実索引に合わせてこちらを組み立てる
/// (型付きの綴りで作ると、読めなくなっていることに検査が気づけない)。
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

/// 種別の番号と宣言を持つ SymbolInformation。番号は rust-analyzer が実際に書くもの。
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

/// 索引を 1 本書き出してルートとともに返す。`tool` が None ならツール名を名乗らない。
fn build(tag: &str, tool: Option<&str>) -> (PathBuf, PathBuf) {
    let root = workdir(tag);
    std::fs::write(root.join("src/lib.rs"), SOURCE).unwrap();

    let greet = symbol("greet().");
    let rough = symbol("Rough#");
    let impl_block = symbol("impl#[Rough][SyntacticLayer]");

    let doc = Document {
        relative_path: "src/lib.rs".to_string(),
        language: "rust".to_string(),
        occurrences: vec![
            // pub fn greet(name: &str) {}  -> greet は 7..12、name は 13..17
            occurrence(vec![0, 7, 12], &greet, 1),
            occurrence(vec![0, 13, 17], "local 0", 1),
            // struct Rough;                -> Rough は 7..12
            occurrence(vec![1, 7, 12], &rough, 1),
            // impl SyntacticLayer for Rough {} -> impl ブロックの定義は行頭から
            occurrence(vec![2, 0, 4], &impl_block, 1),
            // 同じ行の SyntacticLayer は trait への参照 -> 5..19
            occurrence(vec![2, 5, 19], &symbol("SyntacticLayer#"), 0),
        ],
        symbols: vec![
            information(
                &greet,
                17,
                "pub fn greet(name: &str)",
                &["名前を呼ぶ。", "2 行目。"],
            ),
            SymbolInformation {
                enclosing_symbol: greet.clone(),
                ..information("local 0", 37, "name: &str", &[])
            },
            information(&rough, 49, "struct Rough", &[]),
            information(&impl_block, 55, "struct Rough", &[]),
        ],
        ..Default::default()
    };
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from(TextEncoding::UTF8),
            tool_info: tool.map_or(MessageField::none(), |name| {
                MessageField::some(ToolInfo {
                    name: name.to_string(),
                    ..Default::default()
                })
            }),
            ..Default::default()
        }),
        documents: vec![doc],
        ..Default::default()
    };

    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
    (index_path, root)
}

fn load(index_path: &Path, root: &Path) -> Store {
    let bytes = std::fs::read(root.join("src/lib.rs")).unwrap();
    let expected = HashMap::from([(PathBuf::from("src/lib.rs"), sheaf_core::blob_hash(&bytes))]);
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
fn 索引が書いた宣言と種別を答える() {
    let (index_path, root) = build("signature", Some("rust-analyzer"));
    let store = load(&index_path, &root);

    let found = describe_at(&store, &Words, Path::new("src/lib.rs"), 0, 8);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, SymbolKind::Function);
    assert_eq!(
        found[0].signature.as_deref(),
        Some("pub fn greet(name: &str)")
    );
    assert_eq!(found[0].documentation, vec!["名前を呼ぶ。", "2 行目。"]);
}

#[test]
fn 番号の意味はツール名に依らない() {
    // 番号の体系は producer をまたいで同じ (rust-analyzer と scip-go の実索引で
    // 確認済み)。ツール名で表を切り替えると、名乗らない索引で全部が種別なしになる。
    let (index_path, root) = build("no-tool", None);
    let store = load(&index_path, &root);

    let found = describe_at(&store, &Words, Path::new("src/lib.rs"), 0, 8);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, SymbolKind::Function);
}

#[test]
fn ローカル束縛の所属は囲んでいる符号から出す() {
    // ローカルの符号は `local 0` で、綴りを持たない。囲んでいる符号を見ないと
    // どの関数の引数なのかがホバーから消える。
    let (index_path, root) = build("enclosing", Some("rust-analyzer"));
    let store = load(&index_path, &root);

    let found = describe_at(&store, &Words, Path::new("src/lib.rs"), 0, 14);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, SymbolKind::Parameter);
    assert_eq!(found[0].container.as_deref(), Some("greet"));
}

#[test]
fn 型を指す番号は_impl_ブロックと型別名に分かれる() {
    let (index_path, root) = build("impl-kind", Some("rust-analyzer"));
    let store = load(&index_path, &root);

    let found = describe_at(&store, &Words, Path::new("src/lib.rs"), 2, 1);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, SymbolKind::ImplBlock);
}

#[test]
fn 説明の無い符号でも符号自身は答えになる() {
    // SymbolInformation を持たない符号 (SyntacticLayer 自身の定義は別クレート)
    // でも、何を指しているかは答えられる。所属の綴りを組み立てるのに要る。
    let (index_path, root) = build("bare", Some("rust-analyzer"));
    let store = load(&index_path, &root);

    let found = describe_at(&store, &Words, Path::new("src/lib.rs"), 2, 8);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].symbol.as_str(), symbol("SyntacticLayer#"));
    assert_eq!(found[0].kind, SymbolKind::Unknown);
    assert_eq!(found[0].signature, None);
}

#[test]
fn 索引が古ければ説明を出さない() {
    let (index_path, root) = build("stale", Some("rust-analyzer"));
    let store = load(&index_path, &root);
    std::fs::write(root.join("src/lib.rs"), format!("// 追記\n{SOURCE}")).unwrap();

    let found = describe_at(&store, &Words, Path::new("src/lib.rs"), 0, 8);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn trait_の位置から実装している_impl_ブロックに届く() {
    let (index_path, root) = build("impls", Some("rust-analyzer"));
    let store = load(&index_path, &root);

    let answer = implementations_at(&store, &Words, Path::new("src/lib.rs"), 2, 8);
    let Implementations::Derived(found) = answer else {
        panic!("{answer:?}");
    };
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].ty, "Rough");
    assert_eq!(found[0].site.line, 2);
    assert_eq!(found[0].site.col, 0);
}

#[test]
fn 実装を持たない語は探した結果として_0_件を返す() {
    // 「索引が答えられない」(Unknown) と「探したが無い」(Derived が空) は別物。
    let (index_path, root) = build("none", Some("rust-analyzer"));
    let store = load(&index_path, &root);

    let answer = implementations_at(&store, &Words, Path::new("src/lib.rs"), 0, 8);
    assert_eq!(answer, Implementations::Derived(Vec::new()), "{answer:?}");
}

#[test]
fn 識別子でない位置と索引が古い位置は区別される() {
    let (index_path, root) = build("notcode", Some("rust-analyzer"));
    let store = load(&index_path, &root);

    // 行頭の空白でも記号でもよい。ここでは 1 行目の空白位置。
    let space = implementations_at(&store, &Words, Path::new("src/lib.rs"), 0, 3);
    assert_eq!(space, Implementations::NotCode, "{space:?}");

    std::fs::write(root.join("src/lib.rs"), format!("// 追記\n{SOURCE}")).unwrap();
    let stale = implementations_at(&store, &Words, Path::new("src/lib.rs"), 2, 8);
    assert_eq!(stale, Implementations::Unknown, "{stale:?}");
}

#[test]
fn 新しい綴りの宣言も読める() {
    // scip の現行仕様どおり Signature.text に本文を置く producer もあり得る。
    // 旧仕様への対応が新仕様を潰していないことを見る。
    let root = workdir("new-signature");
    std::fs::write(root.join("src/lib.rs"), SOURCE).unwrap();
    let greet = symbol("greet().");
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from(TextEncoding::UTF8),
            tool_info: MessageField::some(ToolInfo {
                name: "rust-analyzer".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }),
        documents: vec![Document {
            relative_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            occurrences: vec![occurrence(vec![0, 7, 12], &greet, 1)],
            symbols: vec![SymbolInformation {
                symbol: greet.clone(),
                kind: EnumOrUnknown::from_i32(17),
                signature_documentation: MessageField::some(Signature {
                    text: "pub fn greet(name: &str)".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };
    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
    let store = load(&index_path, &root);

    let found = describe_at(&store, &Words, Path::new("src/lib.rs"), 0, 8);
    assert_eq!(
        found[0].signature.as_deref(),
        Some("pub fn greet(name: &str)")
    );
}

// ここから先は実索引が要る。SHEAF_TEST_INDEX と SHEAF_TEST_ROOT で場所を渡す。
//   cargo test -p sheaf-core --test describe -- --ignored

fn real_store() -> (Store, PathBuf) {
    let index =
        std::env::var("SHEAF_TEST_INDEX").expect("SHEAF_TEST_INDEX に .scip のパスを渡すこと");
    let root =
        std::env::var("SHEAF_TEST_ROOT").expect("SHEAF_TEST_ROOT にソースツリーのルートを渡すこと");
    let index = PathBuf::from(index);
    let root = PathBuf::from(root);
    let expected = sheaf_core::read_provenance(
        &index.with_file_name("index.hashes"),
        &sheaf_core::RustAnalyzer,
    )
    .expect("index.hashes が要る");
    let store = Store::load(
        &[IndexSource {
            index,
            subroot: PathBuf::new(),
            expected,
        }],
        &root,
    )
    .unwrap();
    (store, root)
}

/// ソースの `line_1` 行目にある `needle` の位置で答えを引く。行がずれていたら失敗させる。
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
    let (store, root) = real_store();
    let rel = "src/hover_info.rs";

    // 定数の宣言。索引が書いた型が出る。
    let found = at(&root, rel, 13, "MAX_SIGNATURE_LINES", |p, l, c| {
        describe_at(&store, &Words, p, l, c)
    });
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
    // tree-sitter は名前でも引けず、字面には型が書かれていない位置。索引に
    // 推論結果があることがこの機能の一番の効きどころなので、実索引で押さえる。
    let (store, root) = real_store();
    let found = at(&root, "src/hover_info.rs", 89, "source", |p, l, c| {
        describe_at(&store, &Words, p, l, c)
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
    let (store, root) = real_store();
    // sheaf の SyntacticLayer を conductor 側で実装しているのは Bridge。
    let answer = at(
        &root,
        "src/semantic_index/bridge.rs",
        65,
        "SyntacticLayer",
        |p, l, c| implementations_at(&store, &Words, p, l, c),
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

    // ジェネリックな impl は索引にブロックの符号が無いので、着地はその中の
    // 最初のメソッドになる。impl の行そのものではないが、同じブロックの中に入る。
    let src = std::fs::read_to_string(root.join(&bridge.site.path)).unwrap();
    let landed = src.lines().nth(bridge.site.line as usize).unwrap();
    assert!(landed.contains("fn token_at"), "着地点が動いた: {landed}");

    // 同名の trait を 1 つも持たないので、実装の数は手書きのものと一致する。
    assert!(found.len() >= 2, "テスト側の実装も出るはず: {found:?}");
}

/// scip-typescript の形の索引を 1 本書き出す。
fn build_fenced() -> (PathBuf, PathBuf) {
    let root = workdir("fenced");
    std::fs::write(root.join("src/lib.tsx"), TS_SOURCE).unwrap();

    let greeter = ts_symbol("Greeter#");
    let loud = ts_symbol("Loud#");

    let doc = Document {
        relative_path: "src/lib.tsx".to_string(),
        occurrences: vec![
            // interface Greeter {}            -> Greeter は 10..17
            occurrence(vec![0, 10, 17], &greeter, 1),
            // class Loud implements Greeter {} -> Loud は 6..10、Greeter は 22..29
            occurrence(vec![1, 6, 10], &loud, 1),
            occurrence(vec![1, 22, 29], &greeter, 0),
        ],
        symbols: vec![
            fenced(&greeter, "interface Greeter", &["挨拶できるもの。"]),
            implements(fenced(&loud, "class Loud", &[]), &greeter),
        ],
        ..Default::default()
    };
    let index = Index {
        metadata: MessageField::some(Metadata {
            project_root: format!("file://{}", root.display()),
            text_document_encoding: EnumOrUnknown::from(TextEncoding::UTF8),
            ..Default::default()
        }),
        documents: vec![doc],
        ..Default::default()
    };

    let index_path = root.join("index.scip");
    std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
    (index_path, root)
}

fn load_ts(index_path: &Path, root: &Path) -> Store {
    let bytes = std::fs::read(root.join("src/lib.tsx")).unwrap();
    let expected = HashMap::from([(PathBuf::from("src/lib.tsx"), sheaf_core::blob_hash(&bytes))]);
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
fn 宣言をコードブロックに入れる索引からも種別と宣言を出す() {
    let (index_path, root) = build_fenced();
    let store = load_ts(&index_path, &root);

    let found = describe_at(&store, &Words, Path::new("src/lib.tsx"), 0, 12);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].signature.as_deref(), Some("interface Greeter"));
    // 番号が無いので、種別は宣言の綴りから読む。
    assert_eq!(found[0].kind, SymbolKind::Interface);
    // 宣言に使ったぶんは doc から抜く。
    assert_eq!(found[0].documentation, vec!["挨拶できるもの。"]);
}

#[test]
fn ファイルの名前空間は所属の綴りに入れない() {
    let (index_path, root) = build_fenced();
    let store = load_ts(&index_path, &root);

    let found = describe_at(&store, &Words, Path::new("src/lib.tsx"), 1, 7);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].container.as_deref(), Some("src"));
}

#[test]
fn 索引が関係を書いていれば実装先はそのまま答えになる() {
    let (index_path, root) = build_fenced();
    let store = load_ts(&index_path, &root);

    let found = implementations_at(&store, &Words, Path::new("src/lib.tsx"), 0, 12);
    let Implementations::Exact(found) = found else {
        panic!("索引の relationships から答えていない: {found:?}");
    };
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].ty, "Loud");
    assert_eq!((found[0].site.line, found[0].site.col), (1, 6));
}
