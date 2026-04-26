use crate::{
    add_article_memory, article_memory_status, audit_entity, build_failure_summary_payload,
    build_issue, build_replacement_candidates_report, execute_control, express_auth_status,
    express_packages, fetch_all_states_typed, generate_config_report, hybrid_search_article_memory,
    list_article_memory, normalize_all_article_memory, normalize_article_memory, parse_window,
    refine_live_context_report_with_typed_states, resolve_article_embedding_config,
    resolve_article_normalize_config, resolve_article_value_config, resolve_control_target,
    resolve_entity_payload, search_article_memory, upsert_article_memory_embedding,
    ArticleMemoryAddRequest, ArticleMemoryConfig, ControlAction, ControlConfig, Crawl4aiConfig,
    Crawl4aiSupervisor, ExecuteControlRequest, FailureReason, HaClient, HaMcpClient,
    IngestJobStatus, IngestQueue, IngestRequest, IngestSubmitError, ListFilter,
    ModelProviderConfig, ProxyError, RuntimePaths,
};
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

type HmacSha256 = Hmac<Sha256>;

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
    /// Supervisor view of the long-lived crawl4ai adapter:
    /// `healthy` — /health probe last succeeded
    /// `disabled` — `[crawl4ai].enabled = false` in local.toml
    /// `starting` — enabled, not abandoned, but not healthy yet (boot or restart)
    /// `abandoned` — supervisor exhausted its retry budget; no further respawns
    crawl4ai: &'static str,
    /// Ingest queue's persistence state. "healthy" means recent persists
    /// succeeded. "degraded" means 3+ consecutive failures — user should
    /// investigate disk space. Mirrors `queue.persist_health()`.
    ingest_persist: crate::article_memory::PersistHealth,
    /// MemPalace projection sink counters. `status` ∈ {"live","silenced","disabled"}.
    mempalace: MempalaceHealth,
}

#[derive(Serialize)]
struct MempalaceHealth {
    status: &'static str,
    sent: u64,
    dropped: u64,
    failed: u64,
    child_restarts: u64,
    last_error: Option<String>,
}

#[derive(Serialize)]
struct AuditIssueResponse {
    result_type: &'static str,
    reason: FailureReason,
    issue: crate::Issue,
}

/// Per-profile async mutex map. Each Crawl4AI Chromium profile (keyed by
/// `express-{source}`) gets its own `Arc<Mutex<()>>`; callers hold the guard
/// for the duration of the `crawl4ai_crawl` call so two requests never attach
/// the same `user_data_dir` concurrently (Chromium's `SingletonLock` would
/// otherwise fail the second attach).
pub type Crawl4aiProfileLocks = Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>;

#[derive(Clone)]
pub struct AppState {
    pub client: HaClient,
    pub mcp_client: HaMcpClient,
    pub paths: RuntimePaths,
    pub control_config: Arc<ControlConfig>,
    pub crawl4ai_config: Arc<Crawl4aiConfig>,
    pub crawl4ai_profile_locks: Crawl4aiProfileLocks,
    /// Shared handle to the long-lived crawl4ai HTTP adapter. When crawl4ai
    /// is disabled in `local.toml` or the supervisor fails at boot, this
    /// holds a `Crawl4aiSupervisor::disabled()` stub — every
    /// `crawl4ai_crawl` call through it returns `Crawl4aiError::Disabled`
    /// instead of touching the network.
    pub crawl4ai_supervisor: Arc<Crawl4aiSupervisor>,
    pub article_memory_config: Arc<ArticleMemoryConfig>,
    pub providers: Arc<Vec<ModelProviderConfig>>,
    pub shortcut_secret: String,
    pub ingest_queue: Arc<IngestQueue>,
    pub learned_rules: Arc<crate::article_memory::LearnedRuleStore>,
    pub rule_stats: Arc<crate::article_memory::RuleStatsStore>,
    pub sample_store: Arc<crate::article_memory::SampleStore>,
    /// Fire-and-forget projection into MemPalace. Defaults to a disabled
    /// no-op sink; `local_proxy` overrides with the real one via
    /// `with_mempalace_sink` after constructing `AppState`.
    pub mempalace_sink: crate::mempalace_sink::MemPalaceSink,
    /// Debouncer for `ComponentReachability(component:ha-mcp, …)`. Shared
    /// across handlers so the 30-second sustained-failure window survives
    /// per-request state. `TimeDebouncer::new(Duration::from_secs(30))`.
    pub component_reachability_debouncer: Arc<crate::mempalace_sink::TimeDebouncer>,
}

impl AppState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: HaClient,
        mcp_client: HaMcpClient,
        paths: RuntimePaths,
        control_config: Arc<ControlConfig>,
        crawl4ai_config: Arc<Crawl4aiConfig>,
        crawl4ai_supervisor: Arc<Crawl4aiSupervisor>,
        article_memory_config: Arc<ArticleMemoryConfig>,
        providers: Arc<Vec<ModelProviderConfig>>,
        shortcut_secret: String,
        crawl4ai_profile_locks: Crawl4aiProfileLocks,
        ingest_queue: Arc<IngestQueue>,
        learned_rules: Arc<crate::article_memory::LearnedRuleStore>,
        rule_stats: Arc<crate::article_memory::RuleStatsStore>,
        sample_store: Arc<crate::article_memory::SampleStore>,
    ) -> Self {
        Self {
            client,
            mcp_client,
            paths,
            control_config,
            crawl4ai_config,
            crawl4ai_profile_locks,
            crawl4ai_supervisor,
            article_memory_config,
            providers,
            shortcut_secret,
            ingest_queue,
            learned_rules,
            rule_stats,
            sample_store,
            mempalace_sink: crate::mempalace_sink::MemPalaceSink::disabled(),
            component_reachability_debouncer: Arc::new(crate::mempalace_sink::TimeDebouncer::new(
                std::time::Duration::from_secs(30),
            )),
        }
    }

    /// Attach a live MemPalace sink. Call this after `new` when Davis is
    /// booting; tests leave the default disabled sink.
    pub fn with_mempalace_sink(mut self, sink: crate::mempalace_sink::MemPalaceSink) -> Self {
        self.mempalace_sink = sink;
        self
    }

    /// Return the per-profile lock for `profile`, creating it lazily on first
    /// access. Callers must `.lock().await` the returned `Arc<Mutex<()>>`
    /// before dispatching to `crawl4ai_crawl` to serialize access to the
    /// underlying Chromium `user_data_dir`.
    pub async fn crawl4ai_profile_lock(&self, profile: &str) -> Arc<Mutex<()>> {
        let mut map = self.crawl4ai_profile_locks.lock().await;
        map.entry(profile.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
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
        .route("/zeroclaw/runtime-traces", get(zeroclaw_runtime_traces))
        .route("/express/auth-status", get(express_auth_status_handler))
        .route("/express/packages", get(express_packages_handler))
        .route("/express/search", get(express_search_handler))
        .route("/article-memory/status", get(article_memory_status_handler))
        .route("/article-memory/articles", get(article_memory_list_handler))
        .route("/article-memory/articles", post(article_memory_add_handler))
        .route(
            "/article-memory/normalize",
            post(article_memory_normalize_handler),
        )
        .route("/article-memory/search", get(article_memory_search_handler))
        .route("/article-memory/digest", get(crate::server_digest::handle))
        .route("/article-memory/ingest", post(ingest_submit_handler))
        .route("/article-memory/ingest", get(ingest_list_handler))
        .route("/article-memory/ingest/:job_id", get(ingest_get_handler))
        .route("/article-memory/rules", get(rules_list_handler))
        .route(
            "/article-memory/rules/mark-stale",
            post(rules_mark_stale_handler),
        )
        .route("/article-memory/rules/warmup", post(rules_warmup_handler))
        .route("/ha-mcp/capabilities", get(ha_mcp_capabilities))
        .route("/ha-mcp/live-context", get(ha_mcp_live_context))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

pub fn build_shortcut_bridge_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(shortcut_bridge_health))
        .route("/shortcut", post(shortcut_bridge))
        .layer(tower_http::trace::TraceLayer::new_for_http())
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

async fn health(State(state): State<AppState>) -> Json<Value> {
    // Supervisor state summary. Order matters:
    // 1. `abandoned` wins over everything — we've explicitly given up, so
    //    reporting "healthy" because a probe coincidentally succeeds would
    //    mislead operators.
    // 2. `disabled` before `healthy` because a probe against an
    //    unconfigured supervisor is meaningless.
    // 3. `healthy` vs `starting` is the live probe vs. not-ready distinction.
    let crawl4ai_state = if state.crawl4ai_supervisor.is_abandoned().await {
        "abandoned"
    } else if !state.crawl4ai_config.enabled {
        "disabled"
    } else if state.crawl4ai_supervisor.is_healthy().await {
        "healthy"
    } else {
        "starting"
    };
    let ingest_persist = state.ingest_queue.persist_health();
    let sink_snapshot = state.mempalace_sink.metrics().await;
    let mempalace_status: &'static str = if !sink_snapshot.enabled {
        "disabled"
    } else if sink_snapshot.silenced {
        "silenced"
    } else {
        "live"
    };
    let mempalace = MempalaceHealth {
        status: mempalace_status,
        sent: sink_snapshot.sent,
        dropped: sink_snapshot.dropped,
        failed: sink_snapshot.failed,
        child_restarts: sink_snapshot.child_restarts,
        last_error: sink_snapshot.last_error,
    };
    Json(
        serde_json::to_value(HealthResponse {
            status: "ok",
            service: "davis_local_proxy",
            features: vec![
                "audit",
                "control_resolution",
                "control_execution",
                "advisor_reports",
                "replacement_candidates",
                "zeroclaw_runtime_traces",
                "express_auth_status",
                "express_packages",
                "article_memory_status",
                "article_memory_articles",
                "article_memory_normalize",
                "article_memory_search",
                "ha_mcp_capabilities",
                "ha_mcp_live_context",
                "shortcut_bridge",
                "mempalace_sink",
            ],
            crawl4ai: crawl4ai_state,
            ingest_persist,
            mempalace,
        })
        .unwrap_or_else(|_| json!({"status":"ok"})),
    )
}

async fn shortcut_bridge_health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "shortcut_bridge",
    }))
}

async fn shortcut_bridge(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    if state.shortcut_secret.trim().is_empty() {
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "status": "failed",
                "reason": "missing_webhook_secret",
            }),
        );
    }

    let provided_secret = headers
        .get("x-webhook-secret")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if provided_secret != state.shortcut_secret {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({
                "status": "failed",
                "reason": "invalid_webhook_secret",
            }),
        );
    }

    if serde_json::from_slice::<Value>(&body).is_err() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({
                "status": "failed",
                "reason": "invalid_json",
            }),
        );
    }

    let signature = hmac_sha256_hex(&state.shortcut_secret, &body);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({
                    "status": "failed",
                    "reason": "client_build_failed",
                    "message": err.to_string(),
                }),
            )
        }
    };

    match client
        .post("http://127.0.0.1:3001/shortcut")
        .header("content-type", "application/json")
        .header("x-webhook-signature", signature)
        .body(body)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            json_response(StatusCode::ACCEPTED, json!({"status": "accepted"}))
        }
        Ok(response) => json_response(
            StatusCode::BAD_GATEWAY,
            json!({
                "status": "failed",
                "reason": "zeroclaw_webhook_rejected",
                "upstream_status": response.status().as_u16(),
            }),
        ),
        Err(err) => json_response(
            StatusCode::BAD_GATEWAY,
            json!({
                "status": "failed",
                "reason": "zeroclaw_webhook_unreachable",
                "message": err.to_string(),
            }),
        ),
    }
}

fn hmac_sha256_hex(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
            express_auth_status(
                state.paths.clone(),
                (*state.crawl4ai_config).clone(),
                state.crawl4ai_profile_locks.clone(),
                state.crawl4ai_supervisor.clone(),
            )
            .await,
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
                (*state.crawl4ai_config).clone(),
                state.crawl4ai_profile_locks.clone(),
                state.crawl4ai_supervisor.clone(),
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
                (*state.crawl4ai_config).clone(),
                state.crawl4ai_profile_locks.clone(),
                state.crawl4ai_supervisor.clone(),
                source,
                query,
                false,
            )
            .await,
        )
        .unwrap_or_else(|_| json!({"status":"upstream_error","packages":[],"sources":[] })),
    )
}

async fn article_memory_status_handler(State(state): State<AppState>) -> Json<Value> {
    Json(
        serde_json::to_value(article_memory_status(&state.paths))
            .unwrap_or_else(|_| json!({"status":"upstream_error"})),
    )
}

async fn article_memory_list_handler(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20);
    Json(
        serde_json::to_value(list_article_memory(&state.paths, limit))
            .unwrap_or_else(|_| json!({"status":"upstream_error","articles":[] })),
    )
}

async fn article_memory_search_handler(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let query = params
        .get("q")
        .cloned()
        .or_else(|| params.get("query").cloned())
        .unwrap_or_default();
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(10);
    let semantic = params
        .get("semantic")
        .map(|value| !matches!(value.as_str(), "0" | "false" | "no"))
        .unwrap_or(true);
    if semantic {
        let embedding_config = resolve_article_embedding_config(
            &state.article_memory_config.embedding,
            &state.providers,
        )
        .ok()
        .flatten();
        return Json(
            serde_json::to_value(
                hybrid_search_article_memory(
                    &state.paths,
                    embedding_config.as_ref(),
                    &query,
                    limit,
                )
                .await,
            )
            .unwrap_or_else(|_| json!({"status":"upstream_error","hits":[] })),
        );
    }
    Json(
        serde_json::to_value(search_article_memory(&state.paths, &query, limit))
            .unwrap_or_else(|_| json!({"status":"upstream_error","hits":[] })),
    )
}

async fn article_memory_add_handler(
    State(state): State<AppState>,
    Json(payload): Json<ArticleMemoryAddRequest>,
) -> (StatusCode, Json<Value>) {
    match add_article_memory(&state.paths, payload) {
        Ok(record) => {
            let normalize_config = resolve_article_normalize_config(
                &state.article_memory_config.normalize,
                &state.providers,
            )
            .ok()
            .flatten();
            let value_config = resolve_article_value_config(
                &state.paths,
                &state.article_memory_config.value,
                &state.providers,
            )
            .ok()
            .flatten();
            let normalize_response = normalize_article_memory(
                &state.paths,
                normalize_config.as_ref(),
                value_config.as_ref(),
                &record.id,
            )
            .await;
            let (normalize_status, value_decision, embedding_status) = match normalize_response {
                Ok(response) => {
                    let embedding_status = if response.value_decision.as_deref() == Some("reject") {
                        "skipped_value_rejected".to_string()
                    } else {
                        match resolve_article_embedding_config(
                            &state.article_memory_config.embedding,
                            &state.providers,
                        ) {
                            Ok(Some(config)) => match upsert_article_memory_embedding(
                                &state.paths,
                                &config,
                                &record,
                            )
                            .await
                            {
                                Ok(()) => "ok".to_string(),
                                Err(error) => format!("error: {error}"),
                            },
                            Ok(None) => "disabled".to_string(),
                            Err(error) => format!("config_error: {error}"),
                        }
                    };
                    (
                        response.clean_status,
                        response.value_decision,
                        embedding_status,
                    )
                }
                Err(error) => (format!("error: {error}"), None, "skipped".to_string()),
            };
            json_response(
                StatusCode::CREATED,
                json!({
                    "status": "ok",
                    "article": record,
                    "normalize_status": normalize_status,
                    "value_decision": value_decision,
                    "embedding_status": embedding_status,
                }),
            )
        }
        Err(error) => json_response(
            StatusCode::BAD_REQUEST,
            json!({
                "status": "failed",
                "reason": "invalid_article_memory_record",
                "message": error.to_string(),
            }),
        ),
    }
}

async fn article_memory_normalize_handler(
    State(state): State<AppState>,
    Json(payload): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let no_llm = payload
        .get("no_llm")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let normalize_config = if no_llm {
        None
    } else {
        match resolve_article_normalize_config(
            &state.article_memory_config.normalize,
            &state.providers,
        ) {
            Ok(config) => config,
            Err(error) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({
                        "status": "failed",
                        "reason": "normalize_config_error",
                        "message": error.to_string(),
                    }),
                )
            }
        }
    };
    let value_config = if no_llm {
        None
    } else {
        match resolve_article_value_config(
            &state.paths,
            &state.article_memory_config.value,
            &state.providers,
        ) {
            Ok(config) => config,
            Err(error) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    json!({
                        "status": "failed",
                        "reason": "value_config_error",
                        "message": error.to_string(),
                    }),
                )
            }
        }
    };
    let result = if payload.get("all").and_then(Value::as_bool).unwrap_or(false) {
        normalize_all_article_memory(
            &state.paths,
            normalize_config.as_ref(),
            value_config.as_ref(),
        )
        .await
    } else if let Some(id) = payload.get("id").and_then(Value::as_str) {
        normalize_article_memory(
            &state.paths,
            normalize_config.as_ref(),
            value_config.as_ref(),
            id,
        )
        .await
        .map(|response| vec![response])
    } else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({
                "status": "failed",
                "reason": "missing_article_id",
                "message": "provide id or all=true",
            }),
        );
    };
    match result {
        Ok(responses) => json_response(
            StatusCode::OK,
            json!({
                "status": "ok",
                "articles": responses,
            }),
        ),
        Err(error) => json_response(
            StatusCode::BAD_REQUEST,
            json!({
                "status": "failed",
                "reason": "article_normalize_failed",
                "message": error.to_string(),
            }),
        ),
    }
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

fn record_ha_mcp_reachability(state: &AppState, unhealthy: bool) {
    state.component_reachability_debouncer.record(
        "ha-mcp",
        &crate::mempalace_sink::TripleId::component("ha-mcp"),
        crate::mempalace_sink::Predicate::ComponentReachability,
        &crate::mempalace_sink::TripleId::entity("state.unreachable"),
        unhealthy,
        std::time::Instant::now(),
        &state.mempalace_sink,
    );
}

async fn ha_mcp_live_context(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let call_result = state.mcp_client.live_context_report().await;
    record_ha_mcp_reachability(&state, call_result.is_err());
    match call_result {
        Ok(report) => {
            // Project into MemPalace BEFORE overwriting the snapshot file, so
            // the diff sees the previous cycle.
            let prev = std::fs::read_to_string(state.paths.ha_mcp_live_context_path())
                .ok()
                .and_then(|raw| {
                    serde_json::from_str::<crate::ha_mcp::HaMcpLiveContextReport>(&raw).ok()
                });
            let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            crate::ha_mcp_projection::emit_state_transitions(
                prev.as_ref(),
                &report,
                &state.mempalace_sink,
            );
            crate::ha_mcp_projection::emit_findings_projections(
                prev.as_ref(),
                &report,
                &state.mempalace_sink,
            );
            crate::ha_mcp_projection::emit_findings_drawer(
                &report,
                &timestamp,
                &state.mempalace_sink,
            );
            crate::ha_mcp_projection::emit_refresh_diary(
                &report,
                &timestamp,
                &state.mempalace_sink,
            );
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

#[derive(serde::Deserialize)]
struct IngestListQuery {
    status: Option<String>,
    limit: Option<usize>,
    only_failed: Option<bool>,
}

async fn ingest_submit_handler(
    State(state): State<AppState>,
    Json(payload): Json<IngestRequest>,
) -> (StatusCode, Json<Value>) {
    match state.ingest_queue.submit(payload).await {
        Ok(resp) => (
            StatusCode::ACCEPTED,
            Json(serde_json::to_value(resp).unwrap_or_else(|_| json!({}))),
        ),
        Err(err) => match err {
            IngestSubmitError::InvalidUrl(d) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_url", "detail": d})),
            ),
            IngestSubmitError::InvalidScheme => (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_scheme",
                    "detail": "only http and https schemes are allowed",
                })),
            ),
            IngestSubmitError::PrivateAddressBlocked(d) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "private_address_blocked", "detail": d})),
            ),
            IngestSubmitError::DuplicateSaved {
                existing_article_id,
                finished_at,
            } => (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "duplicate_within_window",
                    "existing_article_id": existing_article_id,
                    "finished_at": finished_at,
                })),
            ),
            IngestSubmitError::IngestDisabled => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "ingest_disabled"})),
            ),
            IngestSubmitError::PersistenceError(d) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "persistence_error", "detail": d})),
            ),
            IngestSubmitError::PersistenceDegraded {
                consecutive_failures,
                last_error,
            } => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "persistence_degraded",
                    "detail": format!("ingest queue persistence degraded after {consecutive_failures} consecutive failures"),
                    "last_error": last_error,
                    "action": "check disk space, free space, then restart daemon to retry"
                })),
            ),
            IngestSubmitError::ArticleExists {
                existing_article_id,
                title,
                url,
            } => (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "article_exists",
                    "existing_article_id": existing_article_id,
                    "title": title,
                    "url": url,
                    "action": "resubmit with \"force\": true to re-crawl and update"
                })),
            ),
        },
    }
}

async fn ingest_get_handler(
    State(state): State<AppState>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> (StatusCode, Json<Value>) {
    match state.ingest_queue.get(&job_id).await {
        Some(job) => (
            StatusCode::OK,
            Json(serde_json::to_value(job).unwrap_or_else(|_| json!({}))),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "job_not_found", "job_id": job_id})),
        ),
    }
}

async fn ingest_list_handler(
    State(state): State<AppState>,
    Query(q): Query<IngestListQuery>,
) -> Json<Value> {
    let status = q.status.as_deref().and_then(|s| match s {
        "pending" => Some(IngestJobStatus::Pending),
        "fetching" => Some(IngestJobStatus::Fetching),
        "cleaning" => Some(IngestJobStatus::Cleaning),
        "judging" => Some(IngestJobStatus::Judging),
        "embedding" => Some(IngestJobStatus::Embedding),
        "saved" => Some(IngestJobStatus::Saved),
        "rejected" => Some(IngestJobStatus::Rejected),
        "failed" => Some(IngestJobStatus::Failed),
        _ => None,
    });
    let filter = ListFilter {
        status,
        limit: q.limit,
        only_failed: q.only_failed.unwrap_or(false),
    };
    let jobs = state.ingest_queue.list(&filter).await;
    let total = jobs.len();
    Json(json!({"jobs": jobs, "total": total}))
}

async fn rules_list_handler(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let snapshot = state.learned_rules.snapshot().await;
    Ok(Json(serde_json::json!({ "rules": snapshot })))
}

#[derive(serde::Deserialize)]
struct MarkStalePayload {
    host: String,
    reason: Option<String>,
}

async fn rules_mark_stale_handler(
    State(state): State<AppState>,
    Json(payload): Json<MarkStalePayload>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let reason = payload.reason.unwrap_or_else(|| "manual".to_string());
    state
        .learned_rules
        .mark_stale(&payload.host, &reason)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;
    Ok(Json(serde_json::json!({"status": "ok"})))
}

async fn rules_warmup_handler(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "warmup not implemented in Phase 2 v1; run articles ingest manually"
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha256_hex_matches_known_vector() {
        assert_eq!(
            hmac_sha256_hex("key", b"The quick brown fox jumps over the lazy dog"),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }
}
