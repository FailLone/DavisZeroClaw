//! End-to-end check that `GET /article-memory/digest` is wired into an axum
//! `Router` and returns a 200 + valid JSON body. The per-record filtering and
//! ranking logic already has dedicated unit tests inside `server_digest.rs`;
//! this test's only job is to prove the *route + query-string + JSON
//! serialization* path works through a live tower service.
//!
//! `handle` takes `State<AppState>`, which would require stitching together a
//! full production state (HA clients, crawl4ai supervisor, ingest queue, etc.)
//! just to exercise a read-only route. `server_digest::router_for_tests`
//! sidesteps that by exposing the same handler shape bound to `RuntimePaths`
//! instead. The handler still goes through `build_digest`, so the code path
//! under test is identical apart from the state type.
//!
//! We seed an empty index on disk so the handler doesn't fall back to the
//! "index missing → empty" branch; that way the test also asserts the
//! load-from-disk path works.
//!
//! Drives the router via `tower::ServiceExt::oneshot`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use davis_zero_claw::server_digest::router_for_tests;
use davis_zero_claw::{init_article_memory, load_article_index, save_article_index, RuntimePaths};
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
async fn digest_endpoint_returns_200_with_empty_index() {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join("runtime"),
    };

    // Bootstrap + re-save an empty index so both file paths exist on disk.
    init_article_memory(&paths).unwrap();
    let index = load_article_index(&paths).unwrap();
    save_article_index(&paths, &index).unwrap();

    let app = router_for_tests(paths);

    let req = Request::builder()
        .uri("/article-memory/digest?since_days=7&top=5")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // The JSON shape is fully covered by unit tests in `server_digest.rs`.
    // Here we only confirm that the bytes deserialize and carry the
    // query-string parameters we passed in — which proves both the axum
    // `Query` extractor and the `Json` responder are wired correctly.
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["window_days"], 7);
    assert_eq!(body["total"], 0);
    assert!(body["top"].is_array());
    assert!(body["recent_translations"].is_array());
}

#[tokio::test]
async fn digest_endpoint_accepts_topic_query_param() {
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join("runtime"),
    };
    init_article_memory(&paths).unwrap();

    let app = router_for_tests(paths);

    let req = Request::builder()
        .uri("/article-memory/digest?topic=rust&since_days=14&top=3")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["topic"], "rust");
    assert_eq!(body["window_days"], 14);
}
