//! 組み込みプロバイダ: Google Gemini の generateContent API (ブロッキング)。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::AiCaller;

const API_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// 唯一の呼び出し元であるスマート worktree 命名に合わせた上限。継ぎ目の一部ではなく
/// Gemini へのリクエストのつまみなので、プロバイダ側に置く。
pub(super) const MAX_TOKENS: u32 = 1024;

pub struct GeminiCaller {
    pub model: String,
    pub max_tokens: u32,
}

impl AiCaller for GeminiCaller {
    fn complete(
        &self,
        system_prompt: &str,
        user_message: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        generate_content(system_prompt, user_message, &self.model, self.max_tokens)
            .map_err(|e| format!("{e}"))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Part {
    text: String,
}

#[derive(Debug, Serialize)]
struct Content {
    role: &'static str,
    parts: Vec<Part>,
}

#[derive(Debug, Serialize)]
struct SystemInstruction {
    parts: Vec<Part>,
}

/// Gemini 2.5 系は thinking が既定で有効で、そのトークンも max_output_tokens に
/// 算入される。構造化出力のタスクでは budget 0 にして切る。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    thinking_budget: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    max_output_tokens: u32,
    thinking_config: ThinkingConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    system_instruction: SystemInstruction,
    contents: Vec<Content>,
    generation_config: GenerationConfig,
}

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

#[derive(Debug, Deserialize)]
struct ApiErrorResponse {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    message: String,
}

/// 最初の candidate の最初の part のテキストを返す。
fn generate_content(
    system_prompt: &str,
    user_message: &str,
    model: &str,
    max_tokens: u32,
) -> Result<String> {
    let api_key =
        std::env::var("GEMINI_API_KEY").context("GEMINI_API_KEY environment variable not set")?;

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
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("Failed to build HTTP client")?;
    // API キーは URL ではなくヘッダで送る。クエリ文字列はプロキシやアクセスログ、
    // リクエストトレースに漏れる。
    let response = client
        .post(format!("{API_BASE_URL}/{model}:generateContent"))
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
