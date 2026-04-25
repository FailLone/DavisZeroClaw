//! Evergreen refresh worker (MVP).
//!
//! Re-judges articles whose `updated_at` crossed the staleness cutoff by
//! simply bumping `updated_at` so the scan window rotates. The MVP does
//! NOT call the LLM judge and does NOT re-crawl — content-drift refresh
//! is deferred to Phase 6+. `rejudged` / `decisions_flipped` therefore
//! stay zero in every cycle; `unchanged_bumped` counts what we touched.

use crate::app_config::RefreshConfig;
use crate::article_memory::types::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
use crate::mempalace_sink::MempalaceEmitter;
use crate::support::{isoformat, now_utc, parse_iso};
use crate::RuntimePaths;
use anyhow::Result;
use chrono::{Duration, Utc};
use std::sync::Arc;
use std::time::Duration as StdDuration;

#[derive(Clone)]
pub struct RefreshWorkerDeps {
    pub config: Arc<RefreshConfig>,
    pub paths: RuntimePaths,
    pub mempalace_sink: Arc<dyn MempalaceEmitter>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RefreshCycleReport {
    pub scanned: usize,
    pub rejudged: usize,
    pub unchanged_bumped: usize,
    pub decisions_flipped: usize,
}

pub struct RefreshWorker;

impl RefreshWorker {
    pub fn spawn(deps: RefreshWorkerDeps) {
        if !deps.config.enabled {
            return;
        }
        let interval_secs = deps.config.interval_secs;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(StdDuration::from_secs(interval_secs));
            interval.tick().await;
            loop {
                interval.tick().await;
                match run_one_cycle(&deps).await {
                    Ok(rep) => tracing::info!(?rep, "refresh cycle ok"),
                    Err(err) => tracing::warn!(error = %err, "refresh cycle errored"),
                }
            }
        });
    }
}

pub async fn run_one_cycle(deps: &RefreshWorkerDeps) -> Result<RefreshCycleReport> {
    let mut report = RefreshCycleReport::default();
    let cutoff = Utc::now() - Duration::days(deps.config.stale_after_days as i64);

    let mut idx = crate::article_memory::load_article_index(&deps.paths)?;

    let mut stale_positions: Vec<usize> = idx
        .articles
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            matches!(
                r.status,
                ArticleMemoryRecordStatus::Saved | ArticleMemoryRecordStatus::Candidate
            )
        })
        .filter(|(_, r)| {
            parse_iso(&r.updated_at)
                .map(|dt| dt < cutoff)
                .unwrap_or(false)
        })
        .map(|(i, _)| i)
        .collect();
    // Oldest first so we rotate through the backlog deterministically.
    stale_positions.sort_by(|a, b| {
        idx.articles[*a]
            .updated_at
            .cmp(&idx.articles[*b].updated_at)
    });
    let limit = deps.config.batch_per_cycle;

    for pos in stale_positions.into_iter().take(limit) {
        report.scanned += 1;
        // For MVP evergreen refresh we do NOT re-call the LLM judge. We simply
        // touch `updated_at` so the scan window rotates; `rejudged` and
        // `decisions_flipped` stay at zero. Content-drift refresh (re-crawl +
        // re-judge) is Phase 6+.
        let record: &mut ArticleMemoryRecord = &mut idx.articles[pos];
        record.updated_at = isoformat(now_utc());
        report.unchanged_bumped += 1;
    }

    crate::article_memory::save_article_index(&deps.paths, &idx)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::article_memory::ArticleMemoryIndex;
    use crate::mempalace_sink::testing::NoopSink;
    use tempfile::TempDir;

    fn test_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().to_path_buf(),
        }
    }

    fn seed_record(idx: &mut ArticleMemoryIndex, id: &str, updated_at: &str) {
        idx.articles.push(ArticleMemoryRecord {
            id: id.into(),
            title: id.into(),
            url: Some(format!("https://ex.com/{id}")),
            source: "t".into(),
            language: Some("en".into()),
            tags: vec![],
            status: ArticleMemoryRecordStatus::Saved,
            value_score: Some(0.6),
            captured_at: updated_at.into(),
            updated_at: updated_at.into(),
            content_path: format!("{id}/content.md"),
            raw_path: None,
            normalized_path: Some(format!("{id}/normalized.md")),
            summary_path: None,
            translation_path: None,
            notes: None,
            clean_status: Some("ok".into()),
            clean_profile: Some("default".into()),
        });
    }

    #[tokio::test]
    async fn picks_stale_records_only() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(&tmp);
        crate::article_memory::init_article_memory(&paths).unwrap();

        let mut idx = crate::article_memory::load_article_index(&paths).unwrap();
        seed_record(&mut idx, "old", "2020-01-01T00:00:00Z"); // very old
        seed_record(&mut idx, "recent", &isoformat(now_utc())); // fresh
        crate::article_memory::save_article_index(&paths, &idx).unwrap();

        let deps = RefreshWorkerDeps {
            config: Arc::new(RefreshConfig {
                enabled: true,
                stale_after_days: 30,
                ..RefreshConfig::default()
            }),
            paths: paths.clone(),
            mempalace_sink: Arc::new(NoopSink),
        };
        let report = run_one_cycle(&deps).await.unwrap();
        assert_eq!(report.scanned, 1, "only the old record is picked");
        assert_eq!(report.unchanged_bumped, 1);
        assert_eq!(report.rejudged, 0, "MVP does not call the LLM judge");
        assert_eq!(report.decisions_flipped, 0);

        // The old record's updated_at should now be recent.
        let idx_after = crate::article_memory::load_article_index(&paths).unwrap();
        let old = idx_after.articles.iter().find(|r| r.id == "old").unwrap();
        let bumped = parse_iso(&old.updated_at).unwrap();
        assert!(
            bumped > Utc::now() - Duration::days(1),
            "old record updated_at was bumped to recent"
        );
    }

    #[tokio::test]
    async fn disabled_worker_does_not_spawn() {
        // Basic sanity: constructing with enabled=false must make spawn() a
        // no-op and run_one_cycle itself returns Ok with zero scans on an
        // empty index (the cycle does not branch on `enabled` — spawn() does).
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(&tmp);
        crate::article_memory::init_article_memory(&paths).unwrap();
        let deps = RefreshWorkerDeps {
            config: Arc::new(RefreshConfig {
                enabled: false,
                ..RefreshConfig::default()
            }),
            paths,
            mempalace_sink: Arc::new(NoopSink),
        };
        // spawn() should short-circuit without panicking or spawning a task.
        RefreshWorker::spawn(deps.clone());
        let report = run_one_cycle(&deps).await.unwrap();
        assert_eq!(report.scanned, 0);
        assert_eq!(report.unchanged_bumped, 0);
    }
}
