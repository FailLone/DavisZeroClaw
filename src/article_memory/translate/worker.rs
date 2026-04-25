//! zh-CN translation worker — runs one cycle per `interval_secs`, picks up
//! non-Chinese articles that haven't been translated yet, pushes each one
//! through `/api/chat` via the private `remote_chat` client, and writes the
//! result as `{article_memory_dir}/{record.id}/translation.md`.
//!
//! Side effects are deliberately narrow:
//!   1. Write the translation file to disk.
//!   2. Update `translation_path` + `updated_at` on the record in the article
//!      memory index.
//!   3. Fire-and-forget `kg_add(article → lang:<target>, Translated)`.
//!
//! Budget-exceeded responses (HTTP 402 from zeroclaw) stop the cycle early so
//! we don't hammer the daemon after it has already told us the wallet is dry.

use crate::app_config::TranslateConfig;
use crate::article_memory::translate::prompt::{user_block, SYSTEM};
use crate::article_memory::translate::remote_chat::{RemoteChat, RemoteChatError};
use crate::article_memory::types::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
use crate::mempalace_sink::{MempalaceEmitter, Predicate, TripleId};
use crate::RuntimePaths;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct TranslateWorkerDeps {
    pub config: Arc<TranslateConfig>,
    pub http: reqwest::Client,
    pub paths: RuntimePaths,
    pub mempalace_sink: Arc<dyn MempalaceEmitter>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct TranslateCycleReport {
    pub scanned: usize,
    pub translated: usize,
    pub skipped_already_done: usize,
    pub failed: usize,
    pub budget_hit: bool,
}

pub struct TranslateWorker;

impl TranslateWorker {
    pub fn spawn(deps: TranslateWorkerDeps) {
        if !deps.config.enabled {
            tracing::info!("translate worker disabled; not spawning");
            return;
        }
        let interval_secs = deps.config.interval_secs;
        tokio::spawn(async move {
            tracing::info!(interval_secs, "translate worker started");
            let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
            // Skip the immediate tick so we don't fire the first cycle the
            // instant the server comes up — match rule_learning_worker.
            interval.tick().await;
            loop {
                interval.tick().await;
                match run_one_cycle(&deps).await {
                    Ok(rep) => tracing::info!(?rep, "translate cycle ok"),
                    Err(err) => tracing::warn!(error = %err, "translate cycle errored"),
                }
            }
        });
    }
}

pub async fn run_one_cycle(deps: &TranslateWorkerDeps) -> Result<TranslateCycleReport> {
    let mut report = TranslateCycleReport::default();
    let remote = RemoteChat::new(&deps.config, deps.http.clone());

    let candidates = list_candidates(deps).context("list translate candidates")?;
    let limit = deps.config.batch_per_cycle;

    for record in candidates.into_iter().take(limit) {
        report.scanned += 1;
        // Defensive: list_candidates already filters on translation_path ==
        // None, but a concurrent write could have landed between list and
        // here. Skipping is cheaper than re-translating.
        if record.translation_path.is_some() {
            report.skipped_already_done += 1;
            continue;
        }
        let Some(markdown) = load_normalized(deps, &record)? else {
            tracing::debug!(article_id = %record.id, "no normalized markdown; skipping");
            continue;
        };
        match remote.translate_to_zh(SYSTEM, &user_block(&markdown)).await {
            Ok(translated) => {
                let rel_path = write_translation_file(deps, &record, &translated)?;
                update_record_translation_path(deps, &record.id, &rel_path)?;
                report.translated += 1;
                emit_translated(deps, &record);
            }
            Err(RemoteChatError::BudgetExceeded { scope, message }) => {
                report.budget_hit = true;
                tracing::warn!(
                    scope = %scope,
                    message = %message,
                    "budget exceeded; stopping translate cycle"
                );
                break;
            }
            Err(err) => {
                report.failed += 1;
                tracing::warn!(article_id = %record.id, error = %err, "translate failed");
            }
        }
    }

    Ok(report)
}

// ---- thin shims over article_memory internals -----------------------------
// The index helpers live behind `article_memory::internals` as `pub(crate)`;
// the translate worker reuses them directly rather than inventing parallel
// load/save functions.

fn list_candidates(deps: &TranslateWorkerDeps) -> Result<Vec<ArticleMemoryRecord>> {
    let idx = crate::article_memory::internals::load_index(&deps.paths)?;
    let mut out: Vec<_> = idx
        .articles
        .into_iter()
        .filter(|r| {
            matches!(
                r.status,
                ArticleMemoryRecordStatus::Saved | ArticleMemoryRecordStatus::Candidate
            )
        })
        .filter(|r| r.translation_path.is_none())
        .filter(|r| {
            r.language
                .as_deref()
                .map(|l| !l.trim().to_lowercase().starts_with("zh"))
                .unwrap_or(false)
        })
        .collect();
    // Oldest updated_at first — approximate "oldest judged" without adding a
    // new column. Stable ISO-8601 strings sort lexicographically.
    out.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
    Ok(out)
}

fn load_normalized(
    deps: &TranslateWorkerDeps,
    record: &ArticleMemoryRecord,
) -> Result<Option<String>> {
    let Some(rel) = record.normalized_path.as_deref() else {
        return Ok(None);
    };
    let abs = deps.paths.article_memory_dir().join(rel);
    if !abs.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&abs)
        .with_context(|| format!("failed to read normalized markdown at {}", abs.display()))?;
    Ok(Some(text))
}

fn write_translation_file(
    deps: &TranslateWorkerDeps,
    record: &ArticleMemoryRecord,
    body: &str,
) -> Result<String> {
    let article_dir = deps.paths.article_memory_dir().join(&record.id);
    std::fs::create_dir_all(&article_dir)
        .with_context(|| format!("failed to create {}", article_dir.display()))?;
    let abs = article_dir.join("translation.md");
    std::fs::write(&abs, body)
        .with_context(|| format!("failed to write translation at {}", abs.display()))?;
    // Return path relative to article_memory_dir so it sits alongside the
    // existing `*_path` fields on ArticleMemoryRecord.
    let rel = PathBuf::from(&record.id).join("translation.md");
    Ok(rel.to_string_lossy().into_owned())
}

fn update_record_translation_path(
    deps: &TranslateWorkerDeps,
    article_id: &str,
    rel_path: &str,
) -> Result<()> {
    let mut idx = crate::article_memory::internals::load_index(&deps.paths)?;
    let now = crate::support::isoformat(crate::support::now_utc());
    if let Some(r) = idx.articles.iter_mut().find(|r| r.id == article_id) {
        r.translation_path = Some(rel_path.to_string());
        r.updated_at = now.clone();
    }
    idx.updated_at = now;
    crate::article_memory::internals::write_index(&deps.paths, &idx)?;
    Ok(())
}

fn emit_translated(deps: &TranslateWorkerDeps, record: &ArticleMemoryRecord) {
    // Use record.id for the subject — matches mempalace_projection.rs's
    // convention so an article has one stable KG identity across predicates.
    let Ok(subject) = TripleId::try_article(&TripleId::safe_slug(&record.id)) else {
        return;
    };
    let Ok(object) = TripleId::try_tag(&format!("lang:{}", deps.config.target_language)) else {
        return;
    };
    deps.mempalace_sink
        .kg_add(subject, Predicate::ArticleTranslated, object);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::TranslateConfig;
    use crate::mempalace_sink::SpySink;
    use tempfile::TempDir;

    fn test_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn deps_with(paths: RuntimePaths, base: &str) -> TranslateWorkerDeps {
        let cfg = TranslateConfig {
            enabled: true,
            zeroclaw_base_url: base.into(),
            batch_per_cycle: 5,
            interval_secs: 60,
            ..TranslateConfig::default()
        };
        TranslateWorkerDeps {
            config: Arc::new(cfg),
            http: reqwest::Client::new(),
            paths,
            mempalace_sink: Arc::new(SpySink::default()),
        }
    }

    #[tokio::test]
    async fn no_candidates_empty_cycle() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(&tmp);
        // Initialize an empty article memory index so load_index succeeds.
        crate::article_memory::init_article_memory(&paths).unwrap();
        let deps = deps_with(paths, "http://127.0.0.1:1");
        let report = run_one_cycle(&deps).await.unwrap();
        assert_eq!(report.scanned, 0);
        assert_eq!(report.translated, 0);
        assert_eq!(report.failed, 0);
        assert_eq!(report.skipped_already_done, 0);
        assert!(!report.budget_hit);
    }
}
