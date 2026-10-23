//! 会話ログ (.jsonl) の末尾を読んで、題と文脈の量を取る。
//!
//! 長いセッションのログは数十 MB になり、一覧の更新のたびに全体は読み直せない。
//! 題も usage も追記され続けるので、普段は末尾だけで済む。末尾に無いものがあるときだけ、
//! それより前を頭から 1 行ずつ読む。

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

const TAIL_BYTES: u64 = 512 * 1024;
const PROMPT_CHARS: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub title: Title,
    pub context_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Title {
    Custom(String),
    Ai(String),
    FirstPrompt(String),
    None,
}

pub fn summarize(path: &Path) -> io::Result<Summary> {
    let mut file = File::open(path)?;
    let window_start = file.metadata()?.len().saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(window_start))?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;
    // 窓の先頭は行の途中に落ちうる。その行は窓より前の側で頭から読む
    let cut = match window_start {
        0 => 0,
        _ => raw
            .iter()
            .position(|&b| b == b'\n')
            .map_or(raw.len(), |i| i + 1),
    };
    let tail = scan(&String::from_utf8_lossy(&raw[cut..]));

    let before_end = window_start + cut as u64;
    let missing = (tail.custom.is_none() && tail.ai.is_none()) || tail.context_tokens.is_none();
    let before = if before_end > 0 && missing {
        file.seek(SeekFrom::Start(0))?;
        scan_before(BufReader::new(file.take(before_end)))?
    } else {
        Found::default()
    };
    Ok(Found::merge(tail, before))
}

#[derive(Default)]
struct Found {
    custom: Option<String>,
    ai: Option<String>,
    first_prompt: Option<String>,
    context_tokens: Option<u64>,
}

impl Found {
    /// 題は窓の中にあればそれを使い、窓より前のもので上書きしない。
    fn merge(tail: Found, before: Found) -> Summary {
        let context_tokens = tail.context_tokens.or(before.context_tokens);
        let title = if tail.custom.is_some() || tail.ai.is_some() {
            Self::title_of(tail.custom, tail.ai, None)
        } else {
            let first_prompt = before.first_prompt.or(tail.first_prompt);
            Self::title_of(before.custom, before.ai, first_prompt)
        };
        Summary {
            title,
            context_tokens,
        }
    }

    fn title_of(custom: Option<String>, ai: Option<String>, prompt: Option<String>) -> Title {
        match (custom, ai, prompt) {
            (Some(custom), _, _) => Title::Custom(custom),
            (None, Some(ai), _) => Title::Ai(ai),
            (None, None, Some(prompt)) => Title::FirstPrompt(prompt),
            (None, None, None) => Title::None,
        }
    }

    /// 題は最後のもの、最初の発話は最初のもの、usage は最後の assistant のものを採る。
    fn feed(&mut self, line: &str) {
        let Ok(line) = serde_json::from_str::<Line>(line) else {
            return;
        };
        match line.kind.as_deref() {
            Some("custom-title") => {
                self.custom = non_empty(line.custom_title).or(self.custom.take())
            }
            Some("ai-title") => self.ai = non_empty(line.ai_title).or(self.ai.take()),
            Some("user") if self.first_prompt.is_none() => {
                self.first_prompt = line
                    .message
                    .and_then(|m| m.content)
                    .and_then(|content| prompt_of(&content));
            }
            Some("assistant") => {
                let Some(message) = line.message else {
                    return;
                };
                if message.model.as_deref() == Some("<synthetic>") {
                    return;
                }
                if let Some(usage) = message.usage {
                    self.context_tokens = Some(
                        usage.input_tokens
                            + usage.cache_read_input_tokens
                            + usage.cache_creation_input_tokens,
                    );
                }
            }
            _ => {}
        }
    }

    /// 窓より前は大きいので、必要な行だけ JSON を解く。
    fn wants(&self, line: &str) -> bool {
        line.contains("custom-title")
            || line.contains("ai-title")
            || line.contains("usage")
            || (self.first_prompt.is_none() && line.contains("user"))
    }
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "customTitle")]
    custom_title: Option<String>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    model: Option<String>,
    usage: Option<Usage>,
    content: Option<Value>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

fn scan(text: &str) -> Found {
    let mut found = Found::default();
    text.lines().for_each(|line| found.feed(line));
    found
}

fn scan_before(mut reader: impl BufRead) -> io::Result<Found> {
    let mut found = Found::default();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            return Ok(found);
        }
        let line = String::from_utf8_lossy(&buf);
        if found.wants(&line) {
            found.feed(&line);
        }
    }
}

fn non_empty(title: Option<String>) -> Option<String> {
    title.filter(|t| !t.is_empty())
}

/// コマンドの注記 (`<command-name>` など) は人の言葉ではないので採らない。
fn prompt_of(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(text) => text.as_str(),
        Value::Array(parts) => parts.iter().find_map(|part| {
            (part.get("type")?.as_str()? == "text")
                .then(|| part.get("text")?.as_str())
                .flatten()
        })?,
        _ => return None,
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() || text.starts_with('<') {
        return None;
    }
    Some(text.chars().take(PROMPT_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde_json::json;

    use super::*;

    fn log(lines: &[serde_json::Value]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        file.flush().unwrap();
        file
    }

    fn ai_title(title: &str) -> serde_json::Value {
        json!({"type": "ai-title", "aiTitle": title, "sessionId": "s"})
    }

    fn custom_title(title: &str) -> serde_json::Value {
        json!({"type": "custom-title", "customTitle": title, "sessionId": "s"})
    }

    fn user(content: serde_json::Value) -> serde_json::Value {
        json!({"type": "user", "message": {"role": "user", "content": content}})
    }

    fn assistant(model: &str, usage: serde_json::Value) -> serde_json::Value {
        json!({"type": "assistant", "message": {"model": model, "usage": usage, "content": []}})
    }

    fn filler_line() -> String {
        format!(
            "{}\n",
            json!({"type": "attachment", "pad": "x".repeat(1000)})
        )
    }

    fn push_filler(bytes: &mut Vec<u8>, at_least: usize) {
        let start = bytes.len();
        while bytes.len() - start < at_least {
            bytes.extend_from_slice(filler_line().as_bytes());
        }
    }

    fn push_line(bytes: &mut Vec<u8>, line: &serde_json::Value) {
        bytes.extend_from_slice(format!("{line}\n").as_bytes());
    }

    fn push_filler_until_window_cuts_into_previous_line(bytes: &mut Vec<u8>) {
        let start = bytes.len();
        while bytes.len() - start + filler_line().len() < TAIL_BYTES as usize {
            bytes.extend_from_slice(filler_line().as_bytes());
        }
    }

    fn file_of(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        file
    }

    #[test]
    fn 題が窓より前の中ほどにだけあっても拾う() {
        let mut bytes = Vec::new();
        push_filler(&mut bytes, 300 * 1024);
        push_line(&mut bytes, &ai_title("in the middle"));
        push_filler(&mut bytes, 600 * 1024);

        let summary = summarize(file_of(&bytes).path()).unwrap();

        assert_eq!(summary.title, Title::Ai("in the middle".into()));
    }

    #[test]
    fn usageが窓より前にだけあっても拾う() {
        let mut bytes = Vec::new();
        push_line(
            &mut bytes,
            &assistant("claude-opus-5-5", json!({"input_tokens": 11})),
        );
        push_filler(&mut bytes, 600 * 1024);

        let summary = summarize(file_of(&bytes).path()).unwrap();

        assert_eq!(summary.context_tokens, Some(11));
    }

    #[test]
    fn 題の行が窓の先頭をまたいでいても拾う() {
        let mut bytes = Vec::new();
        push_filler(&mut bytes, 300 * 1024);
        let title_start = bytes.len();
        let mut title = ai_title("straddles the window");
        title["pad"] = json!("x".repeat(2000));
        push_line(&mut bytes, &title);
        let title_end = bytes.len();
        push_filler_until_window_cuts_into_previous_line(&mut bytes);
        let window_start = bytes.len() - TAIL_BYTES as usize;
        assert!(title_start < window_start && window_start < title_end);

        let summary = summarize(file_of(&bytes).path()).unwrap();

        assert_eq!(summary.title, Title::Ai("straddles the window".into()));
    }

    #[test]
    fn 窓がちょうど題の行の頭から始まっても拾う() {
        let mut bytes = Vec::new();
        push_filler(&mut bytes, 300 * 1024);
        let window_start = bytes.len();
        push_line(&mut bytes, &ai_title("starts the window"));
        let overhead = format!("{}\n", json!({"type": "attachment", "pad": ""})).len();
        let window_end = window_start + TAIL_BYTES as usize;
        while window_end - bytes.len() > filler_line().len() + overhead {
            bytes.extend_from_slice(filler_line().as_bytes());
        }
        let pad = window_end - bytes.len() - overhead;
        push_line(
            &mut bytes,
            &json!({"type": "attachment", "pad": "x".repeat(pad)}),
        );
        assert_eq!(bytes.len() - TAIL_BYTES as usize, window_start);

        let summary = summarize(file_of(&bytes).path()).unwrap();

        assert_eq!(summary.title, Title::Ai("starts the window".into()));
    }

    #[test]
    fn 題がどこにも無く最初の発話が窓の中にだけあるなら採る() {
        let mut bytes = Vec::new();
        push_filler(&mut bytes, 600 * 1024);
        push_line(&mut bytes, &user(json!("first words in the window")));

        let summary = summarize(file_of(&bytes).path()).unwrap();

        assert_eq!(
            summary.title,
            Title::FirstPrompt("first words in the window".into())
        );
    }

    #[test]
    fn 窓に題があれば窓より前の題で上書きしない() {
        let mut bytes = Vec::new();
        push_line(&mut bytes, &custom_title("old name"));
        push_filler(&mut bytes, 600 * 1024);
        push_line(&mut bytes, &ai_title("recent"));

        let summary = summarize(file_of(&bytes).path()).unwrap();

        assert_eq!(summary.title, Title::Ai("recent".into()));
    }

    #[test]
    fn 両方あればcustomが勝つ() {
        let file = log(&[ai_title("ai"), custom_title("mine"), ai_title("ai")]);

        let summary = summarize(file.path()).unwrap();

        assert_eq!(summary.title, Title::Custom("mine".into()));
    }

    #[test]
    fn aiだけならai() {
        let file = log(&[user(json!("hello")), ai_title("first"), ai_title("first")]);

        assert_eq!(
            summarize(file.path()).unwrap().title,
            Title::Ai("first".into())
        );
    }

    #[test]
    fn 題が無ければ最初の発話の先頭30文字() {
        let prompt = "あ".repeat(40);
        let file = log(&[user(json!(prompt))]);

        assert_eq!(
            summarize(file.path()).unwrap().title,
            Title::FirstPrompt("あ".repeat(30))
        );
    }

    #[test]
    fn 最初の発話は配列のtext要素からも取り空白は畳む() {
        let file = log(&[
            user(json!("<command-name>/clear</command-name>")),
            user(json!([
                {"type": "tool_result", "content": "x"},
                {"type": "text", "text": "  fix   the\n bug "}
            ])),
        ]);

        assert_eq!(
            summarize(file.path()).unwrap().title,
            Title::FirstPrompt("fix the bug".into())
        );
    }

    #[test]
    fn 何も無ければnone() {
        let file = log(&[json!({"type": "summary"})]);

        let summary = summarize(file.path()).unwrap();

        assert_eq!(summary.title, Title::None);
        assert_eq!(summary.context_tokens, None);
    }

    #[test]
    fn usageは三つの和で最後のassistantを採りsyntheticは飛ばす() {
        let file = log(&[
            assistant(
                "claude-opus-5-5",
                json!({"input_tokens": 1, "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 20, "output_tokens": 999}),
            ),
            assistant(
                "claude-opus-5-5",
                json!({"input_tokens": 3, "cache_read_input_tokens": 4000}),
            ),
            assistant("<synthetic>", json!({"input_tokens": 0})),
        ]);

        assert_eq!(summarize(file.path()).unwrap().context_tokens, Some(4003));
    }

    #[test]
    fn 壊れた行は飛ばす() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "{}", ai_title("ok")).unwrap();
        writeln!(file, "{{broken").unwrap();
        file.flush().unwrap();

        assert_eq!(
            summarize(file.path()).unwrap().title,
            Title::Ai("ok".into())
        );
    }

    #[test]
    fn 末尾に題が無く先頭にだけあるなら先頭から拾う() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "{}", ai_title("only at head")).unwrap();
        let filler = json!({"type": "progress", "pad": "x".repeat(1000)});
        for _ in 0..700 {
            writeln!(file, "{filler}").unwrap();
        }
        writeln!(
            file,
            "{}",
            assistant("claude-opus-5-5", json!({"input_tokens": 7}))
        )
        .unwrap();
        file.flush().unwrap();
        assert!(file.as_file().metadata().unwrap().len() > 512 * 1024);

        let summary = summarize(file.path()).unwrap();

        assert_eq!(summary.title, Title::Ai("only at head".into()));
        assert_eq!(summary.context_tokens, Some(7));
    }

    #[test]
    fn 大きなファイルでも末尾の題を拾う() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let filler = json!({"type": "progress", "pad": "x".repeat(1000)});
        for _ in 0..600 {
            writeln!(file, "{filler}").unwrap();
        }
        writeln!(file, "{}", custom_title("tail")).unwrap();
        file.flush().unwrap();

        assert_eq!(
            summarize(file.path()).unwrap().title,
            Title::Custom("tail".into())
        );
    }
}
