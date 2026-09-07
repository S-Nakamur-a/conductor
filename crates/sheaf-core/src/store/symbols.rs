//! 索引が定義を書いているシンボルの列挙。位置クエリと違い、聞かれた 1 点ではなく
//! 索引全体を走査する。

use super::column::{Lines, location_of, usable_range};
use super::{DocEntry, descriptor_name, enclosed_lines, is_definition, is_local, parse_document};
use super::{Store, scip_split};
use crate::{Body, Placement, SymbolDetail, SymbolEntry, SymbolId, SymbolKind};
use scip::types::{Document, Occurrence};
use std::collections::HashMap;
use std::path::Path;

impl Store {
    /// 索引が定義を書いているシンボルを全部返す。パス、行の順。
    ///
    /// 全 Document を 1 回デコードする (実索引 275 Document で約 40ms) ので、
    /// 呼ぶ側が 1 回だけ呼んで持つ。
    pub fn symbols(&self) -> Vec<SymbolEntry> {
        let mut out: Vec<SymbolEntry> = self
            .docs
            .iter()
            .filter_map(|(rel, entry)| {
                let doc = parse_document(&self.bytes[entry.index][entry.span.clone()]).ok()?;
                Some(self.symbols_in_document(rel, entry, &doc))
            })
            .flatten()
            .collect();
        out.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
        out
    }

    fn symbols_in_document(
        &self,
        rel: &Path,
        entry: &DocEntry,
        doc: &Document,
    ) -> Vec<SymbolEntry> {
        let current = self.is_current(rel);
        let overlap = if current {
            overlap_counts(doc)
        } else {
            HashMap::new()
        };
        let content = (current && entry.column_encoding != scip_split::ColumnEncoding::Utf8)
            .then(|| std::fs::read(self.root.join(rel)))
            .transpose()
            .ok()
            .flatten();
        let lines = content.as_deref().map(Lines::of);

        doc.occurrences
            .iter()
            .filter(|occ| !is_local(&occ.symbol) && is_definition(occ.symbol_roles))
            .filter_map(|occ| {
                let name = display_name(&occ.symbol)?;
                let detail = doc
                    .symbols
                    .iter()
                    .find(|info| info.symbol == occ.symbol)
                    .map(|info| self.to_detail(info, rel))
                    .unwrap_or_else(|| empty_detail(&occ.symbol));
                let at = if current {
                    let range = usable_range(&occ.range, entry.column_encoding, lines.as_ref())?;
                    let definition = location_of(&range, rel)?;
                    Placement::Exact {
                        definition,
                        body: body_of(occ, detail.kind, &overlap),
                    }
                } else {
                    Placement::Stale {
                        path: rel.to_path_buf(),
                    }
                };
                Some(SymbolEntry {
                    name: name.to_string(),
                    detail,
                    at,
                })
            })
            .collect()
    }
}

/// `(先頭行, 末尾行) -> 件数`。定義 occurrence が enclosing_range を共有していれば、
/// その範囲の持ち主は 1 つに決まらない (struct と field、impl とその中のメソッド)。
fn overlap_counts(doc: &Document) -> HashMap<(u32, u32), usize> {
    let mut counts = HashMap::new();
    for occ in &doc.occurrences {
        if is_local(&occ.symbol) || !is_definition(occ.symbol_roles) {
            continue;
        }
        if let Some(range) = enclosed_lines(&occ.enclosing_range) {
            *counts.entry(range).or_insert(0usize) += 1;
        }
    }
    counts
}

/// Module/Package はファイル全体を指すので持たせない。範囲を他の定義と共有していて、
/// 自分が struct/enum/trait/class/interface のように持ち主になり得ないときも同様。
fn body_of(
    occ: &Occurrence,
    kind: SymbolKind,
    overlap: &HashMap<(u32, u32), usize>,
) -> Option<Body> {
    if matches!(kind, SymbolKind::Module | SymbolKind::Package) {
        return None;
    }
    let (first_line, last_line) = enclosed_lines(&occ.enclosing_range)?;
    let shared = overlap.get(&(first_line, last_line)).copied().unwrap_or(0) > 1;
    let owns_shared_range = matches!(
        kind,
        SymbolKind::Struct
            | SymbolKind::Enum
            | SymbolKind::Trait
            | SymbolKind::Class
            | SymbolKind::Interface
    );
    (!shared || owns_shared_range).then_some(Body {
        first_line,
        last_line,
    })
}

/// 符号の末尾 descriptor から表示名を取る。`impl` ブロックや、名前を組み立てられない
/// 符号は None (そのシンボルは載せない)。
fn display_name(symbol: &str) -> Option<&str> {
    let descriptors = symbol.split(' ').nth(4)?;
    if descriptors.is_empty() || descriptors.ends_with(']') {
        return None;
    }
    let trimmed = strip_trailing_marker(descriptors);
    let after_separator = trimmed
        .rfind(['/', '#', '.'])
        .map_or(trimmed, |i| &trimmed[i + 1..]);
    // 素の impl のメソッドは `impl#[Store]load` のように、型の角括弧が名前の前に
    // 融着している。角括弧そのものは名前ではないので、最後の `]` より後ろを使う。
    let after_separator = match after_separator.rfind(']') {
        Some(i) if after_separator.starts_with('[') => &after_separator[i + 1..],
        _ => after_separator,
    };
    let name = after_separator.split('(').next().unwrap_or(after_separator);
    let name = descriptor_name(name);
    (!name.is_empty()).then_some(name)
}

fn strip_trailing_marker(descriptors: &str) -> &str {
    ["().", ".", "#", "/", "!"]
        .into_iter()
        .find_map(|suffix| descriptors.strip_suffix(suffix))
        .unwrap_or(descriptors)
}

fn empty_detail(symbol: &str) -> SymbolDetail {
    SymbolDetail {
        symbol: SymbolId(symbol.into()),
        kind: SymbolKind::Unknown,
        container: None,
        signature: None,
        documentation: Vec::new(),
    }
}

fn sort_key(entry: &SymbolEntry) -> (&Path, u32) {
    match &entry.at {
        Placement::Exact { definition, .. } => (definition.path.as_path(), definition.line),
        Placement::Stale { path } => (path.as_path(), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::load_single;
    use super::*;
    use protobuf::{EnumOrUnknown, Message, MessageField};
    use scip::types::symbol_information::Kind as ScipKind;
    use scip::types::{Index, Metadata, Signature, SymbolInformation, TextEncoding};
    use std::path::PathBuf;

    fn hashes_of(root: &Path, rels: &[&str]) -> HashMap<PathBuf, String> {
        rels.iter()
            .map(|rel| {
                let bytes = std::fs::read(root.join(rel)).unwrap();
                (PathBuf::from(rel), crate::blob_hash(&bytes))
            })
            .collect()
    }

    fn info(symbol: &str, kind: ScipKind, enclosing: &str) -> SymbolInformation {
        SymbolInformation {
            symbol: symbol.to_string(),
            kind: EnumOrUnknown::new(kind),
            enclosing_symbol: enclosing.to_string(),
            ..Default::default()
        }
    }

    fn occ(range: Vec<i32>, symbol: &str, roles: i32, enclosing_range: Vec<i32>) -> Occurrence {
        Occurrence {
            range,
            symbol: symbol.to_string(),
            symbol_roles: roles,
            enclosing_range,
            ..Default::default()
        }
    }

    /// 定義 (roles=1) と参照 (roles=0)、ローカル束縛を混ぜた索引を 1 本書く。
    fn write_index(root: &Path, documents: Vec<Document>) -> PathBuf {
        let index = Index {
            metadata: MessageField::some(Metadata {
                project_root: format!("file://{}", root.display()),
                text_document_encoding: EnumOrUnknown::from_i32(TextEncoding::UTF8 as i32),
                ..Default::default()
            }),
            documents,
            ..Default::default()
        };
        let path = root.join("index.scip");
        std::fs::write(&path, index.write_to_bytes().unwrap()).unwrap();
        path
    }

    #[test]
    fn 定義だけが載り参照とローカル束縛は載らない() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn greet() {}\ngreet();\n").unwrap();

        let doc = Document {
            relative_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            occurrences: vec![
                occ(
                    vec![0, 7, 12],
                    "scip-test cargo demo 0.1.0 greet().",
                    1,
                    vec![],
                ),
                occ(
                    vec![1, 0, 5],
                    "scip-test cargo demo 0.1.0 greet().",
                    0,
                    vec![],
                ),
                occ(vec![1, 0, 5], "local 0", 1, vec![]),
            ],
            symbols: vec![info(
                "scip-test cargo demo 0.1.0 greet().",
                ScipKind::Function,
                "",
            )],
            ..Default::default()
        };
        let index_path = write_index(root, vec![doc]);
        let store = load_single(&index_path, root, hashes_of(root, &["src/lib.rs"]));

        let symbols = store.symbols();
        assert_eq!(symbols.len(), 1, "{symbols:?}");
        assert_eq!(symbols[0].name, "greet");
        assert_eq!(symbols[0].detail.kind, SymbolKind::Function);
        let Placement::Exact { definition, .. } = &symbols[0].at else {
            panic!("Exact になっていない: {:?}", symbols[0].at);
        };
        assert_eq!(definition.line, 0);
        assert_eq!(definition.col, 7);
    }

    #[test]
    fn enclosing_rangeが本体の範囲になる() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "/// doc\npub fn greet() {}\n").unwrap();

        let doc = Document {
            relative_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            occurrences: vec![occ(
                vec![1, 7, 12],
                "scip-test cargo demo 0.1.0 greet().",
                1,
                vec![0, 0, 1, 18],
            )],
            symbols: vec![info(
                "scip-test cargo demo 0.1.0 greet().",
                ScipKind::Function,
                "",
            )],
            ..Default::default()
        };
        let index_path = write_index(root, vec![doc]);
        let store = load_single(&index_path, root, hashes_of(root, &["src/lib.rs"]));

        let symbols = store.symbols();
        assert_eq!(symbols.len(), 1);
        let Placement::Exact { body, .. } = &symbols[0].at else {
            panic!("Exact になっていない");
        };
        assert_eq!(
            *body,
            Some(Body {
                first_line: 0,
                last_line: 1
            })
        );
    }

    #[test]
    fn structとfieldが同じ範囲ならstructだけが本体を持つ() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub struct Loud {\n    pub volume: i32,\n}\n",
        )
        .unwrap();

        let shared = vec![0, 0, 2, 1];
        let doc = Document {
            relative_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            occurrences: vec![
                occ(
                    vec![0, 11, 15],
                    "scip-test cargo demo 0.1.0 Loud#",
                    1,
                    shared.clone(),
                ),
                occ(
                    vec![1, 8, 14],
                    "scip-test cargo demo 0.1.0 Loud#volume.",
                    1,
                    shared,
                ),
            ],
            symbols: vec![
                info("scip-test cargo demo 0.1.0 Loud#", ScipKind::Struct, ""),
                info(
                    "scip-test cargo demo 0.1.0 Loud#volume.",
                    ScipKind::Field,
                    "scip-test cargo demo 0.1.0 Loud#",
                ),
            ],
            ..Default::default()
        };
        let index_path = write_index(root, vec![doc]);
        let store = load_single(&index_path, root, hashes_of(root, &["src/lib.rs"]));

        let mut symbols = store.symbols();
        symbols.sort_by_key(|s| s.name.clone());
        let field = symbols.iter().find(|s| s.name == "volume").unwrap();
        let strukt = symbols.iter().find(|s| s.name == "Loud").unwrap();

        let Placement::Exact { body, .. } = &field.at else {
            panic!()
        };
        assert_eq!(*body, None, "field は Body を持たない");
        let Placement::Exact { body, .. } = &strukt.at else {
            panic!()
        };
        assert_eq!(
            *body,
            Some(Body {
                first_line: 0,
                last_line: 2
            }),
            "struct は Body を持つ"
        );
    }

    #[test]
    fn implの中のメソッド2件が同じ範囲なら両方とも本体を持たない() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "impl Loud {\n    fn a(&self) {}\n    fn b(&self) {}\n}\n",
        )
        .unwrap();

        let shared = vec![0, 0, 3, 1];
        let doc = Document {
            relative_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            occurrences: vec![
                occ(
                    vec![1, 7, 8],
                    "scip-test cargo demo 0.1.0 Loud#a().",
                    1,
                    shared.clone(),
                ),
                occ(
                    vec![2, 7, 8],
                    "scip-test cargo demo 0.1.0 Loud#b().",
                    1,
                    shared,
                ),
            ],
            symbols: vec![
                info(
                    "scip-test cargo demo 0.1.0 Loud#a().",
                    ScipKind::Method,
                    "scip-test cargo demo 0.1.0 Loud#",
                ),
                info(
                    "scip-test cargo demo 0.1.0 Loud#b().",
                    ScipKind::Method,
                    "scip-test cargo demo 0.1.0 Loud#",
                ),
            ],
            ..Default::default()
        };
        let index_path = write_index(root, vec![doc]);
        let store = load_single(&index_path, root, hashes_of(root, &["src/lib.rs"]));

        for symbol in store.symbols() {
            let Placement::Exact { body, .. } = &symbol.at else {
                panic!()
            };
            assert_eq!(*body, None, "{}: メソッドは Body を持たない", symbol.name);
        }
    }

    #[test]
    fn module符号は本体を持たない() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "mod outer {}\n").unwrap();

        let doc = Document {
            relative_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            occurrences: vec![occ(
                vec![0, 4, 9],
                "scip-test cargo demo 0.1.0 outer/",
                1,
                vec![0, 0, 0, 13],
            )],
            symbols: vec![info(
                "scip-test cargo demo 0.1.0 outer/",
                ScipKind::Module,
                "",
            )],
            ..Default::default()
        };
        let index_path = write_index(root, vec![doc]);
        let store = load_single(&index_path, root, hashes_of(root, &["src/lib.rs"]));

        let symbols = store.symbols();
        assert_eq!(symbols.len(), 1);
        let Placement::Exact { body, .. } = &symbols[0].at else {
            panic!()
        };
        assert_eq!(*body, None);
    }

    #[test]
    fn 定義のあるファイルを書き換えると古い扱いになる() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn greet() {}\n").unwrap();

        let doc = Document {
            relative_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            occurrences: vec![occ(
                vec![0, 7, 12],
                "scip-test cargo demo 0.1.0 greet().",
                1,
                vec![],
            )],
            symbols: vec![info(
                "scip-test cargo demo 0.1.0 greet().",
                ScipKind::Function,
                "",
            )],
            ..Default::default()
        };
        let index_path = write_index(root, vec![doc]);
        let store = load_single(&index_path, root, hashes_of(root, &["src/lib.rs"]));

        std::fs::write(root.join("src/lib.rs"), "// 変わった\npub fn greet() {}\n").unwrap();

        let symbols = store.symbols();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "greet");
        assert_eq!(
            symbols[0].at,
            Placement::Stale {
                path: PathBuf::from("src/lib.rs")
            }
        );
    }

    #[test]
    fn 表示名の抽出() {
        let build = |descriptors: &str| format!("scip-test cargo demo 0.1.0 {descriptors}");
        for (descriptors, want) in [
            ("a/b/load().", Some("load")),
            ("store/Store#", Some("Store")),
            ("store/Store#load().", Some("load")),
            ("x/Foo#bar.", Some("bar")),
            ("ui/`PanelChrome<'a>`#draw().", Some("draw")),
            ("macros/log!", Some("log")),
            ("impl#[A][B]", None),
            ("demo/", Some("demo")),
            ("x/foo(1).", Some("foo")),
            ("store/load/impl#[Store]load().", Some("load")),
            ("app/focus/impl#[Focus][Eq]eq().", Some("eq")),
        ] {
            let symbol = build(descriptors);
            assert_eq!(display_name(&symbol), want, "{descriptors:?}");
        }
    }

    #[test]
    fn signature_documentationのフィールド不一致に対応する() {
        // rust-analyzer は SymbolInformation.signature_documentation に
        // 型付きでは読めない Document (旧仕様) を書く。to_detail 経由の
        // シグネチャ抽出がこの経路でも効くことを確かめる。
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn greet() {}\n").unwrap();

        let mut information = info(
            "scip-test cargo demo 0.1.0 greet().",
            ScipKind::Function,
            "",
        );
        information.signature_documentation = MessageField::some(Signature {
            text: "pub fn greet()".to_string(),
            ..Default::default()
        });

        let doc = Document {
            relative_path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            occurrences: vec![occ(
                vec![0, 7, 12],
                "scip-test cargo demo 0.1.0 greet().",
                1,
                vec![],
            )],
            symbols: vec![information],
            ..Default::default()
        };
        let index_path = write_index(root, vec![doc]);
        let store = load_single(&index_path, root, hashes_of(root, &["src/lib.rs"]));

        let symbols = store.symbols();
        assert_eq!(
            symbols[0].detail.signature.as_deref(),
            Some("pub fn greet()")
        );
    }
}
