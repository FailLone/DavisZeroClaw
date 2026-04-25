use super::engines::{pick_engine, EngineChoice, ExtractEngineConfig};
use super::learned_rules::{LearnedRuleStore, RuleStatsStore};
use super::llm_extract::llm_html_to_markdown;
use super::quality_gate::{assess as assess_quality, GateResult, QualityGateConfig};
use super::queue::IngestQueue;
use super::rule_samples::SampleStore;
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
    pub learned_rules: Arc<LearnedRuleStore>,
    pub rule_stats: Arc<RuleStatsStore>,
    pub sample_store: Arc<SampleStore>,
    /// Fire-and-forget projection into MemPalace. Defaults to a disabled
    /// sink in tests; wired to the live sink via `local_proxy`.
    pub mempalace_sink: Arc<dyn crate::mempalace_sink::MempalaceEmitter>,
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

    // Stage 1: fetch + quality gate, with optional learned-rules priority.
    let engine_cfg = engine_config_from_toml(&deps.extract_config);
    let gate_cfg = quality_gate_config_from_toml(&deps.quality_gate_config);
    let host = extract_host(&job.url);

    let mut attempted: Vec<EngineChoice> = Vec::new();
    let mut page: Option<crate::Crawl4aiPageResult> = None;
    let mut markdown = String::new();
    let mut gate: Option<GateResult> = None;

    // 1a. Try the learned-rules engine first if we have a non-stale rule for
    // this host. On success (page fetched AND quality gate passes) we skip
    // the Trafilatura ladder entirely. On any failure (crawl error, stale
    // output, gate fail) we log and fall through.
    let learned = match &host {
        Some(h) => deps.learned_rules.get(h).await,
        None => None,
    };
    if let Some(rule) = learned.as_ref().filter(|r| !r.stale) {
        attempted.push(EngineChoice::LearnedRules);
        let rule_json = serde_json::to_value(rule).ok();
        let req = Crawl4aiPageRequest {
            profile_name: job.profile_name.clone(),
            url: job.url.clone(),
            wait_for: None,
            js_code: None,
            markdown: false,
            extract_engine: Some(EngineChoice::LearnedRules.as_str().to_string()),
            openrouter_config: None,
            learned_rule: rule_json,
        };
        match crawl4ai_crawl(&deps.paths, &deps.crawl4ai_config, &deps.supervisor, req).await {
            Ok(p) => {
                let md = p.markdown.clone().unwrap_or_default();
                let html_chars = p.html.as_ref().map(|h| h.chars().count()).unwrap_or(0);
                let gr = assess_quality(&md, html_chars, &gate_cfg);
                if gr.pass {
                    markdown = md;
                    gate = Some(gr);
                    page = Some(p);
                } else {
                    tracing::info!(
                        host = ?host,
                        hard = ?gr.hard_fail_reasons,
                        soft = ?gr.soft_fail_reasons,
                        "learned-rules gate failed; falling through to trafilatura"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(
                    host = ?host,
                    error = %err,
                    "learned-rules crawl failed; falling through to trafilatura"
                );
            }
        }
    }

    // 1b. If learned-rules didn't produce a passing page, run the Phase 1
    // ladder (Trafilatura, optionally upgraded to openrouter-llm).
    if page.is_none() {
        let fetch_engine = pick_engine(&engine_cfg);
        attempted.push(fetch_engine.clone());
        let fetched = match crawl4ai_crawl(
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
                learned_rule: None,
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

        let html_chars = fetched
            .html
            .as_ref()
            .map(|h| h.chars().count())
            .unwrap_or(0);
        let mut md = fetched.markdown.clone().unwrap_or_default();
        let mut gr = assess_quality(&md, html_chars, &gate_cfg);
        let mut fetched = fetched;

        // If gate failed, try Rust-local LLM upgrade.
        if !gr.pass {
            let html_for_llm = fetched.html.as_deref().unwrap_or("");
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
                    hard = ?gr.hard_fail_reasons,
                    soft = ?gr.soft_fail_reasons,
                    "upgrading extraction engine after gate failure"
                );
                md = new_md.clone();
                fetched.markdown = Some(new_md);
                gr = assess_quality(&md, html_chars, &gate_cfg);
            }
        }

        markdown = md;
        gate = Some(gr);
        page = Some(fetched);
    }

    // One of the paths above must have set `page` + `gate`, or the function
    // already returned. Unwrap under that invariant.
    let page = page.expect("page must be set before exiting Stage 1");
    let gate = gate.expect("gate must be set before exiting Stage 1");

    // If still failing after any upgrade attempt, reject.
    if !gate.pass {
        // Capture HTML sample for rule learning (T25 worker consumes from this pool).
        if !gate.hard_fail_reasons.is_empty() {
            if let Some(ref h) = host {
                let html = page.html.clone().unwrap_or_default();
                if let Err(err) = deps.sample_store.push(
                    h,
                    &job.id,
                    &job.url,
                    &html,
                    &markdown,
                    "hard_fail",
                    gate.hard_fail_reasons
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                ) {
                    tracing::warn!(host = %h, error = %err, "failed to push rule sample");
                }
            }
        }
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
        content: markdown.clone(),
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
    let normalize_response = match super::report_context::with_context(
        super::report_context::EngineReportContext {
            engine_chain: attempted.iter().map(|e| e.as_str().to_string()).collect(),
            final_engine: attempted.last().map(|e| e.as_str().to_string()),
        },
        normalize_article_memory(
            &deps.paths,
            normalize_config.as_ref(),
            value_config.as_ref(),
            &record.id,
        ),
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

    // Apply extraction_quality feedback to learned-rules / stats.
    if let Some(ref h) = host {
        if let Ok(value_report) = load_latest_value_report(&deps.paths, &record.id) {
            match value_report.extraction_quality.as_str() {
                "poor" => {
                    let _ = deps.rule_stats.bump_poor(h).await;
                    let _ = deps
                        .learned_rules
                        .mark_stale(h, "extraction_quality=poor")
                        .await;
                    // Save HTML sample — page.html was captured during Stage 1.
                    if let Some(ref html) = page.html {
                        let _ = deps.sample_store.push(
                            h,
                            &job.id,
                            &job.url,
                            html,
                            &markdown,
                            "llm_poor",
                            value_report.extraction_issues.clone(),
                        );
                    }
                }
                "partial" => {
                    if let Ok(streak) = deps.rule_stats.bump_partial(h).await {
                        if streak >= 2 {
                            let _ = deps
                                .learned_rules
                                .mark_stale(h, "consecutive_partial")
                                .await;
                        }
                    }
                }
                _ => {
                    let _ = deps.rule_stats.bump_hit(h).await;
                }
            }
        }
    }

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

    // MemPalace projection: fire-and-forget. Only emit triples + drawer on
    // Saved; Rejected still gets a diary line so the user can retrace
    // "why did that ingest get thrown out".
    let (diary_status, value_decision) = if rejected {
        (
            crate::article_memory::mempalace_projection::IngestDiaryStatus::Rejected,
            Some("reject".to_string()),
        )
    } else {
        (
            crate::article_memory::mempalace_projection::IngestDiaryStatus::Saved,
            Some("save".to_string()),
        )
    };
    if !rejected {
        if let Ok(value_report) = load_latest_value_report(&deps.paths, &record.id) {
            crate::article_memory::mempalace_projection::emit_article_success(
                &value_report,
                deps.mempalace_sink.as_ref(),
            );
        }
    }
    let diary_entry = crate::article_memory::mempalace_projection::IngestDiaryEntry {
        timestamp_iso: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        job_id: job.id.clone(),
        status: diary_status,
        host: host.clone(),
        article_id: Some(record.id.clone()),
        value_decision,
        value_score: normalize_response.value_score,
        reason: None,
    };
    crate::article_memory::mempalace_projection::emit_ingest_diary(
        &diary_entry,
        deps.mempalace_sink.as_ref(),
    );

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

/// Parse the URL and return the lowercase host, if any. Used to look up
/// learned rules keyed by host.
fn extract_host(url_str: &str) -> Option<String> {
    url::Url::parse(url_str)
        .ok()?
        .host_str()
        .map(|s| s.to_lowercase())
}

fn load_latest_value_report(
    paths: &RuntimePaths,
    article_id: &str,
) -> anyhow::Result<crate::article_memory::ArticleValueReport> {
    let p = paths
        .article_memory_value_reports_dir()
        .join(format!("{article_id}.json"));
    let raw = std::fs::read_to_string(&p)?;
    Ok(serde_json::from_str(&raw)?)
}
