//! 行上のどの識別子の出現が *コード* の位置にあるか — コメントや文字列
//! リテラルの中にあるのとは対照的に。
//!
//! シンボルインデックスは定義がどこにあるかを知っているが、画面上の何かを
//! 起点とするクエリ（gd、ホバーポップアップ、Cmd+Click、g プレフィックスの
//! シンボルヒント）はすべて、まず別の問いに答えなければならない:
//! *ユーザが指している単語は実際にコードなのか?* これがないと、doc
//! コメント内の英単語が同名のシンボルに解決されてしまい、UI は意味のない
//! ジャンプを提示することになる。
//!
//! # バイト範囲ではなく出現インデックスを使う理由
//!
//! viewer はタブを展開した行を保持しており（ViewerState::open_file は
//! すべての行を expand_tabs に通す）、そのためソース中のバイトオフセットは
//! 画面上のカラムと一致しない — しかも対象言語の1つである Go は慣習として
//! タブインデントされる。タブの展開は \t をスペースに置き換えるので、
//! カラムは変わるが識別子の並び替えや書き換えは決して起きない。だから
//! 「この行のk番目の識別子」は展開を生き延び、このマスクはそれをキーに
//! している。両側が識別子とみなすものについて一致していなければならず、
//! そのため [identifier_occurrences] がその唯一の定義となっており、
//! マスクの構築 *と* 問い合わせの両方に使われる。
//!
//! # allowlist にする理由
//!
//! このマスクは、マスクされて除外された出現ではなく *コードである* 出現を
//! 記録する。すべてが正しく動いている限り両者は等価だが、何かが壊れた
//! ときに違いが出る。allowlist が空ということは何もジャンプできない
//! ということであり、機能は黙って何もしなくなる。blocklist が空だと
//! すべてがジャンプ可能になるということであり、これはまさにこのモジュール
//! が修正しようとしているバグそのものであって、しかも黙って再発する。
//! 「ジャンプを提示しない」方向に倒れて失敗するのが、ユーザにとって最も
//! コストの低い方向である: 起きないジャンプは何も起きなかったのと同じだが、
//! 間違った場所へのジャンプは読んでいたものを押しのけてしまう。
//!
//! この選択は、文法を持たない言語のファイルも同時に片付ける: それらには
//! マスクが与えられないので、その中では何もジャンプできない。

use std::sync::OnceLock;

use regex::Regex;

/// 1行あたりこのインデックスを超える識別子の出現は、コードではないものとして
/// 扱う。実際のコードはこれに近づくことすらない — このリポジトリで最も
/// 幅の広い行でも識別子は76個 — 固定の上限を設けない代わりとなる溢れ表現は、
/// 何にも使われないためだけに存在する構造になってしまう。
const MAX_TRACKED_PER_LINE: usize = 128;

/// マスクの構築とマスクの問い合わせで共有される、「識別子の出現」の唯一の定義。
///
/// 実装を1つにまとめておくのは整理整頓の好みの問題ではない: マスクは
/// この並びの中の位置をキーにしているので、もし両側が識別子とみなすものに
/// ついて一致しなくなれば、インデックスは黙ってずれてしまい、マスクは
/// 間違った単語について答えることになる。
///
/// 各一致について (start_byte, end_byte, text) を順に生成する。
pub fn identifier_occurrences(line: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").unwrap());
    re.find_iter(line).map(|m| (m.start(), m.end(), m.as_str()))
}

/// 各行ごとに、どの識別子の出現がコード位置にあるかを記録する。
///
/// 行番号に対して密である（ファイルの1行につき1エントリ）。ちょうど1つの
/// 開いているファイルを記述するものだからで、コメントだけの行で無駄になる
/// 分は、マップの間接参照よりも安く済む。
#[derive(Debug, Default, Clone)]
pub struct CodeMask {
    /// lines[i] は、行 i + 1 のk番目の識別子がコードであるときビットkが
    /// 立つ。ベクタの末尾を超える行は何も持たない。
    lines: Vec<u128>,
    /// このマスクがそもそも答えを持っているか、それとも答えが無いのかを表す。
    /// 文法を持たない言語では false になる。
    supported: bool,
}

impl CodeMask {
    /// このファイルの言語をそもそも解析できたかどうか。
    ///
    /// 「コードである識別子が1つもない」とは区別する価値がある。両者は
    /// 扱いが正反対であり、ビットだけを見ても両者を区別できないからである。
    /// ジャンプを提示することは1つの単語についての主張なので、沈黙が安全な
    /// 答えであり、解析できないファイルは何も提示すべきではない。参照の
    /// 一覧表示はリポジトリ全体についての主張であり、そこでは「結果なし」は
    /// 沈黙ではない — それは「存在しない」と主張することになる。検索に答える
    /// 呼び出し側は、ここでは自分が保証できない空の結果を報告するのではなく、
    /// フィルタなしの一致にフォールバックすべきである。
    pub fn is_supported(&self) -> bool {
        self.supported
    }

    /// line_1（1始まり）の occurrence 番目（0始まり）の識別子がコード位置に
    /// あるかどうか。
    ///
    /// このマスクが知らないものはすべて false と答える。これが allowlist を
    /// 静かに失敗させる仕組みになっている: 範囲外の行、[MAX_TRACKED_PER_LINE]
    /// を超える出現、パースできなかったファイル、これらすべてがここに行き着く。
    pub fn is_code(&self, line_1: usize, occurrence: usize) -> bool {
        if line_1 == 0 || occurrence >= MAX_TRACKED_PER_LINE {
            return false;
        }
        match self.lines.get(line_1 - 1) {
            Some(bits) => bits & (1u128 << occurrence) != 0,
            None => false,
        }
    }

    /// line_1 上で col をカバーする識別子がコード位置にあるかどうか。col は
    /// *描画された* 行へのバイトオフセットである。
    ///
    /// マスクを構築したのと同じスキャンを使って col を出現インデックスに
    /// 変換するので、タブ展開（カラムはずらすが順序は変えない）は問題に
    /// ならない。
    pub fn is_code_at_column(&self, rendered_line: &str, line_1: usize, col: usize) -> bool {
        for (k, (start, end, _)) in identifier_occurrences(rendered_line).enumerate() {
            if col >= start && col < end {
                return self.is_code(line_1, k);
            }
        }
        false
    }

    /// path の拡張子で振り分けて source のマスクを構築する。
    ///
    /// 文法を持たない言語や、tree-sitter がパースを拒否したファイルに
    /// 対しては、空のマスク（何もコードではない）を返す。
    pub fn compute(source: &str, path: &str) -> Self {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let Some(grammar) = grammar_for(ext) else {
            return Self::default();
        };

        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&grammar.language).is_err() {
            return Self::default();
        }
        let Some(tree) = parser.parse(source, None) else {
            return Self::default();
        };

        Self::from_masked_ranges(source, &collect_masked_ranges(&tree, source, &grammar))
    }

    /// masked の外側にある識別子の出現すべてにビットを立てる。
    ///
    /// masked は開始オフセットでソートされていて重複しないことが前提。
    /// マスクされたノードで潜らずに止まる pre-order 走査はこの条件を満たす
    /// 結果を生成する。
    fn from_masked_ranges(source: &str, masked: &[(usize, usize)]) -> Self {
        let mut lines = Vec::new();
        let mut line_start = 0usize;
        // 現在の行をまだカバーし得る最初の範囲のインデックス。範囲も行も
        // 単調に進むので、これは決して巻き戻らない。
        let mut cursor = 0usize;

        for line in source.split_inclusive('\n') {
            let mut bits: u128 = 0;
            let trimmed = line.trim_end_matches(['\n', '\r']);

            while cursor < masked.len() && masked[cursor].1 <= line_start {
                cursor += 1;
            }

            for (k, (start, _, _)) in identifier_occurrences(trimmed).enumerate() {
                if k >= MAX_TRACKED_PER_LINE {
                    break;
                }
                let abs = line_start + start;
                // ブロックコメントや複数行文字列は多くの行にまたがり得るので、
                // cursor を消費せずに前方へスキャンする — この同じ行の後の
                // 識別子が、同じ範囲より前に位置することがあり得るため。
                let inside = masked[cursor..]
                    .iter()
                    .take_while(|(s, _)| *s <= abs)
                    .any(|(s, e)| abs >= *s && abs < *e);
                if !inside {
                    bits |= 1u128 << k;
                }
            }

            lines.push(bits);
            line_start += line.len();
        }

        Self {
            lines,
            supported: true,
        }
    }
}

/// Rust のインライン format 引数として捕捉された識別子を、マスクされた範囲
/// から改めて掘り出す。
///
/// format!("{widget:?}") は実在する束縛を名指ししており、ここにある他の
/// すべての言語での相当物はナビゲート可能なままである — TypeScript の
/// ${...} は文法上そもそも独立したノードとして現れる。tree-sitter-rust は
/// format 文字列を分割しないので、リテラル全体が1つの string_content として
/// 渡ってきてしまい、その中の識別子は地の文と一緒にマスクされてしまう。
/// 2021年頃のコードベースにおいてこれは特殊なケースではない: このリポジトリ
/// には159ファイルにわたり945件もそのような参照がある。
///
/// 1つのマスクされた範囲を受け取り、{ident} / {ident:spec} の各識別子を
/// 除外した後もマスクされたままになる部分範囲を返す。{} と {0} は識別子を
/// 保持していないのでそのままにし、エスケープされた波括弧である {{ も
/// 同様にそのままにする。
fn subtract_format_args(source: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let text = &source[start..end];
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut cut = 0usize; // 現在積み上げているマスク区間の開始位置
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        // {{ はリテラルの波括弧をエスケープしている — ここに引数はない。
        if bytes.get(i + 1) == Some(&b'{') {
            i += 2;
            continue;
        }
        let name_start = i + 1;
        let mut j = name_start;
        if bytes.get(j).is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_') {
            j += 1;
            while bytes
                .get(j)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                j += 1;
            }
            // } または : format spec で終わる名前だけが捕捉であり、それ以外は
            // たまたま波括弧の後に続いた地の文である。
            if matches!(bytes.get(j), Some(b'}') | Some(b':')) {
                if name_start > cut {
                    out.push((start + cut, start + name_start));
                }
                cut = j;
                i = j;
                continue;
            }
        }
        i = name_start;
    }

    if cut < bytes.len() {
        out.push((start + cut, end));
    }
    out
}

/// 文法ごとに、内容がコードではなく地の文やリテラルテキストであるノード
/// 種別。名前からの推測ではなく、各文法の実際の出力に対して検証済み —
/// 下にある TypeScript についての注記を参照。
/// ある文法のマスク対象ノード種別と、その中でナビゲート可能なままで
/// なければならないインライン format 捕捉をテキストに含み得る部分集合
/// （[subtract_format_args] を参照）。
struct Grammar {
    language: tree_sitter::Language,
    masked: &'static [&'static str],
    format_capable: &'static [&'static str],
}

fn grammar_for(ext: &str) -> Option<Grammar> {
    const RUST: &[&str] = &[
        "line_comment",
        "block_comment",
        "doc_comment",
        "string_content",
        "raw_string_literal",
        "char_literal",
    ];
    // どちらの文字列形式も format! に届くので、format!(r#"{x}"#) も対象。
    // コメントは対象外: 地の文の中の {x} は何も名指ししていない。
    const RUST_FORMAT: &[&str] = &["string_content", "raw_string_literal"];
    const GO: &[&str] = &[
        "comment",
        "interpreted_string_literal_content",
        "raw_string_literal_content",
    ];
    // Go の %v verb は識別子を持たず、TypeScript の補間はすでに独立した
    // ノードになっているので、どちらも Rust 用の掘り出し処理は必要ない。
    const NONE: &[&str] = &[];
    // string_fragment — template_string ではないのは意図的。テンプレート
    // リテラルは ['', string_fragment, template_substitution,
    // string_fragment, ''] としてパースされるので、ノード全体をマスクすると
    // 補間された式まで飲み込んでしまう。これらは普通のコードなので
    // ジャンプ可能なままであるべき。fragment 単位でマスクすることで、普通の
    // 文字列とテンプレートのリテラル部分をカバーしつつ ${...} には手を
    // つけずに済む。
    const TS: &[&str] = &["comment", "string_fragment"];

    let (language, masked, format_capable) = match ext {
        "rs" => (tree_sitter_rust::LANGUAGE.into(), RUST, RUST_FORMAT),
        "go" => (tree_sitter_go::LANGUAGE.into(), GO, NONE),
        "ts" | "js" => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            TS,
            NONE,
        ),
        "tsx" | "jsx" => (tree_sitter_typescript::LANGUAGE_TSX.into(), TS, NONE),
        _ => return None,
    };
    Some(Grammar {
        language,
        masked,
        format_capable,
    })
}

/// pre-order 走査でマスク対象ノードのバイト範囲を集める。一度一致したら
/// その中には潜らない。範囲はソート済みで重複しない形で出てくる。
fn collect_masked_ranges(
    tree: &tree_sitter::Tree,
    source: &str,
    grammar: &Grammar,
) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut cursor = tree.walk();

    loop {
        let node = cursor.node();
        if grammar.masked.contains(&node.kind()) {
            let (start, stop) = (node.start_byte(), node.end_byte());
            if grammar.format_capable.contains(&node.kind()) {
                // 昇順・重複なしの順で生成される。これは from_masked_ranges
                // が前提としているものと同じ。
                ranges.extend(subtract_format_args(source, start, stop));
            } else {
                ranges.push((start, stop));
            }
            // マスク対象: このサブツリーは丸ごとスキップする。
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    return ranges;
                }
            }
            continue;
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return ranges;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1つの1始まりの行について (occurrence_index, text, is_code) を集める。
    fn row(mask: &CodeMask, source: &str, line_1: usize) -> Vec<(usize, String, bool)> {
        let line = source.lines().nth(line_1 - 1).unwrap();
        identifier_occurrences(line)
            .enumerate()
            .map(|(k, (_, _, text))| (k, text.to_string(), mask.is_code(line_1, k)))
            .collect()
    }

    /// 以下の期待値は実装から導出したものではなく、フィクスチャから手で数えた
    /// もの — マスクを自身の構築方法に照らして検証しても、何をしても通って
    /// しまう。
    #[test]
    fn rust_masks_comments_strings_and_chars() {
        let src = "\
// comment mentions Foo
fn real(x: i32) -> Foo {
    let s = \"Foo in string\";
    let c = 'x';
    bar(Foo)
}
";
        let mask = CodeMask::compute(src, "lib.rs");

        // 行コメント内のすべての単語は地の文である。
        assert_eq!(
            row(&mask, src, 1),
            vec![
                (0, "comment".into(), false),
                (1, "mentions".into(), false),
                (2, "Foo".into(), false),
            ]
        );
        // 宣言はキーワードを含めて丸ごとコードである — キーワードを
        // フィルタするのは呼び出し側の仕事であり、マスクの仕事ではない。
        assert_eq!(
            row(&mask, src, 2),
            vec![
                (0, "fn".into(), true),
                (1, "real".into(), true),
                (2, "x".into(), true),
                (3, "i32".into(), true),
                (4, "Foo".into(), true),
            ]
        );
        // let/s はコードだが、リテラル内の3単語はコードではない。
        assert_eq!(
            row(&mask, src, 3),
            vec![
                (0, "let".into(), true),
                (1, "s".into(), true),
                (2, "Foo".into(), false),
                (3, "in".into(), false),
                (4, "string".into(), false),
            ]
        );
        // char リテラルも識別子を隠す。
        assert_eq!(
            row(&mask, src, 4),
            vec![
                (0, "let".into(), true),
                (1, "c".into(), true),
                (2, "x".into(), false),
            ]
        );
        // 3行目でマスクされたものと同じ名前だが、こちらはコード位置にある。
        assert_eq!(
            row(&mask, src, 5),
            vec![(0, "bar".into(), true), (1, "Foo".into(), true)]
        );
    }

    #[test]
    fn go_masks_comments_and_both_string_forms() {
        let src = "package main\n// Foo does things\nfunc Bar() {\n\ts := \"Foo\"\n\tr := `Foo raw`\n}\n";
        let mask = CodeMask::compute(src, "main.go");

        assert_eq!(
            row(&mask, src, 2),
            vec![
                (0, "Foo".into(), false),
                (1, "does".into(), false),
                (2, "things".into(), false),
            ]
        );
        assert_eq!(
            row(&mask, src, 3),
            vec![(0, "func".into(), true), (1, "Bar".into(), true)]
        );
        // 解釈済み文字列。
        assert_eq!(
            row(&mask, src, 4),
            vec![(0, "s".into(), true), (1, "Foo".into(), false)]
        );
        // raw 文字列 — 文法上は別のノード種別だが、扱いは同じ。
        assert_eq!(
            row(&mask, src, 5),
            vec![
                (0, "r".into(), true),
                (1, "Foo".into(), false),
                (2, "raw".into(), false),
            ]
        );
    }

    /// ノード種別への部分文字列一致テストでは間違えてしまうケース: テンプレート
    /// リテラルの補間はコードであり、名前に "string" を含むノードの中に
    /// 座っていてもジャンプ可能なままでなければならない。
    #[test]
    fn typescript_keeps_template_interpolations_jumpable() {
        let src = "// Foo comment\nconst t = `text ${realCode} more`;\nconst s = \"Foo\";\n";
        let mask = CodeMask::compute(src, "a.ts");

        assert_eq!(
            row(&mask, src, 1),
            vec![(0, "Foo".into(), false), (1, "comment".into(), false)]
        );
        assert_eq!(
            row(&mask, src, 2),
            vec![
                (0, "const".into(), true),
                (1, "t".into(), true),
                (2, "text".into(), false),
                (3, "realCode".into(), true), // ← ${ } の中だが、それでもコード
                (4, "more".into(), false),
            ]
        );
        assert_eq!(
            row(&mask, src, 3),
            vec![
                (0, "const".into(), true),
                (1, "s".into(), true),
                (2, "Foo".into(), false),
            ]
        );
    }

    /// TypeScript のテンプレートのケースに対応する Rust 版: format 文字列に
    /// よって捕捉された識別子は実在する束縛を名指ししており、文法が
    /// リテラル全体を構造のない1つの string_content として渡してくる
    /// にもかかわらず、ナビゲート可能なままでなければならない。
    #[test]
    fn rust_keeps_inline_format_captures_jumpable() {
        let src = "\
fn f(widget: u32) {
    let s = format!(\"{widget} and {}\", widget);
    println!(\"{widget:?} plus {count:>3} prose\");
    let raw = format!(r#\"{widget}\"#);
    let escaped = format!(\"{{widget}} literal\");
    let positional = format!(\"{0} {} text\", widget);
}
";
        let mask = CodeMask::compute(src, "lib.rs");

        assert_eq!(
            row(&mask, src, 2),
            vec![
                (0, "let".into(), true),
                (1, "s".into(), true),
                (2, "format".into(), true),
                (3, "widget".into(), true), // {widget} に取り込まれる
                (4, "and".into(), false),   // 波括弧の間にある地の文
                (5, "widget".into(), true), // 普通の末尾の引数
            ]
        );
        // : の後に format spec があっても名前は捕捉されたままである。
        assert_eq!(
            row(&mask, src, 3),
            vec![
                (0, "println".into(), true),
                (1, "widget".into(), true),
                (2, "plus".into(), false),
                (3, "count".into(), true),
                (4, "prose".into(), false),
            ]
        );
        // raw 文字列も format! に届く。r プレフィックスはマスクされたままに
        // なる: それはリテラル自身の構文の一部であって、何かへの参照では
        // ないからである。
        assert_eq!(
            row(&mask, src, 4),
            vec![
                (0, "let".into(), true),
                (1, "raw".into(), true),
                (2, "format".into(), true),
                (3, "r".into(), false),
                (4, "widget".into(), true),
            ]
        );
        // {{ はエスケープされた波括弧なので、これは何も名指ししていない。
        assert_eq!(
            row(&mask, src, 5),
            vec![
                (0, "let".into(), true),
                (1, "escaped".into(), true),
                (2, "format".into(), true),
                (3, "widget".into(), false),
                (4, "literal".into(), false),
            ]
        );
        // {0} と {} は識別子を持たない。実際の引数の方が持っている。
        assert_eq!(
            row(&mask, src, 6),
            vec![
                (0, "let".into(), true),
                (1, "positional".into(), true),
                (2, "format".into(), true),
                (3, "text".into(), false),
                (4, "widget".into(), true),
            ]
        );
    }

    #[test]
    fn block_comment_spanning_lines_masks_all_of_them() {
        let src = "fn a() {}\n/* Foo\n   Bar\n   Baz */\nfn b() {}\n";
        let mask = CodeMask::compute(src, "lib.rs");

        assert!(mask.is_code(1, 0)); // fn
        for line in 2..=4 {
            assert!(
                row(&mask, src, line).iter().all(|(_, _, code)| !*code),
                "line {line} should be entirely masked"
            );
        }
        assert!(mask.is_code(5, 0)); // fn
    }

    /// タブ展開はカラムをずらすが順序は変えない。これがまさに、マスクが
    /// occurrence index をキーにしている理由そのものである。Go は慣習として
    /// タブインデントされるので、これは特殊なケースではなく一般的なケース
    /// である。
    #[test]
    fn occurrence_indices_survive_tab_expansion() {
        let src = "package main\nfunc f() {\n\tx := \"Foo\"\n}\n";
        let mask = CodeMask::compute(src, "main.go");

        let raw = src.lines().nth(2).unwrap();
        let expanded = raw.replace('\t', "    ");
        assert_ne!(raw, expanded, "fixture must actually contain a tab");

        // 呼び出し側が生の行を渡してきても、描画済みの行を渡してきても
        // 判定は同じになる。
        assert!(mask.is_code_at_column(&expanded, 3, expanded.find('x').unwrap()));
        assert!(!mask.is_code_at_column(&expanded, 3, expanded.find("Foo").unwrap()));
    }

    #[test]
    fn unsupported_language_offers_nothing() {
        let src = "def build(x):\n    return x\n";
        let mask = CodeMask::compute(src, "script.py");
        assert!(!mask.is_code(1, 0));
        assert!(!mask.is_code(2, 0));
    }

    #[test]
    fn out_of_range_lookups_are_not_code() {
        let mask = CodeMask::compute("fn a() {}\n", "lib.rs");
        assert!(!mask.is_code(0, 0), "line numbers are 1-based");
        assert!(!mask.is_code(99, 0), "past end of file");
        assert!(!mask.is_code(1, MAX_TRACKED_PER_LINE), "past the cap");
    }
}
