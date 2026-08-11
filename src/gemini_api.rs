//! Gemini API クライアント。
//!
//! Google Gemini API 向けのブロッキング HTTP クライアント。[api] の継ぎ目
//! (ai_caller.rs を参照) の背後にある 2 つのプロバイダのうちの 1 つ。素の HTTP
//! なのでリポジトリを読めない。リポジトリを読ませる必要のあるタスクは、
//! こちらではなく command プロバイダを使うこと。

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const API_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// Gemini のメッセージ内のテキストパート。
#[derive(Debug, Serialize, Deserialize)]
struct Part {
    text: String,
}

/// content エントリ (role と parts)。
#[derive(Debug, Serialize)]
struct Content {
    role: &'static str,
    parts: Vec<Part>,
}

/// システム指示のラッパー。
#[derive(Debug, Serialize)]
struct SystemInstruction {
    parts: Vec<Part>,
}

/// Gemini 2.5 以降のモデル向けの thinking 設定。
///
/// Gemini 2.5 系はデフォルトで thinking が有効で、thinking のトークンも
/// max_output_tokens に算入される。単純な構造化出力のタスクでは
/// thinking_budget: 0 にして thinking を切る。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    thinking_budget: i32,
}

/// 生成の設定 (内側に thinking 設定を含む)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    max_output_tokens: u32,
    thinking_config: ThinkingConfig,
}

/// Gemini の generateContent API へのリクエストボディ。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    system_instruction: SystemInstruction,
    contents: Vec<Content>,
    generation_config: GenerationConfig,
}

/// Gemini API からのレスポンスボディ。
#[derive(Debug, Deserialize)]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    content: CandidateContent,
}

#[derive(Debug, Deserialize)]
struct CandidateContent {
    parts: Vec<Part>,
}

/// API からのエラーレスポンス。
#[derive(Debug, Deserialize)]
struct ApiErrorResponse {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    message: String,
}

/// Gemini の generateContent API を呼ぶ (ブロッキング)。
///
/// - system_prompt: システム指示の内容
/// - user_message: ユーザーメッセージの内容
/// - model: モデル ID (None ならデフォルトを使う)
/// - max_tokens: 生成する最大トークン数
///
/// 最初の candidate の最初の part のテキストを返す。
pub fn call_messages_api(
    system_prompt: &str,
    user_message: &str,
    model: Option<&str>,
    max_tokens: u32,
) -> Result<String> {
    let api_key =
        std::env::var("GEMINI_API_KEY").context("GEMINI_API_KEY environment variable not set")?;

    let model = model.unwrap_or(DEFAULT_MODEL);
    // API キーは URL ではなく x-goog-api-key ヘッダで送る (Google の推奨方法)。
    // クエリ文字列はプロキシやアクセスログ、リクエストトレースに漏れるため。
    let url = format!("{API_BASE_URL}/{model}:generateContent");

    let request_body = GenerateContentRequest {
        system_instruction: SystemInstruction {
            parts: vec![Part {
                text: system_prompt.to_string(),
            }],
        },
        contents: vec![Content {
            role: "user",
            parts: vec![Part {
                text: user_message.to_string(),
            }],
        }],
        generation_config: GenerationConfig {
            max_output_tokens: max_tokens,
            thinking_config: ThinkingConfig { thinking_budget: 0 },
        },
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;
    let response = client
        .post(&url)
        .header("x-goog-api-key", &api_key)
        .header("content-type", "application/json")
        .json(&request_body)
        .send()
        .context("Failed to send request to Gemini API")?;

    let status = response.status();
    let body = response
        .text()
        .context("Failed to read API response body")?;

    if !status.is_success() {
        if let Ok(err) = serde_json::from_str::<ApiErrorResponse>(&body) {
            bail!("Gemini API error ({}): {}", status, err.error.message);
        }
        bail!("Gemini API error ({status}): {body}");
    }

    log::debug!("Gemini API raw HTTP body: {body:?}");

    let resp: GenerateContentResponse =
        serde_json::from_str(&body).context("Failed to parse Gemini API response")?;

    if let Some(candidates) = &resp.candidates
        && let Some(candidate) = candidates.first()
        && let Some(part) = candidate.content.parts.first()
    {
        return Ok(part.text.clone());
    }

    bail!("No text content in Gemini API response")
}
