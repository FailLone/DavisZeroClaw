use crate::{
    build_issue, isoformat, now_utc, BrowserActionPreview, BrowserActionRequest,
    BrowserActionResponse, BrowserBridgeConfig, BrowserEvaluateRequest, BrowserFocusRequest,
    BrowserOpenRequest, BrowserProfileState, BrowserProfilesResponse, BrowserScreenshotRequest,
    BrowserSnapshotRequest, BrowserStatusResponse, BrowserTabsResponse, BrowserWaitRequest, Issue,
    RuntimePaths, USER_AGENT,
};
use reqwest::Client;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use url::Url;

const ACTIONS_THAT_WRITE: &[&str] = &["click", "type", "fill", "press", "select", "upload"];
const EVALUATE_WRITE_MODE: &str = "write";

pub async fn browser_status(
    paths: RuntimePaths,
    config: BrowserBridgeConfig,
) -> BrowserStatusResponse {
    if !config.enabled {
        return disabled_status();
    }
    let response = fetch_worker_json(worker_url(&config, "/status")).await;
    let result = match response {
        Ok(value) => serde_json::from_value::<BrowserStatusResponse>(value).unwrap_or_else(|_| {
            fallback_status(&config, "browser worker returned invalid status payload")
        }),
        Err(message) => fallback_status(&config, &message),
    };
    persist_status(&paths, &result);
    result
}

pub async fn browser_profiles(config: BrowserBridgeConfig) -> BrowserProfilesResponse {
    if !config.enabled {
        return BrowserProfilesResponse {
            status: "upstream_error".to_string(),
            checked_at: isoformat(now_utc()),
            default_profile: config.default_profile.clone(),
            profiles: fallback_profile_states(
                &config,
                false,
                Some("browser bridge disabled".to_string()),
            ),
        };
    }
    match fetch_worker_json(worker_url(&config, "/profiles")).await {
        Ok(value) => {
            serde_json::from_value::<BrowserProfilesResponse>(value).unwrap_or_else(|_| {
                BrowserProfilesResponse {
                    status: "upstream_error".to_string(),
                    checked_at: isoformat(now_utc()),
                    default_profile: config.default_profile.clone(),
                    profiles: fallback_profile_states(
                        &config,
                        false,
                        Some("browser worker returned invalid profiles payload".to_string()),
                    ),
                }
            })
        }
        Err(message) => BrowserProfilesResponse {
            status: "upstream_error".to_string(),
            checked_at: isoformat(now_utc()),
            default_profile: config.default_profile.clone(),
            profiles: fallback_profile_states(&config, false, Some(message)),
        },
    }
}

pub async fn browser_tabs(
    config: BrowserBridgeConfig,
    profile: Option<String>,
) -> BrowserTabsResponse {
    let resolved_profile = resolve_profile_name(&config, profile.as_deref());
    if !config.enabled {
        return BrowserTabsResponse {
            status: "upstream_error".to_string(),
            checked_at: isoformat(now_utc()),
            profile: resolved_profile,
            tabs: Vec::new(),
            message: Some("browser bridge disabled".to_string()),
            issue: Some(build_issue(
                "browser_bridge_unavailable",
                "browser",
                Vec::new(),
            )),
        };
    }
    match fetch_worker_json(worker_url(
        &config,
        &format!("/tabs?profile={}", urlencoding::encode(&resolved_profile)),
    ))
    .await
    {
        Ok(value) => serde_json::from_value::<BrowserTabsResponse>(value).unwrap_or_else(|_| {
            BrowserTabsResponse {
                status: "upstream_error".to_string(),
                checked_at: isoformat(now_utc()),
                profile: resolved_profile,
                tabs: Vec::new(),
                message: Some("browser worker returned invalid tabs payload".to_string()),
                issue: Some(build_issue(
                    "browser_bridge_unavailable",
                    "browser",
                    Vec::new(),
                )),
            }
        }),
        Err(message) => BrowserTabsResponse {
            status: "upstream_error".to_string(),
            checked_at: isoformat(now_utc()),
            profile: resolved_profile,
            tabs: Vec::new(),
            message: Some(message),
            issue: Some(build_issue(
                "browser_bridge_unavailable",
                "browser",
                Vec::new(),
            )),
        },
    }
}

pub async fn browser_open(
    config: BrowserBridgeConfig,
    mut request: BrowserOpenRequest,
) -> BrowserActionResponse {
    request.profile = Some(resolve_profile_name(&config, request.profile.as_deref()));
    post_worker_json(worker_url(&config, "/open"), &request, &config).await
}

pub async fn browser_focus(
    config: BrowserBridgeConfig,
    mut request: BrowserFocusRequest,
) -> BrowserActionResponse {
    request.profile = Some(resolve_profile_name(&config, request.profile.as_deref()));
    post_worker_json(worker_url(&config, "/focus"), &request, &config).await
}

pub async fn browser_snapshot(
    config: BrowserBridgeConfig,
    mut request: BrowserSnapshotRequest,
) -> BrowserActionResponse {
    request.profile = Some(resolve_profile_name(&config, request.profile.as_deref()));
    post_worker_json(worker_url(&config, "/snapshot"), &request, &config).await
}

pub async fn browser_evaluate(
    config: BrowserBridgeConfig,
    mut request: BrowserEvaluateRequest,
) -> BrowserActionResponse {
    let profile = resolve_profile_name(&config, request.profile.as_deref());
    request.profile = Some(profile.clone());
    let is_write = request.mode.as_deref() == Some(EVALUATE_WRITE_MODE);
    if is_write {
        return enforce_write_policy_for_evaluate(&config, &request, &profile).await;
    }
    post_worker_json(worker_url(&config, "/evaluate"), &request, &config).await
}

pub async fn browser_action(
    paths: RuntimePaths,
    config: BrowserBridgeConfig,
    mut request: BrowserActionRequest,
) -> BrowserActionResponse {
    let profile = resolve_profile_name(&config, request.profile.as_deref());
    request.profile = Some(profile.clone());
    let response = if ACTIONS_THAT_WRITE.contains(&request.action.as_str()) {
        enforce_write_policy_for_action(&paths, &config, &request, &profile).await
    } else {
        post_worker_json(worker_url(&config, "/action"), &request, &config).await
    };
    log_action(&paths, &request, &response);
    if response.status == "requires_confirmation" {
        log_confirmation(&paths, &request, &response);
    }
    response
}

pub async fn browser_screenshot(
    config: BrowserBridgeConfig,
    mut request: BrowserScreenshotRequest,
) -> BrowserActionResponse {
    request.profile = Some(resolve_profile_name(&config, request.profile.as_deref()));
    post_worker_json(worker_url(&config, "/screenshot"), &request, &config).await
}

pub async fn browser_wait(
    config: BrowserBridgeConfig,
    mut request: BrowserWaitRequest,
) -> BrowserActionResponse {
    request.profile = Some(resolve_profile_name(&config, request.profile.as_deref()));
    post_worker_json(worker_url(&config, "/wait"), &request, &config).await
}

pub(crate) async fn browser_evaluate_internal(
    config: &BrowserBridgeConfig,
    request: BrowserEvaluateRequest,
) -> BrowserActionResponse {
    browser_evaluate(config.clone(), request).await
}

pub(crate) async fn browser_tabs_internal(
    config: &BrowserBridgeConfig,
    profile: Option<String>,
) -> BrowserTabsResponse {
    browser_tabs(config.clone(), profile).await
}

fn disabled_status() -> BrowserStatusResponse {
    BrowserStatusResponse {
        status: "upstream_error".to_string(),
        checked_at: isoformat(now_utc()),
        worker_available: false,
        worker_url: None,
        profiles: Vec::new(),
        message: Some("browser bridge disabled".to_string()),
    }
}

fn fallback_status(config: &BrowserBridgeConfig, message: &str) -> BrowserStatusResponse {
    BrowserStatusResponse {
        status: "upstream_error".to_string(),
        checked_at: isoformat(now_utc()),
        worker_available: false,
        worker_url: Some(worker_base_url(config)),
        profiles: fallback_profile_states(config, false, Some(message.to_string())),
        message: Some(message.to_string()),
    }
}

fn fallback_profile_states(
    config: &BrowserBridgeConfig,
    writable: bool,
    message: Option<String>,
) -> Vec<BrowserProfileState> {
    config
        .profiles
        .iter()
        .map(|profile| BrowserProfileState {
            profile: profile.name.clone(),
            mode: profile.mode.clone(),
            browser: profile.browser.clone(),
            status: "upstream_error".to_string(),
            writable,
            fallback_in_use: false,
            current_url: None,
            title: None,
            message: message.clone(),
            issue: Some(build_issue(
                "browser_bridge_unavailable",
                &format!("browser:{}", profile.name),
                Vec::new(),
            )),
        })
        .collect()
}

fn worker_base_url(config: &BrowserBridgeConfig) -> String {
    format!("http://127.0.0.1:{}", config.worker_port)
}

fn worker_url(config: &BrowserBridgeConfig, path: &str) -> String {
    format!("{}{}", worker_base_url(config), path)
}

async fn fetch_worker_json(url: String) -> Result<Value, String> {
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|err| format!("failed to build browser worker client: {err}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| format!("browser worker unavailable: {err}"))?;
    response
        .json::<Value>()
        .await
        .map_err(|err| format!("invalid browser worker JSON: {err}"))
}

async fn post_worker_json<T: Serialize>(
    url: String,
    payload: &T,
    config: &BrowserBridgeConfig,
) -> BrowserActionResponse {
    if !config.enabled {
        return action_error(
            "upstream_error",
            "browser_bridge_unavailable",
            Some("browser bridge disabled".to_string()),
        );
    }
    let client = match Client::builder().user_agent(USER_AGENT).build() {
        Ok(client) => client,
        Err(err) => {
            return action_error(
                "upstream_error",
                "browser_bridge_unavailable",
                Some(format!("failed to build browser worker client: {err}")),
            )
        }
    };
    match client.post(url).json(payload).send().await {
        Ok(response) => match response.json::<BrowserActionResponse>().await {
            Ok(mut body) => {
                if body.checked_at.is_empty() {
                    body.checked_at = isoformat(now_utc());
                }
                body
            }
            Err(err) => action_error(
                "upstream_error",
                "browser_bridge_unavailable",
                Some(format!("invalid browser worker JSON: {err}")),
            ),
        },
        Err(err) => action_error(
            "upstream_error",
            "browser_bridge_unavailable",
            Some(format!("browser worker unavailable: {err}")),
        ),
    }
}

async fn enforce_write_policy_for_evaluate(
    config: &BrowserBridgeConfig,
    request: &BrowserEvaluateRequest,
    profile: &str,
) -> BrowserActionResponse {
    match resolve_tab_origin(config, profile, request.tab_id.as_deref()).await {
        Ok(Some(origin)) if origin_allowed(config, &origin) => {
            post_worker_json(worker_url(config, "/evaluate"), request, config).await
        }
        Ok(Some(origin)) => requires_confirmation_response(
            profile,
            request.tab_id.clone(),
            Some(origin),
            "evaluate".to_string(),
            "page javascript".to_string(),
        ),
        Ok(None) => action_error(
            "write_blocked",
            "write_blocked",
            Some("could not resolve the current page origin for this write".to_string()),
        ),
        Err(message) => action_error(
            "upstream_error",
            "browser_bridge_unavailable",
            Some(message),
        ),
    }
}

async fn enforce_write_policy_for_action(
    _paths: &RuntimePaths,
    config: &BrowserBridgeConfig,
    request: &BrowserActionRequest,
    profile: &str,
) -> BrowserActionResponse {
    match resolve_tab_origin(config, profile, request.tab_id.as_deref()).await {
        Ok(Some(origin)) if origin_allowed(config, &origin) => {
            post_worker_json(worker_url(config, "/action"), request, config).await
        }
        Ok(Some(origin)) => requires_confirmation_response(
            profile,
            request.tab_id.clone(),
            Some(origin),
            request.action.clone(),
            summarize_target(&request.target),
        ),
        Ok(None) => action_error(
            "write_blocked",
            "write_blocked",
            Some("could not resolve the current page origin for this write".to_string()),
        ),
        Err(message) => action_error(
            "upstream_error",
            "browser_bridge_unavailable",
            Some(message),
        ),
    }
}

async fn resolve_tab_origin(
    config: &BrowserBridgeConfig,
    profile: &str,
    tab_id: Option<&str>,
) -> Result<Option<String>, String> {
    let tabs = browser_tabs(config.clone(), Some(profile.to_string())).await;
    if tabs.status == "upstream_error" {
        return Err(tabs
            .message
            .unwrap_or_else(|| "browser tabs unavailable".to_string()));
    }
    let selected_tab = if let Some(tab_id) = tab_id {
        tabs.tabs.into_iter().find(|tab| tab.tab_id == tab_id)
    } else {
        tabs.tabs.into_iter().find(|tab| tab.active)
    };
    Ok(selected_tab
        .and_then(|tab| tab.current_url)
        .and_then(|url| origin_from_url(&url)))
}

fn resolve_profile_name(config: &BrowserBridgeConfig, requested: Option<&str>) -> String {
    let candidate = requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(config.default_profile.as_str());
    if config.profile(candidate).is_some() {
        candidate.to_string()
    } else {
        config.default_profile.clone()
    }
}

fn origin_allowed(config: &BrowserBridgeConfig, origin: &str) -> bool {
    config
        .write_policy
        .allowed_origins
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(origin))
}

fn origin_from_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    Some(format!(
        "{}://{}{}",
        parsed.scheme(),
        parsed.host_str()?,
        parsed
            .port()
            .map(|port| format!(":{port}"))
            .unwrap_or_default()
    ))
}

fn summarize_target(target: &crate::BrowserTarget) -> String {
    if let Some(selector) = target.selector.as_deref() {
        return selector.to_string();
    }
    if let Some(text) = target.text.as_deref() {
        return text.to_string();
    }
    "current page".to_string()
}

fn action_error(status: &str, issue_type: &str, message: Option<String>) -> BrowserActionResponse {
    BrowserActionResponse {
        status: status.to_string(),
        checked_at: isoformat(now_utc()),
        profile: None,
        tab_id: None,
        current_url: None,
        title: None,
        message,
        issue: Some(build_issue(issue_type, "browser", Vec::new())),
        issue_type: Some(issue_type.to_string()),
        action_preview: None,
        data: Value::Null,
    }
}

fn requires_confirmation_response(
    profile: &str,
    tab_id: Option<String>,
    current_url: Option<String>,
    action: String,
    target_summary: String,
) -> BrowserActionResponse {
    BrowserActionResponse {
        status: "requires_confirmation".to_string(),
        checked_at: isoformat(now_utc()),
        profile: Some(profile.to_string()),
        tab_id,
        current_url,
        title: None,
        message: Some("browser write action requires explicit user confirmation".to_string()),
        issue: Some(build_issue(
            "write_confirmation_required",
            "browser",
            Vec::new(),
        )),
        issue_type: Some("write_confirmation_required".to_string()),
        action_preview: Some(BrowserActionPreview {
            action,
            target_summary,
            reason: "origin is not whitelisted for direct browser writes".to_string(),
        }),
        data: Value::Null,
    }
}

fn persist_status(paths: &RuntimePaths, status: &BrowserStatusResponse) {
    if let Some(parent) = paths.browser_bridge_status_path().parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(status) {
        let _ = fs::write(paths.browser_bridge_status_path(), bytes);
    }
}

fn log_action(
    paths: &RuntimePaths,
    request: &BrowserActionRequest,
    response: &BrowserActionResponse,
) {
    append_jsonl(
        paths.browser_actions_log_path(),
        json!({
            "time": isoformat(now_utc()),
            "request": request,
            "response_status": response.status,
            "profile": response.profile,
            "tab_id": response.tab_id,
            "current_url": response.current_url,
            "issue_type": response.issue_type,
        }),
    );
}

fn log_confirmation(
    paths: &RuntimePaths,
    request: &BrowserActionRequest,
    response: &BrowserActionResponse,
) {
    append_jsonl(
        paths.browser_confirmations_log_path(),
        json!({
            "time": isoformat(now_utc()),
            "request": request,
            "response": response,
        }),
    );
}

fn append_jsonl(path: std::path::PathBuf, value: Value) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(line) = serde_json::to_string(&value) {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{line}");
        }
    }
}

pub(crate) fn browser_issue(issue_type: &str, query: &str) -> Issue {
    build_issue(issue_type, query, Vec::new())
}
