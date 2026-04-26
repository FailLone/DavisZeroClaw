use super::*;
use crate::app_config::{
    ArticleMemoryEmbeddingConfig, ArticleMemoryNormalizeConfig, ArticleMemoryValueConfig,
    ModelProviderConfig,
};
use crate::RuntimePaths;
use anyhow::{bail, Result};

pub fn resolve_article_embedding_config(
    embedding: &ArticleMemoryEmbeddingConfig,
    providers: &[ModelProviderConfig],
) -> Result<Option<ResolvedArticleEmbeddingConfig>> {
    if !embedding.enabled {
        return Ok(None);
    }

    let provider = embedding.provider.trim();
    let provider_config = if provider.is_empty() {
        None
    } else {
        providers.iter().find(|item| item.name == provider)
    };
    let api_key = first_non_empty(
        embedding.api_key.trim(),
        provider_config
            .map(|item| item.api_key.as_str())
            .unwrap_or_default(),
    );
    let base_url = first_non_empty(
        embedding.base_url.trim(),
        provider_config
            .map(|item| item.base_url.as_str())
            .unwrap_or_default(),
    )
    .trim_end_matches('/')
    .to_string();
    if api_key.is_empty() || base_url.is_empty() {
        bail!("article memory embedding requires api_key and base_url");
    }

    let provider_name = if !provider.is_empty() {
        provider.to_string()
    } else {
        "direct".to_string()
    };

    Ok(Some(ResolvedArticleEmbeddingConfig {
        provider: provider_name,
        api_key,
        base_url,
        model: embedding.model.trim().to_string(),
        dimensions: embedding.dimensions,
        max_input_chars: embedding.max_input_chars,
    }))
}

pub fn resolve_article_normalize_config(
    normalize: &ArticleMemoryNormalizeConfig,
    providers: &[ModelProviderConfig],
) -> Result<Option<ResolvedArticleNormalizeConfig>> {
    if !normalize.llm_polish && !normalize.llm_summary {
        return Ok(None);
    }
    let provider = normalize.provider.trim();
    let provider_config = if provider.is_empty() {
        None
    } else {
        providers.iter().find(|item| item.name == provider)
    };
    let api_key = first_non_empty(
        normalize.api_key.trim(),
        provider_config
            .map(|item| item.api_key.as_str())
            .unwrap_or_default(),
    );
    let base_url = first_non_empty(
        normalize.base_url.trim(),
        provider_config
            .map(|item| item.base_url.as_str())
            .unwrap_or_default(),
    )
    .trim_end_matches('/')
    .to_string();
    let model = if normalize.model.trim().is_empty() {
        provider_config
            .and_then(|item| item.allowed_models.first())
            .cloned()
            .unwrap_or_default()
    } else {
        normalize.model.trim().to_string()
    };
    if api_key.is_empty() || base_url.is_empty() || model.is_empty() {
        bail!("article memory normalize requires api_key, base_url, and model");
    }
    Ok(Some(ResolvedArticleNormalizeConfig {
        provider: if provider.is_empty() {
            "direct".to_string()
        } else {
            provider.to_string()
        },
        api_key,
        base_url,
        model,
        llm_polish: normalize.llm_polish,
        llm_summary: normalize.llm_summary,
        min_polish_input_chars: normalize.min_polish_input_chars,
        max_polish_input_chars: normalize.max_polish_input_chars,
        summary_input_chars: normalize.summary_input_chars,
        fallback_min_ratio: normalize.fallback_min_ratio,
    }))
}

/// Merges algorithm knobs from `config/davis/article_memory.toml` `[value]`
/// (committed; safe defaults) with credentials from `local.toml`
/// `[article_memory.value]` (gitignored; holds the api_key). Returns `None`
/// when `[value].enabled = false`; `llm_judge` is downgraded to `false`
/// when credentials can't be resolved so deterministic scoring still runs.
pub fn resolve_article_value_config(
    paths: &RuntimePaths,
    creds: &ArticleMemoryValueConfig,
    providers: &[ModelProviderConfig],
) -> Result<Option<ResolvedArticleValueConfig>> {
    let policy = load_article_cleaning_config(paths)?;
    let value = policy.value;
    if !value.enabled {
        return Ok(None);
    }
    let provider = creds.provider.trim();
    let provider_config = if provider.is_empty() {
        None
    } else {
        providers.iter().find(|item| item.name == provider)
    };
    let api_key = first_non_empty(
        creds.api_key.trim(),
        provider_config
            .map(|item| item.api_key.as_str())
            .unwrap_or_default(),
    );
    let base_url = first_non_empty(
        creds.base_url.trim(),
        provider_config
            .map(|item| item.base_url.as_str())
            .unwrap_or_default(),
    )
    .trim_end_matches('/')
    .to_string();
    let model = if creds.model.trim().is_empty() {
        provider_config
            .and_then(|item| item.allowed_models.first())
            .cloned()
            .unwrap_or_default()
    } else {
        creds.model.trim().to_string()
    };
    let llm_judge =
        value.llm_judge && !api_key.is_empty() && !base_url.is_empty() && !model.is_empty();
    Ok(Some(ResolvedArticleValueConfig {
        provider: if provider.is_empty() {
            "direct".to_string()
        } else {
            provider.to_string()
        },
        api_key,
        base_url,
        model,
        llm_judge,
        max_input_chars: value.max_input_chars,
        min_normalized_chars: value.min_normalized_chars,
        save_threshold: value.save_threshold,
        candidate_threshold: value.candidate_threshold,
        target_topics: value.target_topics,
    }))
}
