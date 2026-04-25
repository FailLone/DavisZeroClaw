//! Single chat-completions entry point for article_memory LLM calls.
//!
//! Pre-Phase-2 the project had four near-identical reqwest clients for
//! `/chat/completions`: `cleaning_internals::create_chat_completion`,
//! `cleaning_internals::create_chat_completion_for_value`,
//! `ingest::llm_extract::llm_html_to_markdown`. This module consolidates
//! them. Callers supply `LlmChatRequest` describing their specific call
//! shape (system/user/temperature/max_tokens/timeout).

#![allow(dead_code)]

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use std::time::Duration;

/// Minimal provider credentials the chat endpoint needs.
pub struct LlmProvider<'a> {
    pub name: &'a str,
    pub base_url: &'a str,
    pub api_key: &'a str,
}

/// One chat-completions invocation.
pub struct LlmChatRequest<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub user: &'a str,
    pub temperature: f32,
    pub max_tokens: Option<usize>,
    pub timeout: Duration,
}

/// Call the provider's `/chat/completions` endpoint and return the
/// content of `choices[0].message.content`. Errors on HTTP failure,
/// empty content, or missing fields.
pub async fn chat_completion(
    provider: &LlmProvider<'_>,
    req: &LlmChatRequest<'_>,
) -> Result<String> {
    if provider.api_key.trim().is_empty() {
        bail!("llm provider '{}' has empty api_key", provider.name);
    }
    if provider.base_url.trim().is_empty() {
        bail!("llm provider '{}' has empty base_url", provider.name);
    }

    let endpoint = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );

    let client = reqwest::Client::builder()
        .timeout(req.timeout)
        .build()
        .context("build reqwest client for chat_completion")?;

    let mut payload = json!({
        "model": req.model,
        "messages": [
            {"role": "system", "content": req.system},
            {"role": "user", "content": req.user},
        ],
        "temperature": req.temperature,
    });
    if let Some(max_tokens) = req.max_tokens {
        payload["max_tokens"] = json!(max_tokens);
    }

    let response = client
        .post(endpoint)
        .bearer_auth(provider.api_key)
        .json(&payload)
        .send()
        .await
        .context("chat_completion request failed")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| String::from("<failed to read response>"));
    if !status.is_success() {
        bail!("chat_completion HTTP {status}: {body}");
    }

    let value: serde_json::Value =
        serde_json::from_str(&body).context("chat_completion response was not valid JSON")?;
    value
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("chat_completion response did not contain non-empty content"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_api_key_bails_before_http() {
        let p = LlmProvider {
            name: "openrouter",
            base_url: "https://x",
            api_key: "   ",
        };
        let r = LlmChatRequest {
            model: "gpt-test",
            system: "",
            user: "",
            temperature: 0.0,
            max_tokens: None,
            timeout: Duration::from_secs(5),
        };
        let err = chat_completion(&p, &r).await.unwrap_err().to_string();
        assert!(err.contains("empty api_key"));
    }

    #[tokio::test]
    async fn empty_base_url_bails_before_http() {
        let p = LlmProvider {
            name: "openrouter",
            base_url: "",
            api_key: "sk-test",
        };
        let r = LlmChatRequest {
            model: "gpt-test",
            system: "",
            user: "",
            temperature: 0.0,
            max_tokens: None,
            timeout: Duration::from_secs(5),
        };
        let err = chat_completion(&p, &r).await.unwrap_err().to_string();
        assert!(err.contains("empty base_url"));
    }
}
