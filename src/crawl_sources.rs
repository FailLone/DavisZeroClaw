use crate::{express_auth_status, express_packages, Crawl4aiConfig, RuntimePaths};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CrawlSourceDefinition {
    pub id: &'static str,
    pub category: &'static str,
    pub description: &'static str,
    pub login_profiles: &'static [&'static str],
    pub urls: &'static [&'static str],
}

const EXPRESS_AUTH_LOGIN_PROFILES: &[&str] = &["express-ali", "express-jd"];
const EXPRESS_AUTH_URLS: &[&str] = &[
    "https://buyertrade.taobao.com/trade/itemlist/list_bought_items.htm",
    "https://order.jd.com/center/list.action",
];
const EXPRESS_ALI_LOGIN_PROFILES: &[&str] = &["express-ali"];
const EXPRESS_ALI_URLS: &[&str] =
    &["https://buyertrade.taobao.com/trade/itemlist/list_bought_items.htm"];
const EXPRESS_JD_LOGIN_PROFILES: &[&str] = &["express-jd"];
const EXPRESS_JD_URLS: &[&str] = &["https://order.jd.com/center/list.action"];

const BUILTIN_CRAWL_SOURCES: &[CrawlSourceDefinition] = &[
    CrawlSourceDefinition {
        id: "express-auth",
        category: "express",
        description: "Check managed login status for both Taobao and JD order pages.",
        login_profiles: EXPRESS_AUTH_LOGIN_PROFILES,
        urls: EXPRESS_AUTH_URLS,
    },
    CrawlSourceDefinition {
        id: "express-packages",
        category: "express",
        description: "Fetch package snapshots from both Taobao and JD order pages.",
        login_profiles: EXPRESS_AUTH_LOGIN_PROFILES,
        urls: EXPRESS_AUTH_URLS,
    },
    CrawlSourceDefinition {
        id: "express-ali-packages",
        category: "express",
        description: "Fetch package snapshots from the Taobao order page only.",
        login_profiles: EXPRESS_ALI_LOGIN_PROFILES,
        urls: EXPRESS_ALI_URLS,
    },
    CrawlSourceDefinition {
        id: "express-jd-packages",
        category: "express",
        description: "Fetch package snapshots from the JD order page only.",
        login_profiles: EXPRESS_JD_LOGIN_PROFILES,
        urls: EXPRESS_JD_URLS,
    },
];

pub fn builtin_crawl_sources() -> &'static [CrawlSourceDefinition] {
    BUILTIN_CRAWL_SOURCES
}

pub fn find_builtin_crawl_source(id: &str) -> Option<&'static CrawlSourceDefinition> {
    BUILTIN_CRAWL_SOURCES.iter().find(|source| source.id == id)
}

pub async fn run_builtin_crawl_source(
    paths: RuntimePaths,
    crawl4ai_config: Crawl4aiConfig,
    source_id: &str,
    query: Option<String>,
    refresh: bool,
) -> Result<Value, String> {
    // CLI path: each invocation is single-shot, so a fresh per-invocation
    // lock map is fine. Daemon path threads the AppState-owned map instead.
    let profile_locks = Arc::new(Mutex::new(HashMap::new()));
    match source_id {
        "express-auth" => {
            serialize_response(express_auth_status(paths, crawl4ai_config, profile_locks).await)
        }
        "express-packages" => serialize_response(
            express_packages(paths, crawl4ai_config, profile_locks, None, query, refresh).await,
        ),
        "express-ali-packages" => serialize_response(
            express_packages(
                paths,
                crawl4ai_config,
                profile_locks,
                Some("ali".to_string()),
                query,
                refresh,
            )
            .await,
        ),
        "express-jd-packages" => serialize_response(
            express_packages(
                paths,
                crawl4ai_config,
                profile_locks,
                Some("jd".to_string()),
                query,
                refresh,
            )
            .await,
        ),
        _ => Err(format!("unknown crawl source: {source_id}")),
    }
}

fn serialize_response<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|err| format!("serialize crawl source response: {err}"))
}
