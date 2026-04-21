use super::fixtures::{
    sample_config, sample_local_config_with_browser_port, sample_paths, sample_states,
};
use super::support::{
    sample_mcp_client, spawn_json_router, spawn_proxy_base_url_with_local_config, spawn_test_client,
};
use crate::init_article_memory;
use axum::routing::{get, post};
use axum::{Json, Router};
use reqwest::Client;
use serde_json::{json, Value};

#[tokio::test]
async fn browser_status_and_tabs_routes_proxy_worker_payloads() {
    let browser_router = Router::new()
        .route(
            "/status",
            get(|| async {
                Json(json!({
                    "status":"ok",
                    "checked_at":"2026-04-08T12:00:00Z",
                    "worker_available":true,
                    "worker_url":"http://127.0.0.1:4011",
                    "profiles":[
                        {"profile":"user","mode":"existing_session","browser":"chrome","status":"ok","writable":false,"fallback_in_use":true,"message":"fallback"},
                        {"profile":"managed","mode":"managed","browser":"chromium","status":"ok","writable":true,"fallback_in_use":false,"message":"managed ready"}
                    ],
                    "message":"browser worker ready"
                }))
            }),
        )
        .route(
            "/profiles",
            get(|| async {
                Json(json!({
                    "status":"ok",
                    "checked_at":"2026-04-08T12:00:00Z",
                    "default_profile":"user",
                    "profiles":[
                        {"profile":"user","mode":"existing_session","browser":"chrome","status":"ok","writable":false,"fallback_in_use":true,"message":"fallback"}
                    ]
                }))
            }),
        )
        .route(
            "/tabs",
            get(|| async {
                Json(json!({
                    "status":"ok",
                    "checked_at":"2026-04-08T12:00:00Z",
                    "profile":"user",
                    "tabs":[
                        {"tab_id":"w1:t1","profile":"user","active":true,"writable":false,"current_url":"https://example.com","title":"Example"}
                    ],
                    "message":"read tabs"
                }))
            }),
        );
    let browser_base_url = spawn_json_router(browser_router).await;
    let browser_port = browser_base_url
        .rsplit(':')
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let local_config = sample_local_config_with_browser_port(browser_port);
    let paths = sample_paths();
    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let base_url = spawn_proxy_base_url_with_local_config(
        upstream,
        sample_mcp_client(),
        paths,
        sample_config(),
        local_config,
    )
    .await;

    let status: Value = Client::new()
        .get(format!("{base_url}/browser/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status.get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(
        status
            .get("profiles")
            .and_then(Value::as_array)
            .map(|items| items.len()),
        Some(2)
    );

    let tabs: Value = Client::new()
        .get(format!("{base_url}/browser/tabs?profile=user"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(tabs.get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(
        tabs.get("tabs")
            .and_then(Value::as_array)
            .map(|items| items.len()),
        Some(1)
    );
}

#[tokio::test]
async fn browser_action_route_requires_confirmation_for_non_whitelisted_origin() {
    let browser_router = Router::new()
        .route(
            "/tabs",
            get(|| async {
                Json(json!({
                    "status":"ok",
                    "checked_at":"2026-04-08T12:00:00Z",
                    "profile":"user",
                    "tabs":[
                        {"tab_id":"w1:t1","profile":"user","active":true,"writable":false,"current_url":"https://unsafe.example.com/form","title":"Unsafe"}
                    ]
                }))
            }),
        )
        .route(
            "/action",
            post(|| async {
                Json(json!({
                    "status":"ok",
                    "checked_at":"2026-04-08T12:00:00Z"
                }))
            }),
        );
    let browser_base_url = spawn_json_router(browser_router).await;
    let browser_port = browser_base_url
        .rsplit(':')
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let mut local_config = sample_local_config_with_browser_port(browser_port);
    local_config.browser_bridge.write_policy.allowed_origins =
        vec!["https://buyertrade.taobao.com".to_string()];
    let paths = sample_paths();
    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let base_url = spawn_proxy_base_url_with_local_config(
        upstream,
        sample_mcp_client(),
        paths.clone(),
        sample_config(),
        local_config,
    )
    .await;

    let response: Value = Client::new()
        .post(format!("{base_url}/browser/action"))
        .json(&json!({
            "profile":"user",
            "tab_id":"w1:t1",
            "action":"click",
            "target":{"selector":"button.submit"},
            "payload":{}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        response.get("status").and_then(Value::as_str),
        Some("requires_confirmation")
    );
    assert!(paths.browser_confirmations_log_path().exists());
}

#[tokio::test]
async fn article_ingest_route_extracts_browser_page_into_candidate() {
    let browser_router = Router::new()
        .route(
            "/evaluate",
            post(|| async {
                Json(json!({
                    "status":"ok",
                    "checked_at":"2026-04-17T12:00:00Z",
                    "profile":"user",
                    "tab_id":"w1:t1",
                    "current_url":"https://example.com/agent-memory",
                    "title":"Agent Memory Notes",
                    "data": serde_json::to_string(&json!({
                        "title": "Agent Memory Notes",
                        "url": "https://example.com/agent-memory",
                        "language": "en",
                        "author": "Example Author",
                        "site_name": "Example",
                        "description": "Useful notes about agent memory.",
                        "extraction_selector": "article",
                        "content": "Agent memory systems need durable storage, semantic retrieval, and careful write boundaries."
                    })).unwrap()
                }))
            }),
        );
    let browser_base_url = spawn_json_router(browser_router).await;
    let browser_port = browser_base_url
        .rsplit(':')
        .next()
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let local_config = sample_local_config_with_browser_port(browser_port);
    let paths = sample_paths();
    init_article_memory(&paths).unwrap();
    let (upstream, _service_calls) = spawn_test_client(sample_states()).await;
    let base_url = spawn_proxy_base_url_with_local_config(
        upstream,
        sample_mcp_client(),
        paths,
        sample_config(),
        local_config,
    )
    .await;

    let response = Client::new()
        .post(format!("{base_url}/article-memory/ingest"))
        .json(&json!({
            "profile": "user",
            "tab_id": "w1:t1",
            "tags": ["agent", "memory"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body.get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(
        body.get("article")
            .and_then(|article| article.get("status"))
            .and_then(Value::as_str),
        Some("candidate")
    );
    assert_eq!(
        body.get("article")
            .and_then(|article| article.get("title"))
            .and_then(Value::as_str),
        Some("Agent Memory Notes")
    );
    assert_eq!(
        body.get("embedding_status").and_then(Value::as_str),
        Some("skipped_value_rejected")
    );
}
