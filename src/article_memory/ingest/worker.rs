use super::engines::{EngineChoice, ExtractEngineConfig};
use super::llm_extract::llm_html_to_markdown;
use super::quality_gate::{assess as assess_quality, GateResult, QualityGateConfig};
use super::queue::IngestQueue;
use super::types::{
    IngestJob, IngestJobError, IngestJobStatus, IngestOutcome, IngestOutcomeSummary,
};
use crate::app_config::{
    ArticleMemoryConfig, ArticleMemoryExtractConfig, ArticleMemoryIngestConfig, ImessageConfig,
    ModelProviderConfig, OpenRouterLlmEngineConfig, QualityGateToml,
};
use crate::server::Crawl4aiProfileLocks;
use crate::{
    add_article_memory, add_article_memory_override, crawl4ai_crawl,
    find_article_by_normalized_url, normalize_article_memory, resolve_article_embedding_config,
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
    pub imessage_config: Arc<ImessageConfig>,
    pub extract_config: Arc<ArticleMemoryExtractConfig>,
    pub quality_gate_config: Arc<QualityGateToml>,
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
    execute_job_core(queue, deps, &job).await;
    // Wrapper guarantees terminal notification runs regardless of which
    // stage produced the terminal state. Early-return Failed branches in
    // `execute_job_core` (fetch / markdown / cleaning / judging errors) are
    // now covered alongside the main Saved/Rejected path.
    maybe_notify_terminal(queue, deps, &job).await;
}

async fn maybe_notify_terminal(queue: &IngestQueue, deps: &IngestWorkerDeps, job: &IngestJob) {
    let Some(handle) = job.reply_handle.as_deref() else {
        return;
    };
    let Some(finished_job) = queue.get(&job.id).await else {
        return;
    };
    if !finished_job.status.is_terminal() {
        // Defensive: caller should only reach here after execute_job_core
        // has driven the job to a terminal state. If it hasn't, skip notify
        // so we don't message a Pending job.
        return;
    }
    let resolved_title = finished_job.article_id.as_ref().and_then(|id| {
        super::super::internals::load_index(&deps.paths)
            .ok()
            .and_then(|idx| {
                idx.articles
                    .into_iter()
                    .find(|r| &r.id == id)
                    .map(|r| r.title)
            })
    });
    let text = super::reply_text::build_reply_text(&finished_job, resolved_title.as_deref());
    if text.is_empty() {
        return;
    }
    if let Err(err) =
        crate::imessage_send::notify_user(handle, &text, &deps.imessage_config.allowed_contacts)
            .await
    {
        tracing::warn!(
            job_id = %finished_job.id,
            handle = %handle,
            error = %err,
            "imessage notification failed; job state unchanged"
        );
    }
}

async fn execute_job_core(queue: &IngestQueue, deps: &IngestWorkerDeps, job: &IngestJob) {
    let profile_lock = acquire_profile_lock(&deps.profile_locks, &job.profile_name).await;
    let _guard = profile_lock.lock().await;

    // Stage 1: fetch + quality gate + Rust-local LLM upgrade
    let engine_cfg = engine_config_from_toml(&deps.extract_config);
    let gate_cfg = quality_gate_config_from_toml(&deps.quality_gate_config);

    // Determine the primary fetch engine. OpenRouterLlm is never sent to
    // the Python adapter (the adapter rejects it now). If the operator
    // configures default_engine="openrouter-llm" we still need to fetch
    // HTML first — fall back to trafilatura for the fetch, then let the
    // upgrade path do its thing.
    let fetch_engine = match &engine_cfg.default_engine {
        EngineChoice::OpenRouterLlm => EngineChoice::Trafilatura,
        other => other.clone(),
    };
    let mut attempted: Vec<EngineChoice> = vec![fetch_engine.clone()];

    let mut page = match crawl4ai_crawl(
        &deps.paths,
        &deps.crawl4ai_config,
        &deps.supervisor,
        Crawl4aiPageRequest {
            profile_name: job.profile_name.clone(),
            url: job.url.clone(),
            wait_for: None,
            js_code: None,
            markdown: false,
            extract_engine: Some(fetch_engine.as_str().to_string()),
            openrouter_config: None,
        },
    )
    .await
    {
        Ok(p) => p,
        Err(err) => {
            let issue_type = err.issue_type().to_string();
            let message = err.to_string();
            queue
                .attach_engine_chain(
                    &job.id,
                    attempted.iter().map(|e| e.as_str().to_string()).collect(),
                )
                .await;
            queue
                .finish(
                    &job.id,
                    IngestOutcome::Failed(IngestJobError {
                        issue_type,
                        message,
                        stage: "fetching".into(),
                    }),
                )
                .await;
            return;
        }
    };

    let html_chars = page.html.as_ref().map(|h| h.chars().count()).unwrap_or(0);
    let mut markdown = page.markdown.clone().unwrap_or_default();
    let mut gate: GateResult = assess_quality(&markdown, html_chars, &gate_cfg);

    // If gate failed, try Rust-local LLM upgrade.
    if !gate.pass {
        let html_for_llm = page.html.as_deref().unwrap_or("");
        if let Some(new_md) = try_llm_upgrade(
            &engine_cfg,
            &deps.extract_config.openrouter_llm,
            &deps.providers,
            html_for_llm,
        )
        .await
        {
            attempted.push(EngineChoice::OpenRouterLlm);
            tracing::info!(
                job_id = %job.id,
                from = %fetch_engine,
                to = "openrouter-llm",
                hard = ?gate.hard_fail_reasons,
                soft = ?gate.soft_fail_reasons,
                "upgrading extraction engine after gate failure"
            );
            markdown = new_md.clone();
            page.markdown = Some(new_md);
            gate = assess_quality(&markdown, html_chars, &gate_cfg);
        }
    }

    // If still failing after any upgrade attempt, reject.
    if !gate.pass {
        queue
            .attach_engine_chain(
                &job.id,
                attempted.iter().map(|e| e.as_str().to_string()).collect(),
            )
            .await;
        queue
            .finish(
                &job.id,
                IngestOutcome::Failed(IngestJobError {
                    issue_type: "quality_gate_rejected".into(),
                    message: format!(
                        "quality gate failed; hard={:?} soft={:?}",
                        gate.hard_fail_reasons, gate.soft_fail_reasons
                    ),
                    stage: "fetching".into(),
                }),
            )
            .await;
        return;
    }

    queue
        .attach_engine_chain(
            &job.id,
            attempted.iter().map(|e| e.as_str().to_string()).collect(),
        )
        .await;

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

    let add_req = ArticleMemoryAddRequest {
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
    };

    // On force, reuse the existing article_id (update-in-place) so the
    // worker does not append a duplicate row alongside the record that
    // Rule 0 dedup would have rejected without force.
    let existing_for_force = if job.force {
        find_article_by_normalized_url(&deps.paths, &job.normalized_url)
            .ok()
            .flatten()
    } else {
        None
    };

    let record = match existing_for_force {
        Some(existing) => add_article_memory_override(&deps.paths, add_req, &existing.id),
        None => add_article_memory(&deps.paths, add_req),
    };

    let record = match record {
        Ok(rec) => rec,
        Err(err) => {
            queue
                .finish(
                    &job.id,
                    IngestOutcome::Failed(IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: err.to_string(),
                        stage: "cleaning".into(),
                    }),
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
                .finish(
                    &job.id,
                    IngestOutcome::Failed(IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: format!("resolve_article_normalize_config: {err}"),
                        stage: "judging".into(),
                    }),
                )
                .await;
            return;
        }
    };
    let value_config = match resolve_article_value_config(&deps.paths, &deps.providers) {
        Ok(cfg) => cfg,
        Err(err) => {
            queue
                .finish(
                    &job.id,
                    IngestOutcome::Failed(IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: format!("resolve_article_value_config: {err}"),
                        stage: "judging".into(),
                    }),
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
                .finish(
                    &job.id,
                    IngestOutcome::Failed(IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: err.to_string(),
                        stage: "judging".into(),
                    }),
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

    let outcome = if rejected {
        IngestOutcome::Rejected {
            article_id: Some(record.id.clone()),
            summary,
        }
    } else {
        IngestOutcome::Saved {
            article_id: record.id.clone(),
            summary,
            warnings,
        }
    };
    queue.finish(&job.id, outcome).await;
}

fn engine_config_from_toml(extract: &ArticleMemoryExtractConfig) -> ExtractEngineConfig {
    let default_engine =
        EngineChoice::from_str(&extract.default_engine).unwrap_or(EngineChoice::Trafilatura);
    let ladder: Vec<EngineChoice> = extract
        .fallback_ladder
        .iter()
        .filter_map(|s| EngineChoice::from_str(s))
        .collect();
    ExtractEngineConfig {
        default_engine,
        fallback_ladder: if ladder.is_empty() {
            vec![EngineChoice::Trafilatura, EngineChoice::OpenRouterLlm]
        } else {
            ladder
        },
    }
}

fn quality_gate_config_from_toml(gate: &QualityGateToml) -> QualityGateConfig {
    QualityGateConfig {
        enabled: gate.enabled,
        min_markdown_chars: gate.min_markdown_chars,
        min_kept_ratio: gate.min_kept_ratio,
        min_paragraphs: gate.min_paragraphs,
        max_link_density: gate.max_link_density,
        boilerplate_markers: gate.boilerplate_markers.clone(),
    }
}

fn find_provider<'a>(
    providers: &'a [ModelProviderConfig],
    name: &str,
) -> Option<&'a ModelProviderConfig> {
    if name.is_empty() {
        return None;
    }
    providers.iter().find(|p| p.name == name)
}

/// If gate fails and the ladder permits upgrading to openrouter-llm, try a
/// Rust-local LLM pass over the already-fetched HTML. Returns `Some(new_markdown)`
/// if the LLM produced output (even if it doesn't pass the gate — caller re-runs).
async fn try_llm_upgrade(
    engine_cfg: &ExtractEngineConfig,
    llm_engine: &OpenRouterLlmEngineConfig,
    providers: &[ModelProviderConfig],
    html: &str,
) -> Option<String> {
    let ladder_allows = engine_cfg
        .fallback_ladder
        .iter()
        .any(|e| matches!(e, EngineChoice::OpenRouterLlm));
    if !ladder_allows {
        return None;
    }
    let provider = find_provider(providers, &llm_engine.provider)?;
    match llm_html_to_markdown(provider, llm_engine, html).await {
        Ok(md) => Some(md),
        Err(err) => {
            tracing::warn!(error = %err, "llm upgrade failed; staying with trafilatura output");
            None
        }
    }
}
