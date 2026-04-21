use super::fixtures::sample_local_config;
use crate::*;
use axum::extract::{Path, State};
use axum::http::{StatusCode as AxumStatusCode, Uri};
use axum::routing::{get, post};
use axum::{Json, Router};
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct TestServerState {
    pub(super) states: Arc<Vec<Value>>,
    pub(super) service_calls: Arc<AtomicUsize>,
}

pub(super) async fn test_states_handler(State(state): State<TestServerState>) -> Json<Value> {
    Json(Value::Array((*state.states).clone()))
}

pub(super) async fn test_service_handler(
    State(state): State<TestServerState>,
    Path((_domain, _service)): Path<(String, String)>,
    Json(_payload): Json<Value>,
) -> Json<Value> {
    state.service_calls.fetch_add(1, Ordering::Relaxed);
    Json(Value::Array(Vec::new()))
}

pub(super) async fn spawn_test_client(states: Vec<Value>) -> (HaClient, Arc<AtomicUsize>) {
    let service_calls = Arc::new(AtomicUsize::new(0));
    let state = TestServerState {
        states: Arc::new(states),
        service_calls: service_calls.clone(),
    };
    let app = Router::new()
        .route("/api/states", get(test_states_handler))
        .route("/api/services/:domain/:service", post(test_service_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        HaClient::from_parts(
            Client::builder().user_agent(USER_AGENT).build().unwrap(),
            format!("http://{addr}"),
            "test-token".to_string(),
        ),
        service_calls,
    )
}

pub(super) async fn spawn_upstream_client(router: Router) -> HaClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    HaClient::from_parts(
        Client::builder().user_agent(USER_AGENT).build().unwrap(),
        format!("http://{addr}"),
        "test-token".to_string(),
    )
}

pub(super) async fn spawn_upstream_mcp_client(router: Router) -> HaMcpClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    HaMcpClient::from_parts(
        Client::builder().user_agent(USER_AGENT).build().unwrap(),
        format!("http://{addr}/api/mcp"),
        "test-token".to_string(),
    )
}

pub(super) fn sample_mcp_client() -> HaMcpClient {
    HaMcpClient::from_parts(
        Client::builder().user_agent(USER_AGENT).build().unwrap(),
        "http://127.0.0.1:8123/api/mcp".to_string(),
        "test-token".to_string(),
    )
}

pub(super) async fn spawn_proxy_base_url(
    client: HaClient,
    mcp_client: HaMcpClient,
    paths: RuntimePaths,
    control_config: ControlConfig,
) -> String {
    let local_config = sample_local_config();
    spawn_proxy_base_url_with_local_config(client, mcp_client, paths, control_config, local_config)
        .await
}

pub(super) async fn spawn_proxy_base_url_with_local_config(
    client: HaClient,
    mcp_client: HaMcpClient,
    paths: RuntimePaths,
    control_config: ControlConfig,
    local_config: LocalConfig,
) -> String {
    let routing = ModelRoutingManager::for_tests(paths.clone(), local_config.clone());
    let app = build_app(AppState::new(
        client,
        mcp_client,
        paths,
        Arc::new(control_config),
        Arc::new(local_config.crawl4ai.clone()),
        Arc::new(local_config.article_memory.clone()),
        Arc::new(local_config.providers.clone()),
        routing,
        local_config.webhook.secret,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

pub(super) async fn spawn_json_router(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

pub(super) async fn mcp_handler(Json(payload): Json<Value>) -> Json<Value> {
    let id = payload.get("id").cloned().unwrap_or_else(|| json!(1));
    let method = payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let body = match method {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-03-26",
                "serverInfo": {"name": "home-assistant", "version": "1.26.0"}
            }
        }),
        "tools/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [
                    {"name": "GetLiveContext", "description": "Provides real-time information"},
                    {"name": "HassTurnOn", "description": "Turns on devices"}
                ]
            }
        }),
        "prompts/list" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "prompts": [
                    {"name": "Assist", "description": "Default prompt"}
                ]
            }
        }),
        "tools/call" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [
                    {"type": "text", "text": "{\"success\":true,\"result\":\"Live Context: test area\\n- names: 书房灯带\\n  domain: light\\n  state: 'off'\\n  areas: 书房\\n- names: shu fang san kai 2\\n  domain: switch\\n  state: unavailable\\n  areas: 书房\\n- names: shu fang san kai\\n  domain: switch\\n  state: 'off'\\n  areas: 书房\\n- names: 客厅主灯\\n  domain: light\\n  state: unavailable\\n  areas: 客厅\\n- names: 客厅主灯\\n  domain: switch\\n  state: 'off'\\n  areas: 客厅\\n- names: target_temperature_sensor\\n  domain: sensor\\n  state: '21'\"}"}
                ],
                "isError": false
            }
        }),
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32601,
                "message": "unknown method"
            }
        }),
    };
    Json(body)
}

pub(super) async fn auth_failed_states_handler() -> (AxumStatusCode, Json<Value>) {
    (AxumStatusCode::UNAUTHORIZED, Json(json!({"error":"nope"})))
}

pub(super) async fn audit_config_handler() -> Json<Value> {
    Json(json!({
        "components": ["recorder", "history", "logbook"]
    }))
}

pub(super) async fn audit_history_handler() -> Json<Value> {
    Json(json!([[]]))
}

pub(super) async fn audit_history_with_events_handler() -> Json<Value> {
    Json(json!([[{
        "last_changed": "2026-03-29T08:01:00Z",
        "state": "on",
        "attributes": {
            "friendly_name": "书房灯带",
            "source": "HomeKit Bridge"
        }
    }]]))
}

pub(super) async fn audit_logbook_handler(uri: Uri) -> Json<Value> {
    if uri.query().unwrap_or_default().contains("entity=") {
        Json(json!([]))
    } else {
        Json(json!([{
            "when": "2026-03-29T08:00:00Z",
            "name": "HomeKit",
            "message": "打开了父母间吊灯"
        }]))
    }
}

pub(super) async fn audit_logbook_with_events_handler(uri: Uri) -> Json<Value> {
    if uri.query().unwrap_or_default().contains("entity=") {
        Json(json!([{
            "when": "2026-03-29T08:02:00Z",
            "name": "HomeKit",
            "message": "打开了书房灯带",
            "entity_id": "light.study_strip",
            "context_entity_id": "light.study_strip",
            "context_state": "on",
            "context_user_id": "user-1"
        }]))
    } else {
        Json(json!([]))
    }
}
