use crate::runtime_paths::RuntimePaths;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    pub home_assistant: HomeAssistantConfig,
    pub imessage: ImessageConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
    pub providers: Vec<ModelProviderConfig>,
    pub routing: RoutingConfig,
    #[serde(default)]
    pub crawl4ai: Crawl4aiConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub article_memory: ArticleMemoryConfig,
    #[serde(default)]
    pub query_classification: QueryClassificationOverride,
}

/// User-supplied overrides merged on top of config/davis/query_classification.toml.
/// Append rules here in local.toml under [[query_classification.rules]] to bias
/// routing without editing the shipped defaults. User rules win on priority ties.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryClassificationOverride {
    #[serde(default)]
    pub rules: Vec<QueryClassificationRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryClassificationRule {
    pub hint: String,
    pub keywords: Vec<String>,
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomeAssistantConfig {
    pub url: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImessageConfig {
    pub allowed_contacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebhookConfig {
    #[serde(default)]
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProviderConfig {
    pub name: String,
    pub api_key: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub allowed_models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    #[serde(default)]
    pub default_profile: Option<String>,
    pub profiles: RoutingProfilesConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingProfilesConfig {
    pub home_control: RoutingProfileConfig,
    pub general_qa: RoutingProfileConfig,
    pub research: RoutingProfileConfig,
    pub structured_lookup: RoutingProfileConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingProfileConfig {
    pub provider: String,
    pub model: String,
    #[serde(default = "default_max_fallbacks")]
    pub max_fallbacks: usize,
}

fn default_max_fallbacks() -> usize {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crawl4aiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_crawl4ai_base_url")]
    pub base_url: String,
    #[serde(default = "default_crawl4ai_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_true")]
    pub headless: bool,
    #[serde(default = "default_true")]
    pub magic: bool,
    #[serde(default = "default_true")]
    pub simulate_user: bool,
    #[serde(default = "default_true")]
    pub override_navigator: bool,
    #[serde(default = "default_true")]
    pub remove_overlay_elements: bool,
    #[serde(default = "default_true")]
    pub enable_stealth: bool,
}

/// Flat passthrough for ZeroClaw's [[mcp.servers]] array. Each entry is
/// rendered verbatim into the runtime config. Davis does not special-case
/// any server (mempalace included) — if the user wants it, they declare it
/// here. The `daviszeroclaw memory mempalace install/enable/check` helpers
/// maintain their own entry in this list.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    #[serde(default)]
    pub transport: McpTransport,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default = "default_mcp_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    #[default]
    Stdio,
    Sse,
    Http,
}

fn default_mcp_tool_timeout_secs() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArticleMemoryConfig {
    #[serde(default)]
    pub embedding: ArticleMemoryEmbeddingConfig,
    #[serde(default)]
    pub normalize: ArticleMemoryNormalizeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleMemoryEmbeddingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default = "default_article_embedding_model")]
    pub model: String,
    #[serde(default = "default_article_embedding_dimensions")]
    pub dimensions: usize,
    #[serde(default = "default_article_embedding_max_input_chars")]
    pub max_input_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleMemoryNormalizeConfig {
    #[serde(default)]
    pub llm_polish: bool,
    #[serde(default)]
    pub llm_summary: bool,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_article_normalize_min_polish_input_chars")]
    pub min_polish_input_chars: usize,
    #[serde(default = "default_article_normalize_max_polish_input_chars")]
    pub max_polish_input_chars: usize,
    #[serde(default = "default_article_normalize_summary_input_chars")]
    pub summary_input_chars: usize,
    #[serde(default = "default_article_normalize_fallback_min_ratio")]
    pub fallback_min_ratio: f32,
}

fn default_true() -> bool {
    true
}

fn default_crawl4ai_base_url() -> String {
    "http://127.0.0.1:11235".to_string()
}

fn default_crawl4ai_timeout_secs() -> u64 {
    90
}

fn default_article_embedding_model() -> String {
    "Qwen/Qwen3-Embedding-8B".to_string()
}

fn default_article_embedding_dimensions() -> usize {
    1024
}

fn default_article_embedding_max_input_chars() -> usize {
    12_000
}

fn default_article_normalize_min_polish_input_chars() -> usize {
    1_200
}

fn default_article_normalize_max_polish_input_chars() -> usize {
    24_000
}

fn default_article_normalize_summary_input_chars() -> usize {
    24_000
}

fn default_article_normalize_fallback_min_ratio() -> f32 {
    0.70
}

impl Default for Crawl4aiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: default_crawl4ai_base_url(),
            timeout_secs: default_crawl4ai_timeout_secs(),
            headless: default_true(),
            magic: default_true(),
            simulate_user: default_true(),
            override_navigator: default_true(),
            remove_overlay_elements: default_true(),
            enable_stealth: default_true(),
        }
    }
}

impl Default for ArticleMemoryEmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: String::new(),
            api_key: String::new(),
            base_url: String::new(),
            model: default_article_embedding_model(),
            dimensions: default_article_embedding_dimensions(),
            max_input_chars: default_article_embedding_max_input_chars(),
        }
    }
}

impl Default for ArticleMemoryNormalizeConfig {
    fn default() -> Self {
        Self {
            llm_polish: false,
            llm_summary: false,
            provider: String::new(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            min_polish_input_chars: default_article_normalize_min_polish_input_chars(),
            max_polish_input_chars: default_article_normalize_max_polish_input_chars(),
            summary_input_chars: default_article_normalize_summary_input_chars(),
            fallback_min_ratio: default_article_normalize_fallback_min_ratio(),
        }
    }
}

pub fn load_local_config(paths: &RuntimePaths) -> Result<LocalConfig> {
    let path = paths.local_config_path();
    crate::model_routing::warn_if_secret_file_is_loose(&path);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("local config not found: {}", path.display()))?;
    let config: LocalConfig = toml::from_str(&raw)
        .with_context(|| format!("invalid local config TOML: {}", path.display()))?;
    validate_local_config(config)
}

fn validate_local_config(mut config: LocalConfig) -> Result<LocalConfig> {
    if config.home_assistant.url.trim().is_empty() {
        return Err(anyhow!("home_assistant.url is required"));
    }
    if config.home_assistant.token.trim().is_empty() {
        return Err(anyhow!("home_assistant.token is required"));
    }

    config.imessage.allowed_contacts = config
        .imessage
        .allowed_contacts
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if config.imessage.allowed_contacts.is_empty() {
        return Err(anyhow!("imessage.allowed_contacts must not be empty"));
    }

    config.webhook.secret = config.webhook.secret.trim().to_string();

    if config.providers.is_empty() {
        return Err(anyhow!("providers must not be empty"));
    }

    let mut seen_names = BTreeSet::new();
    for provider in &mut config.providers {
        provider.name = provider.name.trim().to_string();
        provider.api_key = provider.api_key.trim().to_string();
        provider.base_url = provider.base_url.trim().to_string();
        provider.allowed_models = provider
            .allowed_models
            .iter()
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect();

        if provider.name.is_empty() {
            return Err(anyhow!("provider.name is required"));
        }
        if !seen_names.insert(provider.name.clone()) {
            return Err(anyhow!("duplicate provider name: {}", provider.name));
        }
        if provider.api_key.is_empty() {
            return Err(anyhow!(
                "provider.api_key is required for {}",
                provider.name
            ));
        }
        if provider.allowed_models.is_empty() {
            return Err(anyhow!(
                "provider.allowed_models must not be empty for {}",
                provider.name
            ));
        }
    }

    validate_profile(
        "home_control",
        &config.routing.profiles.home_control,
        &config.providers,
    )?;
    validate_profile(
        "general_qa",
        &config.routing.profiles.general_qa,
        &config.providers,
    )?;
    validate_profile(
        "research",
        &config.routing.profiles.research,
        &config.providers,
    )?;
    validate_profile(
        "structured_lookup",
        &config.routing.profiles.structured_lookup,
        &config.providers,
    )?;

    config.crawl4ai.base_url = config
        .crawl4ai
        .base_url
        .trim()
        .trim_end_matches('/')
        .to_string();
    if config.crawl4ai.base_url.is_empty() {
        config.crawl4ai.base_url = default_crawl4ai_base_url();
    }
    if config.crawl4ai.timeout_secs == 0 {
        config.crawl4ai.timeout_secs = default_crawl4ai_timeout_secs();
    }

    validate_mcp_servers(&mut config.mcp)?;
    validate_article_memory_config(&mut config)?;
    validate_query_classification_override(&mut config.query_classification)?;

    Ok(config)
}

fn validate_mcp_servers(mcp: &mut McpConfig) -> Result<()> {
    let mut seen = BTreeSet::new();
    for (index, server) in mcp.servers.iter_mut().enumerate() {
        server.name = server.name.trim().to_string();
        server.command = server.command.trim().to_string();
        server.url = server.url.trim().to_string();
        server.args = server
            .args
            .iter()
            .map(|arg| arg.trim().to_string())
            .collect();
        if server.tool_timeout_secs == 0 {
            server.tool_timeout_secs = default_mcp_tool_timeout_secs();
        }

        if server.name.is_empty() {
            return Err(anyhow!("mcp.servers[{index}].name is required"));
        }
        if !seen.insert(server.name.clone()) {
            return Err(anyhow!("mcp.servers has a duplicate name: {}", server.name));
        }
        match server.transport {
            McpTransport::Stdio => {
                if server.command.is_empty() {
                    return Err(anyhow!(
                        "mcp.servers[{}].command is required for stdio transport",
                        server.name
                    ));
                }
            }
            McpTransport::Sse | McpTransport::Http => {
                if server.url.is_empty() {
                    return Err(anyhow!(
                        "mcp.servers[{}].url is required for sse/http transport",
                        server.name
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_query_classification_override(
    override_cfg: &mut QueryClassificationOverride,
) -> Result<()> {
    for (index, rule) in override_cfg.rules.iter_mut().enumerate() {
        rule.hint = rule.hint.trim().to_string();
        if rule.hint.is_empty() {
            return Err(anyhow!(
                "query_classification.rules[{index}].hint must not be empty"
            ));
        }
        rule.keywords = rule
            .keywords
            .iter()
            .map(|kw| kw.trim().to_string())
            .filter(|kw| !kw.is_empty())
            .collect();
        if rule.keywords.is_empty() {
            return Err(anyhow!(
                "query_classification.rules[{index}].keywords must not be empty (hint={})",
                rule.hint
            ));
        }
    }
    Ok(())
}

fn validate_article_memory_config(config: &mut LocalConfig) -> Result<()> {
    let embedding = &mut config.article_memory.embedding;
    embedding.provider = embedding.provider.trim().to_string();
    embedding.api_key = embedding.api_key.trim().to_string();
    embedding.base_url = embedding.base_url.trim().trim_end_matches('/').to_string();
    embedding.model = embedding.model.trim().to_string();
    if embedding.model.is_empty() {
        embedding.model = default_article_embedding_model();
    }
    if embedding.dimensions == 0 {
        embedding.dimensions = default_article_embedding_dimensions();
    }
    if embedding.max_input_chars == 0 {
        embedding.max_input_chars = default_article_embedding_max_input_chars();
    }
    if !embedding.enabled {
    } else {
        if embedding.provider.is_empty()
            && (embedding.api_key.is_empty() || embedding.base_url.is_empty())
        {
            return Err(anyhow!(
                "article_memory.embedding.provider is required when api_key/base_url are not set"
            ));
        }
        if !embedding.provider.is_empty()
            && !config
                .providers
                .iter()
                .any(|provider| provider.name == embedding.provider)
        {
            return Err(anyhow!(
                "article_memory.embedding.provider does not match a configured provider: {}",
                embedding.provider
            ));
        }
    }

    let normalize = &mut config.article_memory.normalize;
    normalize.provider = normalize.provider.trim().to_string();
    normalize.api_key = normalize.api_key.trim().to_string();
    normalize.base_url = normalize.base_url.trim().trim_end_matches('/').to_string();
    normalize.model = normalize.model.trim().to_string();
    if normalize.min_polish_input_chars == 0 {
        normalize.min_polish_input_chars = default_article_normalize_min_polish_input_chars();
    }
    if normalize.max_polish_input_chars == 0 {
        normalize.max_polish_input_chars = default_article_normalize_max_polish_input_chars();
    }
    if normalize.summary_input_chars == 0 {
        normalize.summary_input_chars = default_article_normalize_summary_input_chars();
    }
    if normalize.fallback_min_ratio <= 0.0 || normalize.fallback_min_ratio > 1.0 {
        normalize.fallback_min_ratio = default_article_normalize_fallback_min_ratio();
    }
    if normalize.llm_polish || normalize.llm_summary {
        if normalize.provider.is_empty()
            && (normalize.api_key.is_empty() || normalize.base_url.is_empty())
        {
            return Err(anyhow!(
                "article_memory.normalize.provider is required when api_key/base_url are not set"
            ));
        }
        if !normalize.provider.is_empty()
            && !config
                .providers
                .iter()
                .any(|provider| provider.name == normalize.provider)
        {
            return Err(anyhow!(
                "article_memory.normalize.provider does not match a configured provider: {}",
                normalize.provider
            ));
        }
    }
    Ok(())
}

fn validate_profile(
    name: &str,
    profile: &RoutingProfileConfig,
    providers: &[ModelProviderConfig],
) -> Result<()> {
    if profile.provider.trim().is_empty() {
        return Err(anyhow!("routing.profiles.{name}.provider is required"));
    }
    if profile.model.trim().is_empty() {
        return Err(anyhow!("routing.profiles.{name}.model is required"));
    }
    if !providers.iter().any(|p| p.name == profile.provider) {
        return Err(anyhow!(
            "routing.profiles.{name}.provider '{}' does not match any configured provider",
            profile.provider
        ));
    }
    if profile.max_fallbacks > 3 {
        return Err(anyhow!(
            "routing.profiles.{name}.max_fallbacks must be <= 3"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod example_config_tests {
    use super::{validate_local_config, LocalConfig};

    const EXAMPLE_TOML: &str = include_str!("../config/davis/local.example.toml");

    /// Guards against schema drift — if the example still compiles but wouldn't
    /// pass validation (stale field names, missing required sections, etc.), a
    /// new user running `cp local.example.toml local.toml` would hit a cryptic
    /// startup error. Catch it in CI instead.
    #[test]
    fn local_example_toml_parses_and_validates() {
        let parsed: LocalConfig = toml::from_str(EXAMPLE_TOML)
            .expect("local.example.toml must parse against the current LocalConfig schema");
        validate_local_config(parsed)
            .expect("local.example.toml must pass validate_local_config unchanged");
    }
}
