//! 行の中のどの識別子がコードの位置にあるか。コメントや文字列の中の英単語が
//! 同名のシンボルに解決されるのを防ぐ。
//!
//! 位置はバイト範囲ではなく「その行の k 番目の識別子」で持つ。viewer はタブを
//! 展開した行を持つのでバイト位置は画面の桁とずれるが、展開は識別子の数も
//! 並びも変えない。両側が識別子とみなすものは [identifier_occurrences] の 1 つ。
//!
//! 記録するのはコードである出現 (allowlist)。空なら何もジャンプしないだけで、
//! blocklist が空だと全部がジャンプ可能になり、直したいバグが黙って再発する。

use std::sync::OnceLock;

use regex::Regex;

use super::language::Grammar;

/// u128 のビット幅。このリポジトリで最も識別子の多い行でも 76 個なので、
/// 溢れの表現を別に持つより固定で切る。
pub(super) const MAX_TRACKED_PER_LINE: usize = 128;

/// 行の中の識別子を (start_byte, end_byte, text) で順に返す。
pub fn identifier_occurrences(line: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").unwrap());
    re.find_iter(line).map(|m| (m.start(), m.end(), m.as_str()))
}

/// 1 ファイル分の、行ごとのコード位置にある識別子の集合。
#[derive(Debug, Default, Clone)]
pub struct CodeMask {
    /// lines[i] のビット k が立っていれば、行 i + 1 の k 番目の識別子はコード。
    lines: Vec<u128>,
    supported: bool,
}

impl CodeMask {
    /// このファイルの言語を解析できたか。
    ///
    /// 「コードの識別子が 1 つもない」とは扱いが逆になる。ジャンプの提示は 1 語への
    /// 主張なので沈黙が安全だが、参照の一覧はリポジトリ全体への主張なので、
    /// 空の結果は「存在しない」と読まれる。検索側はフィルタなしの一致に落ちる。
    pub fn is_supported(&self) -> bool {
        self.supported
    }

    /// 行 line_1 (1 始まり) の occurrence 番目 (0 始まり) の識別子がコードの位置にあるか。
    /// 範囲外の行、上限を超えた出現、解析できなかったファイルはすべて false。
    pub fn is_code(&self, line_1: usize, occurrence: usize) -> bool {
        if line_1 == 0 || occurrence >= MAX_TRACKED_PER_LINE {
            return false;
        }
        self.lines
            .get(line_1 - 1)
            .is_some_and(|bits| bits & (1u128 << occurrence) != 0)
    }

    /// 描画済みの行 rendered_line の桁 col を覆う識別子がコードの位置にあるか。
    pub fn is_code_at_column(&self, rendered_line: &str, line_1: usize, col: usize) -> bool {
        identifier_occurrences(rendered_line)
            .enumerate()
            .find(|(_, (start, end, _))| col >= *start && col < *end)
            .is_some_and(|(k, _)| self.is_code(line_1, k))
    }

    /// path の拡張子で文法を選んで source のマスクを作る。
    /// 文法が無い言語や解析できなかったファイルは、何もコードではない空のマスク。
    pub fn compute(source: &str, path: &str) -> Self {
        let Some(grammar) = Grammar::of_path(std::path::Path::new(path)) else {
            return Self::default();
        };
        let Some(tree) = grammar.parse(source) else {
            return Self::default();
        };
        Self::from_masked_ranges(source, &collect_masked_ranges(&tree, source, grammar))
    }

    /// masked は開始位置順で重複しないことが前提。
    fn from_masked_ranges(source: &str, masked: &[(usize, usize)]) -> Self {
        let mut lines = Vec::new();
        let mut line_start = 0usize;
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

/// マスクするノード種別と、その中で format 捕捉 (`{name}`) を掘り出す種別。
/// 種別名は各文法の実際の出力で確かめたもの。
fn masked_kinds(grammar: Grammar) -> (&'static [&'static str], &'static [&'static str]) {
    match grammar {
        // raw 文字列も format! に届く。コメントの中の {x} は何も名指ししていない。
        Grammar::Rust => (
            &[
                "line_comment",
                "block_comment",
                "doc_comment",
                "string_content",
                "raw_string_literal",
                "char_literal",
            ],
            &["string_content", "raw_string_literal"],
        ),
        // Go の %v には識別子が無い。
        Grammar::Go => (
            &[
                "comment",
                "interpreted_string_literal_content",
                "raw_string_literal_content",
            ],
            &[],
        ),
        // template_string ではなく string_fragment。テンプレートリテラルは
        // [string_fragment, template_substitution, string_fragment] に分かれるので、
        // fragment 単位なら ${...} の中の式がコードのまま残る。
        Grammar::TypeScript | Grammar::Tsx => (&["comment", "string_fragment"], &[]),
    }
}

/// tree-sitter-rust は format 文字列を分割しないので、`format!("{widget:?}")` の
/// widget がリテラルごとマスクされる。このリポジトリだけで 159 ファイル 945 件。
/// 捕捉の名前を除いた区間を返す。{} と {0} と {{ はそのまま。
fn subtract_format_args(source: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let bytes = &source.as_bytes()[start..end];
    let mut out = Vec::new();
    let mut cut = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        if bytes.get(i + 1) == Some(&b'{') {
            i += 2;
            continue;
        }
        let name_start = i + 1;
        let mut j = name_start;
        if bytes
            .get(j)
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        {
            j += 1;
            while bytes
                .get(j)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                j += 1;
            }
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

/// マスク対象ノードのバイト範囲を pre-order で集める。一致したノードの中には潜らない
/// ので、範囲は開始位置順で重複しない。
fn collect_masked_ranges(
    tree: &tree_sitter::Tree,
    source: &str,
    grammar: Grammar,
) -> Vec<(usize, usize)> {
    let (masked, format_capable) = masked_kinds(grammar);
    let mut ranges = Vec::new();
    let mut cursor = tree.walk();

    loop {
        let node = cursor.node();
        let is_masked = masked.contains(&node.kind());
        if is_masked {
            let (start, stop) = (node.start_byte(), node.end_byte());
            if format_capable.contains(&node.kind()) {
                ranges.extend(subtract_format_args(source, start, stop));
            } else {
                ranges.push((start, stop));
            }
        }
        if !is_masked && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return ranges;
            }
        }
    }
}

/// 行の中で飛び先になりうる識別子を (出現番号, 開始桁, 綴り) で列挙する。
pub fn code_identifiers_on_line<'a>(
    line: &'a str,
    line_1: usize,
    mask: &'a CodeMask,
) -> impl Iterator<Item = (usize, usize, String)> + 'a {
    identifier_occurrences(line)
        .enumerate()
        .filter(move |(k, _)| mask.is_code(line_1, *k))
        .filter(|(_, (_, _, word))| word.len() > 1 && !is_rust_keyword(word))
        .map(|(k, (start, _, word))| (k, start, word.to_string()))
}

/// 元ソースの行での k 番目の識別子のバイト範囲。
///
/// 出現番号は viewer のタブ展開済みの行から取るが、索引の列は展開前の位置を指す。
/// 番号はそのまま通り、桁だけがここで戻る。
pub fn occurrence_span_in_source(source_line: &str, k: usize) -> Option<(usize, usize)> {
    identifier_occurrences(source_line)
        .nth(k)
        .map(|(start, end, _)| (start, end))
}

pub fn is_rust_keyword(word: &str) -> bool {
    matches!(
        word,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
    )
}
