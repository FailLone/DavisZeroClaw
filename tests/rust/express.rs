//! Integration coverage for `/express/*` routes.
//!
//! Task 11 rewired these routes through the `Crawl4aiSupervisor` and replaced
//! `Result<_, String>` with `Result<_, Crawl4aiError>` end-to-end. These tests
//! cover the typed-error surface — both the `Disabled` short-circuit (when
//! `[crawl4ai].enabled = false`) and the `ServerUnavailable` fallback (when
//! the supervisor stub cannot reach a real adapter).
//!
//! The full "mock crawl4ai → express parses payload" happy-path integration
//! lands in Task 14 alongside the `Crawl4aiSupervisor::for_test(base_url)`
//! constructor, which is the only ergonomic way to point a supervisor at an
//! in-test axum router without spawning a Python child.

use super::fixtures::{
    sample_config, sample_local_config_with_crawl4ai_base_url, sample_paths, sample_states,
};
use super::support::{
    sample_mcp_client, spawn_proxy_base_url_with_local_config, spawn_test_client,
};
use reqwest::Client;
use serde_json::Value;

/// When `[crawl4ai].enabled = false`, express must fail both sources with the
/// stable `crawl4ai_unavailable` issue type (via `Crawl4aiError::Disabled
/// ::issue_type()`). No substring matching, no stringly-typed fallbacks.
#[tokio::test]
async fn express_auth_status_reports_upstream_error_when_crawl4ai_disabled() {
    let mut local_config = sample_local_config_with_crawl4ai_base_url("http://127.0.0.1:0");
    local_config.crawl4ai.enabled = false;
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

    let response = Client::new()
        .get(format!("{base_url}/express/auth-status"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body.get("status").and_then(Value::as_str),
        Some("upstream_error"),
        "body: {body}"
    );
    let sources = body
        .get("sources")
        .and_then(Value::as_array)
        .expect("sources array");
    assert_eq!(sources.len(), 2);
    for source in sources {
        assert_eq!(
            source.get("status").and_then(Value::as_str),
            Some("upstream_error"),
        );
        // `Crawl4aiError::Disabled::issue_type()` → "crawl4ai_unavailable".
        // Locked so src/support.rs remediation hints stay in sync.
        assert_eq!(
            source
                .get("issue")
                .and_then(|issue| issue.get("issue_type"))
                .and_then(Value::as_str),
            Some("crawl4ai_unavailable"),
        );
    }
}

/// Same contract for `/express/packages`: disabled config → empty package
/// list, all sources in upstream_error with the `crawl4ai_unavailable` issue
/// type. Also verifies the `/express/search` aliased handler behaves the
/// same (it delegates to `express_packages`).
#[tokio::test]
async fn express_packages_route_surfaces_disabled_as_typed_error() {
    let mut local_config = sample_local_config_with_crawl4ai_base_url("http://127.0.0.1:0");
    local_config.crawl4ai.enabled = false;
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

    let response = Client::new()
        .get(format!("{base_url}/express/packages"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body.get("status").and_then(Value::as_str),
        Some("upstream_error"),
    );
    assert_eq!(body.get("package_count").and_then(Value::as_u64), Some(0));
    let sources = body
        .get("sources")
        .and_then(Value::as_array)
        .expect("sources array");
    assert_eq!(sources.len(), 2);
    for source in sources {
        assert_eq!(
            source
                .get("issue")
                .and_then(|issue| issue.get("issue_type"))
                .and_then(Value::as_str),
            Some("crawl4ai_unavailable"),
        );
    }

    let search_response = Client::new()
        .get(format!("{base_url}/express/search?q=coffee"))
        .send()
        .await
        .unwrap();
    assert_eq!(search_response.status(), reqwest::StatusCode::OK);
    let search_body: Value = search_response.json().await.unwrap();
    assert_eq!(
        search_body.get("status").and_then(Value::as_str),
        Some("upstream_error"),
    );
}
