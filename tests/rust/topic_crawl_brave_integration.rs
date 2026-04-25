//! Hits a local axum mock server that impersonates Brave's response shape.

use axum::{routing::get, Json, Router};
// The `article_memory` module is private at the crate root; use the
// lib-level re-exports (lib.rs) which expose both `BraveSearch` and the
// `SearchProvider` trait end-to-end.
use davis_zero_claw::{BraveSearch, SearchProvider};
use serde_json::json;

#[tokio::test]
async fn brave_roundtrip_happy_path() {
    let app = Router::new().route(
        "/res/v1/web/search",
        get(|| async {
            Json(json!({
                "web": {
                    "results": [
                        { "title": "A", "url": "https://a.com", "description": "desc" }
                    ]
                }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let brave = BraveSearch::with_endpoint(
        reqwest::Client::new(),
        "test-key".into(),
        format!("http://{addr}/res/v1/web/search"),
    );
    let hits = brave.search("rust", 5).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].url, "https://a.com");
}
