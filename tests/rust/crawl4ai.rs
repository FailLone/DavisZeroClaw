use super::support::{fake_paths, spawn_json_router};
use crate::{
    crawl4ai_crawl, express_auth_status, Crawl4aiConfig, Crawl4aiError, Crawl4aiPageRequest,
    Crawl4aiSupervisor,
};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Concurrency smoke test for the per-profile lock map owned by `AppState`.
///
/// We exercise the same primitive the production code uses (a nested
/// `tokio::sync::Mutex` map) to prove same-profile acquisitions serialize —
/// `max_seen == 1` for any number of concurrent acquirers. This test does
/// not depend on `crawl4ai_crawl`; its only job is to fail loudly if the
/// lock-map semantics ever regress.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_profile_calls_serialize_under_lock() {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::sync::Mutex;

    type LockMap = Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>;

    async fn acquire(map: LockMap, profile: &str) -> Arc<Mutex<()>> {
        let mut guard = map.lock().await;
        guard
            .entry(profile.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    let map: LockMap = Arc::new(Mutex::new(HashMap::new()));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..5 {
        let map = map.clone();
        let in_flight = in_flight.clone();
        let max_seen = max_seen.clone();
        handles.push(tokio::spawn(async move {
            let lock = acquire(map, "express-ali").await;
            let _guard = lock.lock().await;
            let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(cur, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            in_flight.fetch_sub(1, Ordering::SeqCst);
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(
        max_seen.load(Ordering::SeqCst),
        1,
        "concurrent same-profile calls were not serialized"
    );
}

/// Mock `/health` handler — always returns `{"status":"ok"}`. The real
/// adapter also echoes a `versions` map, but the supervisor's
/// `wait_until_healthy` only keys off the HTTP status so the minimal body
/// is enough to prove the wiring.
async fn mock_health_ok() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Mock `/crawl` handler — returns a `CrawlResponse`-shaped body whose
/// `html` field carries a `data-davis-express-payload` marker encoding a
/// structured express status for source `ali`. Mirrors the contract
/// `src/express.rs::extract_payload_from_html` parses. `status == "empty"`
/// proves a non-error happy path (logged in, zero packages).
async fn mock_crawl_ok_empty_ali(Json(body): Json<Value>) -> Json<Value> {
    let payload = json!({
        "source": "ali",
        "status": "empty",
        "checked_at": "2026-04-22T00:00:00Z",
        "logged_in": true,
        "package_count": 0,
        "current_url": body.get("url").and_then(Value::as_str).unwrap_or(""),
        "title": "mock",
        "message": "mock adapter",
        "packages": []
    });
    let marker = format!(
        "<div data-davis-express-payload=\"{}\"></div>",
        urlencoding::encode(&payload.to_string())
    );
    Json(json!({
        "success": true,
        "url": body.get("url"),
        "redirected_url": null,
        "status_code": 200,
        "html": marker,
        "cleaned_html": null,
        "js_execution_result": null,
        "error_message": null,
    }))
}

/// Mock `/crawl` handler that always returns a typed 503 body shaped like
/// the real adapter's `crawl4ai_unavailable` lifespan failure. Proves
/// `crawl4ai_crawl` maps adapter 503 → `Crawl4aiError::ServerUnavailable`
/// with the stable `crawl4ai_unavailable` issue type.
async fn mock_crawl_503() -> axum::response::Response {
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "detail": {
                "error": "crawl4ai_unavailable",
                "details": "import failed",
            }
        })),
    )
        .into_response()
}

/// Mock `/health` handler that always returns a structured 503 shaped like
/// a broken-venv adapter (`crawl4ai_import_failed`). Kept alongside the
/// other mock handlers at module scope for consistency; consumed by
/// `supervisor_surfaces_adapter_unhealthy_body_quickly`.
async fn mock_health_unhealthy() -> axum::response::Response {
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "status": "unhealthy",
            "reason": "crawl4ai_import_failed",
            "details": "ModuleNotFoundError: No module named 'crawl4ai'",
        })),
    )
        .into_response()
}

/// End-to-end happy path: mock adapter on an ephemeral port, supervisor
/// `for_test` pointed at it, `express_auth_status` fans out to both `ali`
/// and `jd`. Mock returns an `ali` payload; `jd` gets the same mock
/// (identical handler), so both should parse as `status == "empty"` — no
/// `upstream_error`, which would be the symptom of the supervisor wiring
/// regressing or the payload marker parser drifting.
///
/// Closes the Phase 1b coverage gap where the only happy-path test for
/// `/crawl` lived in a manual Python smoke script outside `cargo test`.
#[tokio::test]
async fn express_auth_status_flows_through_mock_supervisor() {
    let app = Router::new()
        .route("/health", get(mock_health_ok))
        .route("/crawl", post(mock_crawl_ok_empty_ali));
    let base_url = spawn_json_router(app).await;

    let tmp = tempfile::tempdir().unwrap();
    let paths = fake_paths(tmp.path());

    // cfg.base_url is unused on the request path — express routes through
    // supervisor.base_url(), not cfg. Keep enabled=true + a short timeout so
    // Crawl4aiError::Disabled can't mask a real regression.
    let cfg = Crawl4aiConfig {
        enabled: true,
        timeout_secs: 5,
        ..Crawl4aiConfig::default()
    };

    let supervisor = Arc::new(Crawl4aiSupervisor::for_test(paths.clone(), base_url));
    let locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>> = Arc::new(Mutex::new(HashMap::new()));

    let response = express_auth_status(paths, cfg, locks, supervisor).await;

    assert_eq!(response.sources.len(), 2, "both ali + jd expected");
    // Neither source should have `upstream_error` — that would mean the
    // supervisor → mock pipeline failed before reaching the parser. At
    // least one source must land on a happy-path status (`ok` / `empty`).
    for source in &response.sources {
        assert_ne!(
            source.status, "upstream_error",
            "source {} returned upstream_error: {:?}",
            source.source, source.message
        );
    }
    let happy = response
        .sources
        .iter()
        .any(|src| matches!(src.status.as_str(), "ok" | "empty"));
    assert!(
        happy,
        "no source reported ok/empty; full response = {response:?}"
    );
}

/// Proves adapter 503 → typed `ServerUnavailable` with the stable
/// `crawl4ai_unavailable` issue type. This string is consumed by
/// `src/support.rs` remediation hints; locking it here keeps the two in
/// sync so a future rename of the enum variant can't silently break the
/// user-facing error bucket.
#[tokio::test]
async fn crawl4ai_503_maps_to_server_unavailable() {
    let app = Router::new()
        .route("/health", get(mock_health_ok))
        .route("/crawl", post(mock_crawl_503));
    let base_url = spawn_json_router(app).await;

    let tmp = tempfile::tempdir().unwrap();
    let paths = fake_paths(tmp.path());

    // cfg.base_url is unused — crawl4ai_crawl reads supervisor.base_url().
    let cfg = Crawl4aiConfig {
        enabled: true,
        timeout_secs: 2,
        ..Crawl4aiConfig::default()
    };

    let supervisor = Crawl4aiSupervisor::for_test(paths.clone(), base_url);

    let err = crawl4ai_crawl(
        &paths,
        &cfg,
        &supervisor,
        Crawl4aiPageRequest {
            profile_name: "test".to_string(),
            url: "https://example.com".to_string(),
            wait_for: None,
            js_code: None,
            markdown: false,
            extract_engine: None,
            openrouter_config: None,
        },
    )
    .await
    .expect_err("503 adapter response must surface as Err");

    assert!(
        matches!(err, Crawl4aiError::ServerUnavailable { .. }),
        "expected ServerUnavailable, got {err:?}"
    );
    assert_eq!(err.issue_type(), "crawl4ai_unavailable");
}

/// Adapter up but `/health` returns 503 with a structured
/// `crawl4ai_import_failed` body. The supervisor's probe loop should bail
/// within the grace window with the body verbatim, *not* wait out the full
/// `STARTUP_TIMEOUT`. This pins down Task 16's contract: broken venvs fail
/// loudly and fast rather than silently timing out after 30 s.
///
/// Drives the `pub(crate)` `wait_until_healthy_with` helper with a 2 s
/// startup cap and a 200 ms grace so the test finishes in well under a
/// second even on slow CI. If the supervisor regressed to "poll until
/// timeout regardless of body," this test would run for ~2 s and assert on
/// the wrong error shape.
#[tokio::test]
async fn supervisor_surfaces_adapter_unhealthy_body_quickly() {
    let app = Router::new().route("/health", get(mock_health_unhealthy));
    let base_url = spawn_json_router(app).await;

    let tmp = tempfile::tempdir().unwrap();
    let paths = fake_paths(tmp.path());
    let supervisor = Crawl4aiSupervisor::for_test(paths, base_url);

    let started = Instant::now();
    let err = supervisor
        .wait_until_healthy_with(Duration::from_secs(2), Duration::from_millis(200))
        .await
        .expect_err("persistent 503 must surface as Err, not succeed");
    let elapsed = started.elapsed();

    // Must bail well inside the outer 2 s startup cap — otherwise the
    // short-circuit branch regressed to "poll until timeout."
    assert!(
        elapsed < Duration::from_millis(1500),
        "expected fast bail (<1.5s), actually took {elapsed:?}"
    );

    match err {
        Crawl4aiError::ServerUnavailable { details } => {
            assert!(
                details.contains("adapter reports unhealthy"),
                "expected the unhealthy-branch prefix, got: {details}"
            );
            assert!(
                details.contains("crawl4ai_import_failed"),
                "expected the adapter's reason verbatim, got: {details}"
            );
            assert!(
                details.contains("ModuleNotFoundError"),
                "expected the adapter's details verbatim, got: {details}"
            );
            assert!(
                !details.contains("did not become healthy"),
                "should NOT be the generic startup-timeout branch, got: {details}"
            );
        }
        other => panic!("expected ServerUnavailable, got {other:?}"),
    }
}
