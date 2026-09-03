//! 行単位のブロック解析。生の Markdown テキストを MdBlock に分割する。

/// 解析済み summary のブロック1つ分。パーサは行単位なので、ほとんどのブロックは
/// ソースの1行に対応する。CodeBlock だけが複数行にまたがる。
#[derive(Debug, PartialEq)]
pub(crate) enum MdBlock {
    /// # heading 〜 ###### heading（レベル1〜6）。
    Heading {
        level: u8,
        text: String,
    },
    /// 通常のテキスト行。著者が入れた改行はそのまま保持する（1行につき1ブロック）。
    Paragraph(String),
    /// - item / * item / + item、または 1. item / 1) item。
    ListItem {
        /// 順序付き項目なら Some("1")（著者が書いた番号をそのまま保持）、
        /// 箇条書きなら None。
        ordered: Option<String>,
        /// GFM のタスクマーカー: None は通常項目、Some(false) は [ ]（未完了）、
        /// Some(true) は [x]（完了）。
        checked: Option<bool>,
        text: String,
        /// マーカーの前にある先頭空白の桁数（ネストの字下げ）。
        indent: usize,
    },
    /// > quoted text。
    Quote(String),
    /// フェンス付きコードブロック。lang は info 文字列の最初のトークン（あれば）。
    CodeBlock {
        lang: Option<String>,
        lines: Vec<String>,
    },
    /// GFM のパイプテーブル: ヘッダー行、アライメント行、0行以上の本体行からなる。
    /// aligns はヘッダー列ごとに1要素持つ。
    Table {
        headers: Vec<String>,
        aligns: Vec<Align>,
        rows: Vec<Vec<String>>,
    },
    Rule,
    /// 空のソース行（段落間の余白として保持する）。
    Blank,
}

/// MdBlock::Table の列ごとのテキスト配置。デリミタ行のコロンから決まる
/// （:-- は左寄せ、--: は右寄せ、:-: は中央寄せ）。
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

/// text をブロックに分割する。行は \n で区切り、末尾の \r（CRLF 入力）は取り除いて、
/// フェンス検出やコード本体がきれいな状態を保つようにする。
pub(crate) fn parse_blocks(text: &str) -> Vec<MdBlock> {
    let lines: Vec<&str> = text.split('\n').map(strip_cr).collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        if let Some((fence_char, fence_len, info)) = fence_open(trimmed) {
            let lang = info
                .split_whitespace()
                .next()
                .map(str::to_string)
                .filter(|s| !s.is_empty());
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() {
                if is_fence_close(lines[i].trim_start(), fence_char, fence_len) {
                    i += 1;
                    break;
                }
                body.push(lines[i].to_string());
                i += 1;
            }
            blocks.push(MdBlock::CodeBlock { lang, lines: body });
            continue;
        }

        // GFM テーブル — ヘルパーがテーブル全体を消費する。
        if let Some((table, consumed)) = parse_table_at(&lines, i) {
            blocks.push(table);
            i += consumed;
            continue;
        }

        if trimmed.is_empty() {
            blocks.push(MdBlock::Blank);
        } else if is_hr(trimmed) {
            blocks.push(MdBlock::Rule);
        } else if let Some((level, htext)) = parse_heading(trimmed) {
            blocks.push(MdBlock::Heading { level, text: htext });
        } else if let Some(rest) = trimmed.strip_prefix('>') {
            blocks.push(MdBlock::Quote(
                rest.strip_prefix(' ').unwrap_or(rest).to_string(),
            ));
        } else if let Some(item) = parse_list_item(line) {
            blocks.push(item);
        } else {
            blocks.push(MdBlock::Paragraph(trimmed.to_string()));
        }
        i += 1;
    }
    blocks
}

fn strip_cr(s: &str) -> &str {
    s.strip_suffix('\r').unwrap_or(s)
}

fn fence_open(s: &str) -> Option<(char, usize, &str)> {
    let first = s.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let len = s.chars().take_while(|&c| c == first).count();
    if len < 3 {
        return None;
    }
    // フェンス文字はどちらも ASCII なので、len はバイトオフセットと一致する。
    Some((first, len, s[len..].trim()))
}

fn is_fence_close(s: &str, fence_char: char, fence_len: usize) -> bool {
    let len = s.chars().take_while(|&c| c == fence_char).count();
    len >= fence_len && s.chars().skip(len).all(char::is_whitespace)
}

/// 「---」「***」「___」（同じマーカーが3つ以上、間に空白を挟んでもよい）。
fn is_hr(s: &str) -> bool {
    let marks: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
    if marks.len() < 3 {
        return false;
    }
    let first = marks[0];
    matches!(first, '-' | '*' | '_') && marks.iter().all(|&c| c == first)
}

/// # 〜 ###### → (level, heading_text)。ハッシュの後にスペースが必要
/// （#nofilter、C#、#242 のような issue 参照が段落のまま扱われるように）。
fn parse_heading(s: &str) -> Option<(u8, String)> {
    let hashes = s.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &s[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((hashes as u8, rest.trim_start().to_string()))
}

/// 箇条書き（- / * / +）と順序付き（N. / N)）を ListItem にする。項目テキスト先頭の
/// GFM タスクマーカー（[ ] / [x] ）は checked に切り出す。
fn parse_list_item(line: &str) -> Option<MdBlock> {
    let indent = line.len() - line.trim_start().len();
    let s = line.trim_start();

    if let Some(rest) = s
        .strip_prefix("- ")
        .or_else(|| s.strip_prefix("* "))
        .or_else(|| s.strip_prefix("+ "))
    {
        let (checked, text) = split_task_marker(rest);
        return Some(MdBlock::ListItem {
            ordered: None,
            checked,
            text: text.to_string(),
            indent,
        });
    }

    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        let after = &s[digits.len()..];
        if let Some(rest) = after
            .strip_prefix(". ")
            .or_else(|| after.strip_prefix(") "))
        {
            let (checked, text) = split_task_marker(rest);
            return Some(MdBlock::ListItem {
                ordered: Some(digits),
                checked,
                text: text.to_string(),
                indent,
            });
        }
    }
    None
}

/// "[ ] foo" → (Some(false), "foo")、"[x] foo" → (Some(true), "foo")。マーカーの後は
/// スペースか終端が要るので、"[ ]x" や "[y]" は素の文字列のまま返す。
fn split_task_marker(text: &str) -> (Option<bool>, &str) {
    for (pat, val) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
        if let Some(rest) = text.strip_prefix(pat) {
            if rest.is_empty() {
                return (Some(val), "");
            }
            if let Some(after) = rest.strip_prefix(' ') {
                return (Some(val), after);
            }
        }
    }
    (None, text)
}

/// デリミタ行がゲート。全セルが有効な :?-+:? でなければ 1 行も消費せず候補ごと却下するので、
/// a | b のような段落をテーブルと誤解しない。
fn parse_table_at(lines: &[&str], i: usize) -> Option<(MdBlock, usize)> {
    let header_line = lines.get(i)?;
    if !header_line.contains('|') {
        return None;
    }
    let delim_line = lines.get(i + 1)?;
    let aligns = parse_alignments(&split_table_row(delim_line))?;
    let headers = split_table_row(header_line);
    if headers.is_empty() {
        return None;
    }

    let mut rows = Vec::new();
    let mut j = i + 2;
    while let Some(l) = lines.get(j) {
        if l.trim().is_empty() || !l.contains('|') {
            break;
        }
        rows.push(split_table_row(l));
        j += 1;
    }

    Some((
        MdBlock::Table {
            headers,
            aligns,
            rows,
        },
        j - i,
    ))
}

/// テーブルの行1本を、前後の | が作る空セルを取り除きつつトリム済みセルへ
/// 分割する。"| a | b |" と "a | b" はどちらも ["a", "b"] になる。
/// （エスケープされた \| やコード内のパイプは対象外。）
pub(crate) fn split_table_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// いずれかのセルが有効な :?-+:? でなければ None。「これはテーブルか」の判定も兼ねる。
fn parse_alignments(cells: &[String]) -> Option<Vec<Align>> {
    if cells.is_empty() {
        return None;
    }
    cells
        .iter()
        .map(|c| {
            let c = c.trim();
            let left = c.starts_with(':');
            let right = c.ends_with(':');
            let core = c.trim_start_matches(':').trim_end_matches(':');
            if core.is_empty() || !core.chars().all(|ch| ch == '-') {
                return None;
            }
            Some(match (left, right) {
                (true, true) => Align::Center,
                (false, true) => Align::Right,
                _ => Align::Left,
            })
        })
        .collect()
}
