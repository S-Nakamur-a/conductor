//! Transcript フレーバーのフェンス付きコードブロック描画。実物の Claude Code はコードを
//! カードの装飾なしで（背景も、字下げも、余白の空行もなし）描画し、トークンの色付けは
//! ターミナルの基本 ANSI 8色だけを使う。色の選択は syntect の RGB テーマ出力を変換する
//! のではなく、スコープ名を分類することで行う（render.rs の render_code_block にある
//! Rich フレーバーの「カード」は、これとは意図的に無関係）。
//!
//! 以下の分類ルールは、two_face::syntax::extra_newlines() が Rust/Python/Bash/JSON に
//! 対して実際に出力するスコープスタックを調べ（正確なフィクスチャはこのモジュールの
//! テストを参照）、それを実物のキャプチャのトークンごとの色と突き合わせて作った。
//! 必要な区別のいくつか（let と str、ls と grep、Some/None と Option/String）は、
//! これらのシンタックス（sublime-syntax パッケージ）のスコープ名だけでは表現されておらず
//! 両方に同じスコープが使い回されている。そのためスコープでは埋められない部分を、
//! 少数のリテラルトークンによる特例で補っている。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::ScopeRegionIterator;
use syntect::parsing::{ParseState, ScopeStack, ScopeStackOp, SyntaxSet};

use super::wrap::{spans_to_cells, wrap_cells};

/// Rust のプリミティブ型名。同梱の rust.sublime-syntax はこれらを let/const の
/// ストレージキーワードと同じスコープ（storage.type.rust、それ以上のサフィックスなし）に
/// 分類するため、リテラルのテキストでしか見分けられない。このリストは Rust に実在する
/// プリミティブを網羅しているという意味では完全だが、受け入れフィクスチャを通すために
/// 書いたものであり、実物のキャプチャとトークンごとに突き合わせて検証したわけではない。
/// 検証済みの表ではなく、あくまで最善の推測として扱うこと。
const RUST_PRIMITIVE_TYPES: &[&str] = &[
    "str", "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
    "i128", "isize", "f32", "f64",
];

/// 文法上は単なる variable.function.shell としかスコープされない（grep のような任意の
/// 外部コマンドと見分けがつかない）が、実物ではビルトインとして色付けされる Bash の
/// コマンド名。
///
/// これはビルトインコマンドの網羅的な参照表ではない。受け入れフィクスチャがたまたま
/// 使っている1語（ls）だけを含む。ここに載っていないもの（cd、pwd。export は下で別途
/// 扱う、など）は Category::Reset にフォールスルーするが、それが実物と一致するかは
/// 未検証。このリストは実際の実物キャプチャから拡張すること。「これはビルトインの
/// はず」という推測で足さないこと。
const BASH_COMMANDS_MEASURED_AS_BUILTIN: &[&str] = &["ls"];

/// 実物の基本 ANSI 色に対応するトークンのカテゴリ1つ分。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Comment,
    Keyword,
    FunctionName,
    Type,
    Builtin,
    Number,
    StringLit,
    Reset,
}

impl Category {
    fn style(self) -> Style {
        match self {
            Category::Comment => Style::default().fg(Color::Green),
            Category::Keyword => Style::default().fg(Color::Blue),
            Category::FunctionName => Style::default().fg(Color::Yellow),
            Category::Type => Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
            Category::Builtin => Style::default().fg(Color::Cyan),
            Category::Number => Style::default().fg(Color::Green),
            Category::StringLit => Style::default().fg(Color::Red),
            Category::Reset => Style::default().fg(Color::Reset),
        }
    }
}

/// トークン列が現在 string.quoted.* の範囲内にあるかどうか、またその範囲が埋め込み式
/// （f文字列の展開、シェルの $VAR 展開）によって中断されたかどうかを追跡する。
/// 実物のキャプチャで確認した実物特有の癖: 文字列が一度中断されると、その時点から
/// 閉じデリミタを含めて文字列の赤色ではなくデフォルト色に戻る。
#[derive(Default)]
struct StringState {
    in_string: bool,
    interrupted: bool,
}

impl StringState {
    /// None は「トークンが string.quoted.* にスコープされていない」で、呼び出し元は他の分類を
    /// 続けること。Python の {x} のように、文字列のスコープを外れつつ範囲内に留まる場合がある。
    fn observe(&mut self, scopes: &[String]) -> Option<bool> {
        let has_quoted = scopes.iter().any(|s| s.starts_with("string.quoted."));
        if has_quoted && !self.in_string {
            self.in_string = true;
            self.interrupted = false;
        }
        let is_interpolation = scopes.iter().any(|s| {
            s.contains("interpolation.")
                || s.contains("expansion")
                || s.contains("embedded")
                || s.starts_with("variable.")
        });
        if self.in_string && is_interpolation {
            self.interrupted = true;
        }
        let result = has_quoted.then_some(!self.interrupted);
        if has_quoted && scopes.iter().any(|s| s.contains("string.end.")) {
            self.in_string = false;
            self.interrupted = false;
        }
        result
    }
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// トークン1つを、そのスコープスタック全体（最も外側が先頭）と行内の次のトークンの
/// 生テキスト（呼び出し箇所フォールバック用）から分類する。
fn classify(
    text: &str,
    scopes: &[String],
    next_text: Option<&str>,
    string_state: &mut StringState,
) -> Category {
    // コメントは、その中に他の何がネストしていても優先される。
    if scopes.iter().any(|s| s.starts_with("comment.")) {
        return Category::Comment;
    }
    // JSON のオブジェクトキー: 文字列としてスコープされるが、実物では文字列値ではなく
    // ビルトイン/キーワードのように色付けされる。下の汎用の文字列ルールより前に
    // チェックする必要がある。
    if scopes.iter().any(|s| s.starts_with("meta.mapping.key.")) {
        return Category::Builtin;
    }
    // 文字列リテラルの中身とデリミタ（状態機械。StringState を参照）。
    if let Some(uninterrupted) = string_state.observe(scopes) {
        return if uninterrupted {
            Category::StringLit
        } else {
            Category::Reset
        };
    }
    // フォーマット文字列の接頭辞（Python の開き引用符の前の f）は string.quoted.* より
    // 前に来るが、文字列の一部として読める。
    if scopes.iter().any(|s| s.starts_with("storage.type.string.")) {
        return Category::StringLit;
    }
    if scopes.iter().any(|s| s.starts_with("constant.numeric.")) {
        return Category::Number;
    }
    // true/false/null など（JSON）。
    if scopes.iter().any(|s| s.starts_with("constant.language.")) {
        return Category::Keyword;
    }
    if scopes
        .iter()
        .any(|s| s.starts_with("keyword.control.") || s.starts_with("keyword.declaration."))
    {
        return Category::Keyword;
    }
    // Rust の fn/struct: それぞれ曖昧さのない独自の storage.type.* サブスコープを持つ。
    if scopes
        .iter()
        .any(|s| s.starts_with("storage.type.function.") || s.starts_with("storage.type.struct."))
    {
        return Category::Keyword;
    }
    // 素の storage.type.rust: Rust の文法はこの同じスコープを、ストレージキーワード
    // （let、const）とプリミティブ型名（str、bool、usize、...）の両方に使い回す。
    // リテラルのテキストでしか見分けられない。
    if scopes.iter().any(|s| s == "storage.type.rust") {
        return if RUST_PRIMITIVE_TYPES.contains(&text) {
            Category::Type
        } else {
            Category::Keyword
        };
    }
    if scopes
        .iter()
        .any(|s| s.starts_with("entity.name.function."))
    {
        return Category::FunctionName;
    }
    // 文法が明示的にタグ付けするビルトイン関数（Python の range、Bash の echo）。
    // 単語らしいテキストに絞っているのは、support.function. プレフィックスが句読点
    // （Bash の [ ] テストブラケット）にも使い回されるスコープを巻き込まないため。
    if is_identifier(text) && scopes.iter().any(|s| s.starts_with("support.function.")) {
        return Category::Builtin;
    }
    // Bash の汎用コマンド名スコープは、ビルトイン（ls）と通常の外部コマンド（grep）を
    // 同一に扱う。リテラルの許可リストでしか見分けられない。
    if scopes.iter().any(|s| s == "variable.function.shell")
        && BASH_COMMANDS_MEASURED_AS_BUILTIN.contains(&text)
    {
        return Category::Builtin;
    }
    // export のような Bash のビルトインキーワード。
    if scopes.iter().any(|s| s == "storage.modifier.shell") {
        return Category::Builtin;
    }
    // 型 — Rust はこのスコープを enum のバリアントにも使い回す（Some は呼び出し、
    // None は言語定数）。それ以外はここでは本当に型名（Option、String、Vec、...）。
    if scopes.iter().any(|s| s.starts_with("support.type.")) {
        return match text {
            "None" => Category::Keyword,
            "Some" => Category::FunctionName,
            _ => Category::Type,
        };
    }
    // クラス/構造体の名前は関数名としてではなく無装飾で表示する。class C(object): の
    // ような定義では名前の直後に ( が続くため、これを外すと下の呼び出し箇所
    // フォールバックに誤って引っかかってしまう。
    if scopes
        .iter()
        .any(|s| s.starts_with("entity.name.class.") || s.starts_with("entity.name.struct."))
    {
        return Category::Reset;
    }
    // Python の基底クラス位置: 認識済みのビルトイン（object）だけが言語定数として
    // 読まれ、カスタムの基底クラスは無装飾のまま。ここで受け入れフィクスチャが
    // 使っているビルトインは object のみで、この位置で同様に読まれる Python の
    // 全ビルトイン（int、Exception、...）を網羅した検証済みリストではない。
    // 実際の実物キャプチャからのみ拡張すること。
    if scopes
        .iter()
        .any(|s| s.starts_with("entity.other.inherited-class."))
    {
        return if text == "object" {
            Category::Keyword
        } else {
            Category::Reset
        };
    }
    // Rust の as キャストキーワードは、無装飾のままであるべき記号演算子（&、+、...）と
    // 同じスコープを共有するため、リテラルトークンでしか判別できない。フィクスチャが
    // 使う keyword.operator.rust のトークンは as のみで、このスコープ下に他の
    // 単語らしい演算子が文法上存在するとしても実物とは未検証で、現状は
    // Category::Reset にフォールスルーする。
    if text == "as" && scopes.iter().any(|s| s.starts_with("keyword.operator.")) {
        return Category::Keyword;
    }
    // フォールバック: 文法がこれを関数名としてタグ付けしなかった場合（Rust の
    // String::from(...) のような、認識されないパス修飾つき呼び出しなど）でも、
    // 識別子の直後に ( が続けば、どの言語でも実物では呼び出しとして読まれる。
    if is_identifier(text) && next_text.is_some_and(|n| n.starts_with('(')) {
        return Category::FunctionName;
    }
    Category::Reset
}

/// stack と string_state を呼び出し間で引き継ぐことで、複数トークンにまたがる構文
/// (原理的には複数行文字列も) を一貫して分類できる。
fn classify_line(
    line: &str,
    ops: &[(usize, ScopeStackOp)],
    stack: &mut ScopeStack,
    string_state: &mut StringState,
) -> Vec<Span<'static>> {
    // 1回目のパス: すべての op を適用し、空でない各領域のテキストとスコープスタックを
    // 記録する。2回目のパス（下）ではトークン1個分の先読みで分類する。これは
    // 呼び出し箇所フォールバックに必要。
    let mut tokens: Vec<(String, Vec<String>)> = Vec::new();
    for (text, op) in ScopeRegionIterator::new(ops, line) {
        let _ = stack.apply(op);
        let trimmed = text.trim_end_matches('\n');
        if trimmed.is_empty() {
            continue;
        }
        let scopes = stack.as_slice().iter().map(|s| s.build_string()).collect();
        tokens.push((trimmed.to_string(), scopes));
    }

    let mut spans = Vec::with_capacity(tokens.len());
    for i in 0..tokens.len() {
        let (text, scopes) = &tokens[i];
        let next = tokens.get(i + 1).map(|(t, _)| t.as_str());
        let category = classify(text, scopes, next, string_state);
        spans.push(Span::styled(text.clone(), category.style()));
    }
    spans
}

/// フェンス付きコードブロックを実物の Claude Code と同じ見た目で描画する:
/// カードの装飾（背景/字下げ/パディング）なし、ソースの字下げは保持、width で
/// ハードラップ、トークンは classify に従って基本 ANSI 8色で色付けする。
pub(crate) fn render_code_block_transcript(
    lang: Option<&str>,
    lines: &[String],
    width: usize,
    syntax_set: &SyntaxSet,
) -> Vec<Line<'static>> {
    let syntax = lang
        .and_then(|l| syntax_set.find_syntax_by_token(l))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut parse_state = ParseState::new(syntax);
    let mut scope_stack = ScopeStack::new();
    let mut string_state = StringState::default();

    let mut out = Vec::with_capacity(lines.len());
    for raw in lines {
        // タブを展開し、表示幅の計算（ひいては折り返し）が正しく行われるようにする。
        let expanded = raw.replace('\t', "    ");
        let with_nl = format!("{expanded}\n");
        let spans = match parse_state.parse_line(&with_nl, syntax_set) {
            Ok(ops) => classify_line(&with_nl, &ops, &mut scope_stack, &mut string_state),
            Err(_) => vec![Span::raw(expanded.clone())],
        };
        let cells = spans_to_cells(&spans);
        let wrapped = if cells.is_empty() {
            vec![Line::from("")]
        } else {
            wrap_cells(&cells, width.max(1), true)
        };
        out.extend(wrapped);
    }
    out
}
