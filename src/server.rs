use crate::{
    audit_entity, browser_action, browser_evaluate, browser_focus, browser_open, browser_profiles,
    browser_screenshot, browser_snapshot, browser_status, browser_tabs, browser_wait,
    build_failure_summary_payload, build_issue, build_replacement_candidates_report,
    execute_control, express_auth_status, express_packages, fetch_all_states_typed,
    generate_config_report, parse_window, refine_live_context_report_with_typed_states,
    resolve_control_target, resolve_entity_payload, BrowserActionRequest, BrowserBridgeConfig,
    BrowserEvaluateRequest, BrowserFocusRequest, BrowserOpenRequest, BrowserScreenshotRequest,
    BrowserSnapshotRequest, BrowserWaitRequest, ControlAction, ControlConfig,
    ExecuteControlRequest, FailureReason, HaClient, HaMcpClient, ModelRoutingManager, ProxyError,
    RuntimePaths,
};
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Serialize)]
struct IssueResponse {
    status: String,
    reason: FailureReason,
    issue: crate::Issue,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    service: &'static str,
    features: Vec<&'static str>,
}

#[derive(Serialize)]
struct AuditIssueResponse {
    result_type: &'static str,
    reason: FailureReason,
    issue: crate::Issue,
}

#[derive(Clone)]
pub struct AppState {
    pub client: HaClient,
    pub mcp_client: HaMcpClient,
    pub paths: RuntimePaths,
    pub control_config: Arc<ControlConfig>,
    pub browser_config: Arc<BrowserBridgeConfig>,
    pub routing: Arc<ModelRoutingManager>,
}

impl AppState {
    pub fn new(
        client: HaClient,
        mcp_client: HaMcpClient,
        paths: RuntimePaths,
        control_config: Arc<ControlConfig>,
        browser_config: Arc<BrowserBridgeConfig>,
        routing: Arc<ModelRoutingManager>,
    ) -> Self {
        Self {
            client,
            mcp_client,
            paths,
            control_config,
            browser_config,
            routing,
        }
    }
}

pub fn build_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/resolve-entity", get(resolve_entity))
        .route("/audit", get(audit))
        .route("/resolve-control-target", get(resolve_control))
        .route("/execute-control", post(execute_control_handler))
        .route("/advisor/failure-summary", get(failure_summary))
        .route("/advisor/config-report", get(config_report))
        .route(
            "/advisor/replacement-candidates",
            get(replacement_candidates),
        )
        .route("/model-routing/status", get(model_routing_status))
        .route("/model-routing/plan", get(model_routing_plan))
        .route("/model-routing/scorecard", get(model_routing_scorecard))
        .route(
            "/model-routing/observations",
            get(model_routing_observations),
        )
        .route("/zeroclaw/runtime-traces", get(zeroclaw_runtime_traces))
        .route("/express/auth-status", get(express_auth_status_handler))
        .route("/express/packages", get(express_packages_handler))
        .route("/express/search", get(express_search_handler))
        .route("/browser/status", get(browser_status_handler))
        .route("/browser/profiles", get(browser_profiles_handler))
        .route("/browser/tabs", get(browser_tabs_handler))
        .route("/browser/open", post(browser_open_handler))
        .route("/browser/focus", post(browser_focus_handler))
        .route("/browser/snapshot", post(browser_snapshot_handler))
        .route("/browser/evaluate", post(browser_evaluate_handler))
        .route("/browser/action", post(browser_action_handler))
        .route("/browser/screenshot", post(browser_screenshot_handler))
        .route("/browser/wait", post(browser_wait_handler))
        .route("/ha-mcp/capabilities", get(ha_mcp_capabilities))
        .route("/ha-mcp/live-context", get(ha_mcp_live_context))
        .with_state(state)
}

fn json_response<T: Serialize>(status: StatusCode, value: T) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(serde_json::to_value(value).unwrap_or_else(|_| json!({"status":"failed"}))),
    )
}

fn bad_request_issue_response(
    status: StatusCode,
    response_status: &str,
    query_entity: &str,
) -> (StatusCode, Json<Value>) {
    json_response(
        status,
        IssueResponse {
            status: response_status.to_string(),
            reason: FailureReason::BadRequest,
            issue: build_issue("bad_request", query_entity, vec![]),
        },
    )
}

fn proxy_issue_response(err: ProxyError, query_entity: &str) -> (StatusCode, Json<Value>) {
    let (status, reason) = match err {
        ProxyError::MissingCredentials => (
            StatusCode::INTERNAL_SERVER_ERROR,
            FailureReason::MissingCredentials,
        ),
        ProxyError::AuthFailed => (StatusCode::OK, FailureReason::HaAuthFailed),
        ProxyError::Unreachable | ProxyError::Invalid(_) => {
            (StatusCode::OK, FailureReason::HaUnreachable)
        }
    };
    json_response(
        status,
        IssueResponse {
            status: "failed".to_string(),
            reason: reason.clone(),
            issue: build_issue(reason.as_str(), query_entity, vec![]),
        },
    )
}

async fn health() -> Json<Value> {
    Json(
        serde_json::to_value(HealthResponse {
            status: "ok",
            service: "ha_proxy",
            features: vec![
                "audit",
                "control_resolution",
                "control_execution",
                "advisor_reports",
                "replacement_candidates",
                "model_routing_status",
                "model_routing_plan",
                "model_routing_scorecard",
                "model_routing_observations",
                "zeroclaw_runtime_traces",
                "express_auth_status",
                "express_packages",
                "browser_status",
                "browser_profiles",
                "browser_tabs",
                "ha_mcp_capabilities",
                "ha_mcp_live_context",
            ],
        })
        .unwrap_or_else(|_| json!({"status":"ok"})),
    )
}

async fn resolve_entity(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let query_entity = params.get("entity_id").cloned().unwrap_or_default();
    if query_entity.trim().is_empty() {
        return bad_request_issue_response(StatusCode::BAD_REQUEST, "config_issue", "");
    }
    json_response(
        StatusCode::OK,
        resolve_entity_payload(&state.client, &query_entity).await,
    )
}

async fn resolve_control(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let query_entity = params
        .get("query")
        .cloned()
        .or_else(|| params.get("entity_id").cloned())
        .unwrap_or_default();
    let action = params
        .get("action")
        .map(|value| ControlAction::from_query(value))
        .unwrap_or(ControlAction::TurnOn);
    if query_entity.trim().is_empty() {
        return bad_request_issue_response(StatusCode::BAD_REQUEST, "failed", "");
    }
    match resolve_control_target(
        &state.client,
        &query_entity,
        action.as_str(),
        &state.control_config,
    )
    .await
    {
        Ok(result) => json_response(StatusCode::OK, result),
        Err(err) => proxy_issue_response(err, &query_entity),
    }
}

async fn failure_summary(State(state): State<AppState>) -> Json<Value> {
    Json(build_failure_summary_payload(&state.paths))
}

async fn config_report(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match generate_config_report(
        &state.client,
        &state.mcp_client,
        &state.paths,
        &state.control_config,
    )
    .await
    {
        Ok(report) => json_response(StatusCode::OK, report),
        Err(err) => proxy_issue_response(err, ""),
    }
}

async fn replacement_candidates(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match state.mcp_client.live_context_report().await {
        Ok(mut report) => {
            if let Ok(states) = fetch_all_states_typed(&state.client).await {
                refine_live_context_report_with_typed_states(&mut report, &states);
            }
            json_response(StatusCode::OK, build_replacement_candidates_report(&report))
        }
        Err(ProxyError::MissingCredentials) => {
            proxy_issue_response(ProxyError::MissingCredentials, "")
        }
        Err(ProxyError::AuthFailed) => proxy_issue_response(ProxyError::AuthFailed, ""),
        Err(ProxyError::Unreachable) => proxy_issue_response(ProxyError::Unreachable, ""),
        Err(ProxyError::Invalid(message)) => json_response(
            StatusCode::OK,
            json!({
                "status": "failed",
                "reason": FailureReason::HaUnreachable,
                "issue": build_issue("ha_unreachable", "", vec![]),
                "details": message,
            }),
        ),
    }
}

async fn audit(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Json<Value>) {
    let query_entity = params.get("entity_id").cloned().unwrap_or_default();
    if query_entity.trim().is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            AuditIssueResponse {
                result_type: "config_issue",
                reason: FailureReason::BadRequest,
                issue: build_issue("bad_request", "", vec![]),
            },
        );
    }
    let (start, end) = match parse_window(&params) {
        Ok(window) => window,
        Err(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                AuditIssueResponse {
                    result_type: "config_issue",
                    reason: FailureReason::BadRequest,
                    issue: build_issue("bad_request", &query_entity, vec![]),
                },
            )
        }
    };
    json_response(
        StatusCode::OK,
        audit_entity(&state.client, &query_entity, start, end).await,
    )
}

async fn execute_control_handler(
    State(state): State<AppState>,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    let payload: ExecuteControlRequest = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                crate::ExecuteControlResponse {
                    status: "failed".to_string(),
                    reason: Some(FailureReason::BadRequest),
                    issue: Some(build_issue("bad_request", "", vec![])),
                    ..Default::default()
                },
            )
        }
    };
    let response =
        execute_control(&state.client, &state.paths, &state.control_config, payload).await;
    json_response(StatusCode::OK, response)
}

async fn model_routing_status(State(state): State<AppState>) -> Json<Value> {
    Json(
        serde_json::to_value(state.routing.status().await)
            .unwrap_or_else(|_| json!({"status":"error","route_ready":false})),
    )
}

async fn model_routing_plan(State(state): State<AppState>) -> Json<Value> {
    Json(
        serde_json::to_value(state.routing.plan().await)
            .unwrap_or_else(|_| json!({"status":"error"})),
    )
}

async fn model_routing_scorecard(State(state): State<AppState>) -> Json<Value> {
    Json(
        serde_json::to_value(state.routing.scorecard().await)
            .unwrap_or_else(|_| json!({"status":"error"})),
    )
}

async fn model_routing_observations(State(state): State<AppState>) -> Json<Value> {
    Json(
        serde_json::to_value(state.routing.observations().await)
            .unwrap_or_else(|_| json!({"status":"error"})),
    )
}

async fn zeroclaw_runtime_traces(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .map(|value| value.clamp(1, 200))
        .unwrap_or(20);
    let trace_path = state.paths.zeroclaw_runtime_trace_path();
    let trace_path_str = trace_path.display().to_string();

    if !trace_path.exists() {
        return Json(json!({
            "status": "empty",
            "trace_path": trace_path_str,
            "returned": 0,
            "total_entries": 0,
            "entries": [],
        }));
    }

    let raw = match std::fs::read_to_string(&trace_path) {
        Ok(contents) => contents,
        Err(err) => {
            return Json(json!({
                "status": "error",
                "trace_path": trace_path_str,
                "message": format!("failed to read runtime trace: {err}"),
                "entries": [],
            }));
        }
    };

    let total_entries = raw.lines().filter(|line| !line.trim().is_empty()).count();
    let mut entries = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(limit)
        .map(|line| serde_json::from_str::<Value>(line).unwrap_or_else(|_| json!({ "raw": line })))
        .collect::<Vec<_>>();
    entries.reverse();

    Json(json!({
        "status": if entries.is_empty() { "empty" } else { "ok" },
        "trace_path": trace_path_str,
        "returned": entries.len(),
        "total_entries": total_entries,
        "entries": entries,
    }))
}

async fn express_auth_status_handler(State(state): State<AppState>) -> Json<Value> {
    Json(
        serde_json::to_value(
            express_auth_status(state.paths.clone(), (*state.browser_config).clone()).await,
        )
        .unwrap_or_else(|_| json!({"status":"upstream_error","sources":[] })),
    )
}

async fn express_packages_handler(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let source = params.get("source").cloned();
    let query = params.get("q").cloned();
    let force_refresh = params
        .get("refresh")
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    Json(
        serde_json::to_value(
            express_packages(
                state.paths.clone(),
                (*state.browser_config).clone(),
                source,
                query,
                force_refresh,
            )
            .await,
        )
        .unwrap_or_else(|_| json!({"status":"upstream_error","packages":[],"sources":[] })),
    )
}

async fn express_search_handler(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let source = params.get("source").cloned();
    let query = params
        .get("q")
        .cloned()
        .or_else(|| params.get("query").cloned());
    Json(
        serde_json::to_value(
            express_packages(
                state.paths.clone(),
                (*state.browser_config).clone(),
                source,
                query,
                false,
            )
            .await,
        )
        .unwrap_or_else(|_| json!({"status":"upstream_error","packages":[],"sources":[] })),
    )
}

async fn browser_status_handler(State(state): State<AppState>) -> Json<Value> {
    Json(
        serde_json::to_value(
            browser_status(state.paths.clone(), (*state.browser_config).clone()).await,
        )
        .unwrap_or_else(|_| json!({"status":"upstream_error"})),
    )
}

async fn browser_profiles_handler(State(state): State<AppState>) -> Json<Value> {
    Json(
        serde_json::to_value(browser_profiles((*state.browser_config).clone()).await)
            .unwrap_or_else(|_| json!({"status":"upstream_error","profiles":[] })),
    )
}

async fn browser_tabs_handler(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let profile = params.get("profile").cloned();
    Json(
        serde_json::to_value(browser_tabs((*state.browser_config).clone(), profile).await)
            .unwrap_or_else(|_| json!({"status":"upstream_error","tabs":[] })),
    )
}

async fn browser_open_handler(
    State(state): State<AppState>,
    Json(payload): Json<BrowserOpenRequest>,
) -> Json<Value> {
    Json(
        serde_json::to_value(browser_open((*state.browser_config).clone(), payload).await)
            .unwrap_or_else(|_| json!({"status":"upstream_error"})),
    )
}

async fn browser_focus_handler(
    State(state): State<AppState>,
    Json(payload): Json<BrowserFocusRequest>,
) -> Json<Value> {
    Json(
        serde_json::to_value(browser_focus((*state.browser_config).clone(), payload).await)
            .unwrap_or_else(|_| json!({"status":"upstream_error"})),
    )
}

async fn browser_snapshot_handler(
    State(state): State<AppState>,
    Json(payload): Json<BrowserSnapshotRequest>,
) -> Json<Value> {
    Json(
        serde_json::to_value(browser_snapshot((*state.browser_config).clone(), payload).await)
            .unwrap_or_else(|_| json!({"status":"upstream_error"})),
    )
}

async fn browser_evaluate_handler(
    State(state): State<AppState>,
    Json(payload): Json<BrowserEvaluateRequest>,
) -> Json<Value> {
    Json(
        serde_json::to_value(browser_evaluate((*state.browser_config).clone(), payload).await)
            .unwrap_or_else(|_| json!({"status":"upstream_error"})),
    )
}

async fn browser_action_handler(
    State(state): State<AppState>,
    Json(payload): Json<BrowserActionRequest>,
) -> Json<Value> {
    Json(
        serde_json::to_value(
            browser_action(
                state.paths.clone(),
                (*state.browser_config).clone(),
                payload,
            )
            .await,
        )
        .unwrap_or_else(|_| json!({"status":"upstream_error"})),
    )
}

async fn browser_screenshot_handler(
    State(state): State<AppState>,
    Json(payload): Json<BrowserScreenshotRequest>,
) -> Json<Value> {
    Json(
        serde_json::to_value(browser_screenshot((*state.browser_config).clone(), payload).await)
            .unwrap_or_else(|_| json!({"status":"upstream_error"})),
    )
}

async fn browser_wait_handler(
    State(state): State<AppState>,
    Json(payload): Json<BrowserWaitRequest>,
) -> Json<Value> {
    Json(
        serde_json::to_value(browser_wait((*state.browser_config).clone(), payload).await)
            .unwrap_or_else(|_| json!({"status":"upstream_error"})),
    )
}

async fn ha_mcp_capabilities(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match state.mcp_client.capabilities().await {
        Ok(capabilities) => {
            if let Ok(raw) = serde_json::to_vec_pretty(&capabilities) {
                let _ = std::fs::write(state.paths.ha_mcp_capabilities_path(), raw);
            }
            json_response(StatusCode::OK, capabilities)
        }
        Err(ProxyError::MissingCredentials) => {
            proxy_issue_response(ProxyError::MissingCredentials, "")
        }
        Err(ProxyError::AuthFailed) => proxy_issue_response(ProxyError::AuthFailed, ""),
        Err(ProxyError::Unreachable) => proxy_issue_response(ProxyError::Unreachable, ""),
        Err(ProxyError::Invalid(message)) => json_response(
            StatusCode::OK,
            json!({
                "status": "failed",
                "reason": FailureReason::HaUnreachable,
                "issue": build_issue("ha_unreachable", "", vec![]),
                "details": message,
            }),
        ),
    }
}

async fn ha_mcp_live_context(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    match state.mcp_client.live_context_report().await {
        Ok(report) => {
            if let Ok(raw) = serde_json::to_vec_pretty(&report) {
                let _ = std::fs::write(state.paths.ha_mcp_live_context_path(), raw);
            }
            json_response(StatusCode::OK, report)
        }
        Err(ProxyError::MissingCredentials) => {
            proxy_issue_response(ProxyError::MissingCredentials, "")
        }
        Err(ProxyError::AuthFailed) => proxy_issue_response(ProxyError::AuthFailed, ""),
        Err(ProxyError::Unreachable) => proxy_issue_response(ProxyError::Unreachable, ""),
        Err(ProxyError::Invalid(message)) => json_response(
            StatusCode::OK,
            json!({
                "status": "failed",
                "reason": FailureReason::HaUnreachable,
                "issue": build_issue("ha_unreachable", "", vec![]),
                "details": message,
            }),
        ),
    }
}
