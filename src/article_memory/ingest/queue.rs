// Consumed starting Task 9 (worker.rs) and Task 10 (server.rs).
// Remove this attribute once IngestQueue is wired into AppState.
#![allow(dead_code)]
use super::host_profile::{normalize_url, resolve_profile, validate_url_for_ingest};
use super::types::{
    IngestJob, IngestJobError, IngestJobStatus, IngestOutcomeSummary, IngestRequest,
    IngestResponse, IngestSubmitError, ListFilter,
};
use crate::app_config::ArticleMemoryIngestConfig;
use crate::support::{isoformat, now_utc};
use crate::RuntimePaths;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

const INGEST_JOBS_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IngestQueueState {
    #[serde(default = "default_state_version")]
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub jobs: HashMap<String, IngestJob>,
    #[serde(default)]
    pub pending: VecDeque<String>,
}

fn default_state_version() -> u32 {
    INGEST_JOBS_VERSION
}

pub struct IngestQueue {
    pub(super) inner: Mutex<IngestQueueState>,
    persistence_path: PathBuf,
    notify: Arc<Notify>,
    config: Arc<ArticleMemoryIngestConfig>,
}

impl IngestQueue {
    /// Load from disk or create a fresh queue. Any job found in an active
    /// status is reset to Failed with issue_type = "daemon_restart".
    pub fn load_or_create(paths: &RuntimePaths, config: Arc<ArticleMemoryIngestConfig>) -> Self {
        let persistence_path = paths.article_memory_ingest_jobs_path();
        let state = Self::read_or_default(&persistence_path);
        let state = Self::reset_active_to_failed(state);
        let queue = Self {
            inner: Mutex::new(state),
            persistence_path,
            notify: Arc::new(Notify::new()),
            config,
        };
        // Best-effort persistence of the reset state. Non-fatal on failure,
        // but log so operators can see a read-only-disk situation.
        if let Err(e) = queue.persist_blocking() {
            tracing::error!(
                path = %queue.persistence_path.display(),
                error = %e,
                "failed to persist ingest queue on boot; queue will be in-memory-only this session"
            );
        }
        queue
    }

    fn read_or_default(path: &PathBuf) -> IngestQueueState {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(_) => {
                return IngestQueueState {
                    version: INGEST_JOBS_VERSION,
                    updated_at: isoformat(now_utc()),
                    jobs: HashMap::new(),
                    pending: VecDeque::new(),
                }
            }
        };
        match serde_json::from_str::<IngestQueueState>(&raw) {
            Ok(state) => state,
            Err(error) => {
                tracing::error!(error = %error, path = %path.display(), "failed to parse ingest_jobs.json; starting with empty queue");
                IngestQueueState {
                    version: INGEST_JOBS_VERSION,
                    updated_at: isoformat(now_utc()),
                    jobs: HashMap::new(),
                    pending: VecDeque::new(),
                }
            }
        }
    }

    fn reset_active_to_failed(mut state: IngestQueueState) -> IngestQueueState {
        let now = isoformat(now_utc());
        for job in state.jobs.values_mut() {
            if job.status.is_active() {
                let stage = job.status.as_str().to_string();
                job.status = IngestJobStatus::Failed;
                job.error = Some(IngestJobError {
                    issue_type: "daemon_restart".to_string(),
                    message: format!("daemon restarted mid-job, status was {stage}"),
                    stage,
                });
                job.finished_at = Some(now.clone());
            }
        }
        state.pending.clear();
        state.updated_at = now;
        state
    }

    pub fn notify_handle(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    pub async fn submit(&self, req: IngestRequest) -> Result<IngestResponse, IngestSubmitError> {
        if !self.config.enabled {
            return Err(IngestSubmitError::IngestDisabled);
        }
        validate_url_for_ingest(&req.url, &self.config).map_err(|err| match err {
            super::host_profile::UrlValidationError::InvalidUrl => {
                IngestSubmitError::InvalidUrl("could not parse".to_string())
            }
            super::host_profile::UrlValidationError::InvalidScheme => {
                IngestSubmitError::InvalidScheme
            }
            super::host_profile::UrlValidationError::MissingHost => {
                IngestSubmitError::InvalidUrl("missing host".to_string())
            }
            super::host_profile::UrlValidationError::PrivateAddressBlocked(d) => {
                IngestSubmitError::PrivateAddressBlocked(d)
            }
        })?;
        let normalized = normalize_url(&req.url)
            .map_err(|_| IngestSubmitError::InvalidUrl("could not normalize".to_string()))?;

        let mut state = self.inner.lock().await;

        // Dedup rule 1: same URL still in an active job → idempotent response
        if let Some(existing) = state
            .jobs
            .values()
            .find(|j| j.normalized_url == normalized && j.status.is_active())
        {
            return Ok(IngestResponse {
                job_id: existing.id.clone(),
                status: existing.status.clone(),
                submitted_at: existing.submitted_at.clone(),
                deduped: true,
            });
        }

        // Dedup rule 2: same URL Saved within window → 409.
        // Conservative: if `finished_at` is malformed/unparseable, treat the
        // record as if it were within the window (block the duplicate) and
        // log — better to make the user resubmit after investigation than to
        // silently let duplicates slip through on corrupted timestamps.
        let window_hours = self.config.dedup_window_hours as i64;
        let mut recent_hit: Option<IngestJob> = None;
        let mut recent_ts: Option<chrono::DateTime<chrono::Utc>> = None;
        for j in state.jobs.values() {
            if j.normalized_url != normalized || j.status != IngestJobStatus::Saved {
                continue;
            }
            let within_window = match j.finished_at.as_deref() {
                Some(ts) => match crate::support::parse_time(ts) {
                    Some(t) => {
                        if (now_utc() - t) > Duration::hours(window_hours) {
                            // Outside the window; skip.
                            continue;
                        }
                        // Capture the most recent candidate for the 409 body.
                        if recent_ts.is_none_or(|prev| t > prev) {
                            recent_ts = Some(t);
                        }
                        true
                    }
                    None => {
                        tracing::warn!(
                            job_id = %j.id,
                            finished_at = %ts,
                            "malformed finished_at on Saved ingest job; treating as within dedup window"
                        );
                        true
                    }
                },
                None => {
                    tracing::warn!(
                        job_id = %j.id,
                        "missing finished_at on Saved ingest job; treating as within dedup window"
                    );
                    true
                }
            };
            if within_window {
                recent_hit = Some(j.clone());
            }
        }
        if let Some(recent) = recent_hit {
            return Err(IngestSubmitError::DuplicateSaved {
                existing_article_id: recent.article_id.clone(),
                finished_at: recent.finished_at.clone().unwrap_or_default(),
            });
        }

        let resolved = resolve_profile(&req.url, &self.config);
        let job_id = Uuid::new_v4().to_string();
        let submitted_at = isoformat(now_utc());
        let job = IngestJob {
            id: job_id.clone(),
            url: req.url.clone(),
            normalized_url: normalized,
            title_override: req.title.clone(),
            tags: req.tags.clone(),
            source_hint: req.source_hint.clone(),
            profile_name: resolved.profile,
            resolved_source: resolved.source,
            status: IngestJobStatus::Pending,
            article_id: None,
            outcome: None,
            error: None,
            warnings: Vec::new(),
            submitted_at: submitted_at.clone(),
            started_at: None,
            finished_at: None,
            attempts: 1,
        };
        state.jobs.insert(job_id.clone(), job.clone());
        state.pending.push_back(job_id.clone());
        state.updated_at = submitted_at.clone();
        self.persist_locked(&state)
            .map_err(|e| IngestSubmitError::PersistenceError(e.to_string()))?;
        drop(state);
        self.notify.notify_one();
        Ok(IngestResponse {
            job_id,
            status: IngestJobStatus::Pending,
            submitted_at,
            deduped: false,
        })
    }

    /// Wait for a pending job, take it, mark it Fetching, persist, return it.
    /// Re-checks the queue after each notify to survive the classic race where
    /// notify_one fires before the new entry commits.
    pub async fn next_pending(&self) -> IngestJob {
        loop {
            {
                let mut state = self.inner.lock().await;
                if let Some(id) = state.pending.pop_front() {
                    if let Some(job) = state.jobs.get_mut(&id) {
                        job.status = IngestJobStatus::Fetching;
                        job.started_at = Some(isoformat(now_utc()));
                        let cloned = job.clone();
                        state.updated_at = isoformat(now_utc());
                        if let Err(e) = self.persist_locked(&state) {
                            tracing::error!(job_id = %id, error = %e, "failed to persist next_pending transition to Fetching");
                        }
                        return cloned;
                    }
                }
            }
            self.notify.notified().await;
        }
    }

    pub async fn mark_status(&self, job_id: &str, status: IngestJobStatus) -> std::io::Result<()> {
        let mut state = self.inner.lock().await;
        if let Some(job) = state.jobs.get_mut(job_id) {
            job.status = status;
            state.updated_at = isoformat(now_utc());
            self.persist_locked(&state)
        } else {
            Ok(())
        }
    }

    pub async fn attach_article_id(&self, job_id: &str, article_id: String) {
        let mut state = self.inner.lock().await;
        if let Some(job) = state.jobs.get_mut(job_id) {
            job.article_id = Some(article_id);
            state.updated_at = isoformat(now_utc());
            if let Err(e) = self.persist_locked(&state) {
                tracing::error!(job_id = %job_id, error = %e, "failed to persist attach_article_id");
            }
        }
    }

    pub async fn finish_saved(
        &self,
        job_id: &str,
        article_id: String,
        summary: IngestOutcomeSummary,
        warnings: Vec<String>,
    ) {
        let mut state = self.inner.lock().await;
        if let Some(job) = state.jobs.get_mut(job_id) {
            job.status = IngestJobStatus::Saved;
            job.article_id = Some(article_id);
            job.outcome = Some(summary);
            job.warnings = warnings;
            job.finished_at = Some(isoformat(now_utc()));
            state.updated_at = isoformat(now_utc());
            if let Err(e) = self.persist_locked(&state) {
                tracing::error!(job_id = %job_id, error = %e, "failed to persist finish_saved; in-memory state says Saved but disk may be stale");
            }
        }
    }

    pub async fn finish_rejected(
        &self,
        job_id: &str,
        article_id: Option<String>,
        summary: IngestOutcomeSummary,
    ) {
        let mut state = self.inner.lock().await;
        if let Some(job) = state.jobs.get_mut(job_id) {
            job.status = IngestJobStatus::Rejected;
            job.article_id = article_id;
            job.outcome = Some(summary);
            job.finished_at = Some(isoformat(now_utc()));
            state.updated_at = isoformat(now_utc());
            if let Err(e) = self.persist_locked(&state) {
                tracing::error!(job_id = %job_id, error = %e, "failed to persist finish_rejected");
            }
        }
    }

    pub async fn finish_failed(&self, job_id: &str, error: IngestJobError) {
        let mut state = self.inner.lock().await;
        if let Some(job) = state.jobs.get_mut(job_id) {
            job.status = IngestJobStatus::Failed;
            job.error = Some(error);
            job.finished_at = Some(isoformat(now_utc()));
            state.updated_at = isoformat(now_utc());
            if let Err(e) = self.persist_locked(&state) {
                tracing::error!(job_id = %job_id, error = %e, "failed to persist finish_failed");
            }
        }
    }

    pub async fn get(&self, job_id: &str) -> Option<IngestJob> {
        let state = self.inner.lock().await;
        state.jobs.get(job_id).cloned()
    }

    pub async fn list(&self, filter: &ListFilter) -> Vec<IngestJob> {
        let state = self.inner.lock().await;
        let mut jobs: Vec<IngestJob> = state
            .jobs
            .values()
            .filter(|j| {
                if filter.only_failed && j.status != IngestJobStatus::Failed {
                    return false;
                }
                if let Some(s) = &filter.status {
                    if &j.status != s {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        jobs.sort_by(|a, b| b.submitted_at.cmp(&a.submitted_at));
        if let Some(limit) = filter.limit {
            jobs.truncate(limit);
        }
        jobs
    }

    fn persist_locked(&self, state: &IngestQueueState) -> std::io::Result<()> {
        if let Some(parent) = self.persistence_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_vec_pretty(state)
            .map_err(|e| std::io::Error::other(format!("serialize ingest jobs: {e}")))?;
        fs::write(&self.persistence_path, body)
    }

    fn persist_blocking(&self) -> std::io::Result<()> {
        let state = self
            .inner
            .try_lock()
            .map_err(|_| std::io::Error::other("queue locked during boot persist"))?;
        self.persist_locked(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::{ArticleMemoryHostProfile, ArticleMemoryIngestConfig};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_paths() -> (TempDir, RuntimePaths) {
        let tmp = TempDir::new().unwrap();
        let paths = RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        };
        (tmp, paths)
    }

    fn test_config() -> Arc<ArticleMemoryIngestConfig> {
        Arc::new(ArticleMemoryIngestConfig {
            host_profiles: vec![ArticleMemoryHostProfile {
                match_suffix: "zhihu.com".into(),
                profile: "articles-zhihu".into(),
                source: Some("zhihu".into()),
            }],
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn submit_creates_pending_job_and_persists() {
        let (_tmp, paths) = test_paths();
        let queue = IngestQueue::load_or_create(&paths, test_config());
        let resp = queue
            .submit(IngestRequest {
                url: "https://zhihu.com/p/1".into(),
                title: None,
                tags: vec!["tag1".into()],
                source_hint: Some("cli".into()),
            })
            .await
            .unwrap();
        assert_eq!(resp.status, IngestJobStatus::Pending);
        assert!(!resp.deduped);
        let job = queue.get(&resp.job_id).await.unwrap();
        assert_eq!(job.profile_name, "articles-zhihu");
        assert_eq!(job.resolved_source.as_deref(), Some("zhihu"));
        // disk file exists and round-trips
        let raw = std::fs::read_to_string(paths.article_memory_ingest_jobs_path()).unwrap();
        let state: IngestQueueState = serde_json::from_str(&raw).unwrap();
        assert!(state.jobs.contains_key(&resp.job_id));
        assert_eq!(state.pending.len(), 1);
    }

    #[tokio::test]
    async fn submit_rejects_invalid_url() {
        let (_tmp, paths) = test_paths();
        let queue = IngestQueue::load_or_create(&paths, test_config());
        let err = queue
            .submit(IngestRequest {
                url: "not a url".into(),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap_err();
        match err {
            IngestSubmitError::InvalidUrl(_) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_rejects_ssrf_targets() {
        let (_tmp, paths) = test_paths();
        let queue = IngestQueue::load_or_create(&paths, test_config());
        let err = queue
            .submit(IngestRequest {
                url: "http://127.0.0.1/admin".into(),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, IngestSubmitError::PrivateAddressBlocked(_)));
    }

    #[tokio::test]
    async fn submit_dedup_returns_existing_for_in_flight() {
        let (_tmp, paths) = test_paths();
        let queue = IngestQueue::load_or_create(&paths, test_config());
        let r1 = queue
            .submit(IngestRequest {
                url: "https://zhihu.com/p/1".into(),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap();
        let r2 = queue
            .submit(IngestRequest {
                url: "https://zhihu.com/p/1#anchor".into(),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap();
        assert_eq!(r1.job_id, r2.job_id);
        assert!(r2.deduped);
    }

    #[tokio::test]
    async fn submit_when_disabled_errors() {
        let (_tmp, paths) = test_paths();
        let mut cfg = (*test_config()).clone();
        cfg.enabled = false;
        let queue = IngestQueue::load_or_create(&paths, Arc::new(cfg));
        let err = queue
            .submit(IngestRequest {
                url: "https://zhihu.com/p/1".into(),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, IngestSubmitError::IngestDisabled));
    }

    #[tokio::test]
    async fn next_pending_blocks_until_submit() {
        let (_tmp, paths) = test_paths();
        let queue = Arc::new(IngestQueue::load_or_create(&paths, test_config()));
        let q2 = queue.clone();
        let handle = tokio::spawn(async move {
            let job = q2.next_pending().await;
            job
        });
        // give the worker a moment to park
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!handle.is_finished());
        queue
            .submit(IngestRequest {
                url: "https://zhihu.com/p/1".into(),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap();
        let job = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, IngestJobStatus::Fetching);
    }

    #[tokio::test]
    async fn notify_race_safety_all_submissions_consumed() {
        let (_tmp, paths) = test_paths();
        let queue = Arc::new(IngestQueue::load_or_create(&paths, test_config()));
        // 5 concurrent submits (unique URLs) and 5 concurrent next_pending calls.
        let mut submit_handles = Vec::new();
        for i in 0..5 {
            let q = queue.clone();
            submit_handles.push(tokio::spawn(async move {
                q.submit(IngestRequest {
                    url: format!("https://zhihu.com/p/{i}"),
                    title: None,
                    tags: vec![],
                    source_hint: None,
                })
                .await
                .unwrap()
            }));
        }
        let mut pop_handles = Vec::new();
        for _ in 0..5 {
            let q = queue.clone();
            pop_handles.push(tokio::spawn(async move { q.next_pending().await }));
        }
        for h in submit_handles {
            h.await.unwrap();
        }
        let mut ids = Vec::new();
        for h in pop_handles {
            let job = tokio::time::timeout(std::time::Duration::from_secs(2), h)
                .await
                .unwrap()
                .unwrap();
            ids.push(job.id);
        }
        ids.sort();
        ids.dedup();
        assert_eq!(
            ids.len(),
            5,
            "expected every submission to be observed exactly once"
        );
    }

    #[tokio::test]
    async fn load_or_create_resets_active_jobs_to_failed() {
        let (_tmp, paths) = test_paths();
        // write a fake state with one Fetching job
        std::fs::create_dir_all(paths.article_memory_dir()).unwrap();
        let state = IngestQueueState {
            version: 1,
            updated_at: "2026-04-24T00:00:00Z".into(),
            jobs: HashMap::from([(
                "abc".into(),
                IngestJob {
                    id: "abc".into(),
                    url: "https://zhihu.com/p/1".into(),
                    normalized_url: "https://zhihu.com/p/1".into(),
                    title_override: None,
                    tags: vec![],
                    source_hint: None,
                    profile_name: "articles-zhihu".into(),
                    resolved_source: Some("zhihu".into()),
                    status: IngestJobStatus::Fetching,
                    article_id: None,
                    outcome: None,
                    error: None,
                    warnings: vec![],
                    submitted_at: "2026-04-23T23:00:00Z".into(),
                    started_at: Some("2026-04-23T23:00:01Z".into()),
                    finished_at: None,
                    attempts: 1,
                },
            )]),
            pending: VecDeque::new(),
        };
        std::fs::write(
            paths.article_memory_ingest_jobs_path(),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();
        let queue = IngestQueue::load_or_create(&paths, test_config());
        let job = queue.get("abc").await.unwrap();
        assert_eq!(job.status, IngestJobStatus::Failed);
        let err = job.error.unwrap();
        assert_eq!(err.issue_type, "daemon_restart");
        assert_eq!(err.stage, "fetching");
    }

    #[tokio::test]
    async fn submit_dedup_conflicts_for_recent_saved() {
        let (_tmp, paths) = test_paths();
        let queue = IngestQueue::load_or_create(&paths, test_config());
        // Submit, then manually mark it Saved with a fresh timestamp.
        let resp = queue
            .submit(IngestRequest {
                url: "https://zhihu.com/p/1".into(),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap();
        queue
            .finish_saved(
                &resp.job_id,
                "article-abc".to_string(),
                IngestOutcomeSummary {
                    clean_status: "polished".into(),
                    clean_profile: "zhihu".into(),
                    value_decision: Some("save".into()),
                    value_score: Some(0.9),
                    normalized_chars: 1200,
                    polished: true,
                    summary_generated: true,
                    embedded: true,
                },
                Vec::new(),
            )
            .await;
        // Second submission must 409.
        let err = queue
            .submit(IngestRequest {
                url: "https://zhihu.com/p/1".into(),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap_err();
        match err {
            IngestSubmitError::DuplicateSaved {
                existing_article_id,
                finished_at,
            } => {
                assert_eq!(existing_article_id.as_deref(), Some("article-abc"));
                assert!(!finished_at.is_empty());
            }
            other => panic!("expected DuplicateSaved, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_dedup_allows_after_window_expires() {
        let (_tmp, paths) = test_paths();
        // Tight window so we can reason about "outside"
        let cfg = ArticleMemoryIngestConfig {
            dedup_window_hours: 1,
            host_profiles: vec![ArticleMemoryHostProfile {
                match_suffix: "zhihu.com".into(),
                profile: "articles-zhihu".into(),
                source: Some("zhihu".into()),
            }],
            ..Default::default()
        };
        let queue = IngestQueue::load_or_create(&paths, Arc::new(cfg));
        let resp = queue
            .submit(IngestRequest {
                url: "https://zhihu.com/p/1".into(),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap();
        queue
            .finish_saved(
                &resp.job_id,
                "article-old".to_string(),
                IngestOutcomeSummary {
                    clean_status: "polished".into(),
                    clean_profile: "zhihu".into(),
                    value_decision: Some("save".into()),
                    value_score: Some(0.9),
                    normalized_chars: 1200,
                    polished: true,
                    summary_generated: true,
                    embedded: true,
                },
                Vec::new(),
            )
            .await;
        // Manually backdate finished_at to 2 hours ago (outside 1h window).
        {
            let mut state = queue.inner.lock().await;
            let job = state.jobs.get_mut(&resp.job_id).unwrap();
            let two_hours_ago = now_utc() - Duration::hours(2);
            job.finished_at = Some(isoformat(two_hours_ago));
        }
        // Second submission should now be accepted as a NEW job.
        let resp2 = queue
            .submit(IngestRequest {
                url: "https://zhihu.com/p/1".into(),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap();
        assert_ne!(resp.job_id, resp2.job_id);
        assert!(!resp2.deduped);
    }
}
