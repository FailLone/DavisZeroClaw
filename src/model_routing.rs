use crate::app_config::{load_local_config, LocalConfig, ModelProviderConfig, RoutingProfileConfig};
use crate::ha_client::normalize_ha_url;
use crate::runtime_paths::RuntimePaths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RoutingProfile {
    HomeControl,
    GeneralQa,
    Research,
    StructuredLookup,
}

impl RoutingProfile {
    pub fn all() -> [Self; 4] {
        [
            Self::HomeControl,
            Self::GeneralQa,
            Self::Research,
            Self::StructuredLookup,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HomeControl => "home_control",
            Self::GeneralQa => "general_qa",
            Self::Research => "research",
            Self::StructuredLookup => "structured_lookup",
        }
    }

    fn profile_config<'a>(&self, config: &'a LocalConfig) -> &'a RoutingProfileConfig {
        match self {
            Self::HomeControl => &config.routing.profiles.home_control,
            Self::GeneralQa => &config.routing.profiles.general_qa,
            Self::Research => &config.routing.profiles.research,
            Self::StructuredLookup => &config.routing.profiles.structured_lookup,
        }
    }
}

// ── Config rendering ─────────────────────────────────────────────────

/// Render the ZeroClaw runtime config.toml from local.toml + template.
/// Called once at startup. After this, model changes go through ZeroClaw's
/// built-in `model_routing_config` tool (no daemon restart needed).
pub fn render_runtime_config(paths: &RuntimePaths, config: &LocalConfig) -> Result<()> {
    let template = std::fs::read_to_string(paths.config_template_path())
        .context("failed to read ZeroClaw config template")?;

    let default_profile = config
        .routing
        .default_profile
        .as_deref()
        .unwrap_or("general_qa");

    let rendered = template
        .replace(
            "__DAVIS_IMESSAGE_CONFIG__",
            &render_imessage_config(config),
        )
        .replace(
            "__DAVIS_WEBHOOK_SECRET_CONFIG__",
            &render_webhook_secret_config(config),
        )
        .replace(
            "__DAVIS_PROVIDERS_CONFIG__",
            &render_providers_config(config, default_profile),
        )
        .replace(
            "__DAVIS_QUERY_CLASSIFICATION_CONFIG__",
            &render_query_classification(),
        )
        .replace(
            "__DAVIS_MCP_SERVERS_CONFIG__",
            &render_mcp_servers_config(paths, config),
        )
        .replace(
            "__DAVIS_REPO_ROOT__",
            &toml_escape(&paths.repo_root.display().to_string()),
        )
        .replace("__DAVIS_MODEL_FALLBACKS__", &render_model_fallbacks(config))
        .replace(
            "__DAVIS_HA_URL__",
            &toml_escape(
                &normalize_ha_url(&config.home_assistant.url).map_err(anyhow::Error::msg)?,
            ),
        )
        .replace(
            "__DAVIS_HA_TOKEN__",
            &toml_escape(&config.home_assistant.token),
        );

    std::fs::create_dir_all(&paths.runtime_dir)?;
    let runtime_path = paths.runtime_config_path();
    std::fs::write(&runtime_path, rendered)?;
    restrict_secret_file_permissions(&runtime_path);
    Ok(())
}

/// Tighten file mode to 0600 on Unix. Rendered runtime config contains API
/// keys and the HA token, so world/group-readable bits are never needed.
/// Silent on failure — best-effort hardening, not a correctness gate.
#[cfg(unix)]
fn restrict_secret_file_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_secret_file_permissions(_path: &std::path::Path) {}

/// Emit a warning on stderr when a file containing secrets is world- or
/// group-readable. Soft check — we never bail, so first-time onboarding
/// (where `cp local.example.toml local.toml` inherits umask 0644) still works.
#[cfg(unix)]
pub fn warn_if_secret_file_is_loose(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        eprintln!(
            "warning: {} is mode {:o}; contains secrets — run `chmod 600 {}`",
            path.display(),
            mode,
            path.display()
        );
    }
}

#[cfg(not(unix))]
pub fn warn_if_secret_file_is_loose(_path: &std::path::Path) {}

// ── Environment variables ────────────────────────────────────────────

pub fn zeroclaw_env_vars(config: &LocalConfig) -> Vec<(String, String)> {
    let mut exports = Vec::new();
    let mut seen = BTreeSet::new();
    for provider in &config.providers {
        for env_name in provider_api_key_env_names(&provider.name) {
            if seen.insert(env_name.clone()) {
                exports.push((env_name, provider.api_key.clone()));
            }
        }
    }
    exports
}

pub fn check_local_config(paths: &RuntimePaths) -> Result<LocalConfig> {
    load_local_config(paths)
}

// ── Render helpers ───────────────────────────────────────────────────

fn render_imessage_config(config: &LocalConfig) -> String {
    let contacts = config
        .imessage
        .allowed_contacts
        .iter()
        .map(|item| format!("\"{}\"", toml_escape(item)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[channels.imessage]\nenabled = true\nallowed_contacts = [{contacts}]\n")
}

fn render_webhook_secret_config(config: &LocalConfig) -> String {
    if config.webhook.secret.trim().is_empty() {
        "# secret = \"replace-with-your-webhook-secret\"".to_string()
    } else {
        format!("secret = \"{}\"", toml_escape(config.webhook.secret.trim()))
    }
}

fn render_providers_config(config: &LocalConfig, default_profile: &str) -> String {
    // Find the default profile's provider to set as ZeroClaw's fallback provider.
    let default_provider_name = RoutingProfile::all()
        .iter()
        .find(|p| p.as_str() == default_profile)
        .map(|p| p.profile_config(config).provider.as_str())
        .unwrap_or_else(|| config.providers.first().map(|p| p.name.as_str()).unwrap_or(""));

    let fallback_id = config
        .providers
        .iter()
        .find(|p| p.name == default_provider_name)
        .map(|p| zeroclaw_provider_id(p))
        .unwrap_or_default();

    let mut output = String::from("[providers]\n");
    if !fallback_id.is_empty() {
        output.push_str(&format!("fallback = \"{}\"\n", toml_escape(&fallback_id)));
    }

    // Render each provider definition.
    for provider in &config.providers {
        let provider_id = zeroclaw_provider_id(provider);
        output.push_str(&format!(
            "\n[providers.models.{}]\n",
            toml_key_segment(&provider_id)
        ));
        if provider.base_url.trim().is_empty() {
            output.push_str(&format!("name = \"{}\"\n", toml_escape(&provider.name)));
        }
        if !provider.api_key.trim().is_empty() {
            output.push_str(&format!(
                "api_key = \"{}\"\n",
                toml_escape(&provider.api_key)
            ));
        }
        // Use the first model from allowed_models as the default for this provider,
        // or the model from whichever profile references this provider.
        let model = find_model_for_provider(config, &provider.name);
        output.push_str(&format!("model = \"{}\"\n", toml_escape(&model)));
    }

    // Render model routes from profile declarations.
    output.push('\n');
    output.push_str(&render_model_routes(config));

    output
}

fn find_model_for_provider(config: &LocalConfig, provider_name: &str) -> String {
    // First check if any profile uses this provider.
    for profile in RoutingProfile::all() {
        let pc = profile.profile_config(config);
        if pc.provider == provider_name {
            return pc.model.clone();
        }
    }
    // Fallback to first allowed model.
    config
        .providers
        .iter()
        .find(|p| p.name == provider_name)
        .and_then(|p| p.allowed_models.first().cloned())
        .unwrap_or_default()
}

fn render_model_routes(config: &LocalConfig) -> String {
    // NOTE: do NOT emit `api_key` here. ZeroClaw resolves credentials from
    // [providers.models.<id>].api_key (rendered by render_providers_config).
    // Inlining keys per-route duplicates the secret N times on disk.
    let mut blocks = Vec::new();
    for profile in RoutingProfile::all() {
        let pc = profile.profile_config(config);
        if let Some(provider) = config.providers.iter().find(|p| p.name == pc.provider) {
            let provider_id = zeroclaw_provider_id(provider);
            blocks.push(format!(
                "[[providers.model_routes]]\nhint = \"{}\"\nprovider = \"{}\"\nmodel = \"{}\"\n",
                profile.as_str(),
                toml_escape(&provider_id),
                toml_escape(&pc.model),
            ));
        }
    }
    blocks.join("\n")
}

fn render_model_fallbacks(config: &LocalConfig) -> String {
    let mut rendered_keys = BTreeSet::new();
    let mut output = String::from("[reliability.model_fallbacks]\n");
    for profile in RoutingProfile::all() {
        let pc = profile.profile_config(config);
        if !rendered_keys.insert(pc.model.clone()) {
            continue;
        }
        // Fallbacks are other providers' models (up to max_fallbacks).
        let fallback_models: Vec<String> = config
            .providers
            .iter()
            .filter(|p| p.name != pc.provider)
            .filter_map(|p| p.allowed_models.first().cloned())
            .take(pc.max_fallbacks)
            .collect();
        if !fallback_models.is_empty() {
            let rendered = fallback_models
                .iter()
                .map(|m| format!("\"{}\"", toml_escape(m)))
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(&format!(
                "\"{}\" = [{}]\n",
                toml_escape(&pc.model),
                rendered
            ));
        }
    }
    output
}

fn render_query_classification() -> String {
    let rules = [
        (
            "structured_lookup",
            vec![
                "快递",
                "物流",
                "运单",
                "单号",
                "顺丰",
                "京东快递",
                "圆通",
                "订单",
                "淘宝",
                "天猫",
                "京东",
                "购买",
                "买的",
                "买过",
                "在哪里买",
                "哪买",
                "商品",
            ],
            40,
        ),
        (
            "research",
            vec![
                "为什么",
                "原因",
                "谁",
                "explain",
                "why",
                "how",
                "compare",
                "分析",
                "建议",
                "怎么优化",
            ],
            30,
        ),
        (
            "home_control",
            vec![
                "打开", "关闭", "开灯", "关灯", "亮度", "调亮", "调暗", "空调", "风扇", "窗帘",
                "插座", "开关", "灯带", "主灯",
            ],
            20,
        ),
    ];
    let mut output = String::from("[query_classification]\nenabled = true\n");
    for (hint, keywords, priority) in rules {
        let rendered_keywords = keywords
            .into_iter()
            .map(|item| format!("\"{}\"", toml_escape(item)))
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str("\n[[query_classification.rules]]\n");
        output.push_str(&format!("hint = \"{hint}\"\n"));
        output.push_str(&format!("keywords = [{rendered_keywords}]\n"));
        output.push_str(&format!("priority = {priority}\n"));
    }
    output
}

fn render_mcp_servers_config(paths: &RuntimePaths, config: &LocalConfig) -> String {
    let mempalace = &config.memory_integrations.mempalace;
    if !mempalace.enabled {
        return String::new();
    }

    let python = if mempalace.python.trim().is_empty() {
        paths.mempalace_python_path()
    } else {
        std::path::PathBuf::from(mempalace.python.trim())
    };
    let palace_dir = if mempalace.palace_dir.trim().is_empty() {
        paths.mempalace_palace_dir()
    } else {
        std::path::PathBuf::from(mempalace.palace_dir.trim())
    };

    format!(
        r#"
[[mcp.servers]]
name = "mempalace"
command = "{}"
args = ["-m", "mempalace.mcp_server", "--palace", "{}"]
tool_timeout_secs = {}
"#,
        toml_escape(&python.display().to_string()),
        toml_escape(&palace_dir.display().to_string()),
        mempalace.tool_timeout_secs
    )
}

// ── Utility ──────────────────────────────────────────────────────────

fn zeroclaw_provider_id(provider: &ModelProviderConfig) -> String {
    if provider.base_url.trim().is_empty() {
        provider.name.clone()
    } else {
        format!("custom:{}", provider.base_url.trim())
    }
}

fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn toml_key_segment(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        value.to_string()
    } else {
        format!("\"{}\"", toml_escape(value))
    }
}

fn provider_api_key_env_names(provider_name: &str) -> Vec<String> {
    let normalized = provider_name
        .trim()
        .to_ascii_uppercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();

    match provider_name.trim().to_ascii_lowercase().as_str() {
        "qwen" | "dashscope" => vec!["DASHSCOPE_API_KEY".to_string(), "QWEN_API_KEY".to_string()],
        "moonshot" | "kimi" => vec!["MOONSHOT_API_KEY".to_string(), "KIMI_API_KEY".to_string()],
        "glm" | "zhipu" => vec!["GLM_API_KEY".to_string(), "ZHIPU_API_KEY".to_string()],
        "doubao" | "ark" | "volcengine" => vec![
            "DOUBAO_API_KEY".to_string(),
            "ARK_API_KEY".to_string(),
            "VOLCENGINE_API_KEY".to_string(),
        ],
        _ if normalized.is_empty() => Vec::new(),
        _ => vec![format!("{normalized}_API_KEY")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> LocalConfig {
        toml::from_str(
            r#"
[home_assistant]
url = "http://127.0.0.1:8123/api/mcp"
token = "test-token"

[imessage]
allowed_contacts = ["+8613800138000"]

[[providers]]
name = "openrouter"
api_key = "test-key"
base_url = ""
allowed_models = ["openai/gpt-4o", "anthropic/claude-sonnet-4.6"]

[[providers]]
name = "deepseek"
api_key = "key-2"
base_url = "https://api.deepseek.com/v1"
allowed_models = ["deepseek-chat"]

[routing]

[routing.profiles.home_control]
provider = "openrouter"
model = "anthropic/claude-sonnet-4.6"
max_fallbacks = 1

[routing.profiles.general_qa]
provider = "openrouter"
model = "openai/gpt-4o"
max_fallbacks = 2

[routing.profiles.research]
provider = "openrouter"
model = "anthropic/claude-sonnet-4.6"
max_fallbacks = 1

[routing.profiles.structured_lookup]
provider = "openrouter"
model = "openai/gpt-4o"
max_fallbacks = 1
"#,
        )
        .unwrap()
    }

    #[test]
    fn zeroclaw_env_vars_export_provider_keys() {
        let config = sample_config();
        let exports = zeroclaw_env_vars(&config);
        assert!(exports.contains(&("OPENROUTER_API_KEY".to_string(), "test-key".to_string())));
        assert!(exports.contains(&("DEEPSEEK_API_KEY".to_string(), "key-2".to_string())));
    }

    #[test]
    fn render_providers_config_includes_routes_and_fallback() {
        let config = sample_config();
        let rendered = render_providers_config(&config, "general_qa");
        assert!(rendered.contains("[providers]\nfallback = \"openrouter\""));
        assert!(rendered.contains("[providers.models.openrouter]"));
        assert!(rendered.contains("[[providers.model_routes]]"));
        assert!(rendered.contains("hint = \"home_control\""));
        assert!(rendered.contains("hint = \"general_qa\""));
    }

    #[test]
    fn render_model_routes_respects_per_profile_model() {
        let config = sample_config();
        let rendered = render_model_routes(&config);
        // home_control uses sonnet, general_qa uses gpt-4o — they must differ.
        assert!(
            rendered.contains("hint = \"home_control\"\nprovider = \"openrouter\"\nmodel = \"anthropic/claude-sonnet-4.6\""),
            "home_control route should use claude-sonnet-4.6, got:\n{rendered}"
        );
        assert!(
            rendered.contains("hint = \"general_qa\"\nprovider = \"openrouter\"\nmodel = \"openai/gpt-4o\""),
            "general_qa route should use openai/gpt-4o, got:\n{rendered}"
        );
        assert!(
            rendered.contains("hint = \"research\"\nprovider = \"openrouter\"\nmodel = \"anthropic/claude-sonnet-4.6\""),
            "research route should use claude-sonnet-4.6, got:\n{rendered}"
        );
        assert!(
            rendered.contains("hint = \"structured_lookup\"\nprovider = \"openrouter\"\nmodel = \"openai/gpt-4o\""),
            "structured_lookup route should use openai/gpt-4o, got:\n{rendered}"
        );
    }

    #[test]
    fn render_model_routes_does_not_inline_api_key() {
        let rendered = render_model_routes(&sample_config());
        assert!(
            !rendered.contains("api_key"),
            "model_routes must not inline api_key; credentials belong to [providers.models.<id>]. Got:\n{rendered}"
        );
        assert!(
            !rendered.contains("test-key"),
            "model_routes must not leak provider secrets into per-route entries. Got:\n{rendered}"
        );
    }

    #[test]
    fn render_providers_config_uses_custom_provider_id_for_base_url() {
        let config = sample_config();
        let rendered = render_providers_config(&config, "general_qa");
        assert!(rendered.contains("[providers.models.\"custom:https://api.deepseek.com/v1\"]"));
    }

    #[test]
    fn render_model_fallbacks_generates_cross_provider_fallbacks() {
        let config = sample_config();
        let rendered = render_model_fallbacks(&config);
        assert!(rendered.contains("[reliability.model_fallbacks]"));
        assert!(rendered.contains("deepseek-chat"));
    }

    #[test]
    fn query_classification_includes_expected_hints() {
        let rendered = render_query_classification();
        assert!(rendered.contains("hint = \"structured_lookup\""));
        assert!(rendered.contains("hint = \"home_control\""));
        assert!(rendered.contains("hint = \"research\""));
        assert!(rendered.contains("\"淘宝\""));
        assert!(rendered.contains("\"打开\""));
    }
}
