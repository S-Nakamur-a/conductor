//! シンボルのホバー情報。Viewer のカーソル下にあるシンボルのシグネチャ、
//! doc コメント、参照数。
//!
//! 既存の tree-sitter 製 [SymbolIndex](crate::symbol_index::SymbolIndex) の上に
//! 作ってある (language server は使わない)。インデックスが定義位置を特定し、
//! そのファイルを範囲を限って読んで宣言のシグネチャと直上の doc コメント
//! ブロックを取り出す。何も見つからなければ None を返し、呼び出し側は
//! 黙っていられる。

use crate::symbol_index::SymbolIndex;

/// シグネチャとして集める最大行数。超えたら … で切り詰める。
const MAX_SIGNATURE_LINES: usize = 8;
/// doc コメントとして集める最大行数。超えたら … で切り詰める。
const MAX_DOC_LINES: usize = 12;
/// ポップアップが参照を数えるのを諦めて「50+」と出すまでの件数。
/// フレーム内で走る経路なので作業量に上限を設ける。呼び出し側を参照。
const REF_COUNT_CAP: usize = 50;

/// 描画に使える形にしたシンボルのホバー情報。
pub struct HoverInfo {
    /// シンボル名 (ポップアップのタイトル)。
    pub symbol_name: String,
    /// 定義の種別 (例: "fn", "struct")。索引が答えていればそちら、
    /// 無ければシンボルインデックス由来。どちらも無ければ空。
    pub kind: String,
    /// 定義があるファイルのパス (リポジトリルートからの相対)。
    pub file_path: String,
    /// 1 始まりの定義行。
    pub line: usize,
    /// 定義の直上にある doc コメントの各行 (コメント記号は除去済み)。
    pub doc_lines: Vec<String>,
    /// 宣言のシグネチャの各行 (インデントを揃え、本体を開く波括弧は除去済み)。
    pub signature_lines: Vec<String>,
    /// 名前に一致した定義の数 (2 以上なら、表示しているのは複数あるうちの 1 つ)。
    pub def_count: usize,
    /// リポジトリ全体でのコード位置としての参照数。数えるのは
    /// [REF_COUNT_CAP] まで。[ref_count_capped](Self::ref_count_capped) を参照。
    pub ref_count: usize,
    /// 上限で数えるのを止めたか。true のとき実際の総数は ref_count ちょうどではなく
    /// 「それ以上」を意味する。末尾の + として描画する。
    pub ref_count_capped: bool,
    /// その語を囲んでいるものの綴り (`app::types::App`)。索引が答え、かつ
    /// 綴りを組み立てられたときだけ。名前だけではどの型のフィールドか判らない。
    pub container: Option<String>,
    /// 聞かれた位置がその定義そのものだったか。シグネチャを出しても画面に見えて
    /// いるものの写しにしかならないので、描画側はそれを省く。
    pub on_definition_line: bool,
    /// シグネチャが索引由来か。索引のシグネチャは型が解決済みで、字面とは違うものを
    /// 見せている (`let source: String` に対して字面は `let source = read(..)?`)ので、
    /// 定義行の上でも省かない。
    pub signature_from_index: bool,
}

/// 索引がその語について書いていた説明。
///
/// 組み立ては呼び出し側 ([`crate::app::App`]) の仕事で、ここは受け取るだけ。
/// ホバーの組み立てを意味索引に依存させないため。
#[derive(Default)]
pub struct IndexedDetail {
    /// 種別のラベル ("fn", "struct")。読めなければ空。
    pub kind: String,
    /// 索引が書いた宣言。行に割ってある。
    pub signature_lines: Vec<String>,
    /// 索引が持っている doc コメント。
    pub doc_lines: Vec<String>,
}

impl IndexedDetail {
    fn is_empty(&self) -> bool {
        self.kind.is_empty() && self.signature_lines.is_empty() && self.doc_lines.is_empty()
    }
}

/// ホバーが説明する定義の位置。意味索引が位置で答えたものでも、
/// シンボルインデックスを名前で引いたものでも、ここから先の扱いは同じ。
pub struct DefSite {
    pub file_path: String,
    /// 1 始まり。
    pub line: usize,
    /// 定義の種別 ("Struct" など)。位置しか分からないときは空。
    pub kind: String,
    /// 候補の総数。2 以上なら、表示しているのはそのうちの 1 つ。
    pub def_count: usize,
    /// 索引がその語について書いていた説明。無ければソースから読み取る。
    pub detail: Option<IndexedDetail>,
}

/// シンボルインデックスを名前で引いて定義位置を決める。複数箇所で定義されて
/// いる名前については current_file 内の定義を優先する。
pub fn resolve_def_site(
    index: &SymbolIndex,
    symbol: &str,
    current_file: Option<&str>,
) -> Option<DefSite> {
    if !index.is_available() {
        return None;
    }
    let defs = index.find_definitions(symbol);
    let def = defs
        .iter()
        .find(|d| Some(d.file_path.as_str()) == current_file)
        .or_else(|| defs.first())?;
    Some(DefSite {
        file_path: def.file_path.clone(),
        line: def.line,
        kind: format!("{:?}", def.kind),
        def_count: defs.len(),
        detail: None,
    })
}

/// 定義位置からホバー情報を組み立てる。ファイルが読めない、行がファイルの外、
/// のいずれかなら (黙って) None を返す。
///
/// 索引が説明を持っていればそちらを使う。索引の宣言は producer が型を解決した
/// もので、定義行を読み直して作る写しより中身が濃い (`let source: String` に対して
/// 字面は `let source = std::fs::read_to_string(..)?`)。doc だけは索引が持たない
/// ことが多い (doc コメントのある項目に限られる) ので、無ければソースから拾う。
pub fn build_hover_info(index: &SymbolIndex, symbol: &str, def: DefSite) -> Option<HoverInfo> {
    let root = index.root();
    let source = std::fs::read_to_string(root.join(&def.file_path)).ok()?;
    let lines: Vec<&str> = source.lines().collect();
    let def_idx = def.line.checked_sub(1)?;
    if def_idx >= lines.len() {
        return None;
    }
    let indexed = def.detail.filter(|d| !d.is_empty());
    let signature_from_index = indexed
        .as_ref()
        .is_some_and(|d| !d.signature_lines.is_empty());
    let kind = match indexed.as_ref().map(|d| d.kind.as_str()) {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => def.kind,
    };
    let doc_lines = match indexed.as_ref().filter(|d| !d.doc_lines.is_empty()) {
        Some(d) => d.doc_lines.clone(),
        None => extract_doc_comment(&lines, def_idx),
    };
    let signature_lines = match indexed.filter(|d| !d.signature_lines.is_empty()) {
        Some(d) => d.signature_lines,
        None => extract_signature(&lines, def_idx),
    };

    // 正確な数ではなく上限付き。これはポインタがシンボル上で止まるたびに UI
    // スレッドで走るが、ありふれた名前の正確な数を出すにはその名前が現れる全ファイルを
    // パースすることになる (ここでの new は約 157ms = 10 フレーム落ち)。
    // 上限を超えたぶんはポップアップに「50+」と出す。
    let (ref_count, ref_count_capped) = index.count_references_upto(symbol, &root, REF_COUNT_CAP);

    Some(HoverInfo {
        symbol_name: symbol.to_string(),
        kind,
        file_path: def.file_path,
        line: def.line,
        doc_lines,
        signature_lines,
        def_count: def.def_count,
        ref_count,
        ref_count_capped,
        on_definition_line: false,
        container: None,
        signature_from_index,
    })
}

/// def_idx (0 始まり) から始まる宣言のシグネチャを取り出す。{ で終わる最初の行
/// (波括弧は除去) まで、または ; / = で終わる宣言までを含め、上限は
/// [MAX_SIGNATURE_LINES]。各行は最初の行のインデントぶんだけ左へ詰める。
fn extract_signature(lines: &[&str], def_idx: usize) -> Vec<String> {
    let indent = lines[def_idx].len() - lines[def_idx].trim_start().len();
    let mut out = Vec::new();
    let mut depth = 0i32;
    for raw in lines.iter().skip(def_idx).take(MAX_SIGNATURE_LINES) {
        let dedented = if raw.len() >= indent && raw[..indent.min(raw.len())].trim().is_empty() {
            &raw[indent..]
        } else {
            raw.trim_start()
        };
        let trimmed_end = dedented.trim_end();
        if let Some(stripped) = trimmed_end.strip_suffix('{') {
            let s = stripped.trim_end();
            if !s.is_empty() {
                out.push(s.to_string());
            }
            return out;
        }
        out.push(trimmed_end.to_string());
        if trimmed_end.ends_with(';') {
            return out;
        }
        depth += bracket_delta(trimmed_end);
        // 構造体のフィールドや enum の要素は , で終わる。囲みの外側にいるときだけ
        // 区切りとして扱う — 引数を複数行に割った関数の途中で切ってしまわないように。
        if depth <= 0 && trimmed_end.ends_with(',') {
            return out;
        }
    }
    if lines.len() > def_idx + MAX_SIGNATURE_LINES {
        out.push("…".to_string());
    }
    out
}

/// 行が開いた括弧の数から閉じた括弧の数を引いたもの。角括弧は数えない
/// (Option<String> のような型と大小比較を字面で見分けられないため)。
fn bracket_delta(line: &str) -> i32 {
    line.chars().fold(0, |acc, c| match c {
        '(' | '[' | '{' => acc + 1,
        ')' | ']' | '}' => acc - 1,
        _ => acc,
    })
}

/// def_idx (0 始まり) の直上にあるコメントブロックを集める。属性・デコレータの行
/// (#[...], @...) は読み飛ばす。///, //!, // (Rust/Go) と
/// /** ... */ (TS/JS) の形式に対応し、記号は除去する。上限は [MAX_DOC_LINES]
/// (概要を持つ先頭側を残す)。
fn extract_doc_comment(lines: &[&str], def_idx: usize) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut i = def_idx;
    let mut in_block = false; // 下から上へ走査中の /* ... */ の内側かどうか
    while i > 0 {
        i -= 1;
        let t = lines[i].trim();
        if in_block {
            let body = t
                .trim_start_matches("/**")
                .trim_start_matches("/*")
                .trim_start_matches('*')
                .trim();
            collected.push(body.to_string());
            if t.starts_with("/*") {
                break;
            }
            continue;
        }
        // doc ブロックと対象アイテムのあいだにある属性・デコレータは飛ばす。
        if collected.is_empty() && (t.starts_with("#[") || t.starts_with('@') || t == "]") {
            continue;
        }
        if t.ends_with("*/") && !t.starts_with("//") {
            let body = t.trim_end_matches("*/").trim_end();
            // 同じ行に /* の開始があるなら、そのブロックは 1 行だったということ。
            in_block = !body.starts_with("/*");
            let body = body
                .trim_start_matches("/**")
                .trim_start_matches("/*")
                .trim_start_matches('*')
                .trim();
            collected.push(body.to_string());
            if !in_block {
                break;
            }
        } else if let Some(rest) = t
            .strip_prefix("///")
            .or_else(|| t.strip_prefix("//!"))
            .or_else(|| t.strip_prefix("//"))
        {
            collected.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else {
            break;
        }
    }
    collected.reverse();
    // ブロックコメントの区切りが残した先頭・末尾の空行を落とす。
    while collected.first().is_some_and(|l| l.is_empty()) {
        collected.remove(0);
    }
    while collected.last().is_some_and(|l| l.is_empty()) {
        collected.pop();
    }
    if collected.len() > MAX_DOC_LINES {
        collected.truncate(MAX_DOC_LINES);
        collected.push("…".to_string());
    }
    collected
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn sig(src: &str, def_line_1: usize) -> Vec<String> {
        let lines: Vec<&str> = src.lines().collect();
        extract_signature(&lines, def_line_1 - 1)
    }

    fn doc(src: &str, def_line_1: usize) -> Vec<String> {
        let lines: Vec<&str> = src.lines().collect();
        extract_doc_comment(&lines, def_line_1 - 1)
    }

    /// 索引が説明を持っていたことにして、その位置のホバーを組み立てる。
    fn with_index(dir: &Path, symbol: &str, line: usize, detail: IndexedDetail) -> HoverInfo {
        let index = SymbolIndex::new(dir.to_path_buf());
        index.build().unwrap();
        build_hover_info(
            &index,
            symbol,
            DefSite {
                file_path: "lib.rs".to_string(),
                line,
                kind: String::new(),
                def_count: 1,
                detail: Some(detail),
            },
        )
        .expect("hover info")
    }

    fn scratch(tag: &str, src: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hover_idx_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.rs"), src).unwrap();
        dir
    }

    #[test]
    fn 索引の宣言は字面の写しより優先される() {
        // 索引は型を解決済みで、字面には型が書かれていない。ここを字面から
        // 読み直すと、この機能でいちばん効く位置が一番貧しくなる。
        let dir = scratch("prefer", "fn caller() {\n    let total = 1 + 2;\n}\n");
        let info = with_index(
            &dir,
            "total",
            2,
            IndexedDetail {
                kind: "let".to_string(),
                signature_lines: vec!["let total: i32".to_string()],
                doc_lines: Vec::new(),
            },
        );
        assert_eq!(info.signature_lines, vec!["let total: i32"]);
        assert_eq!(info.kind, "let");
        assert!(info.signature_from_index);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 索引が_doc_を持たなければソースから拾う() {
        // 索引の documentation は doc コメントのある項目にしか付かない。
        // 種別と宣言だけ索引から採り、doc は今までどおり読み取る。
        let dir = scratch("doc", "/// 足し算。\npub fn add(a: i64) -> i64 { a }\n");
        let info = with_index(
            &dir,
            "add",
            2,
            IndexedDetail {
                kind: "fn".to_string(),
                signature_lines: vec!["pub fn add(a: i64) -> i64".to_string()],
                doc_lines: Vec::new(),
            },
        );
        assert_eq!(info.doc_lines, vec!["足し算。"]);
        assert_eq!(info.signature_lines, vec!["pub fn add(a: i64) -> i64"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 索引が答えなければ従来どおり字面から組み立てる() {
        let dir = scratch(
            "fallback",
            "/// 足し算。\npub fn add(a: i64) -> i64 {\n    a\n}\n",
        );
        let index = SymbolIndex::new(dir.clone());
        index.build().unwrap();
        let site = resolve_def_site(&index, "add", Some("lib.rs")).expect("定義位置");
        let info = build_hover_info(&index, "add", site).expect("hover info");
        assert_eq!(info.signature_lines, vec!["pub fn add(a: i64) -> i64"]);
        assert_eq!(info.kind, "Function");
        assert!(!info.signature_from_index);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signature_single_line_fn() {
        let src = "pub fn foo(a: usize) -> bool {\n    true\n}\n";
        assert_eq!(sig(src, 1), vec!["pub fn foo(a: usize) -> bool"]);
    }

    #[test]
    fn signature_multi_line_fn() {
        let src = "fn foo(\n    a: usize,\n    b: &str,\n) -> bool {\n    true\n}\n";
        assert_eq!(
            sig(src, 1),
            vec!["fn foo(", "    a: usize,", "    b: &str,", ") -> bool"]
        );
    }

    #[test]
    fn signature_dedents_indented_method() {
        let src = "impl Foo {\n    pub fn bar(&self) -> usize {\n        1\n    }\n}\n";
        assert_eq!(sig(src, 2), vec!["pub fn bar(&self) -> usize"]);
    }

    #[test]
    fn signature_stops_at_semicolon() {
        let src = "type Alias = Vec<String>;\nfn next() {}\n";
        assert_eq!(sig(src, 1), vec!["type Alias = Vec<String>;"]);
    }

    #[test]
    fn doc_rust_triple_slash_with_attribute() {
        let src = "/// Does the thing.\n/// Second line.\n#[derive(Debug)]\npub struct Foo;\n";
        assert_eq!(doc(src, 4), vec!["Does the thing.", "Second line."]);
    }

    #[test]
    fn doc_go_double_slash() {
        let src = "// Foo does the thing.\nfunc Foo() {}\n";
        assert_eq!(doc(src, 2), vec!["Foo does the thing."]);
    }

    #[test]
    fn doc_ts_block_comment() {
        let src = "/**\n * Does the thing.\n * @param a input\n */\nfunction foo(a) {}\n";
        assert_eq!(doc(src, 5), vec!["Does the thing.", "@param a input"]);
    }

    #[test]
    fn doc_none_when_code_above() {
        let src = "let x = 1;\nfn foo() {}\n";
        assert!(doc(src, 2).is_empty());
    }

    #[test]
    fn doc_single_line_block_comment() {
        let src = "/** Does the thing. */\nfunction foo() {}\n";
        assert_eq!(doc(src, 2), vec!["Does the thing."]);
    }

    #[test]
    fn 構造体のフィールドは次のアイテムまで飲み込まない() {
        // 索引がフィールドに答えるようになるまで、ホバーがこの位置に来ることは
        // なかった。, を区切りにしていなかったので、上限の 8 行ぶん次のアイテムを
        // そのまま読み込んでいた。
        let src = "\
pub struct A {
    pub session_name: Option<String>,
}

/// 次のアイテム。
pub struct B {
}
";
        assert_eq!(sig(src, 2), vec!["pub session_name: Option<String>,"]);
    }

    #[test]
    fn 複数行に割った引数は途中で切らない() {
        // 行末の , は囲みの中では区切りにならない。
        let src = "\
pub fn add(
    a: i64,
    b: i64,
) -> i64 {
    a + b
}
";
        assert_eq!(
            sig(src, 1),
            vec!["pub fn add(", "    a: i64,", "    b: i64,", ") -> i64"]
        );
    }

    #[test]
    fn tree_sitter_が定義を知らない位置でも意味索引の位置から組み立てられる() {
        // ホバーが黙る一番多い理由は、tree-sitter が名前で定義を引けないこと
        // (ローカル束縛、フィールド、モジュール名)。意味索引が位置で答えた
        // ときは、その名前の定義を知らなくても中身を出せなければならない。
        let dir = std::env::temp_dir().join(format!("hover_pos_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = "\
fn caller() {
    /// 途中の束縛。
    let total = 1 + 2;
    let _ = total;
}
";
        std::fs::write(dir.join("lib.rs"), src).unwrap();
        let index = SymbolIndex::new(dir.clone());
        index.build().unwrap();

        // 前提: 名前では引けない。
        assert!(
            resolve_def_site(&index, "total", Some("lib.rs")).is_none(),
            "この検査の前提が崩れている: tree-sitter が total の定義を持っている"
        );

        let info = build_hover_info(
            &index,
            "total",
            DefSite {
                file_path: "lib.rs".to_string(),
                line: 3,
                kind: String::new(),
                def_count: 1,
                detail: None,
            },
        )
        .expect("位置から組み立てられる");
        assert_eq!(info.signature_lines, vec!["let total = 1 + 2;"]);
        assert_eq!(info.doc_lines, vec!["途中の束縛。"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn end_to_end_over_real_index() {
        // 一時リポジトリに対して本物の tree-sitter インデックスを作り、
        // 経路全体 (find_definitions → 読み取り → 抽出) を通してホバー情報を解決する。
        let dir = std::env::temp_dir().join(format!("hover_e2e_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = "\
/// Adds two numbers together.
/// Returns their sum.
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

fn caller() {
    let _ = add(1, 2);
}
";
        std::fs::write(dir.join("lib.rs"), src).unwrap();

        let index = SymbolIndex::new(dir.clone());
        index.build().unwrap();

        let site = resolve_def_site(&index, "add", Some("lib.rs")).expect("定義位置");
        let info = build_hover_info(&index, "add", site).expect("hover info");
        assert_eq!(info.symbol_name, "add");
        assert_eq!(info.kind, "Function");
        assert_eq!(info.file_path, "lib.rs");
        assert_eq!(info.line, 3);
        assert_eq!(
            info.doc_lines,
            vec!["Adds two numbers together.", "Returns their sum."]
        );
        assert_eq!(
            info.signature_lines,
            vec!["pub fn add(a: i64, b: i64) -> i64"]
        );
        // "add" は定義箇所と呼び出し箇所の 2 つに現れる。
        assert!(info.ref_count >= 2, "ref_count = {}", info.ref_count);

        // 定義の無い名前は何も返さない (黙る)。
        assert!(resolve_def_site(&index, "nonexistent_symbol", None).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
