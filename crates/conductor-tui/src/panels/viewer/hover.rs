//! カーソル・マウスの下にあるシンボルの説明。宣言、doc コメント、参照数。
//!
//! 索引が位置で答えた説明を優先し、無ければ定義行のソースから読み取る。索引の宣言は
//! producer が型を解決したもので、字面の写しより中身が濃い (`let source: String` に
//! 対して字面は `let source = read_to_string(..)?`)。

use std::path::Path;
use std::time::{Duration, Instant};

use conductor_core::symbol_index::SymbolIndex;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use conductor_core::theme::Theme;

/// マウスが止まってからポップアップが出るまで。
pub const IDLE: Duration = Duration::from_millis(350);
/// マウスが離れてもポップアップを残す猶予。ポップアップまでカーソルを運べるようにする。
pub const GRACE: Duration = Duration::from_millis(700);

/// 宣言として集める最大行数。字面から読むときも索引の宣言を切るときも同じ枠に載る。
pub(super) const MAX_SIGNATURE_LINES: usize = 8;
const MAX_DOC_LINES: usize = 12;
const MAX_DEFINITION_LINES: usize = 24;
/// 定義本体の閉じ波括弧を探して読む最大行数。ここに収まらない宣言は波括弧の
/// 数え上げが信用できないので、本体を出すのをやめる。
const MAX_DEFINITION_SCAN: usize = 512;
/// UI スレッドで走る経路なので作業量に上限を置く (`new` の正確な件数は実測
/// 157ms = 10 フレーム落ち)。
const REF_CAP: usize = 50;

/// マウスが止まるのを待っている候補。
#[derive(Debug)]
pub struct Pending {
    pub word: String,
    /// 1 始まり。
    pub line: usize,
    pub occurrence: usize,
    pub start_col: usize,
    pub anchor: (u16, u16),
    pub since: Instant,
    /// 一度引いた候補は静止し続けても引き直さない。
    pub resolved: bool,
}

/// 出ているポップアップ。
#[derive(Debug)]
pub struct Hover {
    pub word: String,
    /// 索引が答えた種別 ("fn", "struct")。読めなければ空。
    pub kind: String,
    /// その語を囲んでいるものの綴り (`app::types::App`)。索引が答えたときだけ。
    pub container: Option<String>,
    /// 定義のあるファイル (根からの相対) と 1 始まりの行。
    pub path: String,
    pub line: usize,
    pub doc: Vec<String>,
    pub signature: Vec<String>,
    /// 名前に一致した定義の数。2 以上なら、出しているのはそのうちの 1 つ。
    pub def_count: usize,
    pub refs: usize,
    /// 上限で数えるのを止めたか。
    pub refs_capped: bool,
    /// 聞かれた位置が定義そのものだった。字面の写しにしかならないので描画側が省く。
    pub on_definition_line: bool,
    /// 宣言が索引由来か。型が解決済みで字面とは違うものを見せているので、
    /// 定義行の上でも省かない。
    pub signature_from_index: bool,
    /// この定義位置を答えた層。
    pub by: super::code_nav::By,
    /// ポップアップを置く画面上の位置。
    pub anchor: (u16, u16),
    /// マウスが離れた時刻。猶予を数える。
    pub left_at: Option<Instant>,
    /// キーボードで開いたものは、フォーカスやアイドルでは消えない。
    pub pinned: bool,
}

/// 索引がその語について書いていたこと。
#[derive(Debug, Default)]
pub struct Indexed {
    pub kind: String,
    pub signature: Vec<String>,
    pub doc: Vec<String>,
}

impl Indexed {
    fn is_empty(&self) -> bool {
        self.kind.is_empty() && self.signature.is_empty() && self.doc.is_empty()
    }
}

/// ホバーが説明する定義の位置。
#[derive(Debug)]
pub struct DefSite {
    pub path: String,
    /// 1 始まり。
    pub line: usize,
    /// 名前で引いたときの種別。位置しか分からないときは空。
    pub kind: String,
    pub def_count: usize,
    pub detail: Option<Indexed>,
}

/// 名前で定義位置を決める。複数あれば読んでいるファイルの中のものを優先する。
pub fn resolve_def_site(index: &SymbolIndex, word: &str, reading: Option<&str>) -> Option<DefSite> {
    if !index.is_available() {
        return None;
    }
    let defs = index.find_definitions(word, Path::new(reading.unwrap_or("")));
    let def = defs
        .iter()
        .find(|d| Some(d.file_path.as_str()) == reading)
        .or_else(|| defs.first())?;
    Some(DefSite {
        path: def.file_path.clone(),
        line: def.line,
        kind: format!("{:?}", def.kind),
        def_count: defs.len(),
        detail: None,
    })
}

/// 定義位置からポップアップの中身を組み立てる。読めない・行が範囲外なら黙って `None`。
pub fn build(
    index: &SymbolIndex,
    root: &Path,
    word: &str,
    def: DefSite,
    by: super::code_nav::By,
    anchor: (u16, u16),
) -> Option<Hover> {
    let source = std::fs::read_to_string(root.join(&def.path)).ok()?;
    let lines: Vec<&str> = source.lines().collect();
    let def_idx = def.line.checked_sub(1)?;
    if def_idx >= lines.len() {
        return None;
    }
    let indexed = def.detail.filter(|d| !d.is_empty());
    let kind = match indexed.as_ref().map(|d| d.kind.as_str()) {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => def.kind,
    };
    let doc = match indexed.as_ref().filter(|d| !d.doc.is_empty()) {
        Some(d) => d.doc.clone(),
        None => extract_doc(&lines, def_idx),
    };
    // 索引も字面も struct / enum は見出しの 1 行しか答えない。見たいのは中身なので、
    // 本体を切り出せたときだけそちらを採る。
    let members = has_member_list(&kind)
        .then(|| extract_definition(&lines, def_idx))
        .flatten();
    let signature_from_index =
        members.is_none() && indexed.as_ref().is_some_and(|d| !d.signature.is_empty());
    let signature = match members {
        Some(body) => body,
        None => match indexed.filter(|d| !d.signature.is_empty()) {
            Some(d) => d.signature,
            None => extract_signature(&lines, def_idx),
        },
    };
    let (refs, refs_capped) = index.count_references_upto(word, root, REF_CAP);

    Some(Hover {
        word: word.to_string(),
        kind,
        container: None,
        path: def.path,
        line: def.line,
        doc,
        signature,
        def_count: def.def_count,
        refs,
        refs_capped,
        on_definition_line: false,
        signature_from_index,
        by,
        anchor,
        left_at: None,
        pinned: false,
    })
}

/// 種別がメンバの並びを本体に持つものか。class を外しているのは、本体がメソッドの
/// 実装であって宣言の並びではないから。
fn has_member_list(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "struct" | "enum" | "interface" | "trait"
    )
}

/// 波括弧は字面で数えるので文字列リテラル内の波括弧に騙される。定義位置しか
/// 分かっていない他ファイルを構文解析する余裕はここには無い。
fn extract_definition(lines: &[&str], def_idx: usize) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut opened = false;
    for raw in lines.iter().skip(def_idx).take(MAX_DEFINITION_SCAN) {
        let text = dedent(raw, lines[def_idx]);
        if !opened && !text.contains('{') && text.ends_with(';') {
            return None;
        }
        opened |= text.contains('{');
        depth += delta(&text, &['{'], &['}']);
        out.push(text);
        if opened && depth <= 0 {
            if out.len() > MAX_DEFINITION_LINES {
                let hidden = out.len() - MAX_DEFINITION_LINES;
                out.truncate(MAX_DEFINITION_LINES);
                out.push(format!("\u{2026} (+{hidden} lines)"));
            }
            return Some(out);
        }
    }
    None
}

/// `{` で終わる最初の行 (波括弧は除く) か、`;` / `,` で終わる宣言まで。
fn extract_signature(lines: &[&str], def_idx: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    for raw in lines.iter().skip(def_idx).take(MAX_SIGNATURE_LINES) {
        let text = dedent(raw, lines[def_idx]);
        if let Some(stripped) = text.strip_suffix('{') {
            let head = stripped.trim_end();
            if !head.is_empty() {
                out.push(head.to_string());
            }
            return out;
        }
        let ends_with_semicolon = text.ends_with(';');
        // 角括弧を数えないのは、Option<String> のような型と大小比較を字面で
        // 見分けられないため。
        depth += delta(&text, &['(', '[', '{'], &[')', ']', '}']);
        let ends_with_comma = text.ends_with(',');
        out.push(text);
        if ends_with_semicolon {
            return out;
        }
        // 構造体のフィールドや enum の要素は , で終わる。囲みの外側にいるときだけ
        // 区切りとして扱う — 引数を複数行に割った関数の途中で切らないように。
        if depth <= 0 && ends_with_comma {
            return out;
        }
    }
    if lines.len() > def_idx + MAX_SIGNATURE_LINES {
        out.push("\u{2026}".to_string());
    }
    out
}

/// 最初の行のインデントぶんだけ左へ詰める。
fn dedent(raw: &str, first: &str) -> String {
    let indent = first.len() - first.trim_start().len();
    let body = if raw.len() >= indent && raw[..indent.min(raw.len())].trim().is_empty() {
        &raw[indent..]
    } else {
        raw.trim_start()
    };
    body.trim_end().to_string()
}

fn delta(line: &str, open: &[char], close: &[char]) -> i32 {
    line.chars().fold(0, |acc, c| {
        acc + i32::from(open.contains(&c)) - i32::from(close.contains(&c))
    })
}

/// 定義の直上にある doc コメント。属性・デコレータの行は読み飛ばす。
fn extract_doc(lines: &[&str], def_idx: usize) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut i = def_idx;
    // 下から上へ走査しているので、`*/` を見た時点でブロックの内側に入る。
    let mut in_block = false;
    while i > 0 {
        i -= 1;
        let text = lines[i].trim();
        if in_block {
            found.push(strip_block(text));
            if text.starts_with("/*") {
                break;
            }
            continue;
        }
        if found.is_empty() && (text.starts_with("#[") || text.starts_with('@') || text == "]") {
            continue;
        }
        if text.ends_with("*/") && !text.starts_with("//") {
            let body = text.trim_end_matches("*/").trim_end();
            in_block = !body.starts_with("/*");
            found.push(strip_block(body));
            if !in_block {
                break;
            }
        } else if let Some(rest) = text
            .strip_prefix("///")
            .or_else(|| text.strip_prefix("//!"))
            .or_else(|| text.strip_prefix("//"))
        {
            found.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else {
            break;
        }
    }
    found.reverse();
    while found.first().is_some_and(String::is_empty) {
        found.remove(0);
    }
    while found.last().is_some_and(String::is_empty) {
        found.pop();
    }
    if found.len() > MAX_DOC_LINES {
        found.truncate(MAX_DOC_LINES);
        found.push("\u{2026}".to_string());
    }
    found
}

fn strip_block(text: &str) -> String {
    text.trim_start_matches("/**")
        .trim_start_matches("/*")
        .trim_start_matches('*')
        .trim()
        .to_string()
}

/// 描いたポップアップ。矩形は描画とマウスの両方がここから引くので、当たり判定が
/// 描画の副産物にならない。
pub struct Popup {
    pub rect: Rect,
    /// 定義位置の行。押すとそこへ飛ぶ。
    pub def_row: Rect,
    /// 参照数の行。押すと一覧が開く。参照が無ければ大きさ 0。
    pub refs_row: Rect,
    pub body: Vec<Line<'static>>,
    pub footer: Vec<(Rect, Line<'static>)>,
}

/// ポップアップの中身と置き場所を決める。`host` は Viewer の矩形。
pub fn popup(hover: &Hover, theme: &Theme, host: Rect) -> Popup {
    let def_label = def_label(hover);
    let refs_label = refs_label(hover);
    let mut body: Vec<Line<'static>> = Vec::new();
    if hover.signature_from_index || !hover.on_definition_line {
        body.extend(hover.signature.iter().map(|text| {
            Line::from(Span::styled(
                text.clone(),
                Style::default().fg(theme.accent),
            ))
        }));
    }
    if !hover.doc.is_empty() {
        if !body.is_empty() {
            body.push(Line::from(""));
        }
        body.extend(
            hover
                .doc
                .iter()
                .map(|d| Line::from(Span::styled(d.clone(), Style::default().fg(theme.fg)))),
        );
    }

    let header = header(hover);
    let content_w = body
        .iter()
        .map(Line::width)
        .chain([
            def_label.chars().count(),
            refs_label.chars().count(),
            header.chars().count(),
        ])
        .max()
        .unwrap_or(20)
        .clamp(20, 100) as u16;
    let width = (content_w + 4).min(host.width.saturating_sub(2)).max(4);
    if !header.is_empty() {
        body.insert(
            0,
            Line::from(Span::styled(
                header,
                Style::default()
                    .fg(theme.info)
                    .add_modifier(Modifier::ITALIC),
            )),
        );
    }
    let footer_h = 1 + usize::from(hover.refs > 0);
    let height = (body.len() + footer_h) as u16 + 2;
    let height = height.min(host.height.saturating_sub(2)).max(3);
    let rect = place(host, hover.anchor, width, height);

    let inner = Rect::new(
        rect.x + 1,
        rect.y + 1,
        rect.width.saturating_sub(2),
        rect.height.saturating_sub(2),
    );
    let footer_h = (footer_h as u16).min(inner.height);
    let body_h = inner.height.saturating_sub(footer_h);
    let row = |offset: u16| Rect::new(inner.x, inner.y + body_h + offset, inner.width, 1);
    let def_row = if footer_h >= 1 {
        row(0)
    } else {
        Rect::default()
    };
    let refs_row = if footer_h >= 2 {
        row(1)
    } else {
        Rect::default()
    };

    let mut footer = Vec::new();
    if def_row.height > 0 {
        footer.push((
            def_row,
            Line::from(Span::styled(def_label, Style::default().fg(theme.fg))),
        ));
    }
    if refs_row.height > 0 {
        footer.push((
            refs_row,
            Line::from(Span::styled(
                refs_label,
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
        ));
    }
    body.truncate(body_h as usize);
    Popup {
        rect,
        def_row,
        refs_row,
        body,
        footer,
    }
}

fn def_label(hover: &Hover) -> String {
    let mut label = format!("\u{25b8} {}:{}", hover.path, hover.line);
    if hover.def_count > 1 {
        label.push_str(&format!("  (+{} defs)", hover.def_count - 1));
    }
    label.push_str(&format!("  [{}]", hover.by.label()));
    label
}

/// `+` は数え終えていない印。ありふれた名前でちょうど 50 件だったように見せない。
fn refs_label(hover: &Hover) -> String {
    let plus = if hover.refs_capped { "+" } else { "" };
    format!("\u{25b8} {}{plus} refs", hover.refs)
}

/// 所属を左、種別を右に置いた見出し。どちらも無ければ空。
fn header(hover: &Hover) -> String {
    match (
        hover.container.as_deref().unwrap_or(""),
        hover.kind.as_str(),
    ) {
        ("", "") => String::new(),
        ("", kind) => kind.to_string(),
        (container, "") => container.to_string(),
        (container, kind) => format!("{container}  {kind}"),
    }
}

/// 余白があればアンカー行のすぐ下、無ければ上。どちらも host の中に収める。
fn place(host: Rect, anchor: (u16, u16), w: u16, h: u16) -> Rect {
    let (col, row) = anchor;
    let top = host.y + 1;
    let bottom = host.y + host.height.saturating_sub(1);
    let row = row.clamp(top, bottom.saturating_sub(1));
    let y = if bottom.saturating_sub(row + 1) >= h {
        row + 1
    } else {
        row.saturating_sub(h).max(top)
    };
    let max_x = (host.x + host.width).saturating_sub(w);
    Rect::new(col.clamp(host.x, max_x.max(host.x)), y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(src: &str, def_line_1: usize) -> Vec<String> {
        extract_signature(&src.lines().collect::<Vec<_>>(), def_line_1 - 1)
    }

    fn body(src: &str, def_line_1: usize) -> Option<Vec<String>> {
        extract_definition(&src.lines().collect::<Vec<_>>(), def_line_1 - 1)
    }

    fn doc(src: &str, def_line_1: usize) -> Vec<String> {
        extract_doc(&src.lines().collect::<Vec<_>>(), def_line_1 - 1)
    }

    #[test]
    fn 宣言は本体の手前で止まり左へ詰める() {
        /// ソース、定義行 (1 始まり)、期待する宣言、なぜそうなるか。
        type Case = (&'static str, usize, &'static [&'static str], &'static str);
        let cases: &[Case] = &[
            (
                "pub fn foo(a: usize) -> bool {\n    true\n}\n",
                1,
                &["pub fn foo(a: usize) -> bool"],
                "波括弧の手前まで",
            ),
            (
                "fn foo(\n    a: usize,\n    b: &str,\n) -> bool {\n    true\n}\n",
                1,
                &["fn foo(", "    a: usize,", "    b: &str,", ") -> bool"],
                "囲みの中の , は区切りにならない",
            ),
            (
                "impl Foo {\n    pub fn bar(&self) -> usize {\n        1\n    }\n}\n",
                2,
                &["pub fn bar(&self) -> usize"],
                "字下げは左へ詰める",
            ),
            (
                "type Alias = Vec<String>;\nfn next() {}\n",
                1,
                &["type Alias = Vec<String>;"],
                "セミコロンで止まる",
            ),
            (
                "pub struct A {\n    pub name: Option<String>,\n}\n\n/// 次のアイテム。\npub struct B {\n}\n",
                2,
                &["pub name: Option<String>,"],
                "フィールドの , は区切り。止めないと次のアイテムを飲み込む",
            ),
        ];
        for (src, line, expected, why) in cases {
            assert_eq!(sig(src, *line), *expected, "{why}");
        }
    }

    #[test]
    fn メンバを持つ定義は本体ごと出す() {
        let cases: &[(&str, usize, &[&str], &str)] = &[
            (
                "pub struct Foo {\n    pub a: usize,\n    pub b: String,\n}\npub struct Next;\n",
                1,
                &[
                    "pub struct Foo {",
                    "    pub a: usize,",
                    "    pub b: String,",
                    "}",
                ],
                "次のアイテムまで飲み込まない",
            ),
            (
                "enum E {\n    A,\n    B { x: u8 },\n    C(String),\n}\n",
                1,
                &[
                    "enum E {",
                    "    A,",
                    "    B { x: u8 },",
                    "    C(String),",
                    "}",
                ],
                "入れ子の波括弧を数えて閉じる",
            ),
            (
                "mod m {\n    pub struct Inner {\n        pub a: u8,\n    }\n}\n",
                2,
                &["pub struct Inner {", "    pub a: u8,", "}"],
                "字下げは左へ詰める",
            ),
            (
                "pub trait Store {\n    fn len(&self) -> usize;\n}\n",
                1,
                &["pub trait Store {", "    fn len(&self) -> usize;", "}"],
                "trait も interface と同じくメソッドの並びを出す",
            ),
        ];
        for (src, line, expected, why) in cases {
            assert_eq!(body(src, *line).unwrap(), *expected, "{why}");
        }
    }

    #[test]
    fn 本体を持たない宣言は定義全体にならない() {
        assert!(body("pub struct Unit;\n", 1).is_none());
        assert!(body("pub struct Tuple(u32);\n", 1).is_none());
        // class は本体がメソッドの実装なので、そもそも本体を出しに行かない。
        assert!(has_member_list("trait") && has_member_list("Trait"));
        assert!(!has_member_list("class"));
    }

    #[test]
    fn 長すぎる定義は残り行数を添えて切る() {
        let mut src = String::from("pub struct Big {\n");
        for i in 0..40 {
            src.push_str(&format!("    pub f{i}: u8,\n"));
        }
        src.push_str("}\n");
        let out = body(&src, 1).unwrap();
        assert_eq!(out.len(), MAX_DEFINITION_LINES + 1);
        assert_eq!(out.last().unwrap(), "\u{2026} (+18 lines)");
    }

    #[test]
    fn docは記法をまたいで定義の直上から拾う() {
        type Case = (&'static str, usize, &'static [&'static str], &'static str);
        let cases: &[Case] = &[
            (
                "/// Does the thing.\n/// Second line.\n#[derive(Debug)]\npub struct Foo;\n",
                4,
                &["Does the thing.", "Second line."],
                "属性は挟んでいても飛ばす",
            ),
            (
                "// Foo does the thing.\nfunc Foo() {}\n",
                2,
                &["Foo does the thing."],
                "go の行コメント",
            ),
            (
                "/**\n * Does the thing.\n * @param a input\n */\nfunction foo(a) {}\n",
                5,
                &["Does the thing.", "@param a input"],
                "typescript のブロックコメント",
            ),
            (
                "/** Does the thing. */\nfunction foo() {}\n",
                2,
                &["Does the thing."],
                "1 行のブロックコメント",
            ),
            (
                "let x = 1;\nfn foo() {}\n",
                2,
                &[],
                "上がコードなら doc は無い",
            ),
        ];
        for (src, line, expected, why) in cases {
            assert_eq!(doc(src, *line), *expected, "{why}");
        }
    }

    fn hover_fixture() -> Hover {
        Hover {
            word: "add".into(),
            kind: "fn".into(),
            container: None,
            path: "lib.rs".into(),
            line: 3,
            doc: vec!["Adds.".into()],
            signature: vec!["pub fn add(a: i64) -> i64".into()],
            def_count: 1,
            refs: 2,
            refs_capped: false,
            on_definition_line: false,
            signature_from_index: false,
            by: crate::panels::viewer::code_nav::By::TreeSitter,
            anchor: (10, 10),
            left_at: None,
            pinned: false,
        }
    }

    #[test]
    fn フッターの2行はポップアップの中にあり互いに重ならない() {
        let host = Rect::new(0, 0, 80, 30);
        let popup = popup(&hover_fixture(), &Theme::default(), host);
        assert!(popup.rect.y >= host.y && popup.rect.y + popup.rect.height <= host.y + host.height);
        assert_eq!(popup.refs_row.y, popup.def_row.y + 1);
        assert!(popup.def_row.y >= popup.rect.y);
        assert!(popup.refs_row.y < popup.rect.y + popup.rect.height);
    }

    /// gd は名乗るのに同じ位置を説明するホバーが黙ると、名前一致で拾った宣言が
    /// 索引の答えと同じ見た目になる。
    #[test]
    fn 定義位置の行はどの層の答えかを名乗る() {
        use crate::panels::viewer::code_nav::By;
        let mut hover = hover_fixture();
        assert!(
            def_label(&hover).ends_with("[tree-sitter]"),
            "{}",
            def_label(&hover)
        );
        hover.by = By::Index;
        assert!(
            def_label(&hover).ends_with("[index]"),
            "{}",
            def_label(&hover)
        );
    }

    #[test]
    fn 参照が無ければ参照の行そのものを出さない() {
        let mut hover = hover_fixture();
        hover.refs = 0;
        let popup = popup(&hover, &Theme::default(), Rect::new(0, 0, 80, 30));
        assert_eq!(popup.refs_row, Rect::default());
        assert_eq!(popup.footer.len(), 1);
    }

    #[test]
    fn 数え切れなかった参照は件数に印を付ける() {
        let mut hover = hover_fixture();
        hover.refs = REF_CAP;
        hover.refs_capped = true;
        assert_eq!(refs_label(&hover), "\u{25b8} 50+ refs");
        hover.refs_capped = false;
        assert_eq!(refs_label(&hover), "\u{25b8} 50 refs");
    }

    #[test]
    fn 下に余白が無ければアンカーの上に置く() {
        let host = Rect::new(0, 0, 80, 30);
        let above = place(host, (5, 28), 20, 10);
        assert!(above.y + above.height <= 28, "{above:?}");
        let below = place(host, (5, 2), 20, 10);
        assert_eq!(below.y, 3);
    }

    #[test]
    fn 見出しは所属と種別が揃ったときだけ2つ並ぶ() {
        let mut hover = hover_fixture();
        assert_eq!(header(&hover), "fn");
        hover.container = Some("app::App".into());
        assert_eq!(header(&hover), "app::App  fn");
        hover.kind = String::new();
        assert_eq!(header(&hover), "app::App");
        hover.container = None;
        assert!(header(&hover).is_empty());
    }
}
