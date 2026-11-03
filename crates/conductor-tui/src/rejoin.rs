//! 画面の行の境目が折り返しか本物の改行かを、画面の元になった本文と照らして決める。
//!
//! Claude Code は自分で折り返し、行末でカーソルを動かして次の行へ書く。端末の折り返しの
//! 印 (vt100 の row_wrapped) は立たないので、画面だけでは本物の改行と見分けられない。

use std::ops::Range;

use conductor_core::claude_log::{DisplayBlock, LogEntry};
use unicode_width::UnicodeWidthChar;

/// 行と次の行のあいだに入れるもの。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Break {
    Newline,
    /// 語の切れ目で折り返した。
    Space,
    /// 語や全角の並びの途中で折り返した。続きの行の字下げは描画が足したもの。
    Glue,
    /// 端末が自動で折り返した。行末の空白も中身なので削らない。
    SoftWrap,
}

/// 照合で読み飛ばす字。markdown の記号は描くと消え、印は描画が足す。
/// 表の罫線 │ は入れない。ログの | と鍵が食い違うので、セルの折り返しが繋がらずに済む。
const DECORATION: &[char] = &[
    '*', '`', '#', '-', '•', '⏺', '⎿', '❯', '✻', '>', '_', '[', ']', '(', ')',
];

/// 同じ並びがこれより多く現れる行は、どこの写しか決められないとみなす。
const MAX_OCCURRENCES: usize = 64;

/// 鍵の字から本文の位置を引くための間引いた目印の間隔。全字ぶん持つと 45MB のログで数百 MB になる。
const CHECKPOINT_EVERY: usize = 1024;

fn is_key(c: char) -> bool {
    !c.is_whitespace() && !DECORATION.contains(&c)
}

fn is_wide(c: char) -> bool {
    c.width() == Some(2)
}

/// 並びの端がここで切れてよいか。語の途中で見つかった並びは別の文の一部。
fn is_edge(neighbour: Option<char>, end: char) -> bool {
    is_wide(end) || neighbour.is_none_or(|c| !is_key(c) || is_wide(c))
}

/// 会話ログのうち、Claude Code が文として画面に書く部分。ツール呼び出しの入力は
/// 画面では `Bash(…)` のような形に組み直されるので入れない。
pub fn log_text(entries: &[LogEntry]) -> String {
    let mut out = String::new();
    let mut push = |text: &str| {
        out.push_str(text);
        out.push('\n');
    };
    for block in entries.iter().flat_map(|entry| &entry.blocks) {
        match block {
            DisplayBlock::Text(text)
            | DisplayBlock::Notice(text)
            | DisplayBlock::Thinking { text, .. }
            | DisplayBlock::TeammateMessage { body: text, .. } => push(text),
            DisplayBlock::ToolResult { lines, .. } | DisplayBlock::Annotation { lines } => {
                push(&lines.join("\n"))
            }
            DisplayBlock::ToolUse { .. } | DisplayBlock::CompactBoundary => {}
        }
    }
    out
}

fn key_of(text: &str) -> String {
    text.chars().filter(|&c| is_key(c)).collect()
}

/// 本文から鍵の字だけを抜いたもの。
struct Keyed<'a> {
    source: &'a str,
    key: String,
    /// (鍵の byte 位置, 本文の byte 位置) を CHECKPOINT_EVERY 字ごとに。
    checkpoints: Vec<(usize, usize)>,
}

impl<'a> Keyed<'a> {
    fn new(source: &'a str) -> Self {
        let mut key = String::new();
        let mut checkpoints = Vec::new();
        for (at, c) in source.char_indices().filter(|&(_, c)| is_key(c)) {
            if key.len() / CHECKPOINT_EVERY >= checkpoints.len() {
                checkpoints.push((key.len(), at));
            }
            key.push(c);
        }
        Self {
            source,
            key,
            checkpoints,
        }
    }

    /// 鍵の byte 位置 `at` の字が本文で占める範囲。
    fn source_span(&self, at: usize) -> Range<usize> {
        let index = self.checkpoints.partition_point(|&(k, _)| k <= at) - 1;
        let (mut key_at, from) = self.checkpoints[index];
        for (offset, c) in self.source[from..]
            .char_indices()
            .filter(|&(_, c)| is_key(c))
        {
            if key_at == at {
                return from + offset..from + offset + c.len_utf8();
            }
            key_at += c.len_utf8();
        }
        unreachable!("鍵の字は必ず本文にある")
    }

    /// 鍵の範囲 `range` が本文の中で語の境に収まっていれば、その本文の範囲。
    fn bounded(&self, range: Range<usize>) -> Option<Range<usize>> {
        let first = self.key[range.clone()].chars().next()?;
        let last = self.key[range.clone()].chars().next_back()?;
        let start = self.source_span(range.start).start;
        let end = self.source_span(range.end - last.len_utf8()).end;
        let before = self.source[..start].chars().next_back();
        let after = self.source[end..].chars().next();
        (is_edge(before, first) && is_edge(after, last)).then_some(start..end)
    }
}

fn classify(gap: &str) -> Break {
    if gap.contains('\n') {
        Break::Newline
    } else if gap.chars().any(char::is_whitespace) {
        Break::Space
    } else {
        Break::Glue
    }
}

/// 画面の行 `rows` (区画の上から順、選んでいない行も含む) の各境目の区切り。
///
/// 隣り合う行の鍵が本文で続いている所を鎖にし、鎖ごと本文から探す。区切りは本文で
/// 2 つの鍵字の間にある字で決め、出現ごとに区切りが割れたら改行にする。
/// 間違った区切りで黙って写すより、今までどおりの改行の方がましだから。
pub fn breaks(rows: &[String], source: &str) -> Vec<Break> {
    let mut out = vec![Break::Newline; rows.len().saturating_sub(1)];
    if out.is_empty() {
        return out;
    }
    let keyed = Keyed::new(source);
    let keys: Vec<String> = rows.iter().map(|row| key_of(row)).collect();
    let linked: Vec<bool> = keys
        .windows(2)
        .map(|pair| {
            !pair[0].is_empty() && !pair[1].is_empty() && keyed.key.contains(&pair.concat())
        })
        .collect();

    let mut first = 0;
    while first < linked.len() {
        if !linked[first] {
            first += 1;
            continue;
        }
        let last = first + linked[first..].iter().take_while(|&&l| l).count();
        let chain = &keys[first..=last];
        if let Some(found) = chain_breaks(&keyed, chain) {
            out[first..last].copy_from_slice(&found);
        }
        first = last;
    }
    out
}

/// 鎖 `chain` の境目の区切り。どの出現でも同じだった境目だけ決まり、割れた境目は改行。
fn chain_breaks(keyed: &Keyed<'_>, chain: &[String]) -> Option<Vec<Break>> {
    let whole = chain.concat();
    let mut agreed: Option<Vec<Option<Break>>> = None;
    for (count, (at, _)) in keyed.key.match_indices(whole.as_str()).enumerate() {
        if count >= MAX_OCCURRENCES {
            return None;
        }
        if keyed.bounded(at..at + whole.len()).is_none() {
            continue;
        }
        let mut seam = at;
        let found: Vec<Break> = chain[..chain.len() - 1]
            .iter()
            .map(|key| {
                seam += key.len();
                let last = key.chars().next_back().expect("鎖の鍵は空でない");
                let gap_start = keyed.source_span(seam - last.len_utf8()).end;
                let gap_end = keyed.source_span(seam).start;
                classify(&keyed.source[gap_start..gap_end])
            })
            .collect();
        agreed = Some(match agreed {
            None => found.into_iter().map(Some).collect(),
            Some(before) => before
                .into_iter()
                .zip(found)
                .map(|(a, b)| a.filter(|&a| a == b))
                .collect(),
        });
    }
    agreed.map(|seams| {
        seams
            .into_iter()
            .map(|b| b.unwrap_or(Break::Newline))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(rows: &str) -> Vec<String> {
        rows.lines().map(str::to_string).collect()
    }

    /// 本物の Claude Code に書かせた応答の、ログの本文そのまま。これを使うテストの画面の行は
    /// 同じ会話を幅 60 で録ったもの (枠と行末の空白は除いた)。
    const REPLY: &str = "- first bullet item that is long enough to wrap onto a second line in a narrow terminal ok\n- second item\n\n1. numbered item that is also long enough to wrap onto the next line in a narrow window\n2. two\n\n```js\nconst result = await fetchSomething(alphaValue, betaValue, gamma);\n}\n}\nreturn result;\n```\n\naaaaaaaaa bbbbbbbbb ccccccccc ddddddddd eeeeeeeee abcdefg. next words here\n\nThis has **bold text that spans across the wrap point of the line** and `inline code that also spans the wrap point` too.\n\nSee [the docs page](https://example.com/docs/page) and _underscored emphasis that crosses the wrap boundary here_.\n\n| col a | col b |\n|---|---|\n| one | a cell with enough text to wrap inside the narrow table |\n\n1\n2\n3\n\nPR is here:\nhttps://github.com/S-Nakamur-a/conductor/pull/123456789012345678901234567890";

    fn breaks_of(rows: &str, source: &str) -> Vec<Break> {
        breaks(&screen(rows), source)
    }

    #[test]
    fn 箇条書きの折り返しは空白で繋ぎ次の項目は改行のまま() {
        let rows = "⏺ - first bullet item that is long enough to wrap onto a\n    second line in a narrow terminal ok\n  - second item";
        assert_eq!(breaks_of(rows, REPLY), vec![Break::Space, Break::Newline]);
    }

    #[test]
    fn 番号付きの項目の折り返しも繋ぐ() {
        let rows = "  1. numbered item that is also long enough to wrap onto the\n     next line in a narrow window\n  2. two";
        assert_eq!(breaks_of(rows, REPLY), vec![Break::Space, Break::Newline]);
    }

    #[test]
    fn コードの折り返しは繋ぎ本物の改行は残す() {
        let rows = "  const result = await fetchSomething(alphaValue, betaValue,\n  gamma);\n  }\n  }\n  return result;";
        assert_eq!(
            breaks_of(rows, REPLY),
            vec![Break::Space, Break::Newline, Break::Newline, Break::Newline]
        );
    }

    #[test]
    fn 句点で終わる行の折り返しは空白で繋ぐ() {
        let rows =
            "  aaaaaaaaa bbbbbbbbb ccccccccc ddddddddd eeeeeeeee abcdefg.\n  next words here";
        assert_eq!(breaks_of(rows, REPLY), vec![Break::Space]);
    }

    #[test]
    fn 太字やコードの装飾をまたぐ折り返しも繋ぐ() {
        let rows = "  This has bold text that spans across the wrap point of the\n  line and inline code that also spans the wrap point too.";
        assert_eq!(breaks_of(rows, REPLY), vec![Break::Space]);
    }

    #[test]
    fn リンクと下線の強調をまたぐ折り返しも繋ぐ() {
        let rows = "  See the docs page (https://example.com/docs/page) and\n  underscored emphasis that crosses the wrap boundary here.";
        assert_eq!(breaks_of(rows, REPLY), vec![Break::Space]);
    }

    #[test]
    fn 一字だけの行が並んでも別の場所の数字に吸われない() {
        let rows = "  1\n  2\n  3";
        assert_eq!(breaks_of(rows, REPLY), vec![Break::Newline, Break::Newline]);
    }

    #[test]
    fn 途中で切れたurlは区切りなしで繋ぎ前のラベル行とは繋がない() {
        let rows = "  PR is here:\n  https://github.com/S-Nakamur-a/conductor/pull/123456789012\n  345678901234567890";
        assert_eq!(breaks_of(rows, REPLY), vec![Break::Newline, Break::Glue]);
    }

    #[test]
    fn 段落の間の空行と表は改行のまま() {
        let rows = "  │ one  │ a cell with enough text to wrap inside the   │\n  │      │ narrow table                                 │\n\n  1";
        assert_eq!(
            breaks_of(rows, REPLY),
            vec![Break::Newline, Break::Newline, Break::Newline]
        );
    }

    #[test]
    fn 罫線で囲んだ表のセルの折り返しは繋がない() {
        let rows = "  │ 日本語の長い │\n  │ 説明文 │";
        assert_eq!(
            breaks_of(rows, "| 日本語の長い説明文 |"),
            vec![Break::Newline]
        );
    }

    #[test]
    fn 日本語の字の途中の折り返しは区切りなしで繋ぐ() {
        let source = "日本語の文章がどこで折り返されるかを確かめるための長い一文です。ここまでで約四十文字くらいになります。";
        let rows = "⏺ 日本語の文章がどこで折り返されるかを確かめるための長い一文\n  です。ここまでで約四十文字くらいになります。";
        assert_eq!(breaks_of(rows, source), vec![Break::Glue]);
    }

    #[test]
    fn ログに無い行は改行のまま() {
        let rows = "✻ Brewed for 3s · done 2:28 PM\n  (no branch) | Haiku 5.5   medium | ctx\n  ⏵⏵ auto mode on (shift+tab to cycle)";
        assert_eq!(breaks_of(rows, REPLY), vec![Break::Newline, Break::Newline]);
    }

    #[test]
    fn 語の途中にしか見つからない並びでは繋がない() {
        assert_eq!(
            breaks_of("  1\n  2", "see https://example.com/pull/12"),
            vec![Break::Newline]
        );
    }

    #[test]
    fn 区切りが出現ごとに割れたら改行にする() {
        let source = "alpha beta gamma\n\nalpha beta\ngamma";
        assert_eq!(breaks_of("alpha beta\ngamma", source), vec![Break::Newline]);
    }

    #[test]
    fn 目印の間隔より後ろの字も本文の位置を引ける() {
        let filler = "x ".repeat(CHECKPOINT_EVERY * 3);
        let source = format!("{filler}\nwrapped across the edge here");
        assert_eq!(
            breaks_of("wrapped across the\nedge here", &source),
            vec![Break::Space]
        );
    }
}
