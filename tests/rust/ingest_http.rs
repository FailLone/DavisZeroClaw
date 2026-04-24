//! HTTP smoke tests for the `/article-memory/ingest` endpoints. Drive
//! `build_app` directly via `tower::ServiceExt::oneshot` — no TCP socket,
//! no live crawl4ai. The `IngestQueue` in `AppState` is real so submits
//! exercise URL validation, SSRF guards, and the persistence path.

use super::fixtures::{sample_config, sample_local_config};
use super::support::{sample_mcp_client, spawn_test_client};
use crate::{
    build_app, init_article_memory, AppState, Crawl4aiSupervisor, IngestQueue, RuntimePaths,
};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

async fn build_state_for_test() -> (AppState, TempDir) {
    let tmp = TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join("runtime"),
    };
    std::fs::create_dir_all(paths.runtime_dir.join("state")).unwrap();
    init_article_memory(&paths).unwrap();
    let local_config = sample_local_config();
    let (ha_client, _service_calls) = spawn_test_client(vec![]).await;
    let mcp_client = sample_mcp_client();
    let supervisor = Arc::new(Crawl4aiSupervisor::disabled(paths.clone()));
    let ingest_queue = Arc::new(IngestQueue::load_or_create(
        &paths,
        Arc::new(local_config.article_memory.ingest.clone()),
    ));
    let profile_locks: crate::Crawl4aiProfileLocks =
        std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let state = AppState::new(
        ha_client,
        mcp_client,
        paths,
        Arc::new(sample_config()),
        Arc::new(local_config.crawl4ai.clone()),
        supervisor,
        Arc::new(local_config.article_memory.clone()),
        Arc::new(local_config.providers.clone()),
        local_config.webhook.secret.clone(),
        profile_locks,
        ingest_queue,
    );
    (state, tmp)
}

#[tokio::test]
async fn post_ingest_returns_202_with_job_id() {
    let (state, _tmp) = build_state_for_test().await;
    let app = build_app(state);
    let body = serde_json::to_vec(&json!({"url": "https://zhihu.com/p/1"})).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/article-memory/ingest")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn post_ingest_invalid_url_returns_400() {
    let (state, _tmp) = build_state_for_test().await;
    let app = build_app(state);
    let body = serde_json::to_vec(&json!({"url": "not a url"})).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/article-memory/ingest")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_ingest_by_id_round_trip() {
    // Regression: axum 0.7 uses `:job_id`, not `{job_id}`. A literal
    // brace route silently never matches and every GET 404s even though
    // the job exists in the queue. Submit a job, read list, then GET by
    // id and expect 200.
    let (state, _tmp) = build_state_for_test().await;
    let app = build_app(state);

    let submit = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/article-memory/ingest")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"url": "https://example.com/p/1"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(submit.status(), StatusCode::ACCEPTED);
    let body_bytes = axum::body::to_bytes(submit.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let job_id = body["job_id"]
        .as_str()
        .expect("job_id in response")
        .to_string();

    let get = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/article-memory/ingest/{job_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
}

#[tokio::test]
async fn post_ingest_ssrf_returns_400() {
    let (state, _tmp) = build_state_for_test().await;
    let app = build_app(state);
    let body = serde_json::to_vec(&json!({"url": "http://127.0.0.1/"})).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/article-memory/ingest")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
