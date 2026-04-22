use crate::{Crawl4aiConfig, Crawl4aiTransport, RuntimePaths, USER_AGENT};
use reqwest::Client;
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

/// Extra wall-clock grace on top of config.timeout_secs, buying time for
/// Chromium launch + profile unlock before the inner page_timeout fires.
const CRAWL4AI_SUBPROCESS_GUARD_SECS: u64 = 30;

#[derive(Debug, Clone)]
pub struct Crawl4aiPageRequest {
    pub profile_name: String,
    pub url: String,
    pub wait_for: Option<String>,
    pub js_code: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Crawl4aiPageResult {
    pub success: bool,
    pub current_url: Option<String>,
    pub html: Option<String>,
    pub cleaned_html: Option<String>,
    pub error_message: Option<String>,
    pub status_code: Option<u16>,
    pub raw: Value,
}

#[derive(Serialize)]
struct ConfigEnvelope {
    #[serde(rename = "type")]
    kind: &'static str,
    params: Value,
}

#[tracing::instrument(
    name = "crawl4ai",
    skip(paths, config),
    fields(profile = %request.profile_name, url = %request.url, transport = ?config.transport),
)]
pub async fn crawl4ai_crawl(
    paths: &RuntimePaths,
    config: &Crawl4aiConfig,
    request: Crawl4aiPageRequest,
) -> Result<Crawl4aiPageResult, String> {
    if !config.enabled {
        tracing::warn!("crawl4ai called while disabled in local config");
        return Err("crawl4ai is disabled in local config".to_string());
    }

    migrate_legacy_profiles(paths)
        .map_err(|err| format!("crawl4ai profile migration failed: {err}"))?;
    let profile_dir = paths.crawl4ai_profiles_root().join(&request.profile_name);
    std::fs::create_dir_all(&profile_dir).map_err(|err| {
        format!(
            "failed to create crawl4ai profile directory {}: {err}",
            profile_dir.display()
        )
    })?;

    let result = match config.transport {
        Crawl4aiTransport::Server => crawl_via_server(paths, config, request).await,
        Crawl4aiTransport::Python => crawl_via_python(paths, config, request).await,
    };
    match &result {
        Ok(page) => tracing::info!(
            success = page.success,
            status_code = ?page.status_code,
            final_url = ?page.current_url,
            "crawl4ai complete",
        ),
        Err(err) => tracing::warn!(error = %err, "crawl4ai failed"),
    }
    result
}

async fn crawl_via_server(
    paths: &RuntimePaths,
    config: &Crawl4aiConfig,
    request: Crawl4aiPageRequest,
) -> Result<Crawl4aiPageResult, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(config.timeout_secs))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|err| format!("build crawl4ai http client: {err}"))?;

    let profile_dir = paths.crawl4ai_profiles_root().join(&request.profile_name);
    let browser_config = ConfigEnvelope {
        kind: "BrowserConfig",
        params: json!({
            "browser_type": "chromium",
            "headless": config.headless,
            "use_managed_browser": true,
            "use_persistent_context": true,
            "user_data_dir": profile_dir.display().to_string(),
            "enable_stealth": config.enable_stealth,
            "viewport_width": 1440,
            "viewport_height": 960,
            "verbose": false,
        }),
    };
    let mut crawler_params = json!({
        "stream": false,
        "cache_mode": "bypass",
        "page_timeout": config.timeout_secs.saturating_mul(1000),
        "delay_before_return_html": 1.0,
        "magic": config.magic,
        "simulate_user": config.simulate_user,
        "override_navigator": config.override_navigator,
        "remove_overlay_elements": config.remove_overlay_elements,
    });
    if let Some(wait_for) = request.wait_for {
        crawler_params["wait_for"] = Value::String(wait_for);
    }
    if let Some(js_code) = request.js_code {
        crawler_params["js_code"] = Value::String(js_code);
    }
    let payload = json!({
        "urls": [request.url],
        "browser_config": browser_config,
        "crawler_config": ConfigEnvelope {
            kind: "CrawlerRunConfig",
            params: crawler_params,
        },
    });

    let response = client
        .post(format!("{}/crawl", config.base_url))
        .json(&payload)
        .send()
        .await
        .map_err(|err| format!("crawl4ai request failed: {err}"))?;
    let status = response.status();
    let body = response
        .json::<Value>()
        .await
        .map_err(|err| format!("crawl4ai returned invalid json: {err}"))?;
    if !status.is_success() {
        return Err(format!(
            "crawl4ai request failed with status {}: {}",
            status.as_u16(),
            compact_json(&body)
        ));
    }

    let result = extract_first_result(&body)?;
    Ok(parse_result_value(result))
}

async fn crawl_via_python(
    paths: &RuntimePaths,
    config: &Crawl4aiConfig,
    request: Crawl4aiPageRequest,
) -> Result<Crawl4aiPageResult, String> {
    crawl_via_python_with_guard(paths, config, request, CRAWL4AI_SUBPROCESS_GUARD_SECS).await
}

/// Same as `crawl_via_python`, but the wall-clock guard (seconds added on top
/// of `config.timeout_secs`) is caller-provided. Production code goes through
/// the wrapper with `CRAWL4AI_SUBPROCESS_GUARD_SECS`; tests pass a small value
/// so the suite doesn't pay the full 30s on every run.
pub(crate) async fn crawl_via_python_with_guard(
    paths: &RuntimePaths,
    config: &Crawl4aiConfig,
    request: Crawl4aiPageRequest,
    guard_secs: u64,
) -> Result<Crawl4aiPageResult, String> {
    let python = resolve_python(paths, config);
    let profile_dir = paths.crawl4ai_profiles_root().join(&request.profile_name);
    let payload = json!({
        "profile_path": profile_dir.display().to_string(),
        "url": request.url,
        "wait_for": request.wait_for,
        "js_code": request.js_code,
        "timeout_secs": config.timeout_secs,
        "headless": config.headless,
        "magic": config.magic,
        "simulate_user": config.simulate_user,
        "override_navigator": config.override_navigator,
        "remove_overlay_elements": config.remove_overlay_elements,
        "enable_stealth": config.enable_stealth,
    });
    let raw = serde_json::to_vec(&payload)
        .map_err(|err| format!("serialize crawl4ai adapter payload: {err}"))?;

    let mut child = Command::new(&python)
        .arg("-m")
        .arg("crawl4ai_adapter")
        .arg("crawl")
        .arg("--runtime-dir")
        .arg(paths.runtime_dir.display().to_string())
        .current_dir(&paths.repo_root)
        .env("PYTHONPATH", paths.repo_root.display().to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("spawn crawl4ai_adapter crawl: {err}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&raw)
            .await
            .map_err(|err| format!("write crawl4ai adapter payload: {err}"))?;
        drop(stdin);
    }

    let budget = Duration::from_secs(config.timeout_secs.saturating_add(guard_secs));
    let output = match timeout(budget, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(format!("wait for crawl4ai_adapter crawl: {err}")),
        Err(_) => {
            return Err(format!(
                "crawl4ai adapter subprocess timed out after {}s",
                budget.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("crawl4ai adapter failed: {stderr}"));
    }
    let body = parse_adapter_json(&output.stdout)
        .map_err(|err| format!("parse crawl4ai adapter response: {err}"))?;
    if !body
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let message = body
            .get("error")
            .or_else(|| body.get("error_message"))
            .and_then(Value::as_str)
            .unwrap_or("crawl4ai adapter returned an error");
        let details = body.get("details").and_then(Value::as_str).unwrap_or("");
        return Err(if details.is_empty() {
            message.to_string()
        } else {
            format!("{message}: {details}")
        });
    }
    Ok(parse_result_value(body))
}

fn extract_first_result(body: &Value) -> Result<Value, String> {
    if let Some(items) = body.get("results").and_then(Value::as_array) {
        return items
            .first()
            .cloned()
            .ok_or_else(|| "crawl4ai returned an empty results array".to_string());
    }
    if let Some(items) = body.as_array() {
        return items
            .first()
            .cloned()
            .ok_or_else(|| "crawl4ai returned an empty results array".to_string());
    }
    if body.get("url").is_some() && body.get("success").is_some() {
        return Ok(body.clone());
    }
    Err(format!(
        "crawl4ai returned an unexpected payload: {}",
        compact_json(body)
    ))
}

fn parse_adapter_json(stdout: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(stdout);
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return Ok(value);
        }
    }
    Err(format!("no json payload found in adapter stdout: {text}"))
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

fn parse_result_value(result: Value) -> Crawl4aiPageResult {
    Crawl4aiPageResult {
        success: result
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        current_url: result
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                result
                    .get("redirected_url")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        html: result
            .get("html")
            .and_then(Value::as_str)
            .map(str::to_string),
        cleaned_html: result
            .get("cleaned_html")
            .and_then(Value::as_str)
            .map(str::to_string),
        error_message: result
            .get("error_message")
            .or_else(|| result.get("error"))
            .and_then(Value::as_str)
            .map(str::to_string),
        status_code: result
            .get("status_code")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok()),
        raw: result,
    }
}

fn resolve_python(paths: &RuntimePaths, config: &Crawl4aiConfig) -> String {
    if !config.python.is_empty() {
        return config.python.clone();
    }
    let runtime_python = paths
        .runtime_dir
        .join("crawl4ai-venv")
        .join("bin")
        .join("python");
    if runtime_python.is_file() {
        return runtime_python.display().to_string();
    }
    "python3".to_string()
}

fn migrate_legacy_profiles(paths: &RuntimePaths) -> std::io::Result<()> {
    let legacy = paths.crawl4ai_legacy_profiles_root();
    let current = paths.crawl4ai_profiles_root();
    if current.exists() || !legacy.exists() {
        return Ok(());
    }
    if let Some(parent) = current.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(legacy, current)
}
