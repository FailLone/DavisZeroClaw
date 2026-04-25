//! Hourly background worker that turns accumulated rule samples into
//! LearnedRule entries. Calls the learning LLM (configured via
//! RuleLearningConfig), validates the rule against its samples, and
//! writes passing rules to `learned_rules.json`. Failing rules land in
//! `quarantine_rules/` with validation errors attached.

#![allow(dead_code)]

use super::learned_rules::{LearnedRuleStore, RuleStatsStore};
use super::quality_gate::QualityGateConfig;
use super::rule_learning::{
    build_learn_prompt, parse_learn_response, validate_rule, ValidationResult, LEARN_SYSTEM_PROMPT,
};
use super::rule_samples::SampleStore;
use crate::app_config::{ModelProviderConfig, RuleLearningConfig};
use crate::article_memory::llm_client::{chat_completion, LlmChatRequest, LlmProvider};
use crate::RuntimePaths;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct RuleLearningDeps {
    pub paths: RuntimePaths,
    pub learned_rules: Arc<LearnedRuleStore>,
    pub rule_stats: Arc<RuleStatsStore>,
    pub sample_store: Arc<SampleStore>,
    pub providers: Arc<Vec<ModelProviderConfig>>,
    pub config: Arc<RuleLearningConfig>,
    pub quality_gate: Arc<QualityGateConfig>,
}

pub struct RuleLearningWorker;

impl RuleLearningWorker {
    /// Spawn the hourly worker on the current runtime.
    pub fn spawn(deps: RuleLearningDeps) {
        if !deps.config.enabled {
            tracing::info!("rule learning worker disabled; not spawning");
            return;
        }
        tokio::spawn(async move {
            tracing::info!("rule learning worker started");
            // Initial quick scan, then hourly.
            if let Err(err) = run_scan(&deps).await {
                tracing::error!(error = %err, "rule learning initial scan failed");
            }
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            interval.tick().await; // skip the immediate first tick
            loop {
                interval.tick().await;
                if let Err(err) = run_scan(&deps).await {
                    tracing::error!(error = %err, "rule learning scan failed");
                }
            }
        });
    }
}

async fn run_scan(deps: &RuleLearningDeps) -> Result<()> {
    let ready = deps.sample_store.ready_hosts(deps.config.samples_required);
    if ready.is_empty() {
        return Ok(());
    }
    tracing::info!(hosts = ?ready, "rule learning: hosts ready to learn");
    for host in ready {
        match learn_one_host(deps, &host).await {
            Ok(()) => tracing::info!(host = %host, "rule learning: host complete"),
            Err(err) => tracing::warn!(host = %host, error = %err, "rule learning: host failed"),
        }
    }
    Ok(())
}

async fn learn_one_host(deps: &RuleLearningDeps, host: &str) -> Result<()> {
    let samples = deps
        .sample_store
        .load_samples(host, deps.config.samples_required)?;
    if samples.len() < deps.config.samples_required {
        return Ok(());
    }

    let prompt = build_learn_prompt(host, &samples);
    let provider = deps
        .providers
        .iter()
        .find(|p| p.name == deps.config.learning_provider)
        .with_context(|| {
            format!(
                "provider '{}' not configured",
                deps.config.learning_provider
            )
        })?;

    let response = chat_completion(
        &LlmProvider {
            name: &provider.name,
            base_url: &provider.base_url,
            api_key: &provider.api_key,
        },
        &LlmChatRequest {
            model: &deps.config.learning_model,
            system: LEARN_SYSTEM_PROMPT,
            user: &prompt,
            temperature: 0.0,
            max_tokens: Some(2000),
            timeout: Duration::from_secs(120),
        },
    )
    .await
    .context("learning LLM call failed")?;

    let rule = parse_learn_response(host, &response, samples.len())
        .context("parse learning LLM response")?;

    let validation = validate_rule(&rule, &samples, &deps.quality_gate);
    if !validation.ok {
        write_quarantine(&deps.paths, host, &rule, &validation)?;
        if deps.config.notify_on_quarantine {
            // Fire-and-forget notification — daemon picks up primary handle.
            tracing::warn!(
                host = %host, errors = ?validation.errors,
                "learning rule failed validation; quarantined"
            );
        }
        return Ok(());
    }

    deps.learned_rules.upsert(rule.clone()).await?;
    deps.rule_stats
        .reset_for_new_rule(host, &rule.version)
        .await?;
    deps.sample_store.clear(host)?;
    tracing::info!(
        host = %host, version = %rule.version, confidence = rule.confidence,
        "learned rule saved"
    );
    Ok(())
}

fn write_quarantine(
    paths: &RuntimePaths,
    host: &str,
    rule: &super::rule_types::LearnedRule,
    validation: &ValidationResult,
) -> Result<PathBuf> {
    let dir = paths.article_memory_dir().join("quarantine_rules");
    fs::create_dir_all(&dir)?;
    let ts = crate::support::isoformat(crate::support::now_utc()).replace(':', "-");
    let path = dir.join(format!("{host}-{ts}.json"));
    let body = serde_json::json!({
        "host": host,
        "rule": rule,
        "errors": validation.errors,
        "extracted_chars_median": validation.extracted_chars_median,
    });
    fs::write(&path, serde_json::to_string_pretty(&body)?)?;
    Ok(path)
}
