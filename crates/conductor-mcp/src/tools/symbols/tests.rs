use std::path::PathBuf;

use conductor_core::semantic_index::{Location, SymbolDetail, SymbolId};

use super::*;

fn detail(kind: SymbolKind, container: Option<&str>, signature: Option<&str>) -> SymbolDetail {
    SymbolDetail {
        symbol: SymbolId::new("test"),
        kind,
        container: container.map(str::to_string),
        signature: signature.map(str::to_string),
        documentation: vec![],
    }
}

#[allow(clippy::too_many_arguments)]
fn exact(
    name: &str,
    path: &str,
    line: u32,
    kind: SymbolKind,
    container: Option<&str>,
    signature: Option<&str>,
    body: Option<(u32, u32)>,
) -> SymbolEntry {
    SymbolEntry {
        name: name.to_string(),
        detail: detail(kind, container, signature),
        at: Placement::Exact {
            definition: Location {
                path: PathBuf::from(path),
                line,
                col: 0,
            },
            body: body.map(|(first_line, last_line)| Body {
                first_line,
                last_line,
            }),
        },
    }
}

fn stale(name: &str, path: &str, kind: SymbolKind) -> SymbolEntry {
    SymbolEntry {
        name: name.to_string(),
        detail: detail(kind, None, None),
        at: Placement::Stale {
            path: PathBuf::from(path),
        },
    }
}

fn query<'a>(text: Option<&'a str>, path: Option<&'a str>, kind: Option<KindFilter>) -> Query<'a> {
    Query {
        text,
        path,
        kind,
        limit: 20,
    }
}

#[test]
fn 完全一致前置一致部分一致の順に並ぶ() {
    let entries = [
        exact("reload", "c.rs", 0, SymbolKind::Function, None, None, None),
        exact("loader", "b.rs", 0, SymbolKind::Function, None, None, None),
        exact("load", "a.rs", 0, SymbolKind::Function, None, None, None),
    ];
    let hits = select(&entries, &query(Some("load"), None, None));
    let names: Vec<&str> = hits.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["load", "loader", "reload"]);
}

#[test]
fn 同順位はpathと行で並ぶ() {
    let entries = [
        exact("run", "b.rs", 5, SymbolKind::Function, None, None, None),
        exact("run", "a.rs", 9, SymbolKind::Function, None, None, None),
        exact("run", "a.rs", 1, SymbolKind::Function, None, None, None),
    ];
    let hits = select(&entries, &query(Some("run"), None, None));
    let at: Vec<(&str, u32)> = hits
        .iter()
        .map(|e| match &e.at {
            Placement::Exact { definition, .. } => {
                (definition.path.to_str().unwrap(), definition.line)
            }
            Placement::Stale { .. } => unreachable!(),
        })
        .collect();
    assert_eq!(at, [("a.rs", 1), ("a.rs", 9), ("b.rs", 5)]);
}

#[test]
fn 区切り付きの照合は所属を先に絞り込む() {
    let entries = [
        exact(
            "update",
            "explorer.rs",
            0,
            SymbolKind::Method,
            Some("Explorer"),
            None,
            None,
        ),
        exact(
            "update",
            "viewer.rs",
            0,
            SymbolKind::Method,
            Some("Viewer"),
            None,
            None,
        ),
    ];
    let hits = select(&entries, &query(Some("Viewer::update"), None, None));
    assert_eq!(hits.len(), 1);
    assert_eq!(entry_path(hits[0]), Path::new("viewer.rs"));
}

#[test]
fn 名前一致は所属を挟んだ完全一致だけ通す() {
    let e = exact(
        "load",
        "a.rs",
        0,
        SymbolKind::Method,
        Some("sheaf_core::store::Store"),
        None,
        None,
    );
    assert!(matches_name(&e, "load", false));
    assert!(matches_name(&e, "Store::load", false));
    assert!(!matches_name(&e, "loa", false), "部分一致は通さない");
    assert!(
        !matches_name(&e, "Other::load", false),
        "所属が違えば通さない"
    );
    assert!(!matches_name(&e, "LOAD", false), "大文字小文字は区別する");
    assert!(matches_name(&e, "LOAD", true), "case_insensitive なら通す");
}

#[test]
fn 所属の部分一致では無関係なモジュールに化けない() {
    let module_entry = exact(
        "load",
        "crates/sheaf-core/src/store/load.rs",
        0,
        SymbolKind::Module,
        Some("sheaf_core::store"),
        None,
        None,
    );
    assert!(!matches_name(&module_entry, "Store::load", false));
}

#[test]
fn kindで絞り込める() {
    let entries = [
        exact("load", "a.rs", 0, SymbolKind::Function, None, None, None),
        exact("load", "b.rs", 0, SymbolKind::Method, None, None, None),
    ];
    let filter = parse_kind("method").unwrap();
    let hits = select(&entries, &query(None, None, Some(filter)));
    assert_eq!(hits.len(), 1);
    assert_eq!(entry_path(hits[0]), Path::new("b.rs"));
}

#[test]
fn 未知のkindはエラーになる() {
    let err = parse_kind("bogus").unwrap_err();
    assert!(err.contains("bogus"));
    assert!(err.contains("function"));
}

#[test]
fn 一覧の書式は1件2行で所属は型のときだけ付く() {
    let method = exact(
        "load",
        "crates/sheaf-core/src/store.rs",
        38,
        SymbolKind::Method,
        Some("sheaf_core::store::Store"),
        Some("pub fn load(sources: &[IndexSource],\n    root: &Path) -> Result<Self>"),
        Some((38, 241)),
    );
    let function = exact(
        "note_open",
        "crates/conductor-tui/src/index.rs",
        99,
        SymbolKind::Function,
        Some("crate"),
        None,
        None,
    );
    let associated = exact(
        "new",
        "crates/sheaf-core/src/store.rs",
        300,
        SymbolKind::Function,
        Some("Store"),
        None,
        Some((300, 310)),
    );
    let hits: Vec<&SymbolEntry> = vec![&method, &function, &associated];
    let text = render_list(&hits, hits.len(), 20);
    assert_eq!(
        text,
        "3 symbols\n\
         crates/sheaf-core/src/store.rs:39  method Store::load  [39-242]\n\
         \x20 pub fn load(sources: &[IndexSource], root: &Path) -> Result<Self>\n\
         crates/conductor-tui/src/index.rs:100  fn note_open\n\
         crates/sheaf-core/src/store.rs:301  fn Store::new  [301-311]"
    );
}

#[test]
fn 一覧の見出しは総数と表示数が違うときだけ絞り込みを促す() {
    let e = exact("load", "a.rs", 0, SymbolKind::Function, None, None, None);
    let hits: Vec<&SymbolEntry> = vec![&e];

    assert_eq!(
        render_list(&hits, 57, 20).lines().next(),
        Some("57 symbols, showing 20 (narrow with kind or path)")
    );
    assert_eq!(render_list(&hits, 1, 20).lines().next(), Some("1 symbols"));
    assert_eq!(render_list(&[], 0, 20), "No symbols match.");
}

#[test]
fn 古いファイルの一覧行は行番号を持たない() {
    let e = stale("foo", "crates/x/src/lib.rs", SymbolKind::Function);
    let hits: Vec<&SymbolEntry> = vec![&e];
    let text = render_list(&hits, 1, 20);
    assert!(
        text.contains(
            "crates/x/src/lib.rs  fn foo  [stale: file changed since the index was built]"
        )
    );
}

#[test]
fn 本体は指定した行範囲をそのまま返す() {
    let e = exact(
        "foo",
        "src/lib.rs",
        2,
        SymbolKind::Function,
        None,
        None,
        Some((2, 4)),
    );
    let content = "line0\nline1\nfn foo() {\n    body\n}\nline5\n".to_string();
    let text = render_body(&e, |_| Ok(content.clone()));
    assert_eq!(text, "src/lib.rs:3-5  fn foo\nfn foo() {\n    body\n}");
}

#[test]
fn モジュールは本体を読まずファイル全体だと案内する() {
    let e = exact(
        "mymod",
        "crates/x/mod.rs",
        0,
        SymbolKind::Module,
        None,
        None,
        None,
    );
    let text = render_body(&e, |_| panic!("モジュールは読んではいけない"));
    assert_eq!(
        text,
        "mymod is a module; its definition is the whole file crates/x/mod.rs."
    );
}

#[test]
fn 親の範囲しかない本体は宣言行と案内を返す() {
    let e = exact(
        "count",
        "src/foo.rs",
        10,
        SymbolKind::Field,
        Some("Foo"),
        None,
        None,
    );
    let mut lines = vec![String::new(); 15];
    lines[10] = "    count: u32,".to_string();
    let content = lines.join("\n");
    let text = render_body(&e, |_| Ok(content.clone()));
    assert_eq!(
        text,
        "src/foo.rs:11  field Foo::count\n    count: u32,\n\
         The index only records the enclosing block for this symbol; Read src/foo.rs from line 11."
    );
}

#[test]
fn 古いファイルの本体は再構築を促す文になる() {
    let e = stale("foo", "crates/old.rs", SymbolKind::Function);
    let text = render_body(&e, |_| panic!("古いファイルは読んではいけない"));
    assert_eq!(
        text,
        "crates/old.rs changed since the index was built; line numbers are unknown. \
         Read the file, or rebuild the index (conductor: Repo ▸ Rebuild Code Index)."
    );
}

#[test]
fn 本体が500行を超えると打ち切られる() {
    let e = exact(
        "big",
        "src/big.rs",
        0,
        SymbolKind::Function,
        None,
        None,
        Some((0, 500)),
    );
    let content = (0..501)
        .map(|i| format!("L{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let text = render_body(&e, |_| Ok(content.clone()));
    assert!(text.ends_with("… truncated, 1 more lines. Read src/big.rs with offset 501."));
    assert!(text.contains("L499"));
    assert!(!text.contains("L500"));
}
