//! End-to-end ingest worker tests. Uses a mock crawl4ai axum router spun up
//! through `Crawl4aiSupervisor::for_test` to avoid starting a real Python
//! adapter.
//!
//! These tests exercise `IngestWorkerPool` + `execute_job` without going
//! through `AppState`, proving the worker-pool wiring is correct in
//! isolation. Concurrency invariants are asserted via `max_in_flight`
//! atomic counters rather than wall-clock timing (see spec §11.3).

use crate::article_memory::{
    IngestJob, IngestJobStatus, IngestQueue, IngestRequest, IngestWorkerDeps, IngestWorkerPool,
};
use crate::{
    init_article_memory, ArticleMemoryConfig, ArticleMemoryHostProfile, ArticleMemoryIngestConfig,
    Crawl4aiConfig, Crawl4aiSupervisor, RuntimePaths,
};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct MockState {
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    markdown_body: Arc<std::sync::Mutex<String>>,
    status_override: Arc<std::sync::Mutex<Option<u16>>>,
    fail_body: Arc<std::sync::Mutex<Option<Value>>>,
    per_host_delay_ms: Arc<std::sync::Mutex<HashMap<String, u64>>>,
}

async fn mock_health_ok() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn mock_crawl(
    State(state): State<MockState>,
    Json(payload): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if let Some(code) = *state.status_override.lock().unwrap() {
        let body = state.fail_body.lock().unwrap().clone().unwrap_or(json!({}));
        return (StatusCode::from_u16(code).unwrap(), Json(body));
    }
    let in_flight = state.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    state.max_in_flight.fetch_max(in_flight, Ordering::SeqCst);
    let url = payload
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();
    let delay = state
        .per_host_delay_ms
        .lock()
        .unwrap()
        .get(&host)
        .copied()
        .unwrap_or(50);
    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    let markdown = state.markdown_body.lock().unwrap().clone();
    state.in_flight.fetch_sub(1, Ordering::SeqCst);
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "current_url": url,
            "html": "<html><body>mock</body></html>",
            "cleaned_html": "<body>mock</body>",
            "markdown": markdown,
            "error_message": null,
            "status_code": 200,
            "metadata": { "title": "Mock Title" },
        })),
    )
}

fn test_paths() -> (TempDir, RuntimePaths) {
    let tmp = TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join(".runtime").join("davis"),
    };
    std::fs::create_dir_all(paths.runtime_dir.join("state")).unwrap();
    init_article_memory(&paths).unwrap();
    (tmp, paths)
}

fn test_crawl4ai_config() -> Arc<Crawl4aiConfig> {
    Arc::new(Crawl4aiConfig {
        enabled: true,
        base_url: "http://127.0.0.1:0".into(),
        timeout_secs: 30,
        headless: true,
        magic: false,
        simulate_user: false,
        override_navigator: false,
        remove_overlay_elements: true,
        enable_stealth: false,
    })
}

async fn spawn_mock_supervisor(paths: &RuntimePaths, state: MockState) -> Arc<Crawl4aiSupervisor> {
    let app = Router::new()
        .route("/health", get(mock_health_ok))
        .route("/crawl", post(mock_crawl))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Arc::new(Crawl4aiSupervisor::for_test(
        paths.clone(),
        format!("http://{addr}"),
    ))
}

fn default_ingest_cfg() -> Arc<ArticleMemoryIngestConfig> {
    Arc::new(ArticleMemoryIngestConfig {
        enabled: true,
        max_concurrency: 3,
        min_markdown_chars: 100,
        host_profiles: vec![
            ArticleMemoryHostProfile {
                match_suffix: "zhihu.com".into(),
                profile: "articles-zhihu".into(),
                source: Some("zhihu".into()),
            },
            ArticleMemoryHostProfile {
                match_suffix: "example.com".into(),
                profile: "articles-example".into(),
                source: Some("example".into()),
            },
            ArticleMemoryHostProfile {
                match_suffix: "medium.com".into(),
                profile: "articles-medium".into(),
                source: Some("medium".into()),
            },
        ],
        ..Default::default()
    })
}

fn default_article_memory_cfg() -> Arc<ArticleMemoryConfig> {
    Arc::new(ArticleMemoryConfig::default())
}

/// Body containing target-topic keywords so the deterministic value prefilter
/// lands on `candidate`/`save` (not `reject`). Target topics are configured
/// in the built-in `article_memory.toml` and include "agent", "memory",
/// "MCP", etc. — using several lets the score clear `candidate_threshold`
/// without needing to flip any LLM switches in the test harness.
///
/// The normalizer dedupes short repeated lines (<80 chars), so we generate
/// distinct sentences long enough that both the kept_ratio and
/// min_normalized_chars thresholds pass and the value prefilter sees real
/// content rather than "fallback_raw".
fn rich_markdown_body() -> String {
    let mut paragraphs = String::from("# Agent Memory and MCP\n\n");
    for i in 0..25 {
        paragraphs.push_str(&format!(
            "Paragraph {i}: This section explores how an AI agent manages long-term \
             memory across sessions, including persistence strategies, vector indexes, \
             and MCP tool integration patterns that keep context coherent over time. \
             The agent's memory subsystem (iteration {i}) coordinates retrieval, \
             deduplication, and summarization across heterogeneous sources.\n\n"
        ));
    }
    paragraphs
}

async fn wait_for_terminal(queue: &IngestQueue, job_id: &str, timeout_secs: u64) -> IngestJob {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if let Some(job) = queue.get(job_id).await {
            if job.status.is_terminal() {
                return job;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let j = queue.get(job_id).await;
            panic!("timed out waiting for terminal state, last = {j:?}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn ingest_happy_path_end_to_end() {
    let (_tmp, paths) = test_paths();
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = rich_markdown_body();
    let supervisor = spawn_mock_supervisor(&paths, mock.clone()).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
        },
        1,
    );
    let resp = queue
        .submit(IngestRequest {
            url: "https://zhihu.com/p/1".into(),
            title: None,
            tags: vec!["test".into()],
            source_hint: Some("test".into()),
        })
        .await
        .unwrap();
    let job = wait_for_terminal(&queue, &resp.job_id, 10).await;
    assert_eq!(
        job.status,
        IngestJobStatus::Saved,
        "expected Saved, got {:?} (error={:?})",
        job.status,
        job.error
    );
    assert!(job.article_id.is_some());
}

#[tokio::test]
async fn ingest_empty_markdown_rejected() {
    let (_tmp, paths) = test_paths();
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = "too short".into();
    let supervisor = spawn_mock_supervisor(&paths, mock).await;
    let ingest_cfg = Arc::new(ArticleMemoryIngestConfig {
        min_markdown_chars: 600,
        ..(*default_ingest_cfg()).clone()
    });
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
        },
        1,
    );
    let resp = queue
        .submit(IngestRequest {
            url: "https://zhihu.com/p/short".into(),
            title: None,
            tags: vec![],
            source_hint: None,
        })
        .await
        .unwrap();
    let job = wait_for_terminal(&queue, &resp.job_id, 10).await;
    assert_eq!(job.status, IngestJobStatus::Failed);
    assert_eq!(job.error.unwrap().issue_type, "empty_content");
}

#[tokio::test]
async fn ingest_crawl_server_error_surfaces_issue_type() {
    let (_tmp, paths) = test_paths();
    let mock = MockState::default();
    *mock.status_override.lock().unwrap() = Some(503);
    *mock.fail_body.lock().unwrap() = Some(json!({
        "detail": { "error": "crawl4ai_unavailable", "details": "upstream sad" }
    }));
    let supervisor = spawn_mock_supervisor(&paths, mock).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
        },
        1,
    );
    let resp = queue
        .submit(IngestRequest {
            url: "https://zhihu.com/p/503".into(),
            title: None,
            tags: vec![],
            source_hint: None,
        })
        .await
        .unwrap();
    let job = wait_for_terminal(&queue, &resp.job_id, 10).await;
    assert_eq!(job.status, IngestJobStatus::Failed);
    assert_eq!(job.error.unwrap().issue_type, "crawl4ai_unavailable");
}

#[tokio::test]
async fn ingest_same_host_serializes() {
    let (_tmp, paths) = test_paths();
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = rich_markdown_body();
    mock.per_host_delay_ms
        .lock()
        .unwrap()
        .insert("zhihu.com".into(), 150);
    let supervisor = spawn_mock_supervisor(&paths, mock.clone()).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
        },
        3,
    );
    let mut ids = Vec::new();
    for i in 0..3 {
        let resp = queue
            .submit(IngestRequest {
                url: format!("https://zhihu.com/p/{i}"),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap();
        ids.push(resp.job_id);
    }
    for id in &ids {
        let _ = wait_for_terminal(&queue, id, 15).await;
    }
    let max = mock.max_in_flight.load(Ordering::SeqCst);
    assert_eq!(max, 1, "same-host ingests must serialize via profile lock");
}

#[tokio::test]
async fn ingest_different_hosts_parallelize() {
    let (_tmp, paths) = test_paths();
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = rich_markdown_body();
    mock.per_host_delay_ms
        .lock()
        .unwrap()
        .insert("zhihu.com".into(), 200);
    mock.per_host_delay_ms
        .lock()
        .unwrap()
        .insert("example.com".into(), 200);
    mock.per_host_delay_ms
        .lock()
        .unwrap()
        .insert("medium.com".into(), 200);
    let supervisor = spawn_mock_supervisor(&paths, mock.clone()).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
        },
        3,
    );
    let urls = [
        "https://zhihu.com/p/1",
        "https://example.com/p/1",
        "https://medium.com/p/1",
    ];
    let mut ids = Vec::new();
    for u in urls {
        ids.push(
            queue
                .submit(IngestRequest {
                    url: u.into(),
                    title: None,
                    tags: vec![],
                    source_hint: None,
                })
                .await
                .unwrap()
                .job_id,
        );
    }
    for id in &ids {
        let _ = wait_for_terminal(&queue, id, 15).await;
    }
    let max = mock.max_in_flight.load(Ordering::SeqCst);
    assert!(
        max >= 2,
        "cross-host ingests must parallelize; observed max={max}"
    );
}
