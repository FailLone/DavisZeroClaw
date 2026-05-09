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
    #[serde(default)]
    pub zeroclaw: ZeroclawConfig,
    #[serde(default)]
    pub tunnel: Option<TunnelConfig>,
    #[serde(default)]
    pub shortcut: ShortcutConfig,
    #[serde(default)]
    pub router_dhcp: RouterDhcpConfig,
}

/// User-editable settings rendered into zeroclaw's `config.toml` as
/// `[[cron.jobs]]` blocks. Davis itself does no scheduling or delivery;
/// zeroclaw runs the cron and performs the HTTP fetches + Telegram push.
/// See `model_routing::render_digest_cron_jobs`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ZeroclawConfig {
    #[serde(default)]
    pub digest: DigestConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub weekly_topic: Option<String>,
    #[serde(default)]
    pub telegram_chat_id: Option<String>,
    #[serde(default = "default_digest_timezone")]
    pub timezone: String,
}

impl Default for DigestConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            weekly_topic: None,
            telegram_chat_id: None,
            timezone: default_digest_timezone(),
        }
    }
}

fn default_digest_timezone() -> String {
    "Asia/Shanghai".into()
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TunnelConfig {
    pub tunnel_id: Option<String>,
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShortcutConfig {
    #[serde(default)]
    pub external_url: Option<String>,
    #[serde(default)]
    pub lan_url: Option<String>,
    #[serde(default)]
    pub lan_ssids: Vec<String>,
    #[serde(default)]
    pub reply: Option<ShortcutReplyConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutReplyConfig {
    #[serde(default = "default_brief_threshold_chars")]
    pub brief_threshold_chars: usize,
    #[serde(default = "default_shortcut_wait_timeout_secs")]
    pub shortcut_wait_timeout_secs: u64,
    #[serde(default = "default_pending_max_age_secs")]
    pub pending_max_age_secs: u64,
    pub default_imessage_handle: String,
    pub phrases: ShortcutReplyPhrases,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutReplyPhrases {
    pub speak_brief_imessage_full: String,
    pub error_generic: String,
}

fn default_brief_threshold_chars() -> usize {
    60
}

fn default_shortcut_wait_timeout_secs() -> u64 {
    20
}

fn default_pending_max_age_secs() -> u64 {
    300
}

/// Periodic worker that drives the LAN router admin page (Playwright
/// flow lives in `router_adapter/`). Off by default. See
/// `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterDhcpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_router_dhcp_interval_secs")]
    pub interval_secs: u64,
    #[serde(default = "default_router_dhcp_tick_timeout_secs")]
    pub tick_timeout_secs: u64,
    #[serde(default = "default_router_dhcp_url")]
    pub url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

impl Default for RouterDhcpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_router_dhcp_interval_secs(),
            tick_timeout_secs: default_router_dhcp_tick_timeout_secs(),
            url: default_router_dhcp_url(),
            username: String::new(),
            password: String::new(),
        }
    }
}

fn default_router_dhcp_interval_secs() -> u64 {
    600
}

fn default_router_dhcp_tick_timeout_secs() -> u64 {
    90
}

fn default_router_dhcp_url() -> String {
    "http://192.168.0.1".to_string()
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
    /// LLM provider credentials for the value-judge stage. Algorithm
    /// thresholds (save/candidate cutoffs, target topics) live in
    /// `config/davis/article_memory.toml` as `ArticleValueConfig`; creds live
    /// here so they stay out of git.
    #[serde(default)]
    pub value: ArticleMemoryValueConfig,
    #[serde(default)]
    pub ingest: ArticleMemoryIngestConfig,
    #[serde(default)]
    pub extract: ArticleMemoryExtractConfig,
    #[serde(default)]
    pub quality_gate: QualityGateToml,
    #[serde(default)]
    pub rule_learning: RuleLearningConfig,
    #[serde(default)]
    pub discovery: DiscoveryConfig,
    #[serde(default)]
    pub translate: TranslateConfig,
    #[serde(default)]
    pub refresh: RefreshConfig,
}

/// LLM credentials for the article value-judge stage. Mirrors
/// `ArticleMemoryNormalizeConfig`'s shape so either block can reuse a
/// configured `[[providers]]` entry by name, or override `api_key` /
/// `base_url` directly. When `provider` is empty, direct credentials are
/// used verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArticleMemoryValueConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_refresh_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_stale_days")]
    pub stale_after_days: u64,
    #[serde(default = "default_score_delta")]
    pub score_delta_threshold: f32,
    #[serde(default = "default_refresh_batch")]
    pub batch_per_cycle: usize,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_refresh_interval(),
            stale_after_days: default_stale_days(),
            score_delta_threshold: default_score_delta(),
            batch_per_cycle: default_refresh_batch(),
        }
    }
}

fn default_refresh_interval() -> u64 {
    86_400
}
fn default_stale_days() -> u64 {
    30
}
fn default_score_delta() -> f32 {
    0.2
}
fn default_refresh_batch() -> usize {
    20
}

impl RefreshConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.stale_after_days == 0 {
            anyhow::bail!("refresh.stale_after_days must be > 0");
        }
        if !(0.0..=1.0).contains(&self.score_delta_threshold) {
            anyhow::bail!("refresh.score_delta_threshold must be in [0.0, 1.0]");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscoveryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_discovery_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_discovery_max_per_cycle")]
    pub max_per_cycle: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<DiscoverySearchConfig>,
    #[serde(default)]
    pub topics: Vec<DiscoveryTopicConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryTopicConfig {
    pub slug: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub feeds: Vec<String>,
    #[serde(default)]
    pub sitemaps: Vec<String>,
    #[serde(default)]
    pub search_queries: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySearchConfig {
    #[serde(default = "default_search_provider")]
    pub provider: String,
    /// Direct API key. Convenient for single-user setups where `local.toml`
    /// is gitignored. `api_key_env` is preferred for shared configs. When
    /// both are set, `api_key` wins.
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_search_rate_limit")]
    pub rate_limit_per_min: u32,
    #[serde(default = "default_search_results_per_query")]
    pub results_per_query: usize,
}

fn default_discovery_interval() -> u64 {
    43_200
} // 12h
fn default_discovery_max_per_cycle() -> usize {
    20
}
fn default_search_provider() -> String {
    "brave".into()
}
fn default_search_rate_limit() -> u32 {
    60
}
fn default_search_results_per_query() -> usize {
    10
}

impl DiscoveryConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.interval_secs < 60 {
            anyhow::bail!(
                "discovery.interval_secs must be >= 60 (got {})",
                self.interval_secs
            );
        }
        if self.max_per_cycle == 0 {
            anyhow::bail!("discovery.max_per_cycle must be > 0");
        }
        let mut seen = std::collections::HashSet::new();
        for topic in &self.topics {
            if topic.slug.trim().is_empty() {
                anyhow::bail!("discovery topic has empty slug");
            }
            if !seen.insert(topic.slug.clone()) {
                anyhow::bail!("duplicate discovery topic slug: {}", topic.slug);
            }
            if !topic.enabled {
                continue;
            }
            if topic.feeds.is_empty()
                && topic.sitemaps.is_empty()
                && topic.search_queries.is_empty()
            {
                anyhow::bail!(
                    "discovery topic '{}' has no feeds, sitemaps, or search queries",
                    topic.slug
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslateConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_target_language")]
    pub target_language: String,
    #[serde(default = "default_zeroclaw_base_url")]
    pub zeroclaw_base_url: String,
    #[serde(default = "default_translate_budget_scope")]
    pub budget_scope: String,
    #[serde(default = "default_translate_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_translate_batch")]
    pub batch_per_cycle: usize,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

impl Default for TranslateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            target_language: default_target_language(),
            zeroclaw_base_url: default_zeroclaw_base_url(),
            budget_scope: default_translate_budget_scope(),
            interval_secs: default_translate_interval(),
            batch_per_cycle: default_translate_batch(),
            api_key_env: None,
        }
    }
}

fn default_target_language() -> String {
    "zh-CN".into()
}
fn default_zeroclaw_base_url() -> String {
    "http://127.0.0.1:3001".into()
}
fn default_translate_budget_scope() -> String {
    "translation:monthly".into()
}
fn default_translate_interval() -> u64 {
    300
}
fn default_translate_batch() -> usize {
    5
}

impl TranslateConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        url::Url::parse(&self.zeroclaw_base_url)
            .map_err(|e| anyhow::anyhow!("translate.zeroclaw_base_url invalid: {e}"))?;
        if self.interval_secs < 30 {
            anyhow::bail!("translate.interval_secs must be >= 30");
        }
        if self.batch_per_cycle == 0 {
            anyhow::bail!("translate.batch_per_cycle must be > 0");
        }
        Ok(())
    }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryIngestConfig {
    #[serde(default = "default_ingest_enabled")]
    pub enabled: bool,
    #[serde(default = "default_ingest_max_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_ingest_default_profile")]
    pub default_profile: String,
    #[serde(default = "default_ingest_dedup_window_hours")]
    pub dedup_window_hours: u64,
    #[serde(default)]
    pub allow_private_hosts: Vec<String>,
    #[serde(default)]
    pub host_profiles: Vec<ArticleMemoryHostProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryHostProfile {
    #[serde(rename = "match")]
    pub match_suffix: String,
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryExtractConfig {
    #[serde(default = "default_extract_engine")]
    pub default_engine: String,
    #[serde(default = "default_fallback_ladder")]
    pub fallback_ladder: Vec<String>,
    #[serde(default)]
    pub openrouter_llm: OpenRouterLlmEngineConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OpenRouterLlmEngineConfig {
    #[serde(default)]
    pub provider: String,
    #[serde(default = "default_openrouter_llm_model")]
    pub model: String,
    #[serde(default = "default_openrouter_llm_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_openrouter_llm_max_input_chars")]
    pub max_input_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityGateToml {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_gate_min_markdown_chars")]
    pub min_markdown_chars: usize,
    #[serde(default = "default_gate_min_kept_ratio")]
    pub min_kept_ratio: f32,
    #[serde(default = "default_gate_min_paragraphs")]
    pub min_paragraphs: usize,
    #[serde(default = "default_gate_max_link_density")]
    pub max_link_density: f32,
    #[serde(default)]
    pub boilerplate_markers: Vec<String>,
}

impl Default for QualityGateToml {
    fn default() -> Self {
        Self {
            enabled: true,
            min_markdown_chars: default_gate_min_markdown_chars(),
            min_kept_ratio: default_gate_min_kept_ratio(),
            min_paragraphs: default_gate_min_paragraphs(),
            max_link_density: default_gate_max_link_density(),
            boilerplate_markers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleLearningConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_samples_required")]
    pub samples_required: usize,
    #[serde(default = "default_stale_after_partial")]
    pub stale_after_consecutive_issues: u32,
    #[serde(default = "default_learning_provider")]
    pub learning_provider: String,
    #[serde(default = "default_learning_model")]
    pub learning_model: String,
    #[serde(default = "default_true")]
    pub notify_on_quarantine: bool,
}

impl Default for RuleLearningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            samples_required: default_samples_required(),
            stale_after_consecutive_issues: default_stale_after_partial(),
            learning_provider: default_learning_provider(),
            learning_model: default_learning_model(),
            notify_on_quarantine: true,
        }
    }
}

fn default_samples_required() -> usize {
    3
}

fn default_stale_after_partial() -> u32 {
    2
}

fn default_learning_provider() -> String {
    "openrouter".to_string()
}

fn default_learning_model() -> String {
    "openai/gpt-4o".to_string()
}

impl Default for ArticleMemoryExtractConfig {
    fn default() -> Self {
        Self {
            default_engine: default_extract_engine(),
            fallback_ladder: default_fallback_ladder(),
            openrouter_llm: OpenRouterLlmEngineConfig::default(),
        }
    }
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

fn default_ingest_enabled() -> bool {
    true
}

fn default_ingest_max_concurrency() -> usize {
    3
}

fn default_ingest_default_profile() -> String {
    "articles-generic".to_string()
}

fn default_ingest_dedup_window_hours() -> u64 {
    24
}

fn default_extract_engine() -> String {
    "trafilatura".to_string()
}

fn default_fallback_ladder() -> Vec<String> {
    vec!["trafilatura".to_string(), "openrouter-llm".to_string()]
}

fn default_openrouter_llm_model() -> String {
    "google/gemini-2.0-flash-001".to_string()
}

fn default_openrouter_llm_timeout_secs() -> u64 {
    60
}

fn default_openrouter_llm_max_input_chars() -> usize {
    60_000
}

fn default_gate_min_markdown_chars() -> usize {
    500
}

fn default_gate_min_kept_ratio() -> f32 {
    0.05
}

fn default_gate_min_paragraphs() -> usize {
    3
}

fn default_gate_max_link_density() -> f32 {
    0.5
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

impl Default for ArticleMemoryIngestConfig {
    fn default() -> Self {
        Self {
            enabled: default_ingest_enabled(),
            max_concurrency: default_ingest_max_concurrency(),
            default_profile: default_ingest_default_profile(),
            dedup_window_hours: default_ingest_dedup_window_hours(),
            allow_private_hosts: Vec::new(),
            host_profiles: Vec::new(),
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
    validate_shortcut_reply(&mut config.shortcut.reply)?;
    validate_router_dhcp(&mut config.router_dhcp)?;

    Ok(config)
}

/// Catch user-misedited `[router_dhcp]` blocks before the worker starts
/// silently misbehaving. No-op when disabled — defaults are all valid.
fn validate_router_dhcp(cfg: &mut RouterDhcpConfig) -> Result<()> {
    if !cfg.enabled {
        return Ok(());
    }
    cfg.url = cfg.url.trim().to_string();
    cfg.username = cfg.username.trim().to_string();
    cfg.password = cfg.password.trim().to_string();
    if cfg.url.is_empty() {
        return Err(anyhow!("router_dhcp.url must not be empty when enabled"));
    }
    if cfg.username.is_empty() {
        return Err(anyhow!(
            "router_dhcp.username must not be empty when enabled"
        ));
    }
    if cfg.password.is_empty() {
        return Err(anyhow!(
            "router_dhcp.password must not be empty when enabled"
        ));
    }
    if cfg.interval_secs == 0 {
        return Err(anyhow!("router_dhcp.interval_secs must be > 0"));
    }
    if cfg.tick_timeout_secs == 0 {
        return Err(anyhow!("router_dhcp.tick_timeout_secs must be > 0"));
    }
    if cfg.tick_timeout_secs >= cfg.interval_secs {
        return Err(anyhow!(
            "router_dhcp.tick_timeout_secs ({}) must be less than interval_secs ({}) so a stuck tick cannot starve the next one",
            cfg.tick_timeout_secs,
            cfg.interval_secs
        ));
    }
    Ok(())
}

fn validate_shortcut_reply(reply: &mut Option<ShortcutReplyConfig>) -> Result<()> {
    let Some(cfg) = reply.as_mut() else {
        return Ok(());
    };
    cfg.default_imessage_handle = cfg.default_imessage_handle.trim().to_string();
    if cfg.default_imessage_handle.is_empty() {
        return Err(anyhow!(
            "shortcut.reply.default_imessage_handle must not be empty"
        ));
    }
    cfg.phrases.speak_brief_imessage_full =
        cfg.phrases.speak_brief_imessage_full.trim().to_string();
    if cfg.phrases.speak_brief_imessage_full.is_empty() {
        return Err(anyhow!(
            "shortcut.reply.phrases.speak_brief_imessage_full must not be empty"
        ));
    }
    cfg.phrases.error_generic = cfg.phrases.error_generic.trim().to_string();
    if cfg.phrases.error_generic.is_empty() {
        return Err(anyhow!(
            "shortcut.reply.phrases.error_generic must not be empty"
        ));
    }
    if cfg.shortcut_wait_timeout_secs == 0 {
        cfg.shortcut_wait_timeout_secs = default_shortcut_wait_timeout_secs();
    }
    if cfg.pending_max_age_secs == 0 {
        cfg.pending_max_age_secs = default_pending_max_age_secs();
    }
    Ok(())
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

    let value = &mut config.article_memory.value;
    value.provider = value.provider.trim().to_string();
    value.api_key = value.api_key.trim().to_string();
    value.base_url = value.base_url.trim().trim_end_matches('/').to_string();
    value.model = value.model.trim().to_string();
    if !value.provider.is_empty()
        && !config
            .providers
            .iter()
            .any(|provider| provider.name == value.provider)
    {
        return Err(anyhow!(
            "article_memory.value.provider does not match a configured provider: {}",
            value.provider
        ));
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
mod tests {
    use super::{ArticleMemoryConfig, ArticleMemoryExtractConfig, QualityGateToml};

    #[test]
    fn article_memory_ingest_defaults_when_missing() {
        let toml = r#"
            [normalize]
            [embedding]
        "#;
        let cfg: ArticleMemoryConfig = toml::from_str(toml).unwrap();
        assert!(cfg.ingest.enabled);
        assert_eq!(cfg.ingest.max_concurrency, 3);
        assert_eq!(cfg.ingest.default_profile, "articles-generic");
        assert_eq!(cfg.ingest.dedup_window_hours, 24);
        assert!(cfg.ingest.allow_private_hosts.is_empty());
        assert!(cfg.ingest.host_profiles.is_empty());
    }

    #[test]
    fn article_memory_ingest_parses_host_profiles() {
        let toml = r#"
            [normalize]
            [embedding]
            [ingest]
            enabled = false
            max_concurrency = 5
            allow_private_hosts = ["wiki.internal"]
            [[ingest.host_profiles]]
            match = "zhihu.com"
            profile = "articles-zhihu"
            source = "zhihu"
        "#;
        let cfg: ArticleMemoryConfig = toml::from_str(toml).unwrap();
        assert!(!cfg.ingest.enabled);
        assert_eq!(cfg.ingest.max_concurrency, 5);
        assert_eq!(cfg.ingest.allow_private_hosts, vec!["wiki.internal"]);
        assert_eq!(cfg.ingest.host_profiles.len(), 1);
        assert_eq!(cfg.ingest.host_profiles[0].match_suffix, "zhihu.com");
        assert_eq!(cfg.ingest.host_profiles[0].profile, "articles-zhihu");
        assert_eq!(cfg.ingest.host_profiles[0].source.as_deref(), Some("zhihu"));
    }

    #[test]
    fn article_memory_extract_defaults_to_trafilatura() {
        let toml = r#"
[extract]
        "#;
        let cfg: ExtractWrapper = toml::from_str(toml).unwrap();
        assert_eq!(cfg.extract.default_engine, "trafilatura");
        assert_eq!(
            cfg.extract.fallback_ladder,
            vec!["trafilatura".to_string(), "openrouter-llm".to_string()]
        );
    }

    #[test]
    fn quality_gate_defaults_are_sane() {
        let toml = "";
        let cfg: QualityGateWrapper = toml::from_str(toml).unwrap();
        assert!(cfg.quality_gate.enabled);
        assert_eq!(cfg.quality_gate.min_markdown_chars, 500);
    }

    #[derive(serde::Deserialize)]
    struct ExtractWrapper {
        #[serde(default)]
        extract: ArticleMemoryExtractConfig,
    }

    #[derive(serde::Deserialize)]
    struct QualityGateWrapper {
        #[serde(default)]
        quality_gate: QualityGateToml,
    }
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

#[cfg(test)]
mod discovery_config_tests {
    use super::*;

    fn sample_topic() -> DiscoveryTopicConfig {
        DiscoveryTopicConfig {
            slug: "async-rust".into(),
            keywords: vec!["async rust".into()],
            feeds: vec!["https://without.boats/index.xml".into()],
            sitemaps: vec![],
            search_queries: vec![],
            enabled: true,
        }
    }

    #[test]
    fn rejects_empty_slug_when_enabled() {
        let mut topic = sample_topic();
        topic.slug = "".into();
        let cfg = DiscoveryConfig {
            enabled: true,
            interval_secs: 3600,
            max_per_cycle: 10,
            search: None,
            topics: vec![topic],
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("slug"), "{err}");
    }

    #[test]
    fn rejects_duplicate_slug() {
        let cfg = DiscoveryConfig {
            enabled: true,
            interval_secs: 3600,
            max_per_cycle: 10,
            search: None,
            topics: vec![sample_topic(), sample_topic()],
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn rejects_interval_below_60_secs() {
        let cfg = DiscoveryConfig {
            enabled: true,
            interval_secs: 30,
            max_per_cycle: 10,
            search: None,
            topics: vec![sample_topic()],
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("interval_secs"), "{err}");
    }

    #[test]
    fn accepts_disabled_with_no_topics() {
        let cfg = DiscoveryConfig {
            enabled: false,
            interval_secs: 3600,
            max_per_cycle: 10,
            search: None,
            topics: vec![],
        };
        cfg.validate().unwrap();
    }

    #[test]
    fn accepts_enabled_topic_with_no_feeds_but_has_search_queries() {
        let mut topic = sample_topic();
        topic.feeds = vec![];
        topic.search_queries = vec!["async rust tokio".into()];
        let cfg = DiscoveryConfig {
            enabled: true,
            interval_secs: 3600,
            max_per_cycle: 10,
            search: None,
            topics: vec![topic],
        };
        cfg.validate().unwrap();
    }

    #[test]
    fn rejects_topic_with_no_signal_sources() {
        let mut topic = sample_topic();
        topic.feeds = vec![];
        topic.sitemaps = vec![];
        topic.search_queries = vec![];
        let cfg = DiscoveryConfig {
            enabled: true,
            interval_secs: 3600,
            max_per_cycle: 10,
            search: None,
            topics: vec![topic],
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(
            err.contains("no feeds, sitemaps, or search queries"),
            "{err}"
        );
    }
}

#[cfg(test)]
mod translate_config_tests {
    use super::*;

    #[test]
    fn rejects_bad_base_url_when_enabled() {
        let cfg = TranslateConfig {
            enabled: true,
            zeroclaw_base_url: "not a url".into(),
            ..TranslateConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn accepts_disabled() {
        let cfg = TranslateConfig {
            enabled: false,
            ..TranslateConfig::default()
        };
        cfg.validate().unwrap();
    }

    #[test]
    fn rejects_interval_below_30() {
        let cfg = TranslateConfig {
            enabled: true,
            zeroclaw_base_url: "http://127.0.0.1:3001".into(),
            interval_secs: 5,
            ..TranslateConfig::default()
        };
        assert!(cfg.validate().is_err());
    }
}

#[cfg(test)]
mod refresh_config_tests {
    use super::*;

    #[test]
    fn accepts_defaults() {
        let cfg = RefreshConfig::default();
        cfg.validate().unwrap();
    }

    #[test]
    fn rejects_threshold_outside_0_1() {
        let cfg = RefreshConfig {
            enabled: true,
            score_delta_threshold: 2.0,
            ..RefreshConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_stale_days_zero() {
        let cfg = RefreshConfig {
            enabled: true,
            stale_after_days: 0,
            ..RefreshConfig::default()
        };
        assert!(cfg.validate().is_err());
    }
}

#[cfg(test)]
mod router_dhcp_config_tests {
    use super::*;

    const DHCP_BASE_TOML: &str = r#"
[home_assistant]
url = "http://example"
token = "x"
[imessage]
allowed_contacts = ["+15550000000"]
[[providers]]
name = "p"
api_key = "k"
allowed_models = ["m"]
[routing.profiles.home_control]
provider = "p"
model = "m"
[routing.profiles.general_qa]
provider = "p"
model = "m"
[routing.profiles.research]
provider = "p"
model = "m"
[routing.profiles.structured_lookup]
provider = "p"
model = "m"
"#;

    #[test]
    fn router_dhcp_config_defaults_when_section_missing() {
        let cfg: LocalConfig = toml::from_str(DHCP_BASE_TOML).expect("parse");
        assert!(!cfg.router_dhcp.enabled);
        assert_eq!(cfg.router_dhcp.interval_secs, 600);
        assert_eq!(cfg.router_dhcp.tick_timeout_secs, 90);
        assert_eq!(cfg.router_dhcp.url, "http://192.168.0.1");
        assert_eq!(cfg.router_dhcp.username, "");
        assert_eq!(cfg.router_dhcp.password, "");
    }

    #[test]
    fn router_dhcp_config_explicit_values_respected() {
        let toml_text = format!(
            r#"{base}
[router_dhcp]
enabled = true
interval_secs = 1200
tick_timeout_secs = 120
url = "http://192.168.1.1"
username = "admin"
password = "secret"
"#,
            base = DHCP_BASE_TOML
        );
        let cfg: LocalConfig = toml::from_str(&toml_text).expect("parse");
        assert!(cfg.router_dhcp.enabled);
        assert_eq!(cfg.router_dhcp.interval_secs, 1200);
        assert_eq!(cfg.router_dhcp.tick_timeout_secs, 120);
        assert_eq!(cfg.router_dhcp.url, "http://192.168.1.1");
        assert_eq!(cfg.router_dhcp.username, "admin");
        assert_eq!(cfg.router_dhcp.password, "secret");
    }

    #[test]
    fn validate_router_dhcp_noop_when_disabled() {
        // url is whitespace — would fail if enabled, but disabled bypasses checks.
        let mut cfg = RouterDhcpConfig {
            url: "  ".to_string(),
            ..RouterDhcpConfig::default()
        };
        assert!(super::validate_router_dhcp(&mut cfg).is_ok());
    }

    #[test]
    fn validate_router_dhcp_rejects_empty_url() {
        let mut cfg = RouterDhcpConfig {
            enabled: true,
            url: "   ".into(),
            ..RouterDhcpConfig::default()
        };
        let err = super::validate_router_dhcp(&mut cfg).unwrap_err();
        assert!(err.to_string().contains("router_dhcp.url"));
    }

    #[test]
    fn validate_router_dhcp_rejects_empty_credentials() {
        let mut cfg = RouterDhcpConfig {
            enabled: true,
            username: "".into(),
            password: "secret".into(),
            ..RouterDhcpConfig::default()
        };
        assert!(super::validate_router_dhcp(&mut cfg).is_err());
        let mut cfg = RouterDhcpConfig {
            enabled: true,
            username: "admin".into(),
            password: "".into(),
            ..RouterDhcpConfig::default()
        };
        assert!(super::validate_router_dhcp(&mut cfg).is_err());
    }

    #[test]
    fn validate_router_dhcp_rejects_zero_intervals() {
        let mut cfg = RouterDhcpConfig {
            enabled: true,
            interval_secs: 0,
            username: "admin".into(),
            password: "secret".into(),
            ..RouterDhcpConfig::default()
        };
        assert!(super::validate_router_dhcp(&mut cfg).is_err());
        let mut cfg = RouterDhcpConfig {
            enabled: true,
            tick_timeout_secs: 0,
            username: "admin".into(),
            password: "secret".into(),
            ..RouterDhcpConfig::default()
        };
        assert!(super::validate_router_dhcp(&mut cfg).is_err());
    }

    #[test]
    fn validate_router_dhcp_rejects_timeout_geq_interval() {
        let mut cfg = RouterDhcpConfig {
            enabled: true,
            interval_secs: 60,
            tick_timeout_secs: 60,
            username: "admin".into(),
            password: "secret".into(),
            ..RouterDhcpConfig::default()
        };
        let err = super::validate_router_dhcp(&mut cfg).unwrap_err();
        assert!(err.to_string().contains("less than interval_secs"));
    }

    #[test]
    fn validate_router_dhcp_trims_whitespace() {
        let mut cfg = RouterDhcpConfig {
            enabled: true,
            url: "  http://example  ".into(),
            username: "  admin  ".into(),
            password: "  secret  ".into(),
            ..RouterDhcpConfig::default()
        };
        super::validate_router_dhcp(&mut cfg).expect("ok");
        assert_eq!(cfg.url, "http://example");
        assert_eq!(cfg.username, "admin");
        assert_eq!(cfg.password, "secret");
    }
}

#[cfg(test)]
mod shortcut_reply_config_tests {
    use super::*;

    const MINIMAL_TOML: &str = r#"
[home_assistant]
url = "x"
token = "y"

[imessage]
allowed_contacts = ["+15550000000"]

[[providers]]
name = "p"
api_key = "k"
allowed_models = ["m"]

[routing.profiles.home_control]
provider = "p"
model = "m"

[routing.profiles.general_qa]
provider = "p"
model = "m"

[routing.profiles.research]
provider = "p"
model = "m"

[routing.profiles.structured_lookup]
provider = "p"
model = "m"

[shortcut.reply]
default_imessage_handle = "you@icloud.com"

[shortcut.reply.phrases]
speak_brief_imessage_full = "详情我通过短信发你"
error_generic = "戴维斯好像出问题了"
"#;

    #[test]
    fn shortcut_reply_config_defaults_apply() {
        let cfg: LocalConfig = toml::from_str(MINIMAL_TOML).expect("minimal config parses");
        let reply = cfg.shortcut.reply.expect("reply block present");
        assert_eq!(reply.brief_threshold_chars, 60);
        assert_eq!(reply.shortcut_wait_timeout_secs, 20);
        assert_eq!(reply.pending_max_age_secs, 300);
        assert_eq!(reply.default_imessage_handle, "you@icloud.com");
        assert_eq!(
            reply.phrases.speak_brief_imessage_full,
            "详情我通过短信发你"
        );
        assert_eq!(reply.phrases.error_generic, "戴维斯好像出问题了");
    }

    #[test]
    fn shortcut_reply_absent_is_none() {
        let no_reply_toml = MINIMAL_TOML.replace(
            "\n[shortcut.reply]\ndefault_imessage_handle = \"you@icloud.com\"\n\n[shortcut.reply.phrases]\nspeak_brief_imessage_full = \"详情我通过短信发你\"\nerror_generic = \"戴维斯好像出问题了\"\n",
            "",
        );
        let cfg: LocalConfig = toml::from_str(&no_reply_toml).expect("parses");
        assert!(cfg.shortcut.reply.is_none());
    }

    #[test]
    fn empty_imessage_handle_is_rejected() {
        let toml = MINIMAL_TOML.replace(
            "default_imessage_handle = \"you@icloud.com\"",
            "default_imessage_handle = \"   \"",
        );
        let parsed: LocalConfig = toml::from_str(&toml).expect("parses");
        let err = validate_local_config(parsed).expect_err("must reject empty handle");
        let msg = err.to_string();
        assert!(
            msg.contains("default_imessage_handle"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn empty_error_phrase_is_rejected() {
        let toml = MINIMAL_TOML.replace(
            "error_generic = \"戴维斯好像出问题了\"",
            "error_generic = \"\"",
        );
        let parsed: LocalConfig = toml::from_str(&toml).expect("parses");
        let err = validate_local_config(parsed).expect_err("must reject empty phrase");
        assert!(err.to_string().contains("error_generic"));
    }

    #[test]
    fn zero_timeouts_reset_to_defaults() {
        let toml = MINIMAL_TOML.replace(
            "default_imessage_handle = \"you@icloud.com\"",
            "default_imessage_handle = \"you@icloud.com\"\nshortcut_wait_timeout_secs = 0\npending_max_age_secs = 0",
        );
        let parsed: LocalConfig = toml::from_str(&toml).expect("parses");
        let cfg = validate_local_config(parsed).expect("validates");
        let reply = cfg.shortcut.reply.expect("reply present");
        assert_eq!(reply.shortcut_wait_timeout_secs, 20);
        assert_eq!(reply.pending_max_age_secs, 300);
    }
}
