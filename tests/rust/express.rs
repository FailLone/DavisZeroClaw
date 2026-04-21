use super::fixtures::{
    sample_config, sample_local_config_with_crawl4ai_base_url, sample_paths, sample_states,
};
use super::support::{
    sample_mcp_client, spawn_json_router, spawn_proxy_base_url_with_local_config, spawn_test_client,
};
use axum::routing::post;
use axum::{Json, Router};
use reqwest::Client;
use serde_json::{json, Value};

#[tokio::test]
async fn express_auth_status_route_reports_per_source_state() {
    let crawl4ai_router = Router::new()
        .route(
            "/crawl",
            post(|Json(payload): Json<Value>| async move {
                let url = payload
                    .get("urls")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let body = if url.contains("taobao.com") {
                    json!({
                        "results": [{
                            "url":"https://buyertrade.taobao.com/trade/itemlist/list_bought_items.htm",
                            "success":true,
                            "status_code":200,
                            "html":"<html></html>",
                            "js_execution_result":{
                                "source":"ali",
                                "status":"ok",
                                "checked_at":"2026-04-08T12:00:00Z",
                                "logged_in":true,
                                "package_count":0,
                                "current_url":"https://buyertrade.taobao.com/trade/itemlist/list_bought_items.htm",
                                "title":"已买到的宝贝",
                                "message":"ok",
                                "packages":[]
                            }
                        }]
                    })
                } else {
                    json!({
                        "results": [{
                            "url":"https://order.jd.com/center/list.action",
                            "success":true,
                            "status_code":200,
                            "html":"<html></html>",
                            "js_execution_result":{
                                "source":"jd",
                                "status":"needs_reauth",
                                "checked_at":"2026-04-08T12:00:00Z",
                                "logged_in":false,
                                "package_count":0,
                                "title":"京东-欢迎登录",
                                "message":"login required",
                                "issue_type":"auth_required",
                                "packages":[]
                            }
                        }]
                    })
                };
                Json(body)
            }),
        );
    let crawl4ai_base_url = spawn_json_router(crawl4ai_router).await;
    let local_config = sample_local_config_with_crawl4ai_base_url(&crawl4ai_base_url);
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
    assert_eq!(body.get("status").and_then(Value::as_str), Some("partial"));
    assert_eq!(
        body.get("sources")
            .and_then(Value::as_array)
            .map(|items| items.len()),
        Some(2)
    );
}

#[tokio::test]
async fn express_packages_route_aggregates_and_filters_results() {
    let crawl4ai_router = Router::new()
        .route(
            "/crawl",
            post(|Json(payload): Json<Value>| async move {
                let url = payload
                    .get("urls")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let body = if url.contains("taobao.com") {
                    json!({
                        "results": [{
                            "url":"https://buyertrade.taobao.com/trade/itemlist/list_bought_items.htm",
                            "success":true,
                            "status_code":200,
                            "html":"<html></html>",
                            "js_execution_result":{
                                "source":"ali",
                                "status":"ok",
                                "checked_at":"2026-04-08T12:00:00Z",
                                "logged_in":true,
                                "package_count":1,
                                "packages":[{"id":"ali-1","source":"ali","merchant":"taobao","title":"淘宝 蓝牙耳机","shop_name":"数码店","status":"运输中","latest_update":"快件正在运输途中","latest_time":"2026-04-08 10:00","carrier":"顺丰","tracking_no_masked":"SF****1234","raw_source_meta":{}}]
                            }
                        }]
                    })
                } else {
                    json!({
                        "results": [{
                            "url":"https://order.jd.com/center/list.action",
                            "success":true,
                            "status_code":200,
                            "html":"<html></html>",
                            "js_execution_result":{
                                "source":"jd",
                                "status":"ok",
                                "checked_at":"2026-04-08T12:00:00Z",
                                "logged_in":true,
                                "package_count":1,
                                "packages":[{"id":"jd-1","source":"jd","merchant":"jd","title":"京东 咖啡胶囊","shop_name":"京东自营","status":"待取件","latest_update":"已到驿站，请及时取件","latest_time":"2026-04-08 12:00","carrier":"京东快递","pickup_code_masked":"42**","raw_source_meta":{}}]
                            }
                        }]
                    })
                };
                Json(body)
            }),
        );
    let crawl4ai_base_url = spawn_json_router(crawl4ai_router).await;
    let local_config = sample_local_config_with_crawl4ai_base_url(&crawl4ai_base_url);
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
    assert_eq!(body.get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(body.get("package_count").and_then(Value::as_u64), Some(2));
    assert!(body
        .get("speech")
        .and_then(Value::as_str)
        .map(|text| text.contains("共找到 2 个包裹"))
        .unwrap_or(false));

    let filtered = Client::new()
        .get(format!("{base_url}/express/search?q=咖啡"))
        .send()
        .await
        .unwrap();
    assert_eq!(filtered.status(), reqwest::StatusCode::OK);
    let filtered_body: Value = filtered.json().await.unwrap();
    assert_eq!(
        filtered_body.get("package_count").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        filtered_body
            .get("packages")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .and_then(|item| item.get("source"))
            .and_then(Value::as_str),
        Some("jd")
    );
}
