use crate::app_config::{
    load_local_config, LocalConfig, ModelProviderConfig, QueryClassificationRule,
    RoutingProfileConfig,
};
use crate::runtime_paths::RuntimePaths;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Formatted, Item, Table, Value};

const BUILTIN_QUERY_CLASSIFICATION_TOML: &str =
    include_str!("../config/davis/query_classification.toml");

#[derive(Debug, Deserialize)]
struct BuiltinQueryClassification {
    #[serde(default)]
    rules: Vec<QueryClassificationRule>,
}

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
///
/// The template is valid TOML. We parse it with `toml_edit::DocumentMut` so
/// comments survive in the output, then patch specific paths. Sections that
/// vary in shape (provider models, model_routes, query_classification rules,
/// mcp.servers) are constructed as real TOML values and inserted by path —
/// no string templating, so typos surface at render time, not on ZeroClaw
/// startup.
#[tracing::instrument(
    name = "render_runtime_config",
    skip(paths, config),
    fields(
        providers = config.providers.len(),
        mcp_servers = config.mcp.servers.len(),
        classification_overrides = config.query_classification.rules.len(),
        runtime_path = tracing::field::Empty,
    ),
)]
pub fn render_runtime_config(paths: &RuntimePaths, config: &LocalConfig) -> Result<()> {
    let template = std::fs::read_to_string(paths.config_template_path())
        .context("failed to read ZeroClaw config template")?;
    let rendered = render_runtime_config_str(&template, &paths.repo_root, config)?;

    std::fs::create_dir_all(&paths.runtime_dir)?;
    let runtime_path = paths.runtime_config_path();
    tracing::Span::current().record("runtime_path", runtime_path.display().to_string());
    std::fs::write(&runtime_path, rendered)?;
    restrict_secret_file_permissions(&runtime_path);
    tracing::info!("ZeroClaw runtime config rendered");
    Ok(())
}

/// Pure-string version of `render_runtime_config` for testing. No disk I/O.
fn render_runtime_config_str(
    template: &str,
    repo_root: &std::path::Path,
    config: &LocalConfig,
) -> Result<String> {
    let mut doc: DocumentMut = template
        .parse()
        .context("ZeroClaw config template is not valid TOML")?;

    patch_allowed_roots(&mut doc, repo_root)?;
    patch_webhook_secret(&mut doc, config);
    patch_imessage(&mut doc, config);
    patch_providers(&mut doc, config)?;
    patch_query_classification(&mut doc, config);
    patch_model_fallbacks(&mut doc, config);
    patch_mcp_servers(&mut doc, config);

    let rendered = doc.to_string();
    validate_rendered(&rendered)?;
    Ok(rendered)
}

/// Final safety net. The template is parsed TOML and every patch writes
/// through toml_edit, so the output is syntactically guaranteed. This check
/// catches a different class of bug: if anyone reintroduces string-level
/// templating with a `__DAVIS_...__` sentinel and forgets to substitute it,
/// fail loudly here instead of letting ZeroClaw choke at startup.
fn validate_rendered(rendered: &str) -> Result<()> {
    if let Some(line) = rendered.lines().find(|line| line.contains("__DAVIS_")) {
        return Err(anyhow!(
            "rendered runtime config still contains a __DAVIS_ sentinel: {line:?}"
        ));
    }
    toml::from_str::<toml::Value>(rendered).context("rendered runtime config is not valid TOML")?;
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

// ── Render helpers (toml_edit-based) ─────────────────────────────────

fn patch_allowed_roots(doc: &mut DocumentMut, repo_root: &std::path::Path) -> Result<()> {
    let repo_root_str = repo_root.display().to_string();
    let allowed_roots = doc
        .get_mut("autonomy")
        .and_then(Item::as_table_mut)
        .and_then(|t| t.get_mut("allowed_roots"))
        .and_then(Item::as_array_mut)
        .ok_or_else(|| anyhow!("template is missing [autonomy].allowed_roots array"))?;
    // The template ships with a single __DAVIS_REPO_ROOT__ entry that we
    // rewrite in-place, so any handcrafted trailing entries survive.
    if allowed_roots.is_empty() {
        allowed_roots.push(repo_root_str);
    } else {
        allowed_roots.replace(0, repo_root_str);
    }
    Ok(())
}

fn patch_webhook_secret(doc: &mut DocumentMut, config: &LocalConfig) {
    let secret = config.webhook.secret.trim();
    if secret.is_empty() {
        return;
    }
    if let Some(webhook) = doc
        .get_mut("channels")
        .and_then(Item::as_table_mut)
        .and_then(|t| t.get_mut("webhook"))
        .and_then(Item::as_table_mut)
    {
        webhook["secret"] = Item::Value(string_value(secret));
    }
}

fn patch_imessage(doc: &mut DocumentMut, config: &LocalConfig) {
    let channels = match doc.get_mut("channels").and_then(Item::as_table_mut) {
        Some(t) => t,
        None => return,
    };
    let mut imessage = Table::new();
    imessage.set_implicit(false);
    imessage["enabled"] = Item::Value(Value::Boolean(Formatted::new(true)));
    let mut contacts = Array::new();
    for contact in &config.imessage.allowed_contacts {
        contacts.push(contact.clone());
    }
    imessage["allowed_contacts"] = Item::Value(Value::Array(contacts));
    channels["imessage"] = Item::Table(imessage);
}

fn patch_providers(doc: &mut DocumentMut, config: &LocalConfig) -> Result<()> {
    let default_profile = config
        .routing
        .default_profile
        .as_deref()
        .unwrap_or("general_qa");
    let default_provider_name = RoutingProfile::all()
        .iter()
        .find(|p| p.as_str() == default_profile)
        .map(|p| p.profile_config(config).provider.clone())
        .or_else(|| config.providers.first().map(|p| p.name.clone()))
        .unwrap_or_default();
    let fallback_id = config
        .providers
        .iter()
        .find(|p| p.name == default_provider_name)
        .map(zeroclaw_provider_id);

    let mut providers = Table::new();
    providers.set_implicit(false);
    if let Some(id) = fallback_id {
        providers["fallback"] = Item::Value(string_value(&id));
    }

    // [providers.models.<id>]
    let mut models = Table::new();
    models.set_implicit(true);
    for provider in &config.providers {
        let provider_id = zeroclaw_provider_id(provider);
        let mut entry = Table::new();
        entry.set_implicit(false);
        if provider.base_url.trim().is_empty() {
            entry["name"] = Item::Value(string_value(&provider.name));
        }
        if !provider.api_key.trim().is_empty() {
            entry["api_key"] = Item::Value(string_value(&provider.api_key));
        }
        entry["model"] = Item::Value(string_value(&find_model_for_provider(
            config,
            &provider.name,
        )));
        models[provider_id.as_str()] = Item::Table(entry);
    }
    providers["models"] = Item::Table(models);

    // [[providers.model_routes]]
    let mut routes = ArrayOfTables::new();
    for profile in RoutingProfile::all() {
        let pc = profile.profile_config(config);
        if let Some(provider) = config.providers.iter().find(|p| p.name == pc.provider) {
            let provider_id = zeroclaw_provider_id(provider);
            let mut route = Table::new();
            route["hint"] = Item::Value(string_value(profile.as_str()));
            route["provider"] = Item::Value(string_value(&provider_id));
            route["model"] = Item::Value(string_value(&pc.model));
            routes.push(route);
        }
    }
    providers["model_routes"] = Item::ArrayOfTables(routes);

    doc["providers"] = Item::Table(providers);
    Ok(())
}

fn patch_query_classification(doc: &mut DocumentMut, config: &LocalConfig) {
    let merged = merge_classification_rules(&config.query_classification.rules);

    let mut section = Table::new();
    section.set_implicit(false);
    section["enabled"] = Item::Value(Value::Boolean(Formatted::new(true)));

    let mut rules = ArrayOfTables::new();
    for rule in merged {
        let mut table = Table::new();
        table["hint"] = Item::Value(string_value(&rule.hint));
        let mut keywords = Array::new();
        for kw in &rule.keywords {
            keywords.push(kw.clone());
        }
        table["keywords"] = Item::Value(Value::Array(keywords));
        table["priority"] = Item::Value(Value::Integer(Formatted::new(rule.priority as i64)));
        rules.push(table);
    }
    section["rules"] = Item::ArrayOfTables(rules);

    doc["query_classification"] = Item::Table(section);
}

fn patch_model_fallbacks(doc: &mut DocumentMut, config: &LocalConfig) {
    let mut fallbacks = Table::new();
    fallbacks.set_implicit(false);
    let mut emitted = BTreeSet::new();
    for profile in RoutingProfile::all() {
        let pc = profile.profile_config(config);
        if !emitted.insert(pc.model.clone()) {
            continue;
        }
        let models: Vec<String> = config
            .providers
            .iter()
            .filter(|p| p.name != pc.provider)
            .filter_map(|p| p.allowed_models.first().cloned())
            .take(pc.max_fallbacks)
            .collect();
        if models.is_empty() {
            continue;
        }
        let mut arr = Array::new();
        for model in models {
            arr.push(model);
        }
        fallbacks[pc.model.as_str()] = Item::Value(Value::Array(arr));
    }

    // The template ships with an empty [reliability] table so we can
    // insert model_fallbacks underneath without wiping sibling keys.
    if let Some(reliability) = doc.get_mut("reliability").and_then(Item::as_table_mut) {
        reliability["model_fallbacks"] = Item::Table(fallbacks);
    } else {
        let mut reliability = Table::new();
        reliability.set_implicit(false);
        reliability["model_fallbacks"] = Item::Table(fallbacks);
        doc["reliability"] = Item::Table(reliability);
    }
}

fn patch_mcp_servers(doc: &mut DocumentMut, config: &LocalConfig) {
    if config.mcp.servers.is_empty() {
        return;
    }
    let mcp = match doc.get_mut("mcp").and_then(Item::as_table_mut) {
        Some(t) => t,
        None => {
            let mut t = Table::new();
            t.set_implicit(false);
            doc["mcp"] = Item::Table(t);
            doc.get_mut("mcp").unwrap().as_table_mut().unwrap()
        }
    };
    let servers = match mcp.get_mut("servers") {
        Some(Item::ArrayOfTables(a)) => a,
        _ => {
            mcp["servers"] = Item::ArrayOfTables(ArrayOfTables::new());
            mcp.get_mut("servers")
                .unwrap()
                .as_array_of_tables_mut()
                .unwrap()
        }
    };
    for server in &config.mcp.servers {
        let mut table = Table::new();
        table["name"] = Item::Value(string_value(&server.name));
        table["transport"] = Item::Value(string_value(mcp_transport_str(server.transport)));
        match server.transport {
            crate::app_config::McpTransport::Stdio => {
                table["command"] = Item::Value(string_value(&server.command));
                if !server.args.is_empty() {
                    let mut args = Array::new();
                    for arg in &server.args {
                        args.push(arg.clone());
                    }
                    table["args"] = Item::Value(Value::Array(args));
                }
                if !server.env.is_empty() {
                    let mut env = Table::new();
                    for (k, v) in &server.env {
                        env[k.as_str()] = Item::Value(string_value(v));
                    }
                    table["env"] = Item::Table(env);
                }
            }
            crate::app_config::McpTransport::Sse | crate::app_config::McpTransport::Http => {
                table["url"] = Item::Value(string_value(&server.url));
                if !server.headers.is_empty() {
                    let mut headers = Table::new();
                    for (k, v) in &server.headers {
                        headers[k.as_str()] = Item::Value(string_value(v));
                    }
                    table["headers"] = Item::Table(headers);
                }
            }
        }
        table["tool_timeout_secs"] = Item::Value(Value::Integer(Formatted::new(
            server.tool_timeout_secs as i64,
        )));
        servers.push(table);
    }
}

fn string_value(s: &str) -> Value {
    Value::String(Formatted::new(s.to_string()))
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

/// Load the built-in default rules embedded at compile time. Panic on
/// malformed TOML is intentional: a corrupted default file is a build-time
/// bug, not a runtime condition. Verified by
/// `builtin_query_classification_defaults_parse`.
fn load_builtin_classification_rules() -> Vec<QueryClassificationRule> {
    toml::from_str::<BuiltinQueryClassification>(BUILTIN_QUERY_CLASSIFICATION_TOML)
        .expect("config/davis/query_classification.toml must be valid TOML")
        .rules
}

/// Merge user overrides on top of built-in defaults and sort by priority
/// (descending). User rules appear before built-in rules of equal priority
/// so ZeroClaw's classifier sees them first.
fn merge_classification_rules(
    user_rules: &[QueryClassificationRule],
) -> Vec<QueryClassificationRule> {
    let builtin = load_builtin_classification_rules();
    // User first so that after a stable sort_by on priority, user rules
    // keep the lead on ties.
    let mut merged: Vec<QueryClassificationRule> =
        user_rules.iter().cloned().chain(builtin).collect();
    merged.sort_by_key(|rule| std::cmp::Reverse(rule.priority));
    merged
}

fn mcp_transport_str(transport: crate::app_config::McpTransport) -> &'static str {
    match transport {
        crate::app_config::McpTransport::Stdio => "stdio",
        crate::app_config::McpTransport::Sse => "sse",
        crate::app_config::McpTransport::Http => "http",
    }
}

// ── Utility ──────────────────────────────────────────────────────────

fn zeroclaw_provider_id(provider: &ModelProviderConfig) -> String {
    if provider.base_url.trim().is_empty() {
        provider.name.clone()
    } else {
        format!("custom:{}", provider.base_url.trim())
    }
}

/// Known provider aliases and the env-var names ZeroClaw's provider layer
/// reads for each. When none of the aliases match, we fall back to
/// `UPPER_SNAKE(provider_name)_API_KEY` (see `provider_api_key_env_names`).
///
/// Every row maps one or more user-facing names (the `name` field in
/// local.toml's `[[providers]]`) to the env-var names those providers'
/// Rust crates probe in priority order.
const PROVIDER_ENV_ALIASES: &[(&[&str], &[&str])] = &[
    (
        &["qwen", "dashscope"],
        &["DASHSCOPE_API_KEY", "QWEN_API_KEY"],
    ),
    (&["moonshot", "kimi"], &["MOONSHOT_API_KEY", "KIMI_API_KEY"]),
    (&["glm", "zhipu"], &["GLM_API_KEY", "ZHIPU_API_KEY"]),
    (
        &["doubao", "ark", "volcengine"],
        &["DOUBAO_API_KEY", "ARK_API_KEY", "VOLCENGINE_API_KEY"],
    ),
];

fn provider_api_key_env_names(provider_name: &str) -> Vec<String> {
    let lowered = provider_name.trim().to_ascii_lowercase();
    if let Some((_, envs)) = PROVIDER_ENV_ALIASES
        .iter()
        .find(|(aliases, _)| aliases.contains(&lowered.as_str()))
    {
        return envs.iter().map(|s| s.to_string()).collect();
    }

    let normalized = provider_name
        .trim()
        .to_ascii_uppercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    if normalized.is_empty() {
        Vec::new()
    } else {
        vec![format!("{normalized}_API_KEY")]
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

    /// Template stripped to just the sections the renderer reads/patches.
    /// Must stay in sync with the behavior-affecting bits of
    /// config/davis/config.toml; shape only, no comments needed.
    const TEST_TEMPLATE: &str = r#"
schema_version = 2

[channels.webhook]
enabled = true

[autonomy]
level = "full"
allowed_roots = ["__DAVIS_REPO_ROOT__"]

[reliability]

[mcp]
enabled = true
"#;

    fn render_with_test_template(config: &LocalConfig) -> String {
        render_runtime_config_str(
            TEST_TEMPLATE,
            std::path::Path::new("/tmp/davis-test"),
            config,
        )
        .expect("render must succeed on sample config")
    }

    #[test]
    fn render_runtime_config_str_is_valid_toml() {
        let rendered = render_with_test_template(&sample_config());
        let parsed: toml::Value =
            toml::from_str(&rendered).expect("rendered output must be valid TOML");
        assert!(parsed.get("providers").is_some());
        assert!(parsed.get("query_classification").is_some());
    }

    #[test]
    fn render_runtime_config_patches_providers_and_routes() {
        let rendered = render_with_test_template(&sample_config());
        assert!(rendered.contains("fallback = \"openrouter\""), "{rendered}");
        // deepseek has a base_url → custom:... provider id
        assert!(
            rendered.contains("\"custom:https://api.deepseek.com/v1\""),
            "{rendered}"
        );
        assert!(rendered.contains("[[providers.model_routes]]"));
    }

    #[test]
    fn render_runtime_config_respects_per_profile_model() {
        let rendered = render_with_test_template(&sample_config());
        let parsed: toml::Value = toml::from_str(&rendered).unwrap();
        let routes = parsed["providers"]["model_routes"].as_array().unwrap();
        let find = |hint: &str| -> String {
            routes
                .iter()
                .find(|r| r.get("hint").and_then(|v| v.as_str()) == Some(hint))
                .and_then(|r| r.get("model"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_default()
        };
        assert_eq!(find("home_control"), "anthropic/claude-sonnet-4.6");
        assert_eq!(find("general_qa"), "openai/gpt-4o");
        assert_eq!(find("research"), "anthropic/claude-sonnet-4.6");
        assert_eq!(find("structured_lookup"), "openai/gpt-4o");
    }

    #[test]
    fn render_runtime_config_does_not_inline_api_key_in_routes() {
        let rendered = render_with_test_template(&sample_config());
        let parsed: toml::Value = toml::from_str(&rendered).unwrap();
        let routes = parsed["providers"]["model_routes"].as_array().unwrap();
        for route in routes {
            assert!(
                route.get("api_key").is_none(),
                "model_routes must not contain api_key: {route:?}"
            );
        }
        // Secret should still be present in [providers.models.<id>]
        assert!(
            rendered.contains("test-key") || rendered.contains("key-2"),
            "provider api_key must still appear under [providers.models.<id>]"
        );
    }

    #[test]
    fn render_runtime_config_emits_model_fallbacks() {
        let rendered = render_with_test_template(&sample_config());
        let parsed: toml::Value = toml::from_str(&rendered).unwrap();
        let fallbacks = parsed["reliability"]["model_fallbacks"]
            .as_table()
            .expect("model_fallbacks must be a table");
        // The default profile's model should have a fallback list.
        let any_has_deepseek = fallbacks.values().any(|v| {
            v.as_array()
                .map(|a| a.iter().any(|x| x.as_str() == Some("deepseek-chat")))
                .unwrap_or(false)
        });
        assert!(
            any_has_deepseek,
            "cross-provider fallback missing: {fallbacks:?}"
        );
    }

    #[test]
    fn render_runtime_config_patches_allowed_roots() {
        let rendered = render_runtime_config_str(
            TEST_TEMPLATE,
            std::path::Path::new("/opt/davis"),
            &sample_config(),
        )
        .unwrap();
        let parsed: toml::Value = toml::from_str(&rendered).unwrap();
        let roots = parsed["autonomy"]["allowed_roots"].as_array().unwrap();
        assert_eq!(roots[0].as_str(), Some("/opt/davis"));
    }

    #[test]
    fn render_runtime_config_rejects_leftover_sentinels() {
        // Include the required [autonomy]/[reliability]/[mcp] scaffolding so
        // the patch pass succeeds, but plant a stray sentinel somewhere the
        // renderer doesn't touch. validate_rendered must catch it.
        let bad_template = r#"
[autonomy]
allowed_roots = ["__DAVIS_REPO_ROOT__"]

[reliability]

[mcp]
enabled = true

[extra]
legacy_field = "__DAVIS_UNPATCHED__"
"#;
        let err = render_runtime_config_str(
            bad_template,
            std::path::Path::new("/tmp/x"),
            &sample_config(),
        )
        .expect_err("unmatched sentinel must fail the render");
        let msg = err.to_string();
        assert!(
            msg.contains("__DAVIS_"),
            "error should mention the leftover sentinel: {msg}"
        );
    }

    #[test]
    fn render_runtime_config_includes_builtin_classification_defaults() {
        let rendered = render_with_test_template(&sample_config());
        assert!(rendered.contains("structured_lookup"));
        assert!(rendered.contains("淘宝"));
        assert!(rendered.contains("打开"));
    }

    #[test]
    fn builtin_query_classification_defaults_parse() {
        let rules = load_builtin_classification_rules();
        let hints: Vec<&str> = rules.iter().map(|r| r.hint.as_str()).collect();
        assert!(hints.contains(&"structured_lookup"));
        assert!(hints.contains(&"research"));
        assert!(hints.contains(&"home_control"));
    }

    #[test]
    fn user_override_rules_merge_and_sort_by_priority_desc() {
        let user = vec![
            QueryClassificationRule {
                hint: "custom_top".to_string(),
                keywords: vec!["特殊场景".to_string()],
                priority: 100,
            },
            QueryClassificationRule {
                hint: "home_control".to_string(),
                keywords: vec!["开一下".to_string(), "关一下".to_string()],
                priority: 25,
            },
        ];
        let merged = merge_classification_rules(&user);

        // Descending by priority, user rules first on ties.
        let priorities: Vec<i32> = merged.iter().map(|r| r.priority).collect();
        let mut expected = priorities.clone();
        expected.sort_by(|a, b| b.cmp(a));
        assert_eq!(
            priorities, expected,
            "rules must be sorted descending by priority"
        );

        // User's custom_top outranks everything.
        assert_eq!(merged.first().unwrap().hint, "custom_top");

        // Built-in structured_lookup (priority=40) survives.
        assert!(merged
            .iter()
            .any(|r| r.hint == "structured_lookup" && r.priority == 40));

        // User override for home_control (priority=25) appears BEFORE the
        // built-in home_control (priority=20).
        let user_home_idx = merged
            .iter()
            .position(|r| r.hint == "home_control" && r.priority == 25)
            .expect("user home_control rule missing");
        let builtin_home_idx = merged
            .iter()
            .position(|r| r.hint == "home_control" && r.priority == 20)
            .expect("builtin home_control rule missing");
        assert!(
            user_home_idx < builtin_home_idx,
            "user override must precede built-in rule of same hint"
        );
    }

    #[test]
    fn user_override_wins_on_priority_tie() {
        let user = vec![QueryClassificationRule {
            hint: "research".to_string(),
            keywords: vec!["为啥".to_string()],
            priority: 30, // same as built-in research
        }];
        let merged = merge_classification_rules(&user);
        let first_research = merged
            .iter()
            .find(|r| r.hint == "research")
            .expect("research rule missing");
        assert!(
            first_research.keywords.contains(&"为啥".to_string()),
            "on priority tie, user rule must come first"
        );
    }

    #[test]
    fn render_runtime_config_emits_no_mcp_servers_when_none_configured() {
        let rendered = render_with_test_template(&sample_config());
        let parsed: toml::Value = toml::from_str(&rendered).unwrap();
        let servers = parsed["mcp"]
            .get("servers")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        assert_eq!(servers, 0, "no servers configured must yield empty array");
    }

    #[test]
    fn render_runtime_config_emits_stdio_mcp_entry() {
        let mut config = sample_config();
        config.mcp.servers.push(crate::app_config::McpServerConfig {
            name: "mempalace".to_string(),
            transport: crate::app_config::McpTransport::Stdio,
            command: "/p/bin/python".to_string(),
            args: vec![
                "-m".to_string(),
                "mempalace.mcp_server".to_string(),
                "--palace".to_string(),
                "/p/palace".to_string(),
            ],
            env: Default::default(),
            url: String::new(),
            headers: Default::default(),
            tool_timeout_secs: 45,
        });
        let rendered = render_with_test_template(&config);
        let parsed: toml::Value = toml::from_str(&rendered).unwrap();
        let server = &parsed["mcp"]["servers"].as_array().unwrap()[0];
        assert_eq!(server["name"].as_str(), Some("mempalace"));
        assert_eq!(server["transport"].as_str(), Some("stdio"));
        assert_eq!(server["command"].as_str(), Some("/p/bin/python"));
        assert_eq!(server["tool_timeout_secs"].as_integer(), Some(45));
        assert_eq!(server["args"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn render_runtime_config_emits_http_mcp_entry_with_headers() {
        let mut config = sample_config();
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("X-Auth".to_string(), "placeholder-value".to_string());
        config.mcp.servers.push(crate::app_config::McpServerConfig {
            name: "remote".to_string(),
            transport: crate::app_config::McpTransport::Http,
            command: String::new(),
            args: Vec::new(),
            env: Default::default(),
            url: "https://example.com/mcp".to_string(),
            headers,
            tool_timeout_secs: 30,
        });
        let rendered = render_with_test_template(&config);
        let parsed: toml::Value = toml::from_str(&rendered).unwrap();
        let server = &parsed["mcp"]["servers"].as_array().unwrap()[0];
        assert_eq!(server["transport"].as_str(), Some("http"));
        assert_eq!(server["url"].as_str(), Some("https://example.com/mcp"));
        assert_eq!(
            server["headers"]["X-Auth"].as_str(),
            Some("placeholder-value")
        );
        assert!(
            server.get("command").is_none(),
            "http entry must not emit command: {server:?}"
        );
    }

    #[test]
    fn render_runtime_config_preserves_mcp_server_order() {
        let mut config = sample_config();
        for name in ["first", "second"] {
            config.mcp.servers.push(crate::app_config::McpServerConfig {
                name: name.to_string(),
                transport: crate::app_config::McpTransport::Stdio,
                command: "/x".to_string(),
                args: Vec::new(),
                env: Default::default(),
                url: String::new(),
                headers: Default::default(),
                tool_timeout_secs: 30,
            });
        }
        let rendered = render_with_test_template(&config);
        let parsed: toml::Value = toml::from_str(&rendered).unwrap();
        let servers = parsed["mcp"]["servers"].as_array().unwrap();
        assert_eq!(servers[0]["name"].as_str(), Some("first"));
        assert_eq!(servers[1]["name"].as_str(), Some("second"));
    }

    #[test]
    fn render_runtime_config_surfaces_user_classification_override() {
        let mut config = sample_config();
        config
            .query_classification
            .rules
            .push(QueryClassificationRule {
                hint: "home_control".to_string(),
                keywords: vec!["开一下".to_string()],
                priority: 25,
            });
        let rendered = render_with_test_template(&config);
        assert!(
            rendered.contains("开一下"),
            "override keyword missing:\n{rendered}"
        );
    }

    #[test]
    fn render_runtime_config_preserves_template_comments() {
        let template = "# top comment\n[autonomy]\nallowed_roots = [\"__DAVIS_REPO_ROOT__\"]\n[reliability]\n[mcp]\nenabled = true\n";
        let rendered = render_runtime_config_str(
            template,
            std::path::Path::new("/tmp/davis-test"),
            &sample_config(),
        )
        .unwrap();
        assert!(
            rendered.contains("# top comment"),
            "toml_edit must preserve comments:\n{rendered}"
        );
    }
}
