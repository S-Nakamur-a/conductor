//! Transcript フレーバーのフェンス付きコードブロックの色付け（code_colors.rs）に
//! 関する受け入れテスト: 実物の Claude Code のキャプチャと突き合わせた、
//! ソースフィクスチャと期待される8色基本 ANSI カラーのトークン。rendering.rs
//! （プロジェクトのサイズ目安に既に達している）へ追記せず独立ファイルにしているのは、
//! これらが独立して完結した別種のフィクスチャ群であるため。

use super::*;
use ratatui::style::Color;

/// Transcript フレーバーでフェンス付きコードブロック1つを描画する。
fn code_lines(lang: &str, source: &str) -> Vec<Line<'static>> {
    let text = format!("```{lang}\n{source}\n```");
    render_transcript(&text, 100)
}

/// 部分一致ではなく完全一致。隣接する同スタイルのトークンは 1 span に融合するので、
/// 呼び出し側は融合後の連続文字列を渡すこと (文字列リテラルなら "hi" のように)。
fn span<'a>(lines: &'a [Line<'static>], text: &str) -> &'a Span<'static> {
    lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == text)
        .unwrap_or_else(|| panic!("no span with exact content {text:?}"))
}

fn assert_fg(lines: &[Line<'static>], text: &str, color: Color) {
    let s = span(lines, text);
    assert_eq!(
        s.style.fg,
        Some(color),
        "{text:?} expected fg {color:?}, got {:?}",
        s.style.fg
    );
}

fn assert_fg_dim(lines: &[Line<'static>], text: &str, color: Color) {
    let s = span(lines, text);
    assert_eq!(
        s.style.fg,
        Some(color),
        "{text:?} expected fg {color:?}, got {:?}",
        s.style.fg
    );
    assert!(
        s.style.add_modifier.contains(Modifier::DIM),
        "{text:?} expected DIM modifier"
    );
}

/// span を単語以外の文字で分割して完全一致を探す (スタイルなしの識別子は括弧やコロンに
/// くっつくため)。DIM が無いことも見るのは、fg だけだと Builtin (Cyan) を期待した箇所で
/// Type (Cyan + DIM) を黙って通すため。DIM 付きが要るなら双子を増やさずここを拡張すること。
fn assert_word_fg(lines: &[Line<'static>], word: &str, color: Color) {
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    for line in lines {
        for s in &line.spans {
            if s.content
                .split(|c: char| !is_word_char(c))
                .any(|tok| tok == word)
            {
                assert_eq!(
                    s.style.fg,
                    Some(color),
                    "{word:?} expected fg {color:?}, got {:?}",
                    s.style.fg
                );
                assert!(
                    !s.style.add_modifier.contains(Modifier::DIM),
                    "{word:?} unexpectedly carries DIM (Type vs Builtin mix-up?)"
                );
                return;
            }
        }
    }
    panic!("no occurrence of word {word:?}");
}

#[test]
fn transcriptのコードブロックにカードの装飾は無い() {
    // 背景色なし、字下げなし、空のパディング行なし — 内容は0列目から始まり、
    // ブロックはソースの行数ぴったりの行数になる。
    let lines = code_lines("rust", "let x = 1;");
    assert_eq!(lines.len(), 1, "no top/bottom padding rows");
    for span in &lines[0].spans {
        assert_eq!(span.style.bg, None, "no background colour");
    }
    assert_eq!(
        lines[0].spans[0].content.as_ref(),
        "let",
        "content starts at column 0, no left inset"
    );
}

#[test]
fn transcriptのコードブロックのrustの配色() {
    let src = "// a line comment\n\
/// a doc comment\n\
fn f(a: &str, b: bool) -> Option<u32> {\n\
    let s = String::from(\"hi\");   // trailing\n\
    const N: usize = 42;\n\
    if b { Some(N as u32) } else { None }\n\
}\n\
struct S { field: Vec<u8> }";
    let lines = code_lines("rust", src);

    assert_fg(&lines, "// a line comment", Color::Green);
    assert_fg(&lines, "/// a doc comment", Color::Green);
    assert_fg(&lines, "// trailing", Color::Green);
    assert_fg(&lines, "42", Color::Green);

    for kw in ["fn", "let", "const", "if", "as", "else", "struct", "None"] {
        assert_fg(&lines, kw, Color::Blue);
    }

    assert_fg(&lines, "f", Color::Yellow);
    assert_fg(&lines, "from", Color::Yellow);
    assert_fg(&lines, "Some", Color::Yellow);

    for ty in [
        "str", "bool", "Option", "u32", "String", "usize", "Vec", "u8",
    ] {
        assert_fg_dim(&lines, ty, Color::Cyan);
    }

    assert_fg(&lines, "\"hi\"", Color::Red);

    // struct 名は関数/型名としてではなく、スタイルなしとして読まれる。
    // これらの素の識別子は周囲の句読点/空白と一緒により広い Reset span へ
    // 融合されるので、正確な span 内容ではなく単語単位で探す。
    assert_word_fg(&lines, "S", Color::Reset);
    for ident in ["s", "N", "b", "field"] {
        assert_word_fg(&lines, ident, Color::Reset);
    }
}

#[test]
fn transcriptのコードブロックのpythonの配色() {
    let src = "# python comment\n\
import os\n\
def g(x=1, *args):\n\
    s = f\"val {x}\"\n\
    return [i for i in range(10) if i % 2 == 0]\n\
class C(object):\n\
    pass";
    let lines = code_lines("python", src);

    assert_fg(&lines, "# python comment", Color::Green);

    for kw in [
        "import", "def", "return", "for", "in", "if", "class", "object", "pass",
    ] {
        assert_fg(&lines, kw, Color::Blue);
    }

    assert_fg(&lines, "g", Color::Yellow);
    assert_fg(&lines, "range", Color::Cyan);

    for num in ["1", "10", "2", "0"] {
        assert_fg(&lines, num, Color::Green);
    }

    // 補間された f 文字列: 埋め込み式より前のリテラル部分は赤。埋め込み式の
    // 最初のトークン以降は、閉じ引用符を含めて既定色に戻る。
    assert_fg(&lines, "f\"val ", Color::Red);
    assert_fg(&lines, "{x}\"", Color::Reset);

    for ident in ["os", "C", "s", "i", "args"] {
        assert_word_fg(&lines, ident, Color::Reset);
    }
}

#[test]
fn transcriptのコードブロックのbashの配色() {
    let src = "# shell comment\n\
export FOO=bar\n\
if [ -f \"$HOME/.zshrc\" ]; then\n\
  echo \"yes\" | grep -q y && ls -la\n\
fi";
    let lines = code_lines("bash", src);

    assert_fg(&lines, "# shell comment", Color::Green);

    for builtin in ["export", "echo", "ls"] {
        assert_fg(&lines, builtin, Color::Cyan);
    }
    for kw in ["if", "then", "fi"] {
        assert_fg(&lines, kw, Color::Blue);
    }

    assert_fg(&lines, "\"yes\"", Color::Red);
    // 2つ目の文字列の開き引用符: $HOME による中断の直前まで（それ自体は含まない）
    // 赤。その文字列の残り部分は、閉じ引用符も含めて既定色に戻る
    // （末尾の ]; と一緒に1つの Reset span へ融合されるので、単語単位で検査する）。
    assert_fg(&lines, "\"", Color::Red);
    assert_word_fg(&lines, "zshrc", Color::Reset);

    // grep は ls/echo/export と違い、組み込みコマンドとして認識されない。
    assert_word_fg(&lines, "grep", Color::Reset);
}

#[test]
fn transcriptのコードブロックのjsonの配色() {
    let src = "{\"a\": 1, \"b\": true, \"c\": null, \"d\": [\"x\", 2.5]}";
    let lines = code_lines("json", src);

    for key in ["\"a\"", "\"b\"", "\"c\"", "\"d\""] {
        assert_fg(&lines, key, Color::Cyan);
    }
    for num in ["1", "2.5"] {
        assert_fg(&lines, num, Color::Green);
    }
    for lit in ["true", "null"] {
        assert_fg(&lines, lit, Color::Blue);
    }
    assert_fg(&lines, "\"x\"", Color::Red);
}
