mod article_memory_ingest_worker;
mod control;
mod crawl4ai;
mod express;
mod fixtures;
mod routes;
mod support;

#[cfg(test)]
mod advisor_reconciliation {
    use super::fixtures::{sample_config, sample_paths, sample_states};
    use super::support::{
        mcp_handler, spawn_proxy_base_url, spawn_test_client, spawn_upstream_mcp_client,
    };
    use axum::routing::post;
    use axum::Router;
    use reqwest::Client;
    use serde_json::Value;

    #[tokio::test]
    async fn config_report_exposes_replacement_candidates_in_advisor_layer() {
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
        assert!(body
            .get("suggestions")
            .and_then(|value| value.get("replacement_candidates"))
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false));
        assert!(body
            .get("suggestions")
            .and_then(|value| value.get("replacement_candidates"))
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("suggested_actions"))
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false));
        assert!(body
            .get("advanced_opportunities")
            .and_then(Value::as_array)
            .map(|items| items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("entity_reconciliation_review")
            }))
            .unwrap_or(false));
    }
}
