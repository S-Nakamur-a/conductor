//! message.content を表示用ブロックへ正規化する。ツール呼び出しと結果の対応付け、
//! user ターンに CLI が書き込むラッパー形式の畳み込みを含む。

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::model::{DisplayBlock, Role};
use super::sanitize::sanitize_line;
use super::schema::{Block, Content, LogRecord, ToolResultContent};
use super::tool_class::{ResultKind, result_kind};

/// tool_use と tool_result の対応。セッション全体を通して 1 つ持つ。
///
/// result は必ず呼び出しの後のレコードに来るので、呼び出しの印をエラー色にするには
/// 先に全レコードを舐めてエラーだった id を集めておく必要がある。
pub(super) struct ToolPairing {
    errored: HashSet<String>,
    kinds: HashMap<String, ResultKind>,
}

impl ToolPairing {
    pub(super) fn scan(records: &[LogRecord]) -> Self {
        let mut errored = HashSet::new();
        for record in records {
            let Some(Content::Blocks(blocks)) = record.message.as_ref().map(|m| &m.content) else {
                continue;
            };
            for block in blocks {
                if let Block::ToolResult {
                    tool_use_id,
                    is_error: true,
                    ..
                } = block
                {
                    errored.insert(tool_use_id.clone());
                }
            }
        }
        Self {
            errored,
            kinds: HashMap::new(),
        }
    }

    /// 呼び出しを記録し、対応する結果がエラーだったかを返す。
    fn note_use(&mut self, id: &str, name: &str, input: &Value) -> bool {
        if id.is_empty() {
            return false;
        }
        self.kinds.insert(id.to_string(), result_kind(name, input));
        self.errored.contains(id)
    }

    /// 対応の取れない結果は Hidden。どのツールのものか分からないまま描いてもノイズにしかならない。
    fn kind_of(&self, tool_use_id: &str) -> ResultKind {
        self.kinds
            .get(tool_use_id)
            .copied()
            .unwrap_or(ResultKind::Hidden)
    }
}

pub(super) fn result_lines(content: &ToolResultContent) -> Vec<String> {
    match content {
        ToolResultContent::None => Vec::new(),
        ToolResultContent::Text(s) => s.lines().map(sanitize_line).collect(),
        ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .flat_map(|b| b.text.lines().map(sanitize_line))
            .collect(),
    }
}

/// 終了タグが無ければ末尾まで。
fn tag_inner<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let rest = &text[start..];
    Some(rest.find(&close).map_or(rest, |end| &rest[..end]))
}

fn attr_value<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("{name}=\"");
    let start = attrs.find(&needle)? + needle.len();
    let rest = &attrs[start..];
    Some(&rest[..rest.find('"')?])
}

/// summary 属性は読まない。展開時に見える body だけが本体を持つ。
fn parse_teammate_message(lead: &str) -> Option<(String, String)> {
    const OPEN_PREFIX: &str = "<teammate-message";
    const CLOSE: &str = "</teammate-message>";
    let tag_end = lead.find('>')?;
    let id = attr_value(&lead[OPEN_PREFIX.len()..tag_end], "teammate_id")?;
    let rest = &lead[tag_end + 1..];
    let body = rest.find(CLOSE).map_or(rest, |end| &rest[..end]);
    Some((id.to_string(), body.trim().to_string()))
}

/// CLI が user ターンの中に記録するラッパー形式を、画面で見えていた形に畳む。
///
/// ラッパーは先頭にある場合だけ認識する。例外は task-notification で、Claude Code は
/// メッセージのどこにあっても最初の summary だけを残し、周りの文章ごと捨てる
/// (summary が無ければメッセージ全体が消える)。system-reminder は畳まない。
/// Claude Code はインラインでも単独でもそのまま描き、見えないものは isMeta 側で隠れる。
fn normalise_user_text(text: String) -> Option<DisplayBlock> {
    let lead = text.trim_start();
    if lead.starts_with("<teammate-message")
        && let Some((id, body)) = parse_teammate_message(lead)
    {
        return Some(DisplayBlock::TeammateMessage { id, body });
    }
    if lead.contains("<task-notification>") {
        let summary = tag_inner(lead, "summary")
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        return Some(DisplayBlock::Notice(sanitize_line(summary)));
    }
    if lead.starts_with("<command-name>")
        && lead.contains("</command-name>")
        && let Some(name) = tag_inner(lead, "command-name")
    {
        let args = tag_inner(lead, "command-args").unwrap_or("").trim();
        let display = if args.is_empty() {
            name.trim().to_string()
        } else {
            format!("{} {}", name.trim(), args)
        };
        return (!display.is_empty()).then_some(DisplayBlock::Text(display));
    }
    if lead.starts_with("<local-command-stdout>")
        && let Some(stdout) = tag_inner(lead, "local-command-stdout")
    {
        let lines: Vec<String> = stdout
            .trim()
            .lines()
            .map(sanitize_line)
            .filter(|l| !l.trim().is_empty())
            .collect();
        return (!lines.is_empty()).then_some(DisplayBlock::Annotation { lines });
    }
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| DisplayBlock::Text(trimmed.to_string()))
}

/// content の 2 つの表層形式 (素の文字列と型付きブロック配列) を同じ列に平坦化する。
/// ラッパーの畳み込みは user ターンだけ。assistant はタグを正当に引用することがある。
pub(super) fn content_to_display_blocks(
    content: Content,
    role: &Role,
    pairing: &mut ToolPairing,
    thinking_duration_secs: u64,
) -> Vec<DisplayBlock> {
    let text_block = |text: String| -> Option<DisplayBlock> {
        match role {
            _ if text.is_empty() => None,
            Role::User => normalise_user_text(text),
            Role::Assistant => Some(DisplayBlock::Text(text)),
        }
    };
    let blocks = match content {
        Content::Text(s) => return text_block(s).into_iter().collect(),
        Content::Blocks(blocks) => blocks,
    };
    blocks
        .into_iter()
        .filter_map(|block| match block {
            Block::Text { text } => text_block(text),
            Block::Thinking { thinking } => Some(DisplayBlock::Thinking {
                text: thinking,
                duration_secs: thinking_duration_secs,
            }),
            Block::ToolUse { id, name, input } => {
                let errored = pairing.note_use(&id, &name, &input);
                Some(DisplayBlock::ToolUse {
                    name,
                    input,
                    errored,
                })
            }
            Block::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some(DisplayBlock::ToolResult {
                kind: pairing.kind_of(&tool_use_id),
                lines: result_lines(&content),
                is_error,
            }),
            Block::Other => None,
        })
        .collect()
}
