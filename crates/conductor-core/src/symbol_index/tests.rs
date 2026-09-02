use std::path::{Path, PathBuf};

use super::code_mask::MAX_TRACKED_PER_LINE;
use super::extract::extract;
use super::language::Grammar;
use super::*;

fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, body) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    dir
}

fn sym(name: &str, kind: SymbolKind, file: &str) -> Symbol {
    Symbol {
        name: name.to_string(),
        kind,
        scope: Scope::Global,
        file_path: file.to_string(),
        line: 1,
        parent: None,
    }
}

/// 構築を通さずに中身を差し込んだ索引。
fn seeded(symbols: Vec<Symbol>) -> SymbolIndex {
    let idx = SymbolIndex::new(PathBuf::from("/nonexistent"));
    idx.publish(symbols, idx.generation());
    idx
}

fn extracted(grammar: Grammar, source: &str) -> Vec<Symbol> {
    let tree = grammar.parse(source).unwrap();
    let mut symbols = Vec::new();
    extract(grammar, tree.root_node(), source, "test", &mut symbols);
    symbols
}

fn defined(idx: &SymbolIndex, name: &str, from: &str) -> Vec<String> {
    idx.find_definitions(name, Path::new(from))
        .into_iter()
        .map(|s| s.file_path)
        .collect()
}

#[test]
fn 作りたての索引は構築するまで使えない() {
    let idx = SymbolIndex::new(PathBuf::from("/tmp"));
    assert!(!idx.is_available());
    assert_eq!(idx.root(), PathBuf::from("/tmp"));
    assert!(idx.find_definitions("foo", Path::new("")).is_empty());
}

#[test]
fn 宣言の種類を言語ごとに拾う() {
    use SymbolKind::*;
    let rust = "\
pub fn hello_world() {}
struct MyStruct { field_a: u32 }
enum Color { Red, Blue }
trait Drawable { fn draw(&self); }
impl Drawable for MyStruct { fn draw(&self) {} }
type Alias = Vec<u32>;
const MAX_SIZE: usize = 100;
static GLOBAL: &str = \"x\";
mod submodule;
macro_rules! my_macro { () => {}; }
";
    let go = "\
package main
func Hello() {}
func (s *Server) Serve() {}
type Server struct{}
type Handler interface{}
type ID int
const Max = 1
var Global = 2
";
    let ts = "\
function hello() {}
class Widget { render() {} }
interface Props {}
type Alias = string;
enum Color { Red }
const answer = 42;
";
    type Expected = &'static [(usize, &'static str, SymbolKind)];
    let cases: [(Grammar, &str, Expected); 3] = [
        (
            Grammar::Rust,
            rust,
            &[
                (1, "hello_world", Function),
                (2, "MyStruct", Struct),
                (2, "field_a", Field),
                (3, "Color", Enum),
                (3, "Red", EnumVariant),
                (3, "Blue", EnumVariant),
                (4, "Drawable", Trait),
                (4, "draw", Function),
                (5, "impl MyStruct", Impl),
                (5, "draw", Function),
                (6, "Alias", Type),
                (7, "MAX_SIZE", Const),
                (8, "GLOBAL", Static),
                (9, "submodule", Module),
                (10, "my_macro", Macro),
            ],
        ),
        (
            Grammar::Go,
            go,
            &[
                (2, "Hello", Function),
                (3, "Serve", Method),
                (4, "Server", Struct),
                (5, "Handler", Interface),
                (6, "ID", Type),
                (7, "Max", Const),
                (8, "Global", Static),
            ],
        ),
        (
            Grammar::TypeScript,
            ts,
            &[
                (1, "hello", Function),
                (2, "Widget", Struct),
                (2, "render", Method),
                (3, "Props", Interface),
                (4, "Alias", Type),
                (5, "Color", Enum),
                (6, "answer", Const),
            ],
        ),
    ];
    for (grammar, source, expected) in cases {
        let symbols = extracted(grammar, source);
        let got: Vec<(usize, &str, SymbolKind)> = symbols
            .iter()
            .map(|s| (s.line, s.name.as_str(), s.kind))
            .collect();
        assert_eq!(got, expected, "{grammar:?}");
    }

    let rust_symbols = extracted(Grammar::Rust, rust);
    let imp = rust_symbols.iter().find(|s| s.kind == Impl).unwrap();
    assert_eq!(imp.parent.as_deref(), Some("MyStruct"));
}

/// 名前でしか引けないので、載せると別のファイルの同名のローカルが答えになる
/// (.tsx の data が無関係な .ts の const data を引き当てたのがこれ)。
#[test]
fn ローカル束縛は定義の候補にしない() {
    let dir = tree(&[
        (
            "lib/helper.rs",
            "pub const SHARED_RS: u32 = 1;\n\npub fn load() -> u32 {\n    const HIDDEN_RS: u32 = 2;\n    static ALSO_RS: u32 = 3;\n    HIDDEN_RS + ALSO_RS\n}\n",
        ),
        (
            "lib/helper.go",
            "package helper\n\nconst SharedGo = 1\n\nvar (\n\tGroupedGo = 2\n)\n\nfunc load() int {\n\tconst hiddenGo = 3\n\tvar alsoGo = 4\n\treturn hiddenGo + alsoGo\n}\n",
        ),
        (
            "lib/helper.ts",
            "export const sharedTs = 1;\n\nfor (const topLoopTs of [1]) {\n    console.log(topLoopTs);\n}\n\nexport function load() {\n    const hiddenTs = 2;\n    return hiddenTs;\n}\n",
        ),
    ]);
    let idx = SymbolIndex::new(dir.path().to_path_buf());
    idx.build();

    let cases: [(&str, &[&str], &[&str]); 3] = [
        ("main.rs", &["HIDDEN_RS", "ALSO_RS"], &["SHARED_RS"]),
        (
            "main.go",
            &["hiddenGo", "alsoGo"],
            &["SharedGo", "GroupedGo"],
        ),
        ("Page.tsx", &["hiddenTs", "topLoopTs"], &["sharedTs"]),
    ];
    for (from, hidden, visible) in cases {
        for name in hidden {
            assert!(
                defined(&idx, name, from).is_empty(),
                "{from} から見て {name} が定義候補に残っている"
            );
        }
        for name in visible {
            assert_eq!(
                defined(&idx, name, from).len(),
                1,
                "{from} から見て {name} が定義候補から消えている"
            );
        }
    }
}

#[test]
fn フィールドと列挙子は定義として提示しない() {
    let idx = seeded(vec![
        sym("Foo", SymbolKind::Struct, "lib.rs"),
        sym("Foo", SymbolKind::Field, "lib.rs"),
        sym("Foo", SymbolKind::EnumVariant, "lib.rs"),
    ]);
    let defs = idx.find_definitions("Foo", Path::new(""));
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].kind, SymbolKind::Struct);
}

#[test]
fn 実装検索はimplのシンボルに当たる() {
    let idx = seeded(vec![Symbol {
        parent: Some("MyStruct".to_string()),
        ..sym("impl MyStruct", SymbolKind::Impl, "lib.rs")
    }]);
    assert_eq!(idx.find_implementations("MyStruct").len(), 1);
    assert!(idx.find_implementations("Other").is_empty());
}

/// Go の rollbar が TypeScript の const rollbar に当たり、ホバーがその宣言を
/// 答えとして出したのが元の症状。
#[test]
fn 定義の解決は言語を跨がない() {
    let idx = seeded(vec![
        sym("rollbar", SymbolKind::Const, "web/client.ts"),
        sym("rollbar", SymbolKind::Static, "api/log.go"),
    ]);
    let cases: [(&str, &[&str]); 4] = [
        ("api/handler.go", &["api/log.go"]),
        ("web/Page.tsx", &["web/client.ts"]),
        ("", &["web/client.ts", "api/log.go"]),
        ("README.md", &["web/client.ts", "api/log.go"]),
    ];
    for (from, expected) in cases {
        assert_eq!(defined(&idx, "rollbar", from), expected, "from {from}");
    }
}

#[test]
fn 拡張子の分類() {
    let cases = [
        ("a.rs", Some(Language::Rust)),
        ("a.go", Some(Language::Go)),
        ("a.ts", Some(Language::TypeScript)),
        ("a.tsx", Some(Language::TypeScript)),
        ("a.mjs", Some(Language::TypeScript)),
        ("README.md", None),
        ("Makefile", None),
    ];
    for (path, expected) in cases {
        assert_eq!(Language::of_path(Path::new(path)), expected, "{path}");
        let ext = Path::new(path).extension().and_then(|e| e.to_str());
        assert_eq!(
            ext.and_then(language_for_ext).is_some(),
            expected.is_some(),
            "{path} の文法と言語の対応"
        );
    }
    assert!(same_language(Path::new("a.jsx"), Path::new("b.ts")));
    assert!(!same_language(Path::new("a.go"), Path::new("b.ts")));
    assert!(same_language(Path::new("a.go"), Path::new("notes.txt")));
}

#[test]
fn 参照検索はコードでない拡張子を飛ばす() {
    let dir = tree(&[
        ("notes.md", "widget appears here\n"),
        ("Cargo.toml", "widget = \"1.0\"\n"),
        ("config.yaml", "widget: true\n"),
        ("data.json", "{\"widget\": 1}\n"),
        ("lib.rs", "fn widget() {}\n"),
    ]);
    let idx = SymbolIndex::new(dir.path().to_path_buf());
    let refs = idx.find_references("widget", dir.path());
    let files: Vec<&str> = refs.iter().map(|r| r.file_path.as_str()).collect();
    assert_eq!(files, ["lib.rs"]);
}

#[test]
fn 参照検索はコメントと文字列の一致を飛ばす() {
    let dir = tree(&[(
        "lib.rs",
        "// widget does things\nfn real() {\n    let s = \"widget\";\n    widget();\n}\n",
    )]);
    let idx = SymbolIndex::new(dir.path().to_path_buf());
    let refs = idx.find_references("widget", dir.path());
    let lines: Vec<usize> = refs.iter().map(|r| r.line).collect();
    assert_eq!(lines, [4]);
}

/// 全部捨てると「判定できなかった」を「参照は存在しない」と答えることになる。
#[test]
fn パースできない言語の一致は捨てない() {
    let dir = tree(&[
        ("used.py", "def wrapper():\n    return target_name()\n"),
        ("used.rs", "pub fn caller() { target_name(); }\n"),
    ]);
    let idx = SymbolIndex::new(dir.path().to_path_buf());
    let refs = idx.find_references("target_name", dir.path());
    let mut files: Vec<&str> = refs.iter().map(|r| r.file_path.as_str()).collect();
    files.sort();
    assert_eq!(files, ["used.py", "used.rs"]);
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// あえて new を使う。このリポジトリでは約 200 ファイルに言及があり、上限なしだと
/// 約 157ms でフレームを 10 枚落とす。上限が作業量を制限する仕組みなので、
/// 計測するのは上限付きの呼び出しで、最も負荷の高い名前で行う。
#[test]
fn ホバーの参照数はフレームの予算に収まる() {
    let root = workspace_root();
    let idx = SymbolIndex::new(root.clone());
    idx.count_references_upto("new", &root, 50);

    let start = std::time::Instant::now();
    let (count, capped) = idx.count_references_upto("new", &root, 50);
    let elapsed = start.elapsed();

    assert!(count > 0);
    assert!(capped, "new は 50 件を超えるはず");
    assert!(
        elapsed < std::time::Duration::from_millis(30),
        "{elapsed:?} かかった。上限なしの実測は約 157ms"
    );
}

/// 上限なしの検索はユーザ起動なので 1 フレームより長くてよいが、
/// 訪れたファイル全部を解析する形に退化してはならない (その実測は約 121ms)。
#[test]
fn パースは名前が当たったファイルまで遅らせる() {
    let root = workspace_root();
    let idx = SymbolIndex::new(root.clone());
    idx.find_references("count_references_upto", &root);

    let start = std::time::Instant::now();
    let refs = idx.find_references("count_references_upto", &root);
    let elapsed = start.elapsed();

    assert!(!refs.is_empty());
    assert!(
        elapsed < std::time::Duration::from_millis(80),
        "{elapsed:?} かかった"
    );
}

#[test]
fn 根の付け替えは索引の答える対象を差し替える() {
    let a = tree(&[("a.rs", "pub fn only_in_a() {}\n")]);
    let b = tree(&[("b.rs", "pub fn only_in_b() {}\n")]);

    let idx = SymbolIndex::new(a.path().to_path_buf());
    idx.build();
    assert_eq!(defined(&idx, "only_in_a", ""), ["a.rs"]);
    assert!(defined(&idx, "only_in_b", "").is_empty());

    idx.set_root(b.path().to_path_buf());
    assert!(
        !idx.is_available(),
        "再構築が済むまでは何も知らないと答える"
    );
    assert!(defined(&idx, "only_in_a", "").is_empty());

    idx.build();
    assert_eq!(defined(&idx, "only_in_b", ""), ["b.rs"]);
    assert!(defined(&idx, "only_in_a", "").is_empty());
}

/// ファイル保存のたびにその場で再構築するので、同じ root で吹き飛ばすと
/// 保存のたびに「未準備」にばたつく。
#[test]
fn 同じパスへの付け替えは何もしない() {
    let a = tree(&[("a.rs", "pub fn keep() {}\n")]);
    let idx = SymbolIndex::new(a.path().to_path_buf());
    idx.build();

    idx.set_root(a.path().to_path_buf());
    assert!(idx.is_available());
    assert_eq!(defined(&idx, "keep", ""), ["a.rs"]);
}

/// ワーカーは止められず、聞き手がいなくても結果を書きに来る。遅い構築と
/// 付け替えを実際に競わせる代わりに「刻む → root を動かす → 完了する」の
/// 3 手で publish のガードを検証する。
#[test]
fn 付け替え前に始まったビルドは捨てる() {
    let old = tree(&[("old.rs", "pub fn from_old_tree() {}\n")]);
    let new = tree(&[("new.rs", "pub fn from_new_tree() {}\n")]);

    let idx = SymbolIndex::new(old.path().to_path_buf());
    let stamped = idx.generation();
    idx.set_root(new.path().to_path_buf());
    let published = idx.publish(
        vec![sym("from_old_tree", SymbolKind::Function, "old.rs")],
        stamped,
    );

    assert_eq!(published, 0);
    assert!(defined(&idx, "from_old_tree", "").is_empty());
    assert!(
        !idx.is_available(),
        "捨てた構築が索引を準備済みにしてはならない"
    );

    idx.build();
    assert_eq!(defined(&idx, "from_new_tree", ""), ["new.rs"]);
}

/// 1 行分の (綴り, コードか) を並べる。
fn row(mask: &CodeMask, source: &str, line_1: usize) -> Vec<(&'static str, bool)> {
    let line = source.lines().nth(line_1 - 1).unwrap();
    identifier_occurrences(line)
        .enumerate()
        .map(|(k, (_, _, text))| (leak(text), mask.is_code(line_1, k)))
        .collect()
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// 期待値は実装から導出せず、フィクスチャから手で数えたもの。
#[test]
fn マスクは地の文だけを隠す() {
    let rust = "\
// comment mentions Foo
fn real(x: i32) -> Foo {
    let s = \"Foo in string\";
    let c = 'x';
    bar(Foo)
}
";
    let go =
        "package main\n// Foo does things\nfunc Bar() {\n\ts := \"Foo\"\n\tr := `Foo raw`\n}\n";
    let ts = "// Foo comment\nconst t = `text ${realCode} more`;\nconst s = \"Foo\";\n";
    let format = "\
fn f(widget: u32) {
    let s = format!(\"{widget} and {}\", widget);
    println!(\"{widget:?} plus {count:>3} prose\");
    let raw = format!(r#\"{widget}\"#);
    let escaped = format!(\"{{widget}} literal\");
    let positional = format!(\"{0} {} text\", widget);
}
";
    type Rows = &'static [(usize, &'static [(&'static str, bool)])];
    let cases: [(&str, &str, &str, Rows); 4] = [
        (
            "コメント・文字列・文字リテラル。キーワードはコードのまま (除くのは呼び出し側)",
            "lib.rs",
            rust,
            &[
                (
                    1,
                    &[("comment", false), ("mentions", false), ("Foo", false)],
                ),
                (
                    2,
                    &[
                        ("fn", true),
                        ("real", true),
                        ("x", true),
                        ("i32", true),
                        ("Foo", true),
                    ],
                ),
                (
                    3,
                    &[
                        ("let", true),
                        ("s", true),
                        ("Foo", false),
                        ("in", false),
                        ("string", false),
                    ],
                ),
                (4, &[("let", true), ("c", true), ("x", false)]),
                (5, &[("bar", true), ("Foo", true)]),
            ],
        ),
        (
            "Go はコメントと 2 種類の文字列 (文法上は別ノード)",
            "main.go",
            go,
            &[
                (2, &[("Foo", false), ("does", false), ("things", false)]),
                (3, &[("func", true), ("Bar", true)]),
                (4, &[("s", true), ("Foo", false)]),
                (5, &[("r", true), ("Foo", false), ("raw", false)]),
            ],
        ),
        (
            "テンプレートリテラルの補間はコードのまま",
            "a.ts",
            ts,
            &[
                (1, &[("Foo", false), ("comment", false)]),
                (
                    2,
                    &[
                        ("const", true),
                        ("t", true),
                        ("text", false),
                        ("realCode", true),
                        ("more", false),
                    ],
                ),
                (3, &[("const", true), ("s", true), ("Foo", false)]),
            ],
        ),
        (
            "format 捕捉は束縛を名指ししている。{{ と {} と {0} は違う。r 接頭辞は構文",
            "lib.rs",
            format,
            &[
                (
                    2,
                    &[
                        ("let", true),
                        ("s", true),
                        ("format", true),
                        ("widget", true),
                        ("and", false),
                        ("widget", true),
                    ],
                ),
                (
                    3,
                    &[
                        ("println", true),
                        ("widget", true),
                        ("plus", false),
                        ("count", true),
                        ("prose", false),
                    ],
                ),
                (
                    4,
                    &[
                        ("let", true),
                        ("raw", true),
                        ("format", true),
                        ("r", false),
                        ("widget", true),
                    ],
                ),
                (
                    5,
                    &[
                        ("let", true),
                        ("escaped", true),
                        ("format", true),
                        ("widget", false),
                        ("literal", false),
                    ],
                ),
                (
                    6,
                    &[
                        ("let", true),
                        ("positional", true),
                        ("format", true),
                        ("text", false),
                        ("widget", true),
                    ],
                ),
            ],
        ),
    ];
    for (label, path, src, rows) in cases {
        let mask = CodeMask::compute(src, path);
        assert!(mask.is_supported(), "{label}");
        for (line, expected) in rows {
            assert_eq!(row(&mask, src, *line), *expected, "{label}: 行 {line}");
        }
    }
}

#[test]
fn 複数行のブロックコメントは全行をマスクする() {
    let src = "fn a() {}\n/* Foo\n   Bar\n   Baz */\nfn b() {}\n";
    let mask = CodeMask::compute(src, "lib.rs");
    assert!(mask.is_code(1, 0));
    for line in 2..=4 {
        assert!(
            row(&mask, src, line).iter().all(|(_, code)| !*code),
            "行 {line} は丸ごとマスクされるはず"
        );
    }
    assert!(mask.is_code(5, 0));
}

/// Go は慣習としてタブでインデントされるので、特殊ではなく一般のケース。
#[test]
fn 出現番号はタブ展開を生き延びる() {
    let src = "package main\nfunc f() {\n\tx := \"Foo\"\n}\n";
    let mask = CodeMask::compute(src, "main.go");

    let raw = src.lines().nth(2).unwrap();
    let expanded = raw.replace('\t', "    ");
    assert_ne!(raw, expanded, "フィクスチャにタブが要る");

    assert!(mask.is_code_at_column(&expanded, 3, expanded.find('x').unwrap()));
    assert!(!mask.is_code_at_column(&expanded, 3, expanded.find("Foo").unwrap()));
}

#[test]
fn 対応しない言語は何も提示しない() {
    let mask = CodeMask::compute("def build(x):\n    return x\n", "script.py");
    assert!(!mask.is_supported());
    assert!(!mask.is_code(1, 0));
    assert!(!mask.is_code(2, 0));
}

#[test]
fn 範囲外の問い合わせはコードではない() {
    let mask = CodeMask::compute("fn a() {}\n", "lib.rs");
    assert!(!mask.is_code(0, 0), "行番号は 1 始まり");
    assert!(!mask.is_code(99, 0), "ファイルの末尾を越えている");
    assert!(!mask.is_code(1, MAX_TRACKED_PER_LINE), "上限を越えている");
}
