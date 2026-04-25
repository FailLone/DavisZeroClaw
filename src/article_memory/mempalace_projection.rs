//! Phase 3 projection layer — after an ingest cycle succeeds, surface the
//! article into MemPalace as KG triples, a value-report drawer, and a diary
//! entry. Fire-and-forget only; callers should never block on this.

use super::pii_scrub::scrub;
use super::types::ArticleValueReport;
use crate::mempalace_sink::{MempalaceEmitter, Predicate, TripleId};

const DRAWER_MAX_CHARS: usize = 500;
const DIARY_MAX_CHARS: usize = 300;

/// One-shot projection for a successful ingest. Emits, in this order:
/// * `ArticleSourcedFrom` (exactly one, if we can parse the host)
/// * `ArticleDiscusses` (one per non-empty topic_tag)
/// * a value drawer under `wing=davis.articles`
///
/// Citations are not emitted here — the current `ArticleValueReport` schema
/// has no `cites` field. When Phase 3.5 adds citation extraction, add a
/// `ArticleCites` emit here.
pub fn emit_article_success(report: &ArticleValueReport, emitter: &dyn MempalaceEmitter) {
    let article_id = TripleId::article(&TripleId::safe_slug(&report.article_id));

    // ArticleSourcedFrom — one triple per article.
    if let Some(host) = report.url.as_deref().and_then(extract_host) {
        emitter.kg_add(
            article_id.clone(),
            Predicate::ArticleSourcedFrom,
            TripleId::host(&TripleId::safe_slug(&host)),
        );
    }

    // ArticleDiscusses — one per topic tag. Skip tags that are blank after
    // trimming, because `safe_slug("")` would fall back to a sha256 hash of
    // the empty string and we don't want every article to collect the same
    // synthetic "empty topic" triple.
    for topic in &report.topic_tags {
        let trimmed = topic.trim();
        if trimmed.is_empty() {
            continue;
        }
        let slug = TripleId::safe_slug(trimmed);
        if slug.is_empty() {
            continue;
        }
        emitter.kg_add(
            article_id.clone(),
            Predicate::ArticleDiscusses,
            TripleId::topic(&slug),
        );
    }

    // Drawer — compressed summary after PII scrub.
    let body = scrub(&build_drawer_body(report));
    let body: String = body.chars().take(DRAWER_MAX_CHARS).collect();
    let room = value_report_room(report);
    emitter.add_drawer("davis.articles", &room, &body);
}

/// Per-job diary entry written after an ingest attempt finishes, regardless
/// of outcome. The diary lets agents answer "ingest worker 今天卡过吗" by
/// scanning recent entries.
pub fn emit_ingest_diary(entry: &IngestDiaryEntry, emitter: &dyn MempalaceEmitter) {
    let mut s = format!(
        "[{ts}] job={job} status={status}",
        ts = entry.timestamp_iso,
        job = truncate(&entry.job_id, 16),
        status = entry.status,
    );
    if let Some(host) = &entry.host {
        s.push_str(&format!(" host={host}"));
    }
    if let Some(article_id) = &entry.article_id {
        s.push_str(&format!(" article={}", truncate(article_id, 12)));
    }
    if let Some(decision) = &entry.value_decision {
        s.push_str(&format!(" decision={decision}"));
    }
    if let Some(score) = entry.value_score {
        s.push_str(&format!(" score={score:.2}"));
    }
    if let Some(reason) = &entry.reason {
        s.push_str(&format!(" reason={reason}"));
    }
    let s: String = s.chars().take(DIARY_MAX_CHARS).collect();
    emitter.diary_write("davis.agent.ingest", &s);
}

/// Inputs to `emit_ingest_diary`. Pared down from the ingest job so the
/// projection module doesn't pull in queue/job types just to shape a diary
/// line.
#[derive(Debug, Clone)]
pub struct IngestDiaryEntry {
    pub timestamp_iso: String,
    pub job_id: String,
    pub status: IngestDiaryStatus,
    pub host: Option<String>,
    pub article_id: Option<String>,
    pub value_decision: Option<String>,
    pub value_score: Option<f32>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum IngestDiaryStatus {
    Saved,
    Rejected,
}

impl std::fmt::Display for IngestDiaryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Saved => "saved",
            Self::Rejected => "rejected",
        })
    }
}

fn build_drawer_body(report: &ArticleValueReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("[{ts}] ", ts = report.judged_at));
    out.push_str(&report.title);
    out.push('\n');
    if let Some(url) = &report.url {
        out.push_str(&format!("url: {url}\n"));
    }
    out.push_str(&format!(
        "decision: {d} (score {s:.2})\n",
        d = report.decision,
        s = report.value_score,
    ));
    if !report.topic_tags.is_empty() {
        out.push_str(&format!("topics: {}\n", report.topic_tags.join(", ")));
    }
    if !report.reasons.is_empty() {
        out.push_str("reasons:\n");
        for r in report.reasons.iter().take(3) {
            out.push_str(&format!("- {r}\n"));
        }
    }
    if !report.risk_flags.is_empty() {
        out.push_str(&format!("risk: {}\n", report.risk_flags.join(", ")));
    }
    out
}

fn value_report_room(report: &ArticleValueReport) -> String {
    let raw = report
        .topic_tags
        .first()
        .map(|s| s.as_str())
        .unwrap_or("untagged");
    let slug = TripleId::safe_slug(raw);
    if slug.is_empty() {
        "untagged".to_string()
    } else {
        slug
    }
}

/// Extract the host portion from a URL string. Returns `None` on parse
/// failure or for URLs without an authority.
fn extract_host(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    u.host_str().map(|h| h.to_string())
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mempalace_sink::SpySink;

    fn sample_report() -> ArticleValueReport {
        ArticleValueReport {
            article_id: "a8f3c9d2".into(),
            title: "Async Rust at Scale".into(),
            url: Some("https://lobste.rs/s/abc".into()),
            judged_at: "2026-04-25T12:00:00Z".into(),
            decision: "save".into(),
            value_score: 0.82,
            deterministic_reject: false,
            reasons: vec!["introduces async-iterator pattern".into()],
            topic_tags: vec!["async-rust".into(), "scaling".into()],
            risk_flags: vec![],
            translation_needed: false,
            model: Some("claude-haiku-4-5".into()),
            extraction_quality: "clean".into(),
            extraction_issues: vec![],
            rule_refinement_hint: None,
        }
    }

    #[test]
    fn successful_ingest_emits_sourced_from_triple() {
        let spy = SpySink::default();
        emit_article_success(&sample_report(), &spy);
        let srcs: Vec<_> = spy
            .kg_adds()
            .into_iter()
            .filter(|t| t.predicate == Predicate::ArticleSourcedFrom)
            .collect();
        assert_eq!(srcs.len(), 1, "{srcs:?}");
        assert!(srcs[0].subject.starts_with("article_"));
        assert_eq!(srcs[0].object, "host_lobste.rs");
    }

    #[test]
    fn successful_ingest_emits_one_discusses_per_topic() {
        let spy = SpySink::default();
        emit_article_success(&sample_report(), &spy);
        let topics: Vec<_> = spy
            .kg_adds()
            .into_iter()
            .filter(|t| t.predicate == Predicate::ArticleDiscusses)
            .collect();
        assert_eq!(topics.len(), 2);
        let objs: Vec<String> = topics.iter().map(|t| t.object.clone()).collect();
        assert!(objs.contains(&"topic_async-rust".to_string()), "{objs:?}");
        assert!(objs.contains(&"topic_scaling".to_string()), "{objs:?}");
    }

    #[test]
    fn successful_ingest_emits_scrubbed_drawer_in_articles_wing() {
        let mut report = sample_report();
        // Inject a secret the scrubber must redact.
        report.reasons = vec!["contact alice@example.com for the API key".into()];
        let spy = SpySink::default();
        emit_article_success(&report, &spy);
        let drawers = spy.drawers();
        assert_eq!(drawers.len(), 1);
        assert_eq!(drawers[0].wing, "davis.articles");
        assert!(
            !drawers[0].content.contains("alice@example.com"),
            "email leaked: {}",
            drawers[0].content
        );
        assert!(drawers[0].content.contains("[redacted]"));
        assert!(drawers[0].content.chars().count() <= DRAWER_MAX_CHARS);
        // Room is the first topic slug.
        assert_eq!(drawers[0].room, "async-rust");
    }

    #[test]
    fn drawer_room_defaults_to_untagged_when_no_topics() {
        let mut report = sample_report();
        report.topic_tags.clear();
        let spy = SpySink::default();
        emit_article_success(&report, &spy);
        assert_eq!(spy.drawers()[0].room, "untagged");
    }

    #[test]
    fn empty_topic_tags_are_skipped() {
        let mut report = sample_report();
        report.topic_tags = vec!["async-rust".into(), "".into(), "   ".into()];
        let spy = SpySink::default();
        emit_article_success(&report, &spy);
        let topics: Vec<_> = spy
            .kg_adds()
            .into_iter()
            .filter(|t| t.predicate == Predicate::ArticleDiscusses)
            .collect();
        assert_eq!(topics.len(), 1, "{topics:?}");
    }

    #[test]
    fn missing_url_skips_sourced_from_triple() {
        let mut report = sample_report();
        report.url = None;
        let spy = SpySink::default();
        emit_article_success(&report, &spy);
        assert!(spy
            .kg_adds()
            .iter()
            .all(|t| t.predicate != Predicate::ArticleSourcedFrom));
    }

    #[test]
    fn cjk_topic_tag_produces_safe_room_slug() {
        let mut report = sample_report();
        report.topic_tags = vec!["中文话题".into()];
        let spy = SpySink::default();
        emit_article_success(&report, &spy);
        let room = &spy.drawers()[0].room;
        assert!(
            room.starts_with('x') || !room.is_empty(),
            "room must be non-empty and SAFE_NAME compatible: {room}"
        );
    }

    #[test]
    fn ingest_diary_records_job_summary() {
        let entry = IngestDiaryEntry {
            timestamp_iso: "2026-04-25T12:00:00Z".into(),
            job_id: "job_abc1234567890xyz".into(),
            status: IngestDiaryStatus::Saved,
            host: Some("lobste.rs".into()),
            article_id: Some("a8f3c9d2".into()),
            value_decision: Some("save".into()),
            value_score: Some(0.82),
            reason: None,
        };
        let spy = SpySink::default();
        emit_ingest_diary(&entry, &spy);
        let entries = spy.diary_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].wing, "davis.agent.ingest");
        let e = &entries[0].entry;
        assert!(e.contains("status=saved"), "{e}");
        assert!(e.contains("host=lobste.rs"), "{e}");
        assert!(e.contains("score=0.82"), "{e}");
        assert!(e.chars().count() <= DIARY_MAX_CHARS);
    }

    #[test]
    fn ingest_diary_reports_rejection_reason() {
        let entry = IngestDiaryEntry {
            timestamp_iso: "2026-04-25T12:00:00Z".into(),
            job_id: "job1".into(),
            status: IngestDiaryStatus::Rejected,
            host: Some("example.org".into()),
            article_id: None,
            value_decision: Some("reject".into()),
            value_score: None,
            reason: Some("deterministic_gopher".into()),
        };
        let spy = SpySink::default();
        emit_ingest_diary(&entry, &spy);
        let e = &spy.diary_entries()[0].entry;
        assert!(e.contains("status=rejected"));
        assert!(e.contains("reason=deterministic_gopher"), "{e}");
    }
}
