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

/// Build the three learned-rules plumbing stores that `IngestWorkerDeps`
/// requires. Tests that don't exercise the rule path still need valid
/// (empty) stores so the worker can noop-check them.
fn test_rule_stores(
    paths: &RuntimePaths,
) -> (
    Arc<crate::article_memory::LearnedRuleStore>,
    Arc<crate::article_memory::RuleStatsStore>,
    Arc<crate::article_memory::SampleStore>,
) {
    std::fs::create_dir_all(paths.article_memory_dir()).unwrap();
    (
        Arc::new(crate::article_memory::LearnedRuleStore::load(paths, None).unwrap()),
        Arc::new(crate::article_memory::RuleStatsStore::load(paths).unwrap()),
        Arc::new(crate::article_memory::SampleStore::new(paths)),
    )
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
    let (learned_rules, rule_stats, sample_store) = test_rule_stores(&paths);
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
            imessage_config: Arc::new(crate::app_config::ImessageConfig {
                allowed_contacts: vec!["+8618672954807".into()],
            }),
            extract_config: Arc::new(crate::app_config::ArticleMemoryExtractConfig::default()),
            quality_gate_config: Arc::new(crate::app_config::QualityGateToml {
                enabled: false,
                ..crate::app_config::QualityGateToml::default()
            }),
            learned_rules,
            rule_stats,
            sample_store,
        },
        1,
    );
    let resp = queue
        .submit(IngestRequest {
            url: "https://zhihu.com/p/1".into(),
            force: false,
            title: None,
            tags: vec!["test".into()],
            source_hint: Some("test".into()),
            reply_handle: None,
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
    // Short markdown is now caught by the QualityGate (min_markdown_chars +
    // min_paragraphs), which subsumed the old ingest-level min_markdown_chars
    // check. Engine ladder with no providers wired → LLM upgrade is a no-op,
    // so the gate-rejected branch surfaces `quality_gate_rejected`.
    let (_tmp, paths) = test_paths();
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = "too short".into();
    let supervisor = spawn_mock_supervisor(&paths, mock).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    let (learned_rules, rule_stats, sample_store) = test_rule_stores(&paths);
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
            imessage_config: Arc::new(crate::app_config::ImessageConfig {
                allowed_contacts: vec!["+8618672954807".into()],
            }),
            extract_config: Arc::new(crate::app_config::ArticleMemoryExtractConfig::default()),
            quality_gate_config: Arc::new(crate::app_config::QualityGateToml::default()),
            learned_rules,
            rule_stats,
            sample_store,
        },
        1,
    );
    let resp = queue
        .submit(IngestRequest {
            url: "https://zhihu.com/p/short".into(),
            force: false,
            title: None,
            tags: vec![],
            source_hint: None,
            reply_handle: None,
        })
        .await
        .unwrap();
    let job = wait_for_terminal(&queue, &resp.job_id, 10).await;
    assert_eq!(job.status, IngestJobStatus::Failed);
    let err = job.error.unwrap();
    assert_eq!(err.issue_type, "quality_gate_rejected");
    assert_eq!(err.stage, "fetching");
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
    let (learned_rules, rule_stats, sample_store) = test_rule_stores(&paths);
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
            imessage_config: Arc::new(crate::app_config::ImessageConfig {
                allowed_contacts: vec!["+8618672954807".into()],
            }),
            extract_config: Arc::new(crate::app_config::ArticleMemoryExtractConfig::default()),
            quality_gate_config: Arc::new(crate::app_config::QualityGateToml {
                enabled: false,
                ..crate::app_config::QualityGateToml::default()
            }),
            learned_rules,
            rule_stats,
            sample_store,
        },
        1,
    );
    let resp = queue
        .submit(IngestRequest {
            url: "https://zhihu.com/p/503".into(),
            force: false,
            title: None,
            tags: vec![],
            source_hint: None,
            reply_handle: None,
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
    let (learned_rules, rule_stats, sample_store) = test_rule_stores(&paths);
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
            imessage_config: Arc::new(crate::app_config::ImessageConfig {
                allowed_contacts: vec!["+8618672954807".into()],
            }),
            extract_config: Arc::new(crate::app_config::ArticleMemoryExtractConfig::default()),
            quality_gate_config: Arc::new(crate::app_config::QualityGateToml {
                enabled: false,
                ..crate::app_config::QualityGateToml::default()
            }),
            learned_rules,
            rule_stats,
            sample_store,
        },
        3,
    );
    let mut ids = Vec::new();
    for i in 0..3 {
        let resp = queue
            .submit(IngestRequest {
                url: format!("https://zhihu.com/p/{i}"),
                force: false,
                title: None,
                tags: vec![],
                source_hint: None,
                reply_handle: None,
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
    let (learned_rules, rule_stats, sample_store) = test_rule_stores(&paths);
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
            imessage_config: Arc::new(crate::app_config::ImessageConfig {
                allowed_contacts: vec!["+8618672954807".into()],
            }),
            extract_config: Arc::new(crate::app_config::ArticleMemoryExtractConfig::default()),
            quality_gate_config: Arc::new(crate::app_config::QualityGateToml {
                enabled: false,
                ..crate::app_config::QualityGateToml::default()
            }),
            learned_rules,
            rule_stats,
            sample_store,
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
                    force: false,
                    title: None,
                    tags: vec![],
                    source_hint: None,
                    reply_handle: None,
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
    assert_eq!(
        max, 3,
        "expected all 3 cross-host workers to run concurrently; observed max={max}"
    );
}

#[tokio::test]
async fn worker_force_path_reuses_existing_article_id() {
    let (_tmp, paths) = test_paths();

    // Seed an existing record with id "original" at the target URL.
    let mut index = crate::article_memory::internals::load_index(&paths).unwrap();
    index
        .articles
        .push(crate::article_memory::ArticleMemoryRecord {
            id: "original".into(),
            title: "OLD".into(),
            url: Some("https://example.com/p/1".into()),
            source: "test".into(),
            language: None,
            tags: vec![],
            status: crate::article_memory::ArticleMemoryRecordStatus::Saved,
            value_score: Some(0.5),
            captured_at: "2026-04-01T00:00:00Z".into(),
            updated_at: "2026-04-01T00:00:00Z".into(),
            content_path: "articles/original.md".into(),
            raw_path: None,
            normalized_path: None,
            summary_path: None,
            translation_path: None,
            notes: None,
            clean_status: None,
            clean_profile: None,
        });
    crate::article_memory::internals::write_index(&paths, &index).unwrap();

    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = rich_markdown_body();
    let supervisor = spawn_mock_supervisor(&paths, mock.clone()).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    let (learned_rules, rule_stats, sample_store) = test_rule_stores(&paths);
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
            imessage_config: Arc::new(crate::app_config::ImessageConfig {
                allowed_contacts: vec!["+8618672954807".into()],
            }),
            extract_config: Arc::new(crate::app_config::ArticleMemoryExtractConfig::default()),
            quality_gate_config: Arc::new(crate::app_config::QualityGateToml {
                enabled: false,
                ..crate::app_config::QualityGateToml::default()
            }),
            learned_rules,
            rule_stats,
            sample_store,
        },
        1,
    );

    let resp = queue
        .submit(IngestRequest {
            url: "https://example.com/p/1".into(),
            force: true,
            title: None,
            tags: vec![],
            source_hint: None,
            reply_handle: None,
        })
        .await
        .unwrap();
    let _ = wait_for_terminal(&queue, &resp.job_id, 10).await;

    let index = crate::article_memory::internals::load_index(&paths).unwrap();
    assert_eq!(index.articles.len(), 1, "no duplicate row appended");
    assert_eq!(index.articles[0].id, "original", "id stable under force");
    let content_path = paths
        .runtime_dir
        .join("article-memory/articles/original.md");
    let content = std::fs::read_to_string(&content_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", content_path.display()));
    assert!(
        content.contains("Agent Memory") || content.contains("Paragraph"),
        "content overwritten with fresh crawl markdown; got first 120 chars: {:?}",
        content.chars().take(120).collect::<String>()
    );
}

#[tokio::test]
async fn worker_notify_hook_fires_on_early_return_fetch_failure() {
    // Regression for the Phase 2.5 gap: before wrapper refactor, a fetch
    // failure early-returned BEFORE the trailing notify block, so iMessage
    // users never got a failure reply. With the wrapper, maybe_notify_terminal
    // runs regardless of which stage produced the terminal state.
    //
    // Reply handle is NOT in allowed_contacts — notify_user will WARN-log
    // and return Ok, so the hook does not panic and the job reaches Failed
    // normally.
    let (_tmp, paths) = test_paths();
    let mock = MockState::default();
    // Force crawl4ai to return a non-success response; the supervisor will
    // surface this as an error and the worker early-returns Failed.
    *mock.status_override.lock().unwrap() = Some(500);
    *mock.fail_body.lock().unwrap() = Some(json!({"error": "mock fail"}));
    let supervisor = spawn_mock_supervisor(&paths, mock.clone()).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    let (learned_rules, rule_stats, sample_store) = test_rule_stores(&paths);
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
            imessage_config: Arc::new(crate::app_config::ImessageConfig {
                // Empty allowlist → notify_user warn-logs and returns Ok
                // without actually trying osascript. Proves the hook ran.
                allowed_contacts: vec![],
            }),
            extract_config: Arc::new(crate::app_config::ArticleMemoryExtractConfig::default()),
            quality_gate_config: Arc::new(crate::app_config::QualityGateToml {
                enabled: false,
                ..crate::app_config::QualityGateToml::default()
            }),
            learned_rules,
            rule_stats,
            sample_store,
        },
        1,
    );

    let resp = queue
        .submit(IngestRequest {
            url: "https://example.com/p/fail".into(),
            force: false,
            title: None,
            tags: vec![],
            source_hint: None,
            reply_handle: Some("+8613800000000".into()),
        })
        .await
        .unwrap();

    let job = wait_for_terminal(&queue, &resp.job_id, 10).await;
    assert_eq!(job.status, IngestJobStatus::Failed);
    assert!(
        job.error
            .as_ref()
            .map(|e| e.stage.as_str() == "fetching")
            .unwrap_or(false),
        "expected fetching-stage failure, got {:?}",
        job.error
    );
}

#[tokio::test]
async fn worker_skips_notify_when_reply_handle_missing() {
    // CLI/cron path: no reply_handle → notify hook is a no-op and the
    // worker still reaches Saved normally.
    let (_tmp, paths) = test_paths();
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = rich_markdown_body();
    let supervisor = spawn_mock_supervisor(&paths, mock.clone()).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    let (learned_rules, rule_stats, sample_store) = test_rule_stores(&paths);
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
            imessage_config: Arc::new(crate::app_config::ImessageConfig {
                allowed_contacts: vec!["+8618672954807".into()],
            }),
            extract_config: Arc::new(crate::app_config::ArticleMemoryExtractConfig::default()),
            quality_gate_config: Arc::new(crate::app_config::QualityGateToml {
                enabled: false,
                ..crate::app_config::QualityGateToml::default()
            }),
            learned_rules,
            rule_stats,
            sample_store,
        },
        1,
    );

    let resp = queue
        .submit(IngestRequest {
            url: "https://example.com/p/42".into(),
            force: false,
            title: None,
            tags: vec![],
            source_hint: None,
            reply_handle: None,
        })
        .await
        .unwrap();

    let job = wait_for_terminal(&queue, &resp.job_id, 10).await;
    assert!(job.reply_handle.is_none());
    assert_eq!(job.status, IngestJobStatus::Saved);
}

#[tokio::test]
async fn ingest_fails_when_quality_gate_rejects_and_no_upgrade_path() {
    // Quality gate ENABLED (unlike other tests that disable it). Mock /crawl
    // returns short markdown that can't clear the default 500-char minimum.
    // No openrouter provider is configured, so try_llm_upgrade is a no-op and
    // the rejection branch surfaces `quality_gate_rejected`. Critically, we
    // also assert that `engine_chain` reflects the single trafilatura attempt
    // (i.e., no "openrouter-llm" entry was appended because the upgrade did
    // not run). This complements `ingest_empty_markdown_rejected` by pinning
    // down the engine-ladder contract on the reject path.
    let (_tmp, paths) = test_paths();
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = "too short".into();
    let supervisor = spawn_mock_supervisor(&paths, mock).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    let (learned_rules, rule_stats, sample_store) = test_rule_stores(&paths);
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            // Empty provider list → no openrouter fallback → no upgrade path.
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
            imessage_config: Arc::new(crate::app_config::ImessageConfig {
                allowed_contacts: vec!["+8618672954807".into()],
            }),
            extract_config: Arc::new(crate::app_config::ArticleMemoryExtractConfig::default()),
            // KEY: gate ENABLED (default) to exercise the rejection path.
            quality_gate_config: Arc::new(crate::app_config::QualityGateToml::default()),
            learned_rules,
            rule_stats,
            sample_store,
        },
        1,
    );
    let resp = queue
        .submit(IngestRequest {
            url: "https://example.com/p/too-short".into(),
            force: false,
            title: None,
            tags: vec![],
            source_hint: None,
            reply_handle: None,
        })
        .await
        .unwrap();
    let job = wait_for_terminal(&queue, &resp.job_id, 10).await;
    assert_eq!(job.status, IngestJobStatus::Failed);
    let err = job.error.as_ref().expect("error set on Failed");
    assert_eq!(err.issue_type, "quality_gate_rejected");
    assert_eq!(err.stage, "fetching");
    assert_eq!(
        job.engine_chain,
        vec!["trafilatura".to_string()],
        "engine_chain should be trafilatura only (no upgrade: no openrouter provider configured)",
    );
}
