use std::path::Path;

use super::code_mask::MAX_TRACKED_PER_LINE;
use super::*;

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
