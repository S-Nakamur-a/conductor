//! Claude Code の .jsonl セッションログと一対一の serde 型。

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub is_sidechain: bool,
    /// skill 定義のダンプや caveat バナーなど、CLI が user ターンの形で注入した隠しコンテキスト。
    #[serde(default)]
    pub is_meta: bool,
    /// /compact の要約を次のコンテキストへ引き継ぐ疑似 user ターン。
    /// 手元のコーパスでは常に isVisibleInTranscriptOnly を伴う (102 件中 102 件)。
    #[serde(default)]
    pub is_compact_summary: bool,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub attachment: Option<Attachment>,
    #[serde(default)]
    pub message: Option<Message>,
    /// RFC3339。古いレコードには無い。
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// CLI がユーザの代わりに会話へ注入したコンテキスト。
///
/// 手元のコーパスには 29 種類あり (hook_success だけで約 47000 件) 、Claude Code が
/// 可視の行にするのは file と compact_file_reference の 2 種類だけ。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    #[serde(rename = "type", default)]
    pub kind: String,
    /// セッションの cwd からの相対パスに既になっている。
    #[serde(default)]
    pub display_path: Option<String>,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub content: Option<AttachmentContent>,
}

#[derive(Deserialize)]
pub struct AttachmentContent {
    #[serde(default)]
    pub file: Option<AttachmentFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentFile {
    #[serde(default)]
    pub num_lines: Option<u64>,
}

#[derive(Deserialize)]
pub struct Message {
    #[serde(default)]
    pub role: Option<String>,
    pub content: Content,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    ToolUse {
        #[serde(default)]
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    ToolResult {
        #[serde(default)]
        tool_use_id: String,
        #[serde(default)]
        content: ToolResultContent,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Default)]
#[serde(untagged)]
pub enum ToolResultContent {
    #[default]
    None,
    Text(String),
    Blocks(Vec<TextOnly>),
}

#[derive(Deserialize)]
pub struct TextOnly {
    #[serde(default)]
    pub text: String,
}
