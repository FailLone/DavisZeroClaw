// Consumed starting Task 11 (local_proxy boot).
// Remove once IngestWorkerPool is spawned from daemon startup.
#![allow(dead_code)]

use super::queue::IngestQueue;
use super::types::{IngestJob, IngestJobError, IngestJobStatus, IngestOutcomeSummary};
use crate::app_config::{ArticleMemoryConfig, ArticleMemoryIngestConfig, ModelProviderConfig};
use crate::server::Crawl4aiProfileLocks;
use crate::{
    add_article_memory, crawl4ai_crawl, normalize_article_memory, resolve_article_embedding_config,
    resolve_article_normalize_config, resolve_article_value_config,
    upsert_article_memory_embedding, ArticleMemoryAddRequest, ArticleMemoryRecordStatus,
    Crawl4aiConfig, Crawl4aiPageRequest, Crawl4aiSupervisor, RuntimePaths,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct IngestWorkerDeps {
    pub paths: RuntimePaths,
    pub crawl4ai_config: Arc<Crawl4aiConfig>,
    pub supervisor: Arc<Crawl4aiSupervisor>,
    pub profile_locks: Crawl4aiProfileLocks,
    pub article_memory_config: Arc<ArticleMemoryConfig>,
    pub providers: Arc<Vec<ModelProviderConfig>>,
    pub ingest_config: Arc<ArticleMemoryIngestConfig>,
}

pub struct IngestWorkerPool;

impl IngestWorkerPool {
    /// Spawn N workers on the provided Tokio runtime. Returns nothing; the
    /// spawned tasks hold `Arc<IngestQueue>` so they live until the runtime
    /// shuts down.
    pub fn spawn(queue: Arc<IngestQueue>, deps: IngestWorkerDeps, concurrency: usize) {
        let n = concurrency.max(1);
        for worker_id in 0..n {
            let q = queue.clone();
            let d = deps.clone();
            tokio::spawn(async move {
                worker_loop(worker_id, q, d).await;
            });
        }
    }
}

async fn acquire_profile_lock(
    profile_locks: &Crawl4aiProfileLocks,
    profile: &str,
) -> Arc<Mutex<()>> {
    let mut map = profile_locks.lock().await;
    map.entry(profile.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

async fn worker_loop(worker_id: usize, queue: Arc<IngestQueue>, deps: IngestWorkerDeps) {
    tracing::info!(worker_id, "ingest worker started");
    loop {
        let job = queue.next_pending().await;
        execute_job(&queue, &deps, job).await;
    }
}

#[tracing::instrument(
    name = "ingest.execute",
    skip_all,
    fields(job_id = %job.id, url = %job.url, profile = %job.profile_name),
)]
async fn execute_job(queue: &IngestQueue, deps: &IngestWorkerDeps, job: IngestJob) {
    let profile_lock = acquire_profile_lock(&deps.profile_locks, &job.profile_name).await;
    let _guard = profile_lock.lock().await;

    // Stage 1: fetch
    let page = match crawl4ai_crawl(
        &deps.paths,
        &deps.crawl4ai_config,
        &deps.supervisor,
        Crawl4aiPageRequest {
            profile_name: job.profile_name.clone(),
            url: job.url.clone(),
            wait_for: None,
            js_code: None,
            markdown: true,
        },
    )
    .await
    {
        Ok(page) => page,
        Err(err) => {
            let issue_type = err.issue_type().to_string();
            let message = err.to_string();
            queue
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type,
                        message,
                        stage: "fetching".into(),
                    },
                )
                .await;
            return;
        }
    };

    let markdown = match page.markdown.as_deref() {
        Some(m) => m.to_string(),
        None => {
            queue
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type: "empty_content".into(),
                        message: "crawl4ai returned no markdown field".into(),
                        stage: "fetching".into(),
                    },
                )
                .await;
            return;
        }
    };
    if markdown.chars().count() < deps.ingest_config.min_markdown_chars {
        queue
            .finish_failed(
                &job.id,
                IngestJobError {
                    issue_type: "empty_content".into(),
                    message: format!(
                        "markdown length {} below min_markdown_chars {}",
                        markdown.chars().count(),
                        deps.ingest_config.min_markdown_chars
                    ),
                    stage: "fetching".into(),
                },
            )
            .await;
        return;
    }

    // Stage 2: cleaning
    if let Err(e) = queue.mark_status(&job.id, IngestJobStatus::Cleaning).await {
        tracing::warn!(error = %e, "failed to persist Cleaning status");
    }
    let title = job
        .title_override
        .clone()
        .or_else(|| {
            page.metadata
                .as_ref()
                .and_then(|m| m.get("title"))
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| job.url.clone());
    let source = job
        .resolved_source
        .clone()
        .unwrap_or_else(|| "web".to_string());

    let record = match add_article_memory(
        &deps.paths,
        ArticleMemoryAddRequest {
            title,
            url: Some(job.url.clone()),
            source,
            language: None,
            tags: job.tags.clone(),
            content: markdown,
            summary: None,
            translation: None,
            status: ArticleMemoryRecordStatus::Candidate,
            value_score: None,
            notes: None,
        },
    ) {
        Ok(rec) => rec,
        Err(err) => {
            queue
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: err.to_string(),
                        stage: "cleaning".into(),
                    },
                )
                .await;
            return;
        }
    };
    queue.attach_article_id(&job.id, record.id.clone()).await;

    // Stage 3: judging (normalize + optional value judge)
    if let Err(e) = queue.mark_status(&job.id, IngestJobStatus::Judging).await {
        tracing::warn!(error = %e, "failed to persist Judging status");
    }
    let normalize_config = match resolve_article_normalize_config(
        &deps.article_memory_config.normalize,
        &deps.providers,
    ) {
        Ok(cfg) => cfg,
        Err(err) => {
            queue
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: format!("resolve_article_normalize_config: {err}"),
                        stage: "judging".into(),
                    },
                )
                .await;
            return;
        }
    };
    let value_config = match resolve_article_value_config(&deps.paths, &deps.providers) {
        Ok(cfg) => cfg,
        Err(err) => {
            queue
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: format!("resolve_article_value_config: {err}"),
                        stage: "judging".into(),
                    },
                )
                .await;
            return;
        }
    };
    let normalize_response = match normalize_article_memory(
        &deps.paths,
        normalize_config.as_ref(),
        value_config.as_ref(),
        &record.id,
    )
    .await
    {
        Ok(resp) => resp,
        Err(err) => {
            queue
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: err.to_string(),
                        stage: "judging".into(),
                    },
                )
                .await;
            return;
        }
    };

    let rejected = normalize_response.value_decision.as_deref() == Some("reject");

    // Stage 4: embedding (skipped if rejected)
    let mut warnings: Vec<String> = Vec::new();
    let mut embedded = false;
    if !rejected {
        if let Err(e) = queue.mark_status(&job.id, IngestJobStatus::Embedding).await {
            tracing::warn!(error = %e, "failed to persist Embedding status");
        }
        let embedding_config = match resolve_article_embedding_config(
            &deps.article_memory_config.embedding,
            &deps.providers,
        ) {
            Ok(cfg) => cfg,
            Err(err) => {
                warnings.push(format!("embedding_config_invalid: {err}"));
                None
            }
        };
        if let Some(cfg) = embedding_config {
            match upsert_article_memory_embedding(&deps.paths, &cfg, &record).await {
                Ok(_) => embedded = true,
                Err(err) => warnings.push(format!("embedding_failed: {err}")),
            }
        }
    }

    let summary = IngestOutcomeSummary {
        clean_status: normalize_response.clean_status.clone(),
        clean_profile: normalize_response.clean_profile.clone(),
        value_decision: normalize_response.value_decision.clone(),
        value_score: normalize_response.value_score,
        normalized_chars: normalize_response.normalized_chars,
        polished: normalize_response.polished,
        summary_generated: normalize_response.summary_generated,
        embedded,
    };

    if rejected {
        queue
            .finish_rejected(&job.id, Some(record.id.clone()), summary)
            .await;
    } else {
        queue
            .finish_saved(&job.id, record.id.clone(), summary, warnings)
            .await;
    }
}
