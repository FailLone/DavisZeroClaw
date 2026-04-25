//! DiscoveryWorker. Follows the same tokio-interval pattern as
//! `rule_learning_worker.rs` — one tick per `interval_secs`, first tick skipped
//! so worker boot doesn't stampede the network immediately after daemon start.
//!
//! `run_one_cycle` is a pure async function (no globals, no `spawn` inside)
//! so it can be exercised directly from tests with a `MockSearch` + in-memory
//! `IngestQueue`.

use super::feed_ingestor::CandidateLink;
use super::search::{SearchError, SearchProvider};
use crate::app_config::{DiscoveryConfig, DiscoveryTopicConfig};
use crate::article_memory::ingest::{IngestQueue, IngestRequest};
use crate::mempalace_sink::{MempalaceEmitter, Predicate, TripleId};
use crate::RuntimePaths;
use anyhow::Result;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct DiscoveryWorkerDeps {
    pub paths: RuntimePaths,
    pub ingest_queue: Arc<IngestQueue>,
    pub config: Arc<DiscoveryConfig>,
    pub http: reqwest::Client,
    pub search_provider: Option<Arc<dyn SearchProvider>>,
    pub mempalace_sink: Arc<dyn MempalaceEmitter>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CycleReport {
    pub topic: String,
    pub fetched_feeds: usize,
    pub fetched_sitemaps: usize,
    pub search_queries: usize,
    pub candidates_before_dedupe: usize,
    pub submitted: usize,
}

pub struct DiscoveryWorker;

impl DiscoveryWorker {
    pub fn spawn(deps: DiscoveryWorkerDeps) {
        if !deps.config.enabled {
            tracing::info!("discovery worker disabled; not spawning");
            return;
        }
        let interval_secs = deps.config.interval_secs;
        tokio::spawn(async move {
            tracing::info!(interval_secs, "discovery worker started");
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            // Skip the immediate first tick — same pattern as rule_learning_worker,
            // avoids a network stampede the moment the daemon boots.
            interval.tick().await;
            loop {
                interval.tick().await;
                run_all_topics(&deps).await;
            }
        });
    }
}

pub async fn run_all_topics(deps: &DiscoveryWorkerDeps) {
    for topic in deps.config.topics.iter().filter(|t| t.enabled) {
        match run_one_cycle(deps, topic).await {
            Ok(report) => tracing::info!(topic = %topic.slug, ?report, "discovery cycle ok"),
            Err(err) => tracing::warn!(topic = %topic.slug, error = %err, "discovery cycle failed"),
        }
    }
}

pub async fn run_one_cycle(
    deps: &DiscoveryWorkerDeps,
    topic: &DiscoveryTopicConfig,
) -> Result<CycleReport> {
    let mut report = CycleReport {
        topic: topic.slug.clone(),
        ..CycleReport::default()
    };
    // (link, kind, source_host). `kind` is "feed" | "sitemap" | "search";
    // `source_host` is the feed/sitemap host for provenance-tagging, or the
    // search provider name for search hits.
    let mut candidates: Vec<(CandidateLink, &'static str, String)> = Vec::new();

    for feed_url in &topic.feeds {
        report.fetched_feeds += 1;
        match fetch_and_parse(&deps.http, feed_url, super::feed_ingestor::parse_feed).await {
            Ok(items) => {
                let host = host_of(feed_url);
                for link in items {
                    candidates.push((link, "feed", host.clone()));
                }
            }
            Err(err) => tracing::warn!(feed_url, error = %err, "feed fetch failed"),
        }
    }

    for sm_url in &topic.sitemaps {
        report.fetched_sitemaps += 1;
        match fetch_and_parse(&deps.http, sm_url, super::feed_ingestor::parse_sitemap).await {
            Ok(items) => {
                let host = host_of(sm_url);
                for link in items {
                    candidates.push((link, "sitemap", host.clone()));
                }
            }
            Err(err) => tracing::warn!(sm_url, error = %err, "sitemap fetch failed"),
        }
    }

    if let Some(search) = deps.search_provider.as_ref() {
        let results_per_query = deps
            .config
            .search
            .as_ref()
            .map(|s| s.results_per_query)
            .unwrap_or(10);
        for query in &topic.search_queries {
            report.search_queries += 1;
            match search.search(query, results_per_query).await {
                Ok(hits) => {
                    for hit in hits {
                        candidates.push((
                            CandidateLink {
                                url: hit.url,
                                title: Some(hit.title),
                                summary: Some(hit.snippet),
                            },
                            "search",
                            "brave".into(),
                        ));
                    }
                }
                Err(SearchError::RateLimited) => {
                    tracing::warn!("search rate limited, stopping queries for this cycle");
                    break;
                }
                Err(other) => tracing::warn!(error = %other, "search failed"),
            }
        }
    }

    report.candidates_before_dedupe = candidates.len();

    // Keyword filter (case-insensitive substring in title OR summary OR url).
    // Search results are already topic-scoped by the query itself, so we only
    // filter feed/sitemap output.
    if !topic.keywords.is_empty() {
        candidates.retain(|(link, kind, _)| {
            if *kind != "feed" && *kind != "sitemap" {
                return true;
            }
            let hay = format!(
                "{} {} {}",
                link.title.as_deref().unwrap_or(""),
                link.summary.as_deref().unwrap_or(""),
                link.url
            )
            .to_lowercase();
            topic
                .keywords
                .iter()
                .any(|kw| hay.contains(&kw.to_lowercase()))
        });
    }

    // Dedupe within the cycle by URL.
    let mut seen: HashSet<String> = HashSet::new();
    candidates.retain(|(link, _, _)| seen.insert(link.url.clone()));

    // Cap per-cycle throughput.
    candidates.truncate(deps.config.max_per_cycle);

    // Submit to the ingest queue. Dedup inside the queue (Rule 0/1/2) may
    // still reject duplicates we've previously ingested; those are logged at
    // debug and not counted as submitted.
    for (link, kind, source_host) in candidates {
        let source_hint = format!("discovery:{}:{}:{}", topic.slug, kind, source_host);
        let req = IngestRequest {
            url: link.url.clone(),
            force: false,
            title: link.title.clone(),
            tags: vec![format!("topic:{}", topic.slug)],
            source_hint: Some(source_hint.clone()),
            reply_handle: None,
        };
        match deps.ingest_queue.submit(req).await {
            Ok(resp) => {
                report.submitted += 1;
                emit_discovered(deps.mempalace_sink.as_ref(), &link.url, &source_hint);
                tracing::debug!(
                    topic = %topic.slug,
                    url = %link.url,
                    job_id = %resp.job_id,
                    "discovery submitted",
                );
            }
            Err(err) => {
                tracing::debug!(
                    topic = %topic.slug,
                    url = %link.url,
                    error = ?err,
                    "discovery submit rejected (dedupe/validation)",
                );
            }
        }
    }

    Ok(report)
}

async fn fetch_and_parse<F>(
    http: &reqwest::Client,
    url: &str,
    parser: F,
) -> Result<Vec<CandidateLink>>
where
    F: Fn(&[u8]) -> Result<Vec<CandidateLink>>,
{
    let resp = http
        .get(url)
        .timeout(Duration::from_secs(20))
        .send()
        .await?
        .error_for_status()?;
    let bytes = resp.bytes().await?;
    parser(&bytes)
}

fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Emit `ArticleDiscoveredFrom <tag>` for the submitted URL. Fire-and-forget —
/// if the URL or source hint can't be slugified into a TripleId, we silently
/// skip (the ingest worker will emit richer article-level triples once the
/// article lands in the store).
fn emit_discovered(sink: &dyn MempalaceEmitter, url: &str, source_hint: &str) {
    let Ok(subject) = TripleId::try_article(&TripleId::safe_slug(url)) else {
        return;
    };
    let Ok(object) = TripleId::try_tag(source_hint) else {
        return;
    };
    sink.kg_add(subject, Predicate::ArticleDiscoveredFrom, object);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::ArticleMemoryIngestConfig;
    use crate::article_memory::discovery::search::mock::MockSearch;
    use crate::article_memory::discovery::search::SearchHit;
    use crate::mempalace_sink::SpySink;
    use tempfile::TempDir;

    fn test_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn sample_topic() -> DiscoveryTopicConfig {
        DiscoveryTopicConfig {
            slug: "t".into(),
            keywords: vec!["rust".into()],
            feeds: vec![],
            sitemaps: vec![],
            search_queries: vec!["rust tokio".into()],
            enabled: true,
        }
    }

    fn deps_for_test(
        paths: RuntimePaths,
        queue: Arc<IngestQueue>,
        search: Arc<MockSearch>,
    ) -> DiscoveryWorkerDeps {
        let cfg = DiscoveryConfig {
            enabled: true,
            interval_secs: 60,
            max_per_cycle: 10,
            search: None,
            topics: vec![],
        };
        DiscoveryWorkerDeps {
            paths,
            ingest_queue: queue,
            config: Arc::new(cfg),
            http: reqwest::Client::new(),
            search_provider: Some(search),
            mempalace_sink: Arc::new(SpySink::default()),
        }
    }

    fn ingest_config() -> Arc<ArticleMemoryIngestConfig> {
        Arc::new(ArticleMemoryIngestConfig {
            enabled: true,
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn submits_all_unique_search_hits_up_to_max() {
        let search = Arc::new(MockSearch::new());
        search.inject(
            "rust tokio",
            vec![
                SearchHit {
                    url: "https://a.com/1".into(),
                    title: "x".into(),
                    snippet: "".into(),
                },
                SearchHit {
                    url: "https://a.com/2".into(),
                    title: "y".into(),
                    snippet: "".into(),
                },
                SearchHit {
                    url: "https://a.com/1".into(),
                    title: "dup".into(),
                    snippet: "".into(),
                },
            ],
        );
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(&tmp);
        let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_config()));
        let d = deps_for_test(paths, queue, search);
        let report = run_one_cycle(&d, &sample_topic()).await.unwrap();
        assert_eq!(report.submitted, 2, "one url deduped within cycle");
        assert_eq!(report.candidates_before_dedupe, 3);
    }
}
