//! `GET /article-memory/digest?topic=<slug>&since_days=<n>&top=<k>`
//!
//! Returns a topic-scoped + time-scoped summary of the article index. Designed
//! to be consumed by zeroclaw agent cron jobs that then format + deliver a
//! digest to Telegram/Slack. Davis does no scheduling or delivery itself.
//!
//! The handler is a thin wrapper around [`build_digest`]. Splitting them lets
//! unit tests exercise the filtering / ranking logic without constructing a
//! full `AppState` (which owns HA clients, the crawl4ai supervisor, etc).

use crate::article_memory::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
use crate::server::AppState;
use crate::RuntimePaths;
use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct DigestQuery {
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default = "default_since")]
    pub since_days: u32,
    #[serde(default = "default_top")]
    pub top: usize,
}

fn default_since() -> u32 {
    7
}
fn default_top() -> usize {
    10
}

#[derive(Debug, Serialize)]
pub struct DigestResponse {
    pub topic: Option<String>,
    pub window_days: u32,
    pub total: usize,
    pub by_decision: ByDecision,
    pub top: Vec<DigestItem>,
    pub recent_translations: Vec<DigestItem>,
}

#[derive(Debug, Serialize, Default)]
pub struct ByDecision {
    pub saved: usize,
    pub candidate: usize,
    pub rejected: usize,
}

#[derive(Debug, Serialize)]
pub struct DigestItem {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    pub score: Option<f32>,
    pub translated: bool,
    pub updated_at: String,
}

pub async fn handle(
    State(state): State<AppState>,
    Query(q): Query<DigestQuery>,
) -> Json<DigestResponse> {
    Json(build_digest(&state.paths, q))
}

/// Pure filtering/ranking core of the handler. Exposed to unit tests so we
/// can drive it with a seeded `RuntimePaths` without standing up an
/// `AppState`.
pub(crate) fn build_digest(paths: &RuntimePaths, q: DigestQuery) -> DigestResponse {
    let articles = match crate::article_memory::load_article_index(paths) {
        Ok(index) => index.articles,
        // Missing or malformed index is treated as "no articles" — the digest
        // endpoint is a read-only projection and must never 500 just because
        // article-memory hasn't been initialised yet.
        Err(_) => Vec::new(),
    };

    let cutoff = Utc::now() - Duration::days(q.since_days as i64);

    let mut filtered: Vec<ArticleMemoryRecord> = articles
        .into_iter()
        .filter(|r| within_window(r, cutoff))
        .filter(|r| topic_matches(r, q.topic.as_deref()))
        .collect();

    let mut counts = ByDecision::default();
    for r in &filtered {
        match r.status {
            ArticleMemoryRecordStatus::Saved => counts.saved += 1,
            ArticleMemoryRecordStatus::Candidate => counts.candidate += 1,
            ArticleMemoryRecordStatus::Rejected => counts.rejected += 1,
            ArticleMemoryRecordStatus::Archived => {}
        }
    }

    filtered.sort_by(|a, b| {
        b.value_score
            .unwrap_or(0.0)
            .partial_cmp(&a.value_score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top: Vec<DigestItem> = filtered.iter().take(q.top).map(to_item).collect();

    let mut translated: Vec<&ArticleMemoryRecord> = filtered
        .iter()
        .filter(|r| r.translation_path.is_some())
        .collect();
    translated.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let recent_translations: Vec<DigestItem> =
        translated.iter().take(q.top).map(|r| to_item(r)).collect();

    DigestResponse {
        topic: q.topic,
        window_days: q.since_days,
        total: filtered.len(),
        by_decision: counts,
        top,
        recent_translations,
    }
}

fn within_window(r: &ArticleMemoryRecord, cutoff: DateTime<Utc>) -> bool {
    crate::support::parse_iso(&r.updated_at)
        .map(|dt| dt >= cutoff)
        .unwrap_or(false)
}

fn topic_matches(r: &ArticleMemoryRecord, topic: Option<&str>) -> bool {
    match topic {
        None => true,
        Some(t) => {
            let tag = format!("topic:{t}");
            r.tags.iter().any(|candidate| candidate == &tag)
        }
    }
}

fn to_item(r: &ArticleMemoryRecord) -> DigestItem {
    DigestItem {
        id: r.id.clone(),
        title: r.title.clone(),
        url: r.url.clone(),
        score: r.value_score,
        translated: r.translation_path.is_some(),
        updated_at: r.updated_at.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::article_memory::{init_article_memory, load_article_index, save_article_index};
    use tempfile::TempDir;

    fn test_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn record(
        id: &str,
        topic: &str,
        score: f32,
        age_days: i64,
        translated: bool,
    ) -> ArticleMemoryRecord {
        let ts = (Utc::now() - Duration::days(age_days)).to_rfc3339();
        ArticleMemoryRecord {
            id: id.into(),
            title: id.into(),
            url: Some(format!("https://ex.com/{id}")),
            source: "t".into(),
            language: Some("en".into()),
            tags: vec![format!("topic:{topic}")],
            status: ArticleMemoryRecordStatus::Saved,
            value_score: Some(score),
            captured_at: ts.clone(),
            updated_at: ts,
            content_path: String::new(),
            raw_path: None,
            normalized_path: None,
            summary_path: None,
            translation_path: translated.then(|| format!("{id}/translation.md")),
            notes: None,
            clean_status: None,
            clean_profile: None,
        }
    }

    fn seed(paths: &RuntimePaths, records: Vec<ArticleMemoryRecord>) {
        init_article_memory(paths).unwrap();
        let mut idx = load_article_index(paths).unwrap();
        idx.articles = records;
        save_article_index(paths, &idx).unwrap();
    }

    #[tokio::test]
    async fn counts_by_decision_within_window() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(&tmp);
        seed(
            &paths,
            vec![
                record("a1", "rust", 0.9, 1, false),
                record("a2", "rust", 0.6, 2, true),
                record("old", "rust", 0.9, 100, false), // outside 7-day window
            ],
        );

        let resp = build_digest(
            &paths,
            DigestQuery {
                topic: Some("rust".into()),
                since_days: 7,
                top: 10,
            },
        );
        assert_eq!(resp.total, 2);
        assert_eq!(resp.by_decision.saved, 2);
        assert_eq!(resp.by_decision.candidate, 0);
        assert_eq!(resp.by_decision.rejected, 0);
        assert_eq!(resp.top.len(), 2);
        assert_eq!(resp.top[0].id, "a1", "higher score first");
        assert_eq!(resp.window_days, 7);
    }

    #[tokio::test]
    async fn filters_by_topic() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(&tmp);
        seed(
            &paths,
            vec![
                record("a1", "rust", 0.9, 1, false),
                record("b1", "python", 0.9, 1, false),
            ],
        );

        let resp = build_digest(
            &paths,
            DigestQuery {
                topic: Some("rust".into()),
                since_days: 7,
                top: 10,
            },
        );
        assert_eq!(resp.total, 1);
        assert_eq!(resp.top.len(), 1);
        assert_eq!(resp.top[0].id, "a1");
    }

    #[tokio::test]
    async fn recent_translations_only_if_translated() {
        let tmp = TempDir::new().unwrap();
        let paths = test_paths(&tmp);
        seed(
            &paths,
            vec![
                record("a1", "rust", 0.9, 1, true),
                record("a2", "rust", 0.6, 2, false),
            ],
        );

        let resp = build_digest(
            &paths,
            DigestQuery {
                topic: None,
                since_days: 7,
                top: 10,
            },
        );
        assert_eq!(resp.recent_translations.len(), 1);
        assert_eq!(resp.recent_translations[0].id, "a1");
        assert!(resp.recent_translations[0].translated);
    }
}
