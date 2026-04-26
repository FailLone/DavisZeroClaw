//! Brave Search API implementation of `SearchProvider`.
//!
//! Endpoint: https://api.search.brave.com/res/v1/web/search?q={q}&count={n}
//! Header:   X-Subscription-Token: <api_key>

use super::{SearchError, SearchHit, SearchProvider};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

pub struct BraveSearch {
    http: reqwest::Client,
    api_key: String,
    endpoint: String,
}

impl BraveSearch {
    pub fn new(http: reqwest::Client, api_key: String) -> Self {
        Self {
            http,
            api_key,
            endpoint: BRAVE_ENDPOINT.into(),
        }
    }

    /// Dev/test constructor. `new` is the production path; `with_endpoint`
    /// exists so integration tests can point the client at a local mock
    /// server. Explicit enough at the call site that we don't gate it
    /// behind `cfg(test)` — which would also hide it from `tests/*.rs`
    /// integration tests that link against the non-test lib crate.
    pub fn with_endpoint(http: reqwest::Client, api_key: String, endpoint: String) -> Self {
        Self {
            http,
            api_key,
            endpoint,
        }
    }

    pub fn parse_body(body: &[u8]) -> anyhow::Result<Vec<SearchHit>> {
        let raw: BraveResp = serde_json::from_slice(body)?;
        Ok(raw
            .web
            .results
            .into_iter()
            .map(|r| SearchHit {
                url: r.url,
                title: r.title,
                snippet: r.description,
            })
            .collect())
    }
}

#[async_trait]
impl SearchProvider for BraveSearch {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, SearchError> {
        let resp = self
            .http
            .get(&self.endpoint)
            .header("X-Subscription-Token", &self.api_key)
            .header("Accept", "application/json")
            .query(&[("q", query), ("count", &limit.to_string())])
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| SearchError::Unavailable("brave", e.to_string()))?;
        match resp.status().as_u16() {
            200 => {
                let body = resp
                    .bytes()
                    .await
                    .map_err(|e| SearchError::Unavailable("brave", e.to_string()))?;
                let hits = Self::parse_body(&body).map_err(SearchError::Other)?;
                Ok(hits.into_iter().take(limit).collect())
            }
            401 | 403 => Err(SearchError::Auth(format!("brave http {}", resp.status()))),
            429 => Err(SearchError::RateLimited),
            other => {
                let body = resp.text().await.unwrap_or_default();
                Err(SearchError::Unavailable(
                    "brave",
                    format!("http {other}: {body}"),
                ))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct BraveResp {
    web: BraveWeb,
}
#[derive(Debug, Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveResult>,
}
#[derive(Debug, Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    #[serde(default)]
    description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        let body = std::fs::read("tests/fixtures/discovery/brave_sample.json").unwrap();
        let hits = BraveSearch::parse_body(&body).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://tokio.rs/blog/2019-10-scheduler");
    }

    /// Live Brave Search API smoke. Opt-in: needs a Brave API key configured
    /// in `config/davis/local.toml` under `[article_memory.discovery.search]`
    /// (as `api_key` or `api_key_env`), and `--ignored`. Validates that the
    /// real Brave response JSON still deserializes into
    /// `BraveResp`/`BraveWeb`/`BraveResult` unchanged.
    ///
    /// Run: `cargo test --lib discovery::search::brave::tests::live_brave_smoke
    /// -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "hits live Brave API; requires a key in local.toml and opt-in via --ignored"]
    async fn live_brave_smoke() {
        // Load the key from the same local.toml the daemon reads. `local.toml`
        // is gitignored, so a direct `api_key` value is safe. If unset, fall
        // back to the `api_key_env` env var for CI-style setups.
        let paths = crate::RuntimePaths::from_env();
        let cfg = crate::app_config::load_local_config(&paths)
            .expect("load_local_config must succeed; ensure config/davis/local.toml exists");
        let sc = cfg
            .article_memory
            .discovery
            .search
            .as_ref()
            .expect("[article_memory.discovery.search] section is required for this smoke");
        let key = sc
            .api_key
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                let env_name = sc.api_key_env.as_deref().unwrap_or("BRAVE_API_KEY");
                std::env::var(env_name).ok().filter(|s| !s.is_empty())
            })
            .expect("api_key or api_key_env must resolve to a non-empty key");

        let brave = BraveSearch::new(
            reqwest::Client::builder()
                .user_agent("davis-smoke/0.1")
                .build()
                .unwrap(),
            key,
        );
        let hits = brave
            .search("async rust tokio", 5)
            .await
            .expect("Brave search must return 2xx");

        eprintln!("--- brave live smoke: {} hits ---", hits.len());
        for (i, h) in hits.iter().enumerate() {
            eprintln!("  [{i}] {}  —  {}", h.url, h.title);
        }

        assert!(
            !hits.is_empty(),
            "Brave returned zero results for a common query"
        );
        assert!(hits.len() <= 5, "Brave returned more than requested limit");
        for (i, h) in hits.iter().enumerate() {
            assert!(!h.url.is_empty(), "hit[{i}].url empty — schema drift?");
            assert!(!h.title.is_empty(), "hit[{i}].title empty — schema drift?");
            assert!(
                h.url.starts_with("http://") || h.url.starts_with("https://"),
                "hit[{i}].url not absolute: {}",
                h.url,
            );
        }
    }
}
