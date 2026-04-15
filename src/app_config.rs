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
    pub browser_bridge: BrowserBridgeConfig,
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
    pub recompute_interval_minutes: u64,
    pub restart_debounce_minutes: u64,
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
    pub weights: MetricWeights,
    pub minimums: ProfileMinimums,
    pub max_fallbacks: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserBridgeConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_browser_worker_port")]
    pub worker_port: u16,
    #[serde(default = "default_browser_profile_name")]
    pub default_profile: String,
    #[serde(default = "default_browser_profiles")]
    pub profiles: Vec<BrowserProfileConfig>,
    #[serde(default)]
    pub write_policy: BrowserWritePolicyConfig,
    #[serde(default)]
    pub user_session: BrowserUserSessionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProfileConfig {
    pub name: String,
    pub mode: String,
    pub browser: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserWritePolicyConfig {
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    #[serde(default = "default_non_whitelist_behavior")]
    pub default_non_whitelist_behavior: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserUserSessionConfig {
    #[serde(default = "default_true")]
    pub require_remote_debugging: bool,
    #[serde(default = "default_true")]
    pub allow_applescript_fallback: bool,
    #[serde(default = "default_remote_debugging_url")]
    pub remote_debugging_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricWeights {
    pub task_success: f64,
    pub safety: f64,
    pub latency: f64,
    pub stability: f64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMinimums {
    pub task_success: u8,
    pub safety: u8,
}

fn default_true() -> bool {
    true
}

fn default_browser_worker_port() -> u16 {
    3011
}

fn default_browser_profile_name() -> String {
    "user".to_string()
}

fn default_non_whitelist_behavior() -> String {
    "requires_confirmation".to_string()
}

fn default_remote_debugging_url() -> String {
    "http://127.0.0.1:9222".to_string()
}

fn default_browser_profiles() -> Vec<BrowserProfileConfig> {
    vec![
        BrowserProfileConfig {
            name: "user".to_string(),
            mode: "existing_session".to_string(),
            browser: "chrome".to_string(),
        },
        BrowserProfileConfig {
            name: "managed".to_string(),
            mode: "managed".to_string(),
            browser: "chromium".to_string(),
        },
    ]
}

impl Default for BrowserBridgeConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            worker_port: default_browser_worker_port(),
            default_profile: default_browser_profile_name(),
            profiles: default_browser_profiles(),
            write_policy: BrowserWritePolicyConfig::default(),
            user_session: BrowserUserSessionConfig::default(),
        }
    }
}

impl BrowserBridgeConfig {
    pub fn profile(&self, name: &str) -> Option<&BrowserProfileConfig> {
        self.profiles.iter().find(|profile| profile.name == name)
    }
}

impl Default for BrowserWritePolicyConfig {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            default_non_whitelist_behavior: default_non_whitelist_behavior(),
        }
    }
}

impl Default for BrowserUserSessionConfig {
    fn default() -> Self {
        Self {
            require_remote_debugging: default_true(),
            allow_applescript_fallback: default_true(),
            remote_debugging_url: default_remote_debugging_url(),
        }
    }
}

pub fn load_local_config(paths: &RuntimePaths) -> Result<LocalConfig> {
    let path = paths.local_config_path();
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

    validate_profile("home_control", &config.routing.profiles.home_control)?;
    validate_profile("general_qa", &config.routing.profiles.general_qa)?;
    validate_profile("research", &config.routing.profiles.research)?;
    validate_profile(
        "structured_lookup",
        &config.routing.profiles.structured_lookup,
    )?;

    if config.routing.recompute_interval_minutes == 0 {
        return Err(anyhow!("routing.recompute_interval_minutes must be > 0"));
    }
    if config.routing.restart_debounce_minutes == 0 {
        return Err(anyhow!("routing.restart_debounce_minutes must be > 0"));
    }

    config.browser_bridge.default_profile =
        config.browser_bridge.default_profile.trim().to_string();
    if config.browser_bridge.worker_port == 0 {
        return Err(anyhow!("browser_bridge.worker_port must be > 0"));
    }
    if config.browser_bridge.default_profile.is_empty() {
        return Err(anyhow!("browser_bridge.default_profile is required"));
    }
    if config.browser_bridge.profiles.is_empty() {
        return Err(anyhow!("browser_bridge.profiles must not be empty"));
    }
    let mut seen_browser_profiles = BTreeSet::new();
    for profile in &mut config.browser_bridge.profiles {
        profile.name = profile.name.trim().to_string();
        profile.mode = profile.mode.trim().to_string();
        profile.browser = profile.browser.trim().to_string();
        if profile.name.is_empty() {
            return Err(anyhow!("browser_bridge.profiles.name is required"));
        }
        if !matches!(profile.mode.as_str(), "existing_session" | "managed") {
            return Err(anyhow!(
                "browser_bridge.profiles.{}.mode must be existing_session or managed",
                profile.name
            ));
        }
        if !matches!(profile.browser.as_str(), "chrome" | "chromium") {
            return Err(anyhow!(
                "browser_bridge.profiles.{}.browser must be chrome or chromium",
                profile.name
            ));
        }
        if !seen_browser_profiles.insert(profile.name.clone()) {
            return Err(anyhow!(
                "duplicate browser_bridge profile name: {}",
                profile.name
            ));
        }
    }
    if config
        .browser_bridge
        .profile(&config.browser_bridge.default_profile)
        .is_none()
    {
        return Err(anyhow!(
            "browser_bridge.default_profile must match one of browser_bridge.profiles"
        ));
    }
    config.browser_bridge.write_policy.allowed_origins = config
        .browser_bridge
        .write_policy
        .allowed_origins
        .into_iter()
        .map(|origin| origin.trim().to_string())
        .filter(|origin| !origin.is_empty())
        .collect();
    if config
        .browser_bridge
        .write_policy
        .default_non_whitelist_behavior
        .trim()
        .is_empty()
    {
        config
            .browser_bridge
            .write_policy
            .default_non_whitelist_behavior = default_non_whitelist_behavior();
    }
    if config
        .browser_bridge
        .user_session
        .remote_debugging_url
        .trim()
        .is_empty()
    {
        config.browser_bridge.user_session.remote_debugging_url = default_remote_debugging_url();
    }

    Ok(config)
}

fn validate_profile(name: &str, profile: &RoutingProfileConfig) -> Result<()> {
    let weights = [
        profile.weights.task_success,
        profile.weights.safety,
        profile.weights.latency,
        profile.weights.stability,
        profile.weights.cost,
    ];
    if weights.iter().any(|value| *value < 0.0) {
        return Err(anyhow!(
            "routing.profiles.{name}.weights must not contain negative values"
        ));
    }
    let sum: f64 = weights.iter().sum();
    if (sum - 1.0).abs() > 0.001 {
        return Err(anyhow!("routing.profiles.{name}.weights must sum to 1.0"));
    }
    if profile.max_fallbacks > 3 {
        return Err(anyhow!(
            "routing.profiles.{name}.max_fallbacks must be <= 3 in V1"
        ));
    }
    Ok(())
}
