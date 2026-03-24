//! Gemini API client.
//!
//! Provides a blocking HTTP client for the Google Gemini API,
//! replacing the previous `claude -p` CLI invocations for lower latency.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

const API_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// A text part in a Gemini message.
#[derive(Debug, Serialize, Deserialize)]
struct Part {
    text: String,
}

/// A content entry (role + parts).
#[derive(Debug, Serialize)]
struct Content {
    role: &'static str,
    parts: Vec<Part>,
}

/// System instruction wrapper.
#[derive(Debug, Serialize)]
struct SystemInstruction {
    parts: Vec<Part>,
}

/// Thinking config for Gemini 2.5+ models.
///
/// Gemini 2.5 models have thinking enabled by default, and thinking tokens
/// count against `max_output_tokens`. Set `thinking_budget: 0` to disable
/// thinking for simple structured-output tasks.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    thinking_budget: i32,
}

/// Generation config (includes thinking config nested inside).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    max_output_tokens: u32,
    thinking_config: ThinkingConfig,
}

/// Request body for the Gemini generateContent API.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    system_instruction: SystemInstruction,
    contents: Vec<Content>,
    generation_config: GenerationConfig,
}

/// Response body from the Gemini API.
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

/// Error response from the API.
#[derive(Debug, Deserialize)]
struct ApiErrorResponse {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    message: String,
}

/// Call the Gemini generateContent API (blocking).
///
/// - `system_prompt`: system instruction content
/// - `user_message`: user message content
/// - `model`: model ID (uses default if `None`)
/// - `max_tokens`: max tokens to generate
///
/// Returns the text content from the first candidate's first part.
pub fn call_messages_api(
    system_prompt: &str,
    user_message: &str,
    model: Option<&str>,
    max_tokens: u32,
) -> Result<String> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .context("GEMINI_API_KEY environment variable not set")?;

    let model = model.unwrap_or(DEFAULT_MODEL);
    let url = format!("{API_BASE_URL}/{model}:generateContent?key={api_key}");

    let request_body = GenerateContentRequest {
        system_instruction: SystemInstruction {
            parts: vec![Part { text: system_prompt.to_string() }],
        },
        contents: vec![Content {
            role: "user",
            parts: vec![Part { text: user_message.to_string() }],
        }],
        generation_config: GenerationConfig {
            max_output_tokens: max_tokens,
            thinking_config: ThinkingConfig {
                thinking_budget: 0,
            },
        },
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to build HTTP client")?;
    let response = client
        .post(&url)
        .header("content-type", "application/json")
        .json(&request_body)
        .send()
        .context("Failed to send request to Gemini API")?;

    let status = response.status();
    let body = response.text().context("Failed to read API response body")?;

    if !status.is_success() {
        if let Ok(err) = serde_json::from_str::<ApiErrorResponse>(&body) {
            bail!("Gemini API error ({}): {}", status, err.error.message);
        }
        bail!("Gemini API error ({status}): {body}");
    }

    log::debug!("Gemini API raw HTTP body: {body:?}");

    let resp: GenerateContentResponse =
        serde_json::from_str(&body).context("Failed to parse Gemini API response")?;

    if let Some(candidates) = &resp.candidates {
        if let Some(candidate) = candidates.first() {
            if let Some(part) = candidate.content.parts.first() {
                return Ok(part.text.clone());
            }
        }
    }

    bail!("No text content in Gemini API response")
}
