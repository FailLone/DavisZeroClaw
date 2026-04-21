use super::fixtures::{sample_config, sample_paths, sample_states};
use super::support::{
    audit_config_handler, audit_history_handler, audit_logbook_handler, auth_failed_states_handler,
    mcp_handler, sample_mcp_client, spawn_proxy_base_url, spawn_test_client, spawn_upstream_client,
    spawn_upstream_mcp_client, test_states_handler, TestServerState,
};
use crate::*;
use axum::routing::{get, post};
use axum::Router;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

#[tokio::test]
async fn health_route_reports_local_proxy_service() {
    let upstream = spawn_upstream_client(Router::new()).await;
    let base_url = spawn_proxy_base_url(
        upstream,
        sample_mcp_client(),
        sample_paths(),
        sample_config(),
    )
    .await;
    let response = Client::new()
        .get(format!("{base_url}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body.get("service").and_then(Value::as_str),
        Some("davis_local_proxy")
    );
    assert!(body
        .get("features")
        .and_then(Value::as_array)
        .is_some_and(|features| features.iter().any(|feature| feature == "browser_status")));
}

#[tokio::test]
async fn execute_control_route_accepts_json_body_without_content_type() {
    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let base_url = spawn_proxy_base_url(
        upstream,
        sample_mcp_client(),
        sample_paths(),
        sample_config(),
    )
    .await;
    let response = Client::new()
        .post(format!("{base_url}/execute-control"))
        .body(r#"{"query":"书房灯带","action":"turn_on"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body.get("status").and_then(Value::as_str), Some("success"));
}

#[tokio::test]
async fn resolve_control_route_returns_bad_request_reason() {
    let upstream = spawn_upstream_client(Router::new()).await;
    let base_url = spawn_proxy_base_url(
        upstream,
        sample_mcp_client(),
        sample_paths(),
        sample_config(),
    )
    .await;
    let response = Client::new()
        .get(format!("{base_url}/resolve-control-target"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body.get("reason").and_then(Value::as_str),
        Some("bad_request")
    );
}

#[tokio::test]
async fn resolve_entity_route_returns_typed_ok_payload() {
    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let base_url = spawn_proxy_base_url(
        upstream,
        sample_mcp_client(),
        sample_paths(),
        sample_config(),
    )
    .await;
    let response = Client::new()
        .get(format!("{base_url}/resolve-entity?entity_id=书房灯带"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: ResolveEntityPayload = response.json().await.unwrap();
    assert_eq!(body.status, "ok");
    assert_eq!(body.query_entity, "书房灯带");
    assert_eq!(
        body.resolved_entity_id.as_deref(),
        Some("light.study_strip")
    );
    assert_eq!(body.friendly_name.as_deref(), Some("书房灯带"));
    assert_eq!(body.domain.as_deref(), Some("light"));
}

#[tokio::test]
async fn resolve_control_route_maps_auth_failure_to_reason() {
    let upstream =
        spawn_upstream_client(Router::new().route("/api/states", get(auth_failed_states_handler)))
            .await;
    let base_url = spawn_proxy_base_url(
        upstream,
        sample_mcp_client(),
        sample_paths(),
        sample_config(),
    )
    .await;
    let response = Client::new()
        .get(format!(
            "{base_url}/resolve-control-target?query=书房灯带&action=turn_on"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body.get("reason").and_then(Value::as_str),
        Some("ha_auth_failed")
    );
}

#[tokio::test]
async fn execute_control_route_exposes_ambiguous_reason() {
    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let base_url = spawn_proxy_base_url(
        upstream,
        sample_mcp_client(),
        sample_paths(),
        sample_config(),
    )
    .await;
    let response = Client::new()
        .post(format!("{base_url}/execute-control"))
        .json(&json!({
            "query": "父母间吊灯",
            "action": "turn_on"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body.get("status").and_then(Value::as_str), Some("failed"));
    assert_eq!(
        body.get("reason").and_then(Value::as_str),
        Some("resolution_ambiguous")
    );
}

#[tokio::test]
async fn audit_route_returns_bad_request_reason() {
    let upstream = spawn_upstream_client(Router::new()).await;
    let base_url = spawn_proxy_base_url(
        upstream,
        sample_mcp_client(),
        sample_paths(),
        sample_config(),
    )
    .await;
    let response = Client::new()
        .get(format!("{base_url}/audit"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body.get("result_type").and_then(Value::as_str),
        Some("config_issue")
    );
    assert_eq!(
        body.get("reason").and_then(Value::as_str),
        Some("bad_request")
    );
}

#[tokio::test]
async fn audit_route_returns_no_evidence_payload() {
    let upstream = spawn_upstream_client(
        Router::new()
            .route("/api/config", get(audit_config_handler))
            .route("/api/states", get(test_states_handler))
            .route("/api/history/period/:start", get(audit_history_handler))
            .route("/api/logbook/:start", get(audit_logbook_handler))
            .with_state(TestServerState {
                states: Arc::new(sample_states()),
                service_calls: Arc::new(AtomicUsize::new(0)),
            }),
    )
    .await;
    let base_url = spawn_proxy_base_url(
        upstream,
        sample_mcp_client(),
        sample_paths(),
        sample_config(),
    )
    .await;
    let response = Client::new()
        .get(format!(
            "{base_url}/audit?entity_id=switch.parents_chandelier&start=2026-03-29T00:00:00Z&end=2026-03-29T23:59:59Z"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body.get("result_type").and_then(Value::as_str),
        Some("no_evidence")
    );
}

#[tokio::test]
async fn config_report_route_returns_ok_payload() {
    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let base_url = spawn_proxy_base_url(
        upstream,
        sample_mcp_client(),
        sample_paths(),
        sample_config(),
    )
    .await;
    let response = Client::new()
        .get(format!("{base_url}/advisor/config-report"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body.get("status").and_then(Value::as_str), Some("ok"));
    assert!(body.get("counts").is_some());
}

#[tokio::test]
async fn config_report_route_includes_mcp_live_context_when_available() {
    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let mcp_client =
        spawn_upstream_mcp_client(Router::new().route("/api/mcp", post(mcp_handler))).await;
    let base_url =
        spawn_proxy_base_url(upstream, mcp_client, sample_paths(), sample_config()).await;
    let response = Client::new()
        .get(format!("{base_url}/advisor/config-report"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body.get("ha_mcp_live_context")
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str),
        Some("ok")
    );
    assert_eq!(
        body.get("ha_mcp_live_context")
            .and_then(|value| value.get("source_tool"))
            .and_then(Value::as_str),
        Some("GetLiveContext")
    );
    assert!(body
        .get("ha_mcp_live_context")
        .and_then(|value| value.get("findings"))
        .and_then(|value| value.get("bad_names"))
        .and_then(Value::as_array)
        .map(|items| !items.is_empty())
        .unwrap_or(false));
    assert!(body
        .get("ha_mcp_live_context")
        .and_then(|value| value.get("findings"))
        .and_then(|value| value.get("possible_replacements"))
        .and_then(Value::as_array)
        .map(|items| !items.is_empty())
        .unwrap_or(false));
    assert!(body
        .get("ha_mcp_live_context")
        .and_then(|value| value.get("findings"))
        .and_then(|value| value.get("exposed_cross_domain_conflicts"))
        .and_then(Value::as_array)
        .map(|items| !items.is_empty())
        .unwrap_or(false));
    assert!(body
        .get("ha_mcp_live_context")
        .and_then(|value| value.get("findings"))
        .and_then(|value| value.get("missing_area_exposure"))
        .and_then(Value::as_array)
        .map(|items| !items.is_empty())
        .unwrap_or(false));
}

#[tokio::test]
async fn config_report_route_maps_auth_failure_to_reason() {
    let upstream =
        spawn_upstream_client(Router::new().route("/api/states", get(auth_failed_states_handler)))
            .await;
    let base_url = spawn_proxy_base_url(
        upstream,
        sample_mcp_client(),
        sample_paths(),
        sample_config(),
    )
    .await;
    let response = Client::new()
        .get(format!("{base_url}/advisor/config-report"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body.get("reason").and_then(Value::as_str),
        Some("ha_auth_failed")
    );
}

#[tokio::test]
async fn replacement_candidates_route_returns_structured_candidates() {
    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let mcp_client =
        spawn_upstream_mcp_client(Router::new().route("/api/mcp", post(mcp_handler))).await;
    let base_url =
        spawn_proxy_base_url(upstream, mcp_client, sample_paths(), sample_config()).await;
    let response = Client::new()
        .get(format!("{base_url}/advisor/replacement-candidates"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body.get("status").and_then(Value::as_str), Some("ok"));
    assert!(body
        .get("candidate_count")
        .and_then(Value::as_u64)
        .map(|count| count >= 1)
        .unwrap_or(false));
    assert!(body
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("suggested_actions"))
        .and_then(Value::as_array)
        .map(|items| !items.is_empty())
        .unwrap_or(false));
}

#[tokio::test]
async fn zeroclaw_runtime_traces_route_returns_recent_entries() {
    let paths = sample_paths();
    std::fs::write(
        paths.zeroclaw_runtime_trace_path(),
        concat!(
            "{\"event\":\"tool_call_start\",\"tool\":\"http_request\"}\n",
            "{\"event\":\"provider_fallback\",\"provider\":\"deepseek\"}\n",
            "{\"event\":\"tool_call_result\",\"status\":\"error\"}\n"
        ),
    )
    .unwrap();

    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let base_url =
        spawn_proxy_base_url(upstream, sample_mcp_client(), paths, sample_config()).await;
    let response = Client::new()
        .get(format!("{base_url}/zeroclaw/runtime-traces?limit=2"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body.get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(body.get("returned").and_then(Value::as_u64), Some(2));
    assert_eq!(body.get("total_entries").and_then(Value::as_u64), Some(3));
    assert_eq!(
        body.get("entries")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|value| value.get("event"))
            .and_then(Value::as_str),
        Some("provider_fallback")
    );
    assert_eq!(
        body.get("entries")
            .and_then(Value::as_array)
            .and_then(|items| items.get(1))
            .and_then(|value| value.get("event"))
            .and_then(Value::as_str),
        Some("tool_call_result")
    );
}

#[tokio::test]
async fn article_memory_routes_store_and_search_records() {
    let paths = sample_paths();
    init_article_memory(&paths).unwrap();
    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let base_url =
        spawn_proxy_base_url(upstream, sample_mcp_client(), paths, sample_config()).await;

    let add_response = Client::new()
        .post(format!("{base_url}/article-memory/articles"))
        .json(&json!({
            "title": "Agent memory field notes",
            "url": "https://example.com/agent-memory",
            "source": "manual",
            "language": "en",
            "tags": ["agent", "memory"],
            "content": "Durable memory helps agents keep useful research context.",
            "summary": "A practical note about durable memory.",
            "status": "saved",
            "value_score": 0.8
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(add_response.status(), reqwest::StatusCode::CREATED);

    let search_response = Client::new()
        .get(format!("{base_url}/article-memory/search?q=durable"))
        .send()
        .await
        .unwrap();
    assert_eq!(search_response.status(), reqwest::StatusCode::OK);
    let body: Value = search_response.json().await.unwrap();
    assert_eq!(body.get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(body.get("total_hits").and_then(Value::as_u64), Some(1));
    assert_eq!(
        body.get("hits")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|hit| hit.get("title"))
            .and_then(Value::as_str),
        Some("Agent memory field notes")
    );
}
