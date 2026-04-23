use crate::{Crawl4aiConfig, Crawl4aiError, Crawl4aiSupervisor, RuntimePaths, USER_AGENT};
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::Value;

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
struct CrawlRequestBody<'a> {
    profile_path: String,
    url: &'a str,
    wait_for: Option<&'a str>,
    js_code: Option<&'a str>,
    timeout_secs: u64,
    headless: bool,
    magic: bool,
    simulate_user: bool,
    override_navigator: bool,
    remove_overlay_elements: bool,
    enable_stealth: bool,
}

#[tracing::instrument(
    name = "crawl4ai",
    skip(paths, config, supervisor),
    fields(profile = %request.profile_name, url = %request.url),
)]
pub async fn crawl4ai_crawl(
    paths: &RuntimePaths,
    config: &Crawl4aiConfig,
    supervisor: &Crawl4aiSupervisor,
    request: Crawl4aiPageRequest,
) -> Result<Crawl4aiPageResult, Crawl4aiError> {
    if !config.enabled {
        tracing::warn!("crawl4ai called while disabled in local config");
        return Err(Crawl4aiError::Disabled);
    }

    migrate_legacy_profiles(paths).map_err(|err| Crawl4aiError::LocalIo {
        details: format!("profile migration: {err}"),
    })?;
    let profile_dir = paths.crawl4ai_profiles_root().join(&request.profile_name);
    std::fs::create_dir_all(&profile_dir).map_err(|err| Crawl4aiError::LocalIo {
        details: format!("create profile dir {}: {err}", profile_dir.display()),
    })?;

    let body = CrawlRequestBody {
        profile_path: profile_dir.display().to_string(),
        url: &request.url,
        wait_for: request.wait_for.as_deref(),
        js_code: request.js_code.as_deref(),
        timeout_secs: config.timeout_secs,
        headless: config.headless,
        magic: config.magic,
        simulate_user: config.simulate_user,
        override_navigator: config.override_navigator,
        remove_overlay_elements: config.remove_overlay_elements,
        enable_stealth: config.enable_stealth,
    };

    let base = supervisor.base_url().await;
    let client = supervisor.http_client();
    let response = client
        .post(format!("{base}/crawl"))
        .header("user-agent", USER_AGENT)
        .json(&body)
        .send()
        .await
        .map_err(|err| {
            if err.is_timeout() {
                Crawl4aiError::Timeout {
                    budget_secs: config.timeout_secs.saturating_add(10),
                }
            } else {
                Crawl4aiError::ServerUnavailable {
                    details: err.to_string(),
                }
            }
        })?;

    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|err| Crawl4aiError::PayloadMalformed {
            details: format!("decode /crawl response: {err}"),
        })?;

    match status {
        StatusCode::OK => {
            let page = parse_result_value(payload);
            tracing::info!(
                success = page.success,
                status_code = ?page.status_code,
                final_url = ?page.current_url,
                "crawl4ai complete",
            );
            if page.success {
                Ok(page)
            } else {
                Err(Crawl4aiError::CrawlFailed {
                    details: page
                        .error_message
                        .clone()
                        .unwrap_or_else(|| "crawl4ai returned success=false".to_string()),
                })
            }
        }
        StatusCode::GATEWAY_TIMEOUT => Err(Crawl4aiError::Timeout {
            budget_secs: config.timeout_secs,
        }),
        StatusCode::SERVICE_UNAVAILABLE => Err(Crawl4aiError::ServerUnavailable {
            details: compact_json(&payload),
        }),
        StatusCode::INTERNAL_SERVER_ERROR => Err(Crawl4aiError::AdapterCrashed {
            details: compact_json(&payload),
        }),
        other => Err(Crawl4aiError::AdapterCrashed {
            details: format!("unexpected status {other}: {}", compact_json(&payload)),
        }),
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

fn parse_result_value(raw: Value) -> Crawl4aiPageResult {
    Crawl4aiPageResult {
        success: raw.get("success").and_then(Value::as_bool).unwrap_or(false),
        current_url: raw
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                raw.get("redirected_url")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        html: raw.get("html").and_then(Value::as_str).map(str::to_string),
        cleaned_html: raw
            .get("cleaned_html")
            .and_then(Value::as_str)
            .map(str::to_string),
        error_message: raw
            .get("error_message")
            .or_else(|| raw.get("error"))
            .and_then(Value::as_str)
            .map(str::to_string),
        status_code: raw
            .get("status_code")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok()),
        raw,
    }
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
