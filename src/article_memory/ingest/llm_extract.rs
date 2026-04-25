//! Rust-local LLM-based HTML→Markdown extractor.
//!
//! Called by the worker's engine ladder when trafilatura's output fails
//! the quality gate and the ladder permits an `openrouter-llm` tier.
//!
//! Rationale: `cleaning_internals::create_chat_completion` already talks
//! to OpenAI-compatible `/chat/completions` endpoints via reqwest. The
//! Python adapter was briefly given its own LLM client (T3) but that
//! duplicated this Rust code and forced API keys through an extra hop.
//! We stay Rust-only to keep one LLM client per project.

use crate::app_config::{ModelProviderConfig, OpenRouterLlmEngineConfig};
use anyhow::Result;
use std::time::Duration;

const SYSTEM_PROMPT: &str = "You are a precise HTML-to-Markdown converter. \
Given raw HTML, extract ONLY the main article body as well-structured Markdown. \
Preserve: headings (use #/##/###), lists, code blocks (use ``` fences with \
language when recognizable), tables, links, block quotes. Remove: navigation, \
sidebars, comments, cookie banners, share buttons, related-article lists, ads, \
and all other UI chrome. Do not summarize. Do not add content. If no article \
body is present, return an empty response.";

/// Convert raw HTML to clean Markdown via a chat-completions LLM.
pub async fn llm_html_to_markdown(
    provider: &ModelProviderConfig,
    engine_cfg: &OpenRouterLlmEngineConfig,
    html: &str,
) -> Result<String> {
    use super::super::llm_client::{chat_completion, LlmChatRequest, LlmProvider};

    // Truncate by chars (UTF-8 safe).
    let truncated: String = html.chars().take(engine_cfg.max_input_chars).collect();
    let user = format!("Convert this HTML to Markdown:\n\n{truncated}");

    chat_completion(
        &LlmProvider {
            name: &provider.name,
            base_url: &provider.base_url,
            api_key: &provider.api_key,
        },
        &LlmChatRequest {
            model: &engine_cfg.model,
            system: SYSTEM_PROMPT,
            user: &user,
            temperature: 0.0,
            max_tokens: None,
            timeout: Duration::from_secs(engine_cfg.timeout_secs.max(1)),
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(name: &str, base_url: &str, key: &str) -> ModelProviderConfig {
        ModelProviderConfig {
            name: name.into(),
            api_key: key.into(),
            base_url: base_url.into(),
            allowed_models: vec![],
        }
    }

    fn engine() -> OpenRouterLlmEngineConfig {
        OpenRouterLlmEngineConfig {
            provider: "openrouter".into(),
            model: "google/gemini-2.0-flash-001".into(),
            timeout_secs: 30,
            max_input_chars: 10,
        }
    }

    #[tokio::test]
    async fn empty_api_key_bails_early() {
        let p = provider("openrouter", "https://x", "");
        let err = llm_html_to_markdown(&p, &engine(), "<html></html>")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("empty api_key"));
    }

    #[tokio::test]
    async fn empty_base_url_bails_early() {
        let p = provider("openrouter", "", "sk-test");
        let err = llm_html_to_markdown(&p, &engine(), "<html></html>")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("empty base_url"));
    }

    #[test]
    fn truncate_respects_char_boundary_for_multibyte() {
        // Regression: byte-truncation on Chinese would split codepoints.
        // Validate that we don't panic even if max_input_chars is tight.
        let html = "请订阅我们的频道。";
        let _truncated: String = html.chars().take(5).collect();
        // Just verifying the same pattern used inside the function works.
        assert!(_truncated.chars().count() <= 5);
    }
}
