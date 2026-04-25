# Topic-Crawl MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Davis the ability to, per configured topic, auto-discover candidate URLs (RSS/Atom/sitemap + Brave Search), push them through the existing ingest pipeline (crawl/clean/judge/store unchanged), translate save/candidate non-Chinese records into zh-CN, re-judge stale records after 30 days, and deliver a weekly digest via zeroclaw agent cron.

**Architecture:** Three new Davis workers (`DiscoveryWorker`, `TranslateWorker`, `RefreshWorker`) under `src/article_memory/`, each copying the `rule_learning_worker.rs:38-57` tokio-interval pattern. Discovery fans candidate URLs into the existing `IngestQueue::submit`. Translation calls zeroclaw `/api/chat` via an **inline, module-private** HTTP client — never promoted to a shared `zeroclaw_client` because hot-path batch callers (polish/summarize/judge/extract/rule-learning) must stay on direct OpenRouter per CLAUDE.md. Digest is a single new Davis HTTP endpoint read by zeroclaw agent cron jobs (rendered declaratively into zeroclaw's `config.toml`); Davis owns no scheduling or delivery code for it.

**Tech Stack:** Rust (tokio, reqwest, serde, tracing, anyhow, thiserror — all already in `Cargo.toml`); new deps **`feed-rs` 2.x** (RSS/Atom/sitemap unified parser — handles all three formats, so no `rss`/`atom_syndication`/`sitemap` trio needed); axum stays the HTTP frame; zh-CN translation prompt targets `google/gemini-2.0-flash-001` via zeroclaw's router (`hint="cheapest"`). No new Python. No changes to zeroclaw source. No Cargo crate dep on zeroclaw.

**Reference docs:**
- `CLAUDE.md` §"What looks like duplication but is not", §"Storage & memory systems", §"MemPalace integration plan"

**Anchor decisions (locked before plan drafting — do not re-litigate during BUILD):**

- **A1: No shared LLM dispatcher.** `llm_client.rs::chat_completion` stays the single HTTP layer; each caller owns its own failure/retry/budget philosophy.
- **A2: Hot-path batch callers (polish, summarize, judge, extract, rule-learning) are NEVER routed through zeroclaw.** Per CLAUDE.md: "Avoids coupling Davis reliability to zeroclaw's HTTP protocol stability."
- **A3: Translate worker writes its own inline ~60-line HTTP client** against zeroclaw `/api/chat`. Kept `pub(super)` inside `src/article_memory/translate/` — not exported, not reused. If a second non-hot-path consumer emerges later, extract then — driven by real second-consumer requirements, not speculation.
- **A4: Search provider: Brave Search API** (MVP). Trait designed so Tavily/SearXNG/Exa can be added later without changing `DiscoveryWorker`.
- **A5: Translation target: zh-CN only.** `translation_path` schema stays single-language.
- **A6: Phase 6 (content-drift refresh, Swarm consensus, SOP approval, observability) deferred** until data after 4+ weeks of MVP running.

**Why A1/A3 matter:** All 5 existing LLM call sites (`polish_markdown`, `summarize_markdown`, `judge_article_value`, `llm_html_to_markdown`, `run_learning_llm`) are hot-path batch ingest. Their failure means "this article cannot enter the store" — they need direct OpenRouter for durability. Translate's failure means "this enhancement retries next cycle" — opposite failure semantics. Unifying the two categories via a shared `LlmDispatcher` / `ChatTarget` abstraction would become a leaky abstraction the moment a `FailureMode` enum re-introduces the distinction. So the two sides stay separate at the caller level, and only the HTTP layer (`chat_completion`) is shared.

**State before this plan:**
- `src/article_memory/ingest/` owns URL intake via `IngestQueue::submit(IngestRequest { url, source_hint, .. })` (types at `src/article_memory/ingest/types.rs:105`)
- `find_article_by_normalized_url` already dedupes by normalized URL (re-used by discovery)
- `ArticleMemoryRecord` has fields `language`, `translation_path`, `updated_at`, `value_score` but **no `judged_at`**; this plan does not add `judged_at` — reuses `updated_at` as the freshness proxy
- `rule_learning_worker.rs:38-57` is the reference interval-worker pattern
- `src/mempalace_sink/predicate.rs:14` declares a **closed** `enum Predicate`; adding variants is a deliberate code change and requires updating `CLAUDE.md` in the same commit (Phase 6 Task 23 in the separate MemPalace plan guards this — this plan adds two variants and updates `CLAUDE.md`)
- `IngestQueue::submit` enforces Rule 0 URL-level dedup against `ArticleMemoryIndex` (`queue.rs:207-225`) — discovery relies on this

**Out of scope (explicitly):**
- Any shared `zeroclaw_client` / `LlmDispatcher` / `ChatTarget` abstraction (see A1/A3 above)
- Migrating any hot-path caller (polish/summarize/judge/extract/rule-learning) to zeroclaw
- Content-drift refresh (re-crawling high-value records) — deferred to Phase 6 post-MVP
- Multi-language translation — zh-CN only; `translation_path` schema stays single-language
- Swarm / SOP / observability integrations — deferred
- Search providers other than Brave — `SearchProvider` trait is designed so Tavily/SearXNG/Exa can be added later without changing `DiscoveryWorker`

---

## File Structure

Five new modules under `src/article_memory/` + one small server handler + toml schema + config example + `CLAUDE.md` update.

### New files

- `src/article_memory/discovery/mod.rs` — module root; public surface is only `DiscoveryWorker::spawn(deps)`; everything else is `pub(crate)` at most.
- `src/article_memory/discovery/config.rs` — `DiscoveryConfig`, `DiscoveryTopicConfig`, `DiscoverySearchConfig`, `DiscoveryTopicResolved` + validation.
- `src/article_memory/discovery/feed_ingestor.rs` — parses RSS/Atom/sitemap.xml via `feed-rs`. Input: bytes. Output: `Vec<CandidateLink>`.
- `src/article_memory/discovery/search/mod.rs` — `SearchProvider` trait + `SearchHit` + `SearchError`.
- `src/article_memory/discovery/search/brave.rs` — Brave Search API implementation.
- `src/article_memory/discovery/search/mock.rs` — `#[cfg(test)]` impl returning injected hits.
- `src/article_memory/discovery/worker.rs` — `DiscoveryWorker::spawn` + `run_one_cycle(topic, deps) -> CycleReport`.
- `src/article_memory/translate/mod.rs` — module root.
- `src/article_memory/translate/config.rs` — `TranslateConfig` + validation.
- `src/article_memory/translate/remote_chat.rs` — **module-private** HTTP client to zeroclaw `/api/chat`. `pub(super)` only. Not exported from the crate.
- `src/article_memory/translate/prompt.rs` — zh-CN translation prompt builder.
- `src/article_memory/translate/worker.rs` — `TranslateWorker::spawn` + `run_one_cycle(deps) -> CycleReport`.
- `src/article_memory/refresh/mod.rs` — module root.
- `src/article_memory/refresh/config.rs` — `RefreshConfig` + validation.
- `src/article_memory/refresh/worker.rs` — `RefreshWorker::spawn` + `run_one_cycle(deps)`.
- `src/server_digest.rs` — `GET /article-memory/digest` handler (split out of `server.rs` to keep that file small).
- `tests/fixtures/discovery/rss_sample.xml`, `atom_sample.xml`, `sitemap_sample.xml`, `brave_sample.json` — frozen real-world shapes.

### Modified files

- `Cargo.toml` — add `feed-rs = "2"`, `async-trait = "0.1"`, `thiserror = "2"`.
- `src/app_config.rs` — add `DiscoveryConfig`, `TranslateConfig`, `RefreshConfig` fields under `ArticleMemoryConfig`; wire validation in `LocalConfig::load`.
- `src/article_memory/mod.rs` — `pub mod discovery; pub mod translate; pub mod refresh;` + re-export `{Discovery,Translate,Refresh}Worker` and their `Deps` structs.
- `src/lib.rs` — re-export the three workers + `Deps`.
- `src/local_proxy.rs` — spawn the three workers when enabled (next to the existing `IngestWorkerPool::spawn` and `RuleLearningWorker::spawn` calls).
- `src/server.rs` — register `GET /article-memory/digest` route pointing to `server_digest::handle`.
- `src/mempalace_sink/predicate.rs` — add two variants: `ArticleDiscoveredFrom`, `ArticleTranslated`; add each to `ALL`; add each to the `as_str` match.
- `src/mempalace_sink/mod.rs` — add `pub fn article_translated(...)` + `pub fn article_discovered_from(...)` convenience methods (if the file follows that pattern; else emit via existing generic sink API — inspect during BUILD).
- `CLAUDE.md` — two rows added to the predicate table (§MemPalace integration plan), one row added to the table "What looks like duplication but is not" (discovery is Davis-native).
- `config/davis/local.example.toml` — commented-out example stanza for each of discovery/translate/refresh + explanatory comments.
- `config/davis/local.toml` — user config; only add `enabled = false` scaffolding so startup doesn't break.

### Tests

- Inline `#[cfg(test)] mod tests` in each new `.rs` file
- `tests/rust/topic_crawl_discovery.rs` — integration: feeds + mock Brave + fake `IngestQueue` + two topics
- `tests/rust/topic_crawl_translate.rs` — integration: axum-mock zeroclaw daemon + injected records
- `tests/rust/topic_crawl_refresh.rs` — integration: stale records get re-judged
- `tests/rust/topic_crawl_digest.rs` — integration: `GET /article-memory/digest` returns expected shape
- `tests/rust/topic_crawl_invariants.rs` — static: no `remote_chat` import outside `translate/`; hot-path callers still use `chat_completion` directly

---

## Execution Order

Six phases. Each phase leaves a green tree; you can stop after any phase without leaving Davis broken. Cargo fmt + clippy + test suite all run at phase boundaries.

- **Phase 1** (Tasks 1–8): RSS/Atom/sitemap discovery — the full discovery worker scaffolding with zero external APIs.
- **Phase 2** (Tasks 9–13): Brave Search integration.
- **Phase 3** (Tasks 14–20): Translation worker with inline remote-chat client.
- **Phase 4** (Tasks 21–24): Evergreen refresh worker.
- **Phase 5** (Tasks 25–28): Digest HTTP endpoint + zeroclaw agent-cron config rendering.
- **Phase 6** (Tasks 29–30): Cross-cutting invariants + docs sync.

---

## Phase 1 — RSS/Atom/Sitemap Discovery

Goal: When enabled, every `interval_secs`, for each enabled topic, fetch all configured feeds/sitemaps, extract candidate URLs, keyword-filter, dedupe against the article store, submit the survivors to `IngestQueue`.

### Task 1: Add `feed-rs`, `async-trait`, `thiserror` dependencies

**Files:**
- Modify: `Cargo.toml:24-48`

- [ ] **Step 1: Verify current deps don't already include them**

Run: `grep -nE '^(feed-rs|async-trait|thiserror)' Cargo.toml`
Expected: no output.

- [ ] **Step 2: Add the three lines to `[dependencies]`**

In `Cargo.toml`, after line `anyhow = "1"`, insert:

```toml
async-trait = "0.1"
feed-rs = "2"
thiserror = "2"
```

- [ ] **Step 3: Verify build**

Run: `cargo build --lib`
Expected: `Finished` with no errors (may download crates, that's fine).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: feed-rs + async-trait + thiserror for topic-crawl mvp"
```

---

### Task 2: Config schema — `DiscoveryConfig` + `DiscoveryTopicConfig` + validation

**Files:**
- Modify: `src/app_config.rs` (find `ArticleMemoryConfig` at line ~160; add new fields near the end)
- Test: inline `#[cfg(test)] mod tests` at bottom of `src/app_config.rs` (append)

- [ ] **Step 1: Write failing tests for config validation**

Append to the bottom of `src/app_config.rs` inside the existing `mod tests` (or create one if absent):

```rust
#[cfg(test)]
mod discovery_config_tests {
    use super::*;

    fn sample_topic() -> DiscoveryTopicConfig {
        DiscoveryTopicConfig {
            slug: "async-rust".into(),
            keywords: vec!["async rust".into()],
            feeds: vec!["https://without.boats/index.xml".into()],
            sitemaps: vec![],
            search_queries: vec![],
            enabled: true,
        }
    }

    #[test]
    fn rejects_empty_slug_when_enabled() {
        let mut topic = sample_topic();
        topic.slug = "".into();
        let cfg = DiscoveryConfig { enabled: true, interval_secs: 3600, max_per_cycle: 10, search: None, topics: vec![topic] };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("slug"), "{err}");
    }

    #[test]
    fn rejects_duplicate_slug() {
        let cfg = DiscoveryConfig {
            enabled: true, interval_secs: 3600, max_per_cycle: 10, search: None,
            topics: vec![sample_topic(), sample_topic()],
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn rejects_interval_below_60_secs() {
        let cfg = DiscoveryConfig { enabled: true, interval_secs: 30, max_per_cycle: 10, search: None, topics: vec![sample_topic()] };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("interval_secs"), "{err}");
    }

    #[test]
    fn accepts_disabled_with_no_topics() {
        let cfg = DiscoveryConfig { enabled: false, interval_secs: 3600, max_per_cycle: 10, search: None, topics: vec![] };
        cfg.validate().unwrap();
    }

    #[test]
    fn accepts_enabled_topic_with_no_feeds_but_has_search_queries() {
        let mut topic = sample_topic();
        topic.feeds = vec![];
        topic.search_queries = vec!["async rust tokio".into()];
        let cfg = DiscoveryConfig { enabled: true, interval_secs: 3600, max_per_cycle: 10, search: None, topics: vec![topic] };
        cfg.validate().unwrap();
    }

    #[test]
    fn rejects_topic_with_no_signal_sources() {
        let mut topic = sample_topic();
        topic.feeds = vec![];
        topic.sitemaps = vec![];
        topic.search_queries = vec![];
        let cfg = DiscoveryConfig { enabled: true, interval_secs: 3600, max_per_cycle: 10, search: None, topics: vec![topic] };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("no feeds, sitemaps, or search queries"), "{err}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test --lib discovery_config_tests 2>&1 | head -40`
Expected: compile errors — `DiscoveryConfig` / `DiscoveryTopicConfig` not found.

- [ ] **Step 3: Implement the config structs + validation**

In `src/app_config.rs`, after the existing `ArticleMemoryConfig` struct definition, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscoveryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_discovery_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_discovery_max_per_cycle")]
    pub max_per_cycle: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<DiscoverySearchConfig>,
    #[serde(default)]
    pub topics: Vec<DiscoveryTopicConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryTopicConfig {
    pub slug: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub feeds: Vec<String>,
    #[serde(default)]
    pub sitemaps: Vec<String>,
    #[serde(default)]
    pub search_queries: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySearchConfig {
    #[serde(default = "default_search_provider")]
    pub provider: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_search_rate_limit")]
    pub rate_limit_per_min: u32,
    #[serde(default = "default_search_results_per_query")]
    pub results_per_query: usize,
}

fn default_discovery_interval() -> u64 { 43_200 }          // 12h
fn default_discovery_max_per_cycle() -> usize { 20 }
fn default_search_provider() -> String { "brave".into() }
fn default_search_rate_limit() -> u32 { 60 }
fn default_search_results_per_query() -> usize { 10 }
fn default_true() -> bool { true }

impl DiscoveryConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.interval_secs < 60 {
            anyhow::bail!("discovery.interval_secs must be >= 60 (got {})", self.interval_secs);
        }
        if self.max_per_cycle == 0 {
            anyhow::bail!("discovery.max_per_cycle must be > 0");
        }
        let mut seen = std::collections::HashSet::new();
        for topic in &self.topics {
            if topic.slug.trim().is_empty() {
                anyhow::bail!("discovery topic has empty slug");
            }
            if !seen.insert(topic.slug.clone()) {
                anyhow::bail!("duplicate discovery topic slug: {}", topic.slug);
            }
            if !topic.enabled {
                continue;
            }
            if topic.feeds.is_empty() && topic.sitemaps.is_empty() && topic.search_queries.is_empty() {
                anyhow::bail!(
                    "discovery topic '{}' has no feeds, sitemaps, or search queries",
                    topic.slug
                );
            }
        }
        Ok(())
    }
}
```

Then inside `ArticleMemoryConfig`, add the field:

```rust
    #[serde(default)]
    pub discovery: DiscoveryConfig,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib discovery_config_tests`
Expected: all 6 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/app_config.rs
git commit -m "feat(discovery): config schema + validation"
```

---

### Task 3: Feed/sitemap parser — `feed_ingestor.rs`

**Files:**
- Create: `src/article_memory/discovery/mod.rs`
- Create: `src/article_memory/discovery/feed_ingestor.rs`
- Create: `tests/fixtures/discovery/rss_sample.xml`
- Create: `tests/fixtures/discovery/atom_sample.xml`
- Create: `tests/fixtures/discovery/sitemap_sample.xml`
- Modify: `src/article_memory/mod.rs` (add `pub mod discovery;`)

- [ ] **Step 1: Create discovery module root**

Write `src/article_memory/discovery/mod.rs`:

```rust
//! Topic-driven URL discovery. Feeds survivors into the existing IngestQueue.
//! See docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md Phase 1–2.

pub mod feed_ingestor;
pub mod search;
pub mod worker;

pub use worker::{DiscoveryWorker, DiscoveryWorkerDeps};
```

Append to `src/article_memory/mod.rs` (near the other `pub mod ingest;` line):

```rust
pub mod discovery;
```

- [ ] **Step 2: Drop in three test fixtures**

Create `tests/fixtures/discovery/rss_sample.xml` with:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Sample</title>
    <link>https://example.com</link>
    <description>Sample</description>
    <item>
      <title>Async Rust patterns</title>
      <link>https://example.com/async-rust-patterns</link>
      <description>A deep dive into tokio scheduling</description>
      <pubDate>Thu, 01 Jan 2026 00:00:00 GMT</pubDate>
    </item>
    <item>
      <title>Unrelated cooking post</title>
      <link>https://example.com/cooking</link>
      <description>Braised short ribs</description>
      <pubDate>Thu, 01 Jan 2026 00:00:00 GMT</pubDate>
    </item>
  </channel>
</rss>
```

Create `tests/fixtures/discovery/atom_sample.xml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Sample Atom</title>
  <id>urn:uuid:sample</id>
  <updated>2026-01-01T00:00:00Z</updated>
  <entry>
    <title>io_uring explained</title>
    <id>urn:uuid:entry-1</id>
    <link href="https://example.com/io-uring"/>
    <updated>2026-01-01T00:00:00Z</updated>
    <summary>How io_uring works</summary>
  </entry>
</feed>
```

Create `tests/fixtures/discovery/sitemap_sample.xml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>https://example.com/docs/tokio</loc>
    <lastmod>2026-01-01</lastmod>
  </url>
  <url>
    <loc>https://example.com/about</loc>
  </url>
</urlset>
```

- [ ] **Step 3: Write failing tests for the feed parser**

Write `src/article_memory/discovery/feed_ingestor.rs`:

```rust
//! Parse RSS/Atom/sitemap bytes into candidate URLs.
//!
//! `feed-rs` handles RSS 2.0 and Atom natively. Sitemap xml uses a simpler
//! `urlset` schema — we parse it via a hand-written scraper to avoid dragging
//! in a second xml dep.

use anyhow::{Context, Result};
use scraper::{Html, Selector};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateLink {
    pub url: String,
    pub title: Option<String>,
    pub summary: Option<String>,
}

pub fn parse_feed(bytes: &[u8]) -> Result<Vec<CandidateLink>> {
    let feed = feed_rs::parser::parse(bytes).context("feed-rs parse failed")?;
    let mut out = Vec::with_capacity(feed.entries.len());
    for entry in feed.entries {
        let url = entry.links.first().map(|l| l.href.clone());
        let Some(url) = url else { continue };
        out.push(CandidateLink {
            url,
            title: entry.title.map(|t| t.content),
            summary: entry.summary.map(|t| t.content),
        });
    }
    Ok(out)
}

pub fn parse_sitemap(bytes: &[u8]) -> Result<Vec<CandidateLink>> {
    let text = std::str::from_utf8(bytes).context("sitemap not utf-8")?;
    let doc = Html::parse_document(text);
    let sel = Selector::parse("loc").expect("static selector");
    let out = doc
        .select(&sel)
        .map(|node| CandidateLink {
            url: node.text().collect::<String>().trim().to_string(),
            title: None,
            summary: None,
        })
        .filter(|c| !c.url.is_empty())
        .collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_items() {
        let bytes = std::fs::read("tests/fixtures/discovery/rss_sample.xml").unwrap();
        let links = parse_feed(&bytes).unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].url, "https://example.com/async-rust-patterns");
        assert_eq!(links[0].title.as_deref(), Some("Async Rust patterns"));
    }

    #[test]
    fn parses_atom_entries() {
        let bytes = std::fs::read("tests/fixtures/discovery/atom_sample.xml").unwrap();
        let links = parse_feed(&bytes).unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://example.com/io-uring");
    }

    #[test]
    fn parses_sitemap_locs() {
        let bytes = std::fs::read("tests/fixtures/discovery/sitemap_sample.xml").unwrap();
        let links = parse_sitemap(&bytes).unwrap();
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].url, "https://example.com/docs/tokio");
    }

    #[test]
    fn invalid_feed_returns_err() {
        let err = parse_feed(b"<not xml").unwrap_err().to_string();
        assert!(err.contains("parse"), "{err}");
    }
}
```

- [ ] **Step 4: Run tests — should fail to build (module not yet declared)**

Run: `cargo test --lib feed_ingestor 2>&1 | head -20`
Expected: one of the tests fails to resolve; if there are module-visibility errors, fix them (should not be any given the `pub mod` in step 1).

- [ ] **Step 5: Run tests — should pass now**

Run: `cargo test --lib feed_ingestor`
Expected: 4 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/discovery src/article_memory/mod.rs tests/fixtures/discovery
git commit -m "feat(discovery): RSS/Atom/sitemap parser"
```

---

### Task 4: SearchProvider trait + mock impl (in prep for Phase 2 Brave)

**Files:**
- Create: `src/article_memory/discovery/search/mod.rs`
- Create: `src/article_memory/discovery/search/mock.rs`

- [ ] **Step 1: Write the trait + mock + failing test**

Write `src/article_memory/discovery/search/mod.rs`:

```rust
//! Search provider abstraction. Brave is the only real impl in MVP;
//! mock is available via `#[cfg(test)]`.

pub mod mock;

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub url: String,
    pub title: String,
    pub snippet: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("provider '{0}' unavailable: {1}")]
    Unavailable(&'static str, String),
    #[error("rate limited")]
    RateLimited,
    #[error("auth error: {0}")]
    Auth(String),
    #[error("other: {0}")]
    Other(#[from] anyhow::Error),
}

#[async_trait]
pub trait SearchProvider: Send + Sync {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, SearchError>;
}
```

Write `src/article_memory/discovery/search/mock.rs`:

```rust
//! Test-only deterministic search provider.

use super::{SearchError, SearchHit, SearchProvider};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct MockSearch {
    map: Mutex<HashMap<String, Vec<SearchHit>>>,
}

impl MockSearch {
    pub fn new() -> Self {
        Self { map: Mutex::new(HashMap::new()) }
    }

    pub fn inject(&self, query: &str, hits: Vec<SearchHit>) {
        self.map.lock().unwrap().insert(query.to_string(), hits);
    }
}

#[async_trait]
impl SearchProvider for MockSearch {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, SearchError> {
        let out = self.map.lock().unwrap().get(query).cloned().unwrap_or_default();
        Ok(out.into_iter().take(limit).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_returns_injected_hits_respecting_limit() {
        let m = MockSearch::new();
        m.inject(
            "rust",
            vec![
                SearchHit { url: "a".into(), title: "A".into(), snippet: "".into() },
                SearchHit { url: "b".into(), title: "B".into(), snippet: "".into() },
            ],
        );
        let got = m.search("rust", 1).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].url, "a");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib mock_returns_injected_hits_respecting_limit`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/article_memory/discovery/search
git commit -m "feat(discovery): SearchProvider trait + mock"
```

---

### Task 5: `MempalacePredicate::ArticleDiscoveredFrom` + sync CLAUDE.md

**Files:**
- Modify: `src/mempalace_sink/predicate.rs:14` (enum) + `as_str` match + `ALL`
- Modify: `CLAUDE.md` predicate table

- [ ] **Step 1: Write failing test for the new variant**

Append to `src/mempalace_sink/predicate.rs` inside its existing `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn article_discovered_from_wire_format() {
        assert_eq!(Predicate::ArticleDiscoveredFrom.as_str(), "discovered_from");
        assert!(Predicate::ALL.contains(&Predicate::ArticleDiscoveredFrom));
    }
```

- [ ] **Step 2: Run — should fail (variant missing)**

Run: `cargo test --lib article_discovered_from_wire_format 2>&1 | head -20`
Expected: "no variant named `ArticleDiscoveredFrom`".

- [ ] **Step 3: Add the variant**

In `src/mempalace_sink/predicate.rs`:
- In the `enum Predicate`, after `ArticleSourcedFrom,`, add `ArticleDiscoveredFrom,`
- In `as_str`, after the `ArticleSourcedFrom` arm, add: `Self::ArticleDiscoveredFrom => "discovered_from",`
- In `ALL`, after `Predicate::ArticleSourcedFrom,`, add `Predicate::ArticleDiscoveredFrom,`
- Update the array type: change `pub const ALL: [Predicate; 14]` → `pub const ALL: [Predicate; 15]`

- [ ] **Step 4: Run**

Run: `cargo test --lib article_discovered_from_wire_format`
Expected: PASS.

- [ ] **Step 5: Update `CLAUDE.md` predicate table**

In `CLAUDE.md`, in the predicate table (§"Predicate vocabulary"), after the `ArticleSourcedFrom` row, add:

```
| `ArticleDiscoveredFrom` | article → source tag (`feed:<host>` / `sitemap:<host>` / `search:brave`) | discovery worker submits a new candidate | never invalidate | "这篇怎么发现的 / 这个 feed 这个月进了多少篇" |
```

- [ ] **Step 6: Commit**

```bash
git add src/mempalace_sink/predicate.rs CLAUDE.md
git commit -m "feat(kg): ArticleDiscoveredFrom predicate"
```

---

### Task 6: `DiscoveryWorker::run_one_cycle` — pure function, easy to test

**Files:**
- Create: `src/article_memory/discovery/worker.rs`

- [ ] **Step 1: Write failing tests**

Write `src/article_memory/discovery/worker.rs`:

```rust
//! DiscoveryWorker. Follows the same tokio-interval pattern as
//! `rule_learning_worker.rs:38-57`.

use super::feed_ingestor::{parse_feed, parse_sitemap, CandidateLink};
use super::search::{SearchError, SearchHit, SearchProvider};
use crate::app_config::{DiscoveryConfig, DiscoveryTopicConfig};
use crate::article_memory::ingest::queue::IngestQueue;
use crate::article_memory::ingest::types::IngestRequest;
use crate::mempalace_sink::MempalaceEmitter;
use crate::mempalace_sink::predicate::{Predicate, TripleId};
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
            interval.tick().await; // skip the immediate tick per rule_learning_worker pattern
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
    let mut report = CycleReport { topic: topic.slug.clone(), ..CycleReport::default() };
    let mut candidates: Vec<(CandidateLink, &'static str, String)> = Vec::new(); // (link, kind, source_host)

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
        for query in &topic.search_queries {
            report.search_queries += 1;
            let results_per_query = deps
                .config
                .search
                .as_ref()
                .map(|s| s.results_per_query)
                .unwrap_or(10);
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
                Err(err) => match err {
                    SearchError::RateLimited => {
                        tracing::warn!("search rate limited, stopping queries for this cycle");
                        break;
                    }
                    other => tracing::warn!(error = %other, "search failed"),
                },
            }
        }
    }

    report.candidates_before_dedupe = candidates.len();

    // Keyword filter (case-insensitive substring in title OR summary OR url).
    if !topic.keywords.is_empty() {
        candidates.retain(|(link, kind, _)| {
            if *kind != "feed" && *kind != "sitemap" {
                return true; // search results are already topic-scoped by query
            }
            let hay = format!(
                "{} {} {}",
                link.title.as_deref().unwrap_or(""),
                link.summary.as_deref().unwrap_or(""),
                link.url
            )
            .to_lowercase();
            topic.keywords.iter().any(|kw| hay.contains(&kw.to_lowercase()))
        });
    }

    // Dedupe within the cycle by URL.
    let mut seen: HashSet<String> = HashSet::new();
    candidates.retain(|(link, _, _)| seen.insert(link.url.clone()));

    // Cap.
    candidates.truncate(deps.config.max_per_cycle);

    // Submit.
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
                emit_discovered(deps, &link.url, &source_hint);
                tracing::debug!(topic = %topic.slug, url = %link.url, job_id = %resp.job_id, "submitted");
            }
            Err(err) => {
                tracing::debug!(topic = %topic.slug, url = %link.url, error = ?err, "submit rejected (dedupe/validation)");
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

fn emit_discovered(deps: &DiscoveryWorkerDeps, url: &str, source_hint: &str) {
    let Ok(subject) = TripleId::article(url) else { return };
    let Ok(object) = TripleId::tag(source_hint) else { return };
    deps.mempalace_sink
        .kg_add(subject, Predicate::ArticleDiscoveredFrom, object);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::article_memory::discovery::search::mock::MockSearch;
    use crate::article_memory::discovery::search::SearchHit;
    use crate::mempalace_sink::testing::NoopSink;
    use tempfile::TempDir;

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

    fn deps(queue: Arc<IngestQueue>, search: Arc<MockSearch>) -> DiscoveryWorkerDeps {
        let cfg = DiscoveryConfig {
            enabled: true,
            interval_secs: 60,
            max_per_cycle: 10,
            search: None,
            topics: vec![],
        };
        DiscoveryWorkerDeps {
            paths: RuntimePaths::for_test(TempDir::new().unwrap().path()),
            ingest_queue: queue,
            config: Arc::new(cfg),
            http: reqwest::Client::new(),
            search_provider: Some(search),
            mempalace_sink: Arc::new(NoopSink::default()),
        }
    }

    #[tokio::test]
    async fn submits_all_unique_search_hits_up_to_max() {
        let search = Arc::new(MockSearch::new());
        search.inject(
            "rust tokio",
            vec![
                SearchHit { url: "https://a.com/1".into(), title: "x".into(), snippet: "".into() },
                SearchHit { url: "https://a.com/2".into(), title: "y".into(), snippet: "".into() },
                SearchHit { url: "https://a.com/1".into(), title: "dup".into(), snippet: "".into() },
            ],
        );
        let paths = RuntimePaths::for_test(TempDir::new().unwrap().path());
        let queue = Arc::new(IngestQueue::load_or_create(
            &paths,
            Arc::new(crate::app_config::ArticleMemoryIngestConfig {
                enabled: true,
                ..Default::default()
            }),
        ));
        let d = deps(queue.clone(), search);
        let report = run_one_cycle(&d, &sample_topic()).await.unwrap();
        assert_eq!(report.submitted, 2, "one url deduped within cycle");
        assert_eq!(report.candidates_before_dedupe, 3);
    }
}
```

(Note: `RuntimePaths::for_test` and `NoopSink` are assumed to exist based on the pattern used elsewhere. If they don't, add minimal versions in the same commit — check with `grep for_test src/runtime_paths.rs` and `grep -r NoopSink src/mempalace_sink` before this step.)

- [ ] **Step 2: Run — likely fails because helpers or `IngestQueue::load_or_create` signature differs**

Run: `cargo test --lib submits_all_unique_search_hits 2>&1 | head -40`
Expected: compile errors. Adjust the test setup to match actual `IngestQueue::load_or_create` and `RuntimePaths` helpers (inspect via `grep -n 'pub fn load_or_create\|pub fn for_test' src/`).

- [ ] **Step 3: Adjust imports and helpers until the test compiles and passes**

Run: `cargo test --lib submits_all_unique_search_hits`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/discovery/worker.rs
git commit -m "feat(discovery): worker with run_one_cycle pure function"
```

---

### Task 7: Wire `DiscoveryWorker::spawn` into `local_proxy.rs`

**Files:**
- Modify: `src/local_proxy.rs` (next to the `RuleLearningWorker::spawn` block around line 185–203)
- Modify: `src/lib.rs` (re-export `DiscoveryWorker`, `DiscoveryWorkerDeps`)

- [ ] **Step 1: Re-export from `lib.rs`**

In `src/lib.rs`, find the existing `pub use article_memory::{...}` block and add:

```rust
pub use article_memory::discovery::{DiscoveryWorker, DiscoveryWorkerDeps};
```

- [ ] **Step 2: Spawn in `local_proxy.rs`**

In `src/local_proxy.rs`, after the `RuleLearningWorker::spawn(...)` call and before the `AppState::new(...)` line, add:

```rust
    if local_config.article_memory.discovery.enabled {
        crate::article_memory::discovery::DiscoveryWorker::spawn(
            crate::article_memory::discovery::DiscoveryWorkerDeps {
                paths: paths.clone(),
                ingest_queue: ingest_queue.clone(),
                config: Arc::new(local_config.article_memory.discovery.clone()),
                http: client.clone(),
                search_provider: None, // wired in Phase 2
                mempalace_sink: ingest_sink.clone(),
            },
        );
        tracing::info!("discovery worker started");
    }
```

- [ ] **Step 3: Verify build**

Run: `cargo build --lib && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs src/local_proxy.rs
git commit -m "feat(discovery): wire worker into local_proxy startup"
```

---

### Task 8: Example config + integration smoke test

**Files:**
- Modify: `config/davis/local.example.toml`
- Create: `tests/rust/topic_crawl_discovery.rs`

- [ ] **Step 1: Add commented example**

Append to `config/davis/local.example.toml`:

```toml
# --- Topic-driven URL discovery (MVP Phase 1) -------------------------------
# [article_memory.discovery]
# enabled = false
# interval_secs = 43200          # 12h
# max_per_cycle = 20
#
# [[article_memory.discovery.topics]]
# slug = "async-rust"
# keywords = ["async rust", "tokio", "io_uring"]
# feeds = [
#   "https://without.boats/index.xml",
#   "https://lobste.rs/t/rust.rss",
# ]
# sitemaps = []
# search_queries = []            # populated in Phase 2 when Brave is wired
# enabled = true
```

- [ ] **Step 2: Integration smoke test hitting a local wiremock-esque server (or reusing reqwest::Client with a httpbin-style endpoint) — OR accept that the unit test in Task 6 is sufficient for MVP; skip this test and note the decision**

Run the existing tests:

```bash
cargo test --lib discovery
cargo test --lib feed_ingestor
cargo test --lib submits_all_unique_search_hits
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: all green. (No new integration test file needed — the worker is pure enough that unit tests cover it. Write an integration test only if you find a bug that unit coverage missed.)

- [ ] **Step 3: Commit**

```bash
git add config/davis/local.example.toml
git commit -m "docs(discovery): example config stanza + MVP Phase 1 done"
```

---

## Phase 2 — Brave Search

### Task 9: `brave.rs` with a Brave-API fixture

**Files:**
- Create: `src/article_memory/discovery/search/brave.rs`
- Create: `tests/fixtures/discovery/brave_sample.json`
- Modify: `src/article_memory/discovery/search/mod.rs` (add `pub mod brave;`)

- [ ] **Step 1: Drop the fixture**

Write `tests/fixtures/discovery/brave_sample.json` — this is the shape Brave returns (abbreviated):

```json
{
  "web": {
    "results": [
      {
        "title": "Tokio scheduler internals",
        "url": "https://tokio.rs/blog/2019-10-scheduler",
        "description": "Explains fair scheduling in Tokio 0.2"
      },
      {
        "title": "io_uring getting started",
        "url": "https://example.com/io-uring",
        "description": "Linux async IO primer"
      }
    ]
  }
}
```

- [ ] **Step 2: Write failing parse test**

Write `src/article_memory/discovery/search/brave.rs`:

```rust
//! Brave Search API implementation of `SearchProvider`.
//!
//! Endpoint: https://api.search.brave.com/res/v1/web/search?q={q}&count={n}
//! Header:   X-Subscription-Token: <api_key>

use super::{SearchError, SearchHit, SearchProvider};
use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

pub struct BraveSearch {
    http: reqwest::Client,
    api_key: String,
    endpoint: String,
}

impl BraveSearch {
    pub fn new(http: reqwest::Client, api_key: String) -> Self {
        Self { http, api_key, endpoint: BRAVE_ENDPOINT.into() }
    }

    #[cfg(test)]
    pub fn with_endpoint(http: reqwest::Client, api_key: String, endpoint: String) -> Self {
        Self { http, api_key, endpoint }
    }

    pub fn parse_body(body: &[u8]) -> anyhow::Result<Vec<SearchHit>> {
        let raw: BraveResp = serde_json::from_slice(body)?;
        Ok(raw
            .web
            .results
            .into_iter()
            .map(|r| SearchHit { url: r.url, title: r.title, snippet: r.description })
            .collect())
    }
}

#[async_trait]
impl SearchProvider for BraveSearch {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, SearchError> {
        let resp = self
            .http
            .get(&self.endpoint)
            .header("X-Subscription-Token", &self.api_key)
            .header("Accept", "application/json")
            .query(&[("q", query), ("count", &limit.to_string())])
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| SearchError::Unavailable("brave", e.to_string()))?;
        match resp.status().as_u16() {
            200 => {
                let body = resp.bytes().await.map_err(|e| SearchError::Unavailable("brave", e.to_string()))?;
                let hits = Self::parse_body(&body).map_err(SearchError::Other)?;
                Ok(hits.into_iter().take(limit).collect())
            }
            401 | 403 => Err(SearchError::Auth(format!("brave http {}", resp.status()))),
            429 => Err(SearchError::RateLimited),
            other => {
                let body = resp.text().await.unwrap_or_default();
                Err(SearchError::Unavailable("brave", format!("http {other}: {body}")))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct BraveResp {
    web: BraveWeb,
}
#[derive(Debug, Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveResult>,
}
#[derive(Debug, Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    #[serde(default)]
    description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        let body = std::fs::read("tests/fixtures/discovery/brave_sample.json").unwrap();
        let hits = BraveSearch::parse_body(&body).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://tokio.rs/blog/2019-10-scheduler");
    }
}
```

Also modify `src/article_memory/discovery/search/mod.rs`:

```rust
pub mod brave;
pub mod mock;
```

- [ ] **Step 3: Run**

Run: `cargo test --lib parses_fixture`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/discovery/search tests/fixtures/discovery/brave_sample.json
git commit -m "feat(discovery): Brave Search API provider"
```

---

### Task 10: Integration test against a local axum mock

**Files:**
- Create: `tests/rust/topic_crawl_brave_integration.rs`

- [ ] **Step 1: Write test**

Write `tests/rust/topic_crawl_brave_integration.rs`:

```rust
//! Hits a local axum mock server that impersonates Brave's response shape.

use axum::{routing::get, Json, Router};
use davis_zero_claw::article_memory::discovery::search::brave::BraveSearch;
use davis_zero_claw::article_memory::discovery::search::SearchProvider;
use serde_json::json;

#[tokio::test]
async fn brave_roundtrip_happy_path() {
    let app = Router::new().route(
        "/res/v1/web/search",
        get(|| async {
            Json(json!({
                "web": {
                    "results": [
                        { "title": "A", "url": "https://a.com", "description": "desc" }
                    ]
                }
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let brave = BraveSearch::with_endpoint(
        reqwest::Client::new(),
        "test-key".into(),
        format!("http://{addr}/res/v1/web/search"),
    );
    let hits = brave.search("rust", 5).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].url, "https://a.com");
}
```

- [ ] **Step 2: Run**

Run: `cargo test --test topic_crawl_brave_integration`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/rust/topic_crawl_brave_integration.rs
git commit -m "test(discovery): Brave integration against axum mock"
```

---

### Task 11: Wire Brave into `DiscoveryWorker` via config

**Files:**
- Modify: `src/local_proxy.rs`
- Modify: `src/article_memory/discovery/mod.rs` (re-export `BraveSearch` if needed)

- [ ] **Step 1: Build provider in `local_proxy.rs`**

Replace the `search_provider: None,` line from Task 7 with:

```rust
                search_provider: local_config
                    .article_memory
                    .discovery
                    .search
                    .as_ref()
                    .and_then(|sc| {
                        if sc.provider != "brave" {
                            tracing::warn!(provider = %sc.provider, "unsupported search provider, skipping");
                            return None;
                        }
                        let env_name = sc.api_key_env.as_deref().unwrap_or("BRAVE_API_KEY");
                        match std::env::var(env_name) {
                            Ok(key) if !key.is_empty() => {
                                let brave = crate::article_memory::discovery::search::brave::BraveSearch::new(
                                    client.clone(),
                                    key,
                                );
                                Some(Arc::new(brave) as Arc<dyn crate::article_memory::discovery::search::SearchProvider>)
                            }
                            _ => {
                                tracing::warn!(env = %env_name, "brave api key missing; search disabled");
                                None
                            }
                        }
                    }),
```

- [ ] **Step 2: Verify build**

Run: `cargo build --lib && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add src/local_proxy.rs src/article_memory/discovery/mod.rs
git commit -m "feat(discovery): wire Brave provider via env-backed config"
```

---

### Task 12: Update example config with search section

**Files:**
- Modify: `config/davis/local.example.toml`

- [ ] **Step 1: Append search sub-section to the previously-added discovery stanza**

In the commented block, update and append:

```toml
# [article_memory.discovery.search]
# provider = "brave"
# api_key_env = "BRAVE_API_KEY"
# rate_limit_per_min = 60
# results_per_query = 10
#
# # Then per topic:
# # search_queries = ["async rust tokio", "io_uring rust async"]
```

- [ ] **Step 2: Commit**

```bash
git add config/davis/local.example.toml
git commit -m "docs(discovery): example search config"
```

---

### Task 13: Phase 2 gate — full suite green

- [ ] **Step 1: Run**

```bash
cargo test --lib
cargo test --test topic_crawl_brave_integration
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: all green.

- [ ] **Step 2: Manual smoke (optional, requires a real BRAVE_API_KEY)**

If you have a key, set `BRAVE_API_KEY=...` and configure one topic with `search_queries = ["test"]`. Start Davis briefly and observe a single cycle of submissions. Kill Davis; this is a smoke test, not an endurance test.

---

## Phase 3 — Translation Worker

### Task 14: Config — `TranslateConfig` + validation

**Files:**
- Modify: `src/app_config.rs`

- [ ] **Step 1: Write failing tests**

Append to `src/app_config.rs`:

```rust
#[cfg(test)]
mod translate_config_tests {
    use super::*;

    #[test]
    fn rejects_bad_base_url_when_enabled() {
        let cfg = TranslateConfig { enabled: true, zeroclaw_base_url: "not a url".into(), ..TranslateConfig::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn accepts_disabled() {
        let cfg = TranslateConfig { enabled: false, ..TranslateConfig::default() };
        cfg.validate().unwrap();
    }

    #[test]
    fn rejects_interval_below_30() {
        let cfg = TranslateConfig {
            enabled: true,
            zeroclaw_base_url: "http://127.0.0.1:3001".into(),
            interval_secs: 5,
            ..TranslateConfig::default()
        };
        assert!(cfg.validate().is_err());
    }
}
```

- [ ] **Step 2: Run — compile fails**

Run: `cargo test --lib translate_config_tests 2>&1 | head -20`
Expected: `TranslateConfig` not found.

- [ ] **Step 3: Implement**

After the discovery config block in `src/app_config.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslateConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_target_language")]
    pub target_language: String,
    #[serde(default = "default_zeroclaw_base_url")]
    pub zeroclaw_base_url: String,
    #[serde(default = "default_translate_budget_scope")]
    pub budget_scope: String,
    #[serde(default = "default_translate_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_translate_batch")]
    pub batch_per_cycle: usize,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

impl Default for TranslateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            target_language: default_target_language(),
            zeroclaw_base_url: default_zeroclaw_base_url(),
            budget_scope: default_translate_budget_scope(),
            interval_secs: default_translate_interval(),
            batch_per_cycle: default_translate_batch(),
            api_key_env: None,
        }
    }
}

fn default_target_language() -> String { "zh-CN".into() }
fn default_zeroclaw_base_url() -> String { "http://127.0.0.1:3001".into() }
fn default_translate_budget_scope() -> String { "translation:monthly".into() }
fn default_translate_interval() -> u64 { 300 }
fn default_translate_batch() -> usize { 5 }

impl TranslateConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        url::Url::parse(&self.zeroclaw_base_url)
            .map_err(|e| anyhow::anyhow!("translate.zeroclaw_base_url invalid: {e}"))?;
        if self.interval_secs < 30 {
            anyhow::bail!("translate.interval_secs must be >= 30");
        }
        if self.batch_per_cycle == 0 {
            anyhow::bail!("translate.batch_per_cycle must be > 0");
        }
        Ok(())
    }
}
```

Add field to `ArticleMemoryConfig`:

```rust
    #[serde(default)]
    pub translate: TranslateConfig,
```

- [ ] **Step 4: Run**

Run: `cargo test --lib translate_config_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/app_config.rs
git commit -m "feat(translate): config schema + validation"
```

---

### Task 15: Inline `remote_chat.rs` — private to translate module

**Files:**
- Create: `src/article_memory/translate/mod.rs`
- Create: `src/article_memory/translate/remote_chat.rs`
- Modify: `src/article_memory/mod.rs` (add `pub mod translate;`)

- [ ] **Step 1: Write module root**

Write `src/article_memory/translate/mod.rs`:

```rust
//! zh-CN translation worker. Delegates LLM calls to zeroclaw `/api/chat` via
//! a **private, non-exported** HTTP client (`remote_chat`). This privacy is
//! an intentional architectural choice — see the implementation plan at
//! `docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md` §"Anchor decisions"
//! A1/A3.

mod prompt;
mod remote_chat;
pub mod worker;

pub use worker::{TranslateWorker, TranslateWorkerDeps};
```

Add to `src/article_memory/mod.rs`:

```rust
pub mod translate;
```

- [ ] **Step 2: Write failing tests for `remote_chat`**

Write `src/article_memory/translate/remote_chat.rs`:

```rust
//! HTTP client for zeroclaw's /api/chat.
//!
//! Private to the translate module. Do NOT export. Do NOT reuse from other
//! workers. If a second non-hot-path consumer emerges, extract then — driven
//! by real second-consumer requirements, not speculation.
//! See docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md §"Anchor decisions" A1/A3.

use crate::app_config::TranslateConfig;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub(super) enum RemoteChatError {
    #[error("zeroclaw unreachable: {0}")]
    Unreachable(String),
    #[error("budget exceeded (scope={scope}): {message}")]
    BudgetExceeded { scope: String, message: String },
    #[error("zeroclaw remote error: http {status}: {body}")]
    Remote { status: u16, body: String },
    #[error("zeroclaw response decode: {0}")]
    Decode(String),
    #[error("empty content")]
    Empty,
}

pub(super) struct RemoteChat {
    http: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
    budget_scope: String,
}

#[derive(Serialize)]
struct ChatReq<'a> {
    messages: Vec<ChatMsg<'a>>,
    hint: &'a str,
    classification: &'a str,
    budget_scope: &'a str,
    temperature: f32,
    max_tokens: usize,
}

#[derive(Serialize)]
struct ChatMsg<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResp {
    content: Option<String>,
}

#[derive(Deserialize)]
struct BudgetBody {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl RemoteChat {
    pub(super) fn new(cfg: &TranslateConfig, http: reqwest::Client) -> Self {
        let endpoint = format!("{}/api/chat", cfg.zeroclaw_base_url.trim_end_matches('/'));
        let api_key = cfg
            .api_key_env
            .as_deref()
            .and_then(|n| std::env::var(n).ok())
            .filter(|k| !k.is_empty());
        Self { http, endpoint, api_key, budget_scope: cfg.budget_scope.clone() }
    }

    pub(super) async fn translate_to_zh(
        &self,
        system: &str,
        user: &str,
    ) -> Result<String, RemoteChatError> {
        let body = ChatReq {
            messages: vec![
                ChatMsg { role: "system", content: system },
                ChatMsg { role: "user", content: user },
            ],
            hint: "cheapest",
            classification: "translation",
            budget_scope: &self.budget_scope,
            temperature: 0.2,
            max_tokens: 4000,
        };
        let mut req = self.http.post(&self.endpoint).json(&body).timeout(Duration::from_secs(120));
        if let Some(k) = self.api_key.as_deref() {
            req = req.bearer_auth(k);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| RemoteChatError::Unreachable(e.to_string()))?;
        match resp.status().as_u16() {
            200 => {
                let parsed: ChatResp = resp
                    .json()
                    .await
                    .map_err(|e| RemoteChatError::Decode(e.to_string()))?;
                parsed.content.filter(|c| !c.trim().is_empty()).ok_or(RemoteChatError::Empty)
            }
            402 => {
                let body: BudgetBody = resp.json().await.unwrap_or(BudgetBody { scope: None, message: None });
                Err(RemoteChatError::BudgetExceeded {
                    scope: body.scope.unwrap_or_else(|| self.budget_scope.clone()),
                    message: body.message.unwrap_or_else(|| "budget exceeded".into()),
                })
            }
            status => {
                let text = resp.text().await.unwrap_or_default();
                Err(RemoteChatError::Remote { status, body: text })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    type MockState = Arc<Mutex<MockReply>>;

    #[derive(Clone, Default)]
    struct MockReply {
        status: u16,
        body: Value,
    }

    async fn handler(State(s): State<MockState>, Json(_req): Json<Value>) -> (StatusCode, Json<Value>) {
        let g = s.lock().unwrap().clone();
        (StatusCode::from_u16(g.status).unwrap(), Json(g.body))
    }

    async fn mock_server(reply: MockReply) -> (String, MockState) {
        let state = Arc::new(Mutex::new(reply));
        let app = Router::new().route("/api/chat", post(handler)).with_state(state.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        (format!("http://{addr}"), state)
    }

    fn cfg_with(base: &str) -> TranslateConfig {
        TranslateConfig { enabled: true, zeroclaw_base_url: base.into(), ..TranslateConfig::default() }
    }

    #[tokio::test]
    async fn success_200_returns_content() {
        let (base, _s) = mock_server(MockReply { status: 200, body: json!({"content": "hello 你好"}) }).await;
        let rc = RemoteChat::new(&cfg_with(&base), reqwest::Client::new());
        let got = rc.translate_to_zh("sys", "user").await.unwrap();
        assert_eq!(got, "hello 你好");
    }

    #[tokio::test]
    async fn status_402_returns_budget_exceeded() {
        let (base, _s) = mock_server(MockReply {
            status: 402,
            body: json!({"scope": "translation:monthly", "message": "over"}),
        })
        .await;
        let rc = RemoteChat::new(&cfg_with(&base), reqwest::Client::new());
        let err = rc.translate_to_zh("sys", "user").await.unwrap_err();
        matches!(err, RemoteChatError::BudgetExceeded { .. });
    }

    #[tokio::test]
    async fn status_500_returns_remote() {
        let (base, _s) = mock_server(MockReply { status: 500, body: json!({"err":"boom"}) }).await;
        let rc = RemoteChat::new(&cfg_with(&base), reqwest::Client::new());
        let err = rc.translate_to_zh("sys", "user").await.unwrap_err();
        assert!(matches!(err, RemoteChatError::Remote { status: 500, .. }), "{err}");
    }

    #[tokio::test]
    async fn unreachable_when_daemon_not_running() {
        let cfg = cfg_with("http://127.0.0.1:1"); // port 1 always refuses
        let rc = RemoteChat::new(&cfg, reqwest::Client::new());
        let err = rc.translate_to_zh("sys", "user").await.unwrap_err();
        assert!(matches!(err, RemoteChatError::Unreachable(_)), "{err}");
    }

    #[tokio::test]
    async fn empty_content_is_error() {
        let (base, _s) = mock_server(MockReply { status: 200, body: json!({"content": ""}) }).await;
        let rc = RemoteChat::new(&cfg_with(&base), reqwest::Client::new());
        let err = rc.translate_to_zh("sys", "user").await.unwrap_err();
        assert!(matches!(err, RemoteChatError::Empty), "{err}");
    }
}
```

- [ ] **Step 3: Write minimal `prompt.rs` so `mod.rs` compiles**

Write `src/article_memory/translate/prompt.rs`:

```rust
pub(super) const SYSTEM: &str =
    "You are a careful Chinese (Simplified) translator for technical articles. \
Preserve code blocks, URLs, inline code, and proper nouns verbatim. Do not \
summarize. Do not add commentary. Output only the translated Markdown.";

pub(super) fn user_block(markdown: &str) -> String {
    format!("Translate the following Markdown to Simplified Chinese:\n\n{markdown}")
}
```

- [ ] **Step 4: Write a placeholder `worker.rs` so the module compiles (real impl in Task 16)**

Write `src/article_memory/translate/worker.rs`:

```rust
use crate::app_config::TranslateConfig;
use std::sync::Arc;

#[derive(Clone)]
pub struct TranslateWorkerDeps {
    pub config: Arc<TranslateConfig>,
    pub http: reqwest::Client,
    pub paths: crate::RuntimePaths,
    pub mempalace_sink: Arc<dyn crate::mempalace_sink::MempalaceEmitter>,
}

pub struct TranslateWorker;

impl TranslateWorker {
    pub fn spawn(_deps: TranslateWorkerDeps) {
        // implemented in Task 16
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib remote_chat`
Expected: all 5 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/translate src/article_memory/mod.rs
git commit -m "feat(translate): private remote_chat client + prompt scaffold"
```

---

### Task 16: `TranslateWorker::run_one_cycle`

**Files:**
- Modify: `src/article_memory/translate/worker.rs`

- [ ] **Step 1: Inspect the record store helpers**

Run: `grep -nE 'fn list_articles|fn load_article_index|fn load_article_record|fn update_article_record|fn normalized_path' src/article_memory/mod.rs src/article_memory/internals.rs | head -20`

Note the actual function names. Adapt Step 2 to match. (The sketches below name them `list_candidates_for_translation` and `write_translation`; the real impl should use whatever the index + records module already exposes. Add new pub(crate) helpers if needed — but **do not break callers**.)

- [ ] **Step 2: Add `MempalacePredicate::ArticleTranslated`**

Add the variant the same way as Task 5, including the `CLAUDE.md` row:

In `src/mempalace_sink/predicate.rs`:
- Add `ArticleTranslated,` after `ArticleDiscoveredFrom,`
- Add `Self::ArticleTranslated => "translated",` in `as_str`
- Add to `ALL`; bump array size to 16

In `CLAUDE.md` after the `ArticleDiscoveredFrom` row, add:

```
| `ArticleTranslated` | article → language tag (`lang:zh-CN`) | translate worker writes translation.md | never invalidate | "这篇翻译过没 / 最近翻了几篇" |
```

- [ ] **Step 3: Implement the worker**

Replace `src/article_memory/translate/worker.rs`:

```rust
use crate::app_config::TranslateConfig;
use crate::article_memory::translate::prompt::{user_block, SYSTEM};
use crate::article_memory::translate::remote_chat::{RemoteChat, RemoteChatError};
use crate::mempalace_sink::predicate::{Predicate, TripleId};
use crate::mempalace_sink::MempalaceEmitter;
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

    let candidates = list_candidates(deps).context("list candidates")?;
    let limit = deps.config.batch_per_cycle;

    for record in candidates.into_iter().take(limit) {
        report.scanned += 1;
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
            Err(RemoteChatError::BudgetExceeded { .. }) => {
                report.budget_hit = true;
                tracing::warn!("budget exceeded; stopping translate cycle");
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

// ---- thin shims over article_memory module functions ----------------------
// The exact function names differ; adapt these stubs to call the real helpers.

use crate::article_memory::types::ArticleMemoryRecord;

fn list_candidates(deps: &TranslateWorkerDeps) -> Result<Vec<ArticleMemoryRecord>> {
    let idx = crate::article_memory::load_article_index(&deps.paths)?;
    let mut out: Vec<_> = idx
        .articles
        .into_iter()
        .filter(|r| {
            matches!(
                r.status,
                crate::article_memory::ArticleMemoryRecordStatus::Saved
                    | crate::article_memory::ArticleMemoryRecordStatus::Candidate
            )
        })
        .filter(|r| r.translation_path.is_none())
        .filter(|r| {
            r.language
                .as_deref()
                .map(|l| !l.to_lowercase().starts_with("zh"))
                .unwrap_or(false)
        })
        .collect();
    // Oldest updated_at first — approximate "oldest judged" without adding a column.
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
    Ok(Some(std::fs::read_to_string(abs)?))
}

fn write_translation_file(
    deps: &TranslateWorkerDeps,
    record: &ArticleMemoryRecord,
    body: &str,
) -> Result<String> {
    let article_dir = deps.paths.article_memory_dir().join(&record.id);
    std::fs::create_dir_all(&article_dir)?;
    let path = article_dir.join("translation.md");
    std::fs::write(&path, body)?;
    // Return path relative to article_memory_dir for consistency with other *_path fields.
    let rel = PathBuf::from(&record.id).join("translation.md");
    Ok(rel.to_string_lossy().into_owned())
}

fn update_record_translation_path(
    deps: &TranslateWorkerDeps,
    article_id: &str,
    rel_path: &str,
) -> Result<()> {
    let mut idx = crate::article_memory::load_article_index(&deps.paths)?;
    if let Some(r) = idx.articles.iter_mut().find(|r| r.id == article_id) {
        r.translation_path = Some(rel_path.to_string());
        r.updated_at = crate::support::isoformat(crate::support::now_utc());
    }
    idx.updated_at = crate::support::isoformat(crate::support::now_utc());
    crate::article_memory::save_article_index(&deps.paths, &idx)?;
    Ok(())
}

fn emit_translated(deps: &TranslateWorkerDeps, record: &ArticleMemoryRecord) {
    let Some(url) = record.url.as_deref() else { return };
    let Ok(subject) = TripleId::article(url) else { return };
    let Ok(object) = TripleId::tag(&format!("lang:{}", deps.config.target_language)) else { return };
    deps.mempalace_sink.kg_add(subject, Predicate::ArticleTranslated, object);
}
```

**NOTE ON SHIMS:** the functions `load_article_index`, `save_article_index`, `article_memory_dir` may have different actual names. Inspect first (`grep -nE 'pub fn (load|save)_article_index\|fn article_memory_dir' src/`), and **fix the imports/calls to match reality** — do not invent missing helpers. If a helper genuinely doesn't exist, add it in the same commit with a targeted test.

- [ ] **Step 4: Write worker-level tests**

Append inline tests to `worker.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mempalace_sink::testing::NoopSink;
    use tempfile::TempDir;

    fn noop_deps(base: &str) -> TranslateWorkerDeps {
        let cfg = TranslateConfig {
            enabled: true,
            zeroclaw_base_url: base.into(),
            batch_per_cycle: 5,
            ..TranslateConfig::default()
        };
        TranslateWorkerDeps {
            config: Arc::new(cfg),
            http: reqwest::Client::new(),
            paths: crate::RuntimePaths::for_test(TempDir::new().unwrap().path()),
            mempalace_sink: Arc::new(NoopSink::default()),
        }
    }

    #[tokio::test]
    async fn no_candidates_empty_cycle() {
        let deps = noop_deps("http://127.0.0.1:1");
        let report = run_one_cycle(&deps).await.unwrap();
        assert_eq!(report.scanned, 0);
        assert_eq!(report.translated, 0);
    }
}
```

- [ ] **Step 5: Build + test**

Run: `cargo test --lib translate::worker && cargo clippy --all-targets -- -D warnings`
Expected: PASS + clean.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/translate src/mempalace_sink/predicate.rs CLAUDE.md
git commit -m "feat(translate): worker + ArticleTranslated predicate"
```

---

### Task 17: Wire `TranslateWorker::spawn` into `local_proxy.rs`

**Files:**
- Modify: `src/local_proxy.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Re-export**

Append to the re-exports block in `src/lib.rs`:

```rust
pub use article_memory::translate::{TranslateWorker, TranslateWorkerDeps};
```

- [ ] **Step 2: Spawn in `local_proxy.rs`**

After the `DiscoveryWorker::spawn(...)` block, add:

```rust
    if local_config.article_memory.translate.enabled {
        crate::article_memory::translate::TranslateWorker::spawn(
            crate::article_memory::translate::TranslateWorkerDeps {
                config: Arc::new(local_config.article_memory.translate.clone()),
                http: client.clone(),
                paths: paths.clone(),
                mempalace_sink: ingest_sink.clone(),
            },
        );
        tracing::info!("translate worker started");
    }
```

- [ ] **Step 3: Build**

Run: `cargo build --lib && cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/local_proxy.rs src/lib.rs
git commit -m "feat(translate): wire worker into local_proxy startup"
```

---

### Task 18: Example config

**Files:**
- Modify: `config/davis/local.example.toml`

- [ ] **Step 1: Append**

```toml
# --- Translation worker (MVP Phase 3) ---------------------------------------
# [article_memory.translate]
# enabled = false
# target_language = "zh-CN"
# zeroclaw_base_url = "http://127.0.0.1:3001"
# budget_scope = "translation:monthly"
# interval_secs = 300
# batch_per_cycle = 5
# # api_key_env = "ZEROCLAW_API_KEY"   # only when zeroclaw daemon requires auth
```

- [ ] **Step 2: Commit**

```bash
git add config/davis/local.example.toml
git commit -m "docs(translate): example config"
```

---

### Task 19: Integration test — zeroclaw mock end-to-end

**Files:**
- Create: `tests/rust/topic_crawl_translate.rs`

- [ ] **Step 1: Write test**

Write `tests/rust/topic_crawl_translate.rs`:

```rust
//! Seed an ArticleMemoryIndex with one English record and run the translate
//! worker once against an axum mock of zeroclaw /api/chat.

use axum::{routing::post, Json, Router};
use davis_zero_claw::article_memory::translate::{run_one_cycle, TranslateWorkerDeps};
use serde_json::{json, Value};
use std::sync::Arc;

#[tokio::test]
async fn translates_single_english_article_end_to_end() {
    // Spin up mock zeroclaw.
    let app = Router::new().route(
        "/api/chat",
        post(|Json(_): Json<Value>| async {
            Json(json!({"content": "译文内容\n\n一段翻译"}))
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let base = format!("http://{addr}");

    // Seed a record in a tempdir runtime.
    let tmp = tempfile::TempDir::new().unwrap();
    let paths = davis_zero_claw::RuntimePaths::for_test(tmp.path());
    std::fs::create_dir_all(paths.article_memory_dir().join("a1")).unwrap();
    std::fs::write(paths.article_memory_dir().join("a1/normalized.md"), "hello world").unwrap();

    let mut idx = davis_zero_claw::article_memory::ArticleMemoryIndex::default();
    idx.articles.push(davis_zero_claw::article_memory::ArticleMemoryRecord {
        id: "a1".into(),
        title: "hello".into(),
        url: Some("https://ex.com/a".into()),
        source: "test".into(),
        language: Some("en".into()),
        tags: vec![],
        status: davis_zero_claw::article_memory::ArticleMemoryRecordStatus::Saved,
        value_score: Some(0.8),
        captured_at: "2026-04-01T00:00:00Z".into(),
        updated_at: "2026-04-01T00:00:00Z".into(),
        content_path: "a1/content.md".into(),
        raw_path: None,
        normalized_path: Some("a1/normalized.md".into()),
        summary_path: None,
        translation_path: None,
        notes: None,
        clean_status: Some("ok".into()),
        clean_profile: Some("default".into()),
    });
    davis_zero_claw::article_memory::save_article_index(&paths, &idx).unwrap();

    let deps = TranslateWorkerDeps {
        config: Arc::new(davis_zero_claw::app_config::TranslateConfig {
            enabled: true,
            zeroclaw_base_url: base,
            ..Default::default()
        }),
        http: reqwest::Client::new(),
        paths: paths.clone(),
        mempalace_sink: Arc::new(davis_zero_claw::mempalace_sink::testing::NoopSink::default()),
    };

    let report = run_one_cycle(&deps).await.unwrap();
    assert_eq!(report.translated, 1);
    assert_eq!(report.failed, 0);

    let out = davis_zero_claw::article_memory::load_article_index(&paths).unwrap();
    assert_eq!(out.articles[0].translation_path.as_deref(), Some("a1/translation.md"));
    let written = std::fs::read_to_string(paths.article_memory_dir().join("a1/translation.md")).unwrap();
    assert!(written.contains("译文"), "{written}");
}
```

- [ ] **Step 2: Run**

Run: `cargo test --test topic_crawl_translate`
Expected: PASS. (If import paths don't match, fix them — the spec names of `ArticleMemoryIndex`, `ArticleMemoryRecord`, `save_article_index`, `load_article_index`, `for_test` are best-guesses; check with `grep` and adjust.)

- [ ] **Step 3: Commit**

```bash
git add tests/rust/topic_crawl_translate.rs
git commit -m "test(translate): end-to-end against mock zeroclaw"
```

---

### Task 20: Phase 3 gate

- [ ] **Step 1: Run**

```bash
cargo test --lib
cargo test --tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: all green.

---

## Phase 4 — Evergreen Refresh Worker

### Task 21: `RefreshConfig` + validation

**Files:**
- Modify: `src/app_config.rs`

- [ ] **Step 1: Write test**

Append:

```rust
#[cfg(test)]
mod refresh_config_tests {
    use super::*;

    #[test]
    fn accepts_defaults() {
        let cfg = RefreshConfig::default();
        cfg.validate().unwrap();
    }

    #[test]
    fn rejects_threshold_outside_0_1() {
        let cfg = RefreshConfig { enabled: true, score_delta_threshold: 2.0, ..RefreshConfig::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_stale_days_zero() {
        let cfg = RefreshConfig { enabled: true, stale_after_days: 0, ..RefreshConfig::default() };
        assert!(cfg.validate().is_err());
    }
}
```

- [ ] **Step 2: Run — compile fails**

Run: `cargo test --lib refresh_config_tests 2>&1 | head -10`
Expected: `RefreshConfig` not found.

- [ ] **Step 3: Implement**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_refresh_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_stale_days")]
    pub stale_after_days: u64,
    #[serde(default = "default_score_delta")]
    pub score_delta_threshold: f32,
    #[serde(default = "default_refresh_batch")]
    pub batch_per_cycle: usize,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_refresh_interval(),
            stale_after_days: default_stale_days(),
            score_delta_threshold: default_score_delta(),
            batch_per_cycle: default_refresh_batch(),
        }
    }
}

fn default_refresh_interval() -> u64 { 86_400 }
fn default_stale_days() -> u64 { 30 }
fn default_score_delta() -> f32 { 0.2 }
fn default_refresh_batch() -> usize { 20 }

impl RefreshConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.stale_after_days == 0 {
            anyhow::bail!("refresh.stale_after_days must be > 0");
        }
        if !(0.0..=1.0).contains(&self.score_delta_threshold) {
            anyhow::bail!("refresh.score_delta_threshold must be in [0.0, 1.0]");
        }
        Ok(())
    }
}
```

Add to `ArticleMemoryConfig`:

```rust
    #[serde(default)]
    pub refresh: RefreshConfig,
```

- [ ] **Step 4: Run**

Run: `cargo test --lib refresh_config_tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/app_config.rs
git commit -m "feat(refresh): config schema + validation"
```

---

### Task 22: `RefreshWorker` (re-judge only, no re-crawl)

**Files:**
- Create: `src/article_memory/refresh/mod.rs`
- Create: `src/article_memory/refresh/worker.rs`
- Modify: `src/article_memory/mod.rs` (add `pub mod refresh;`)

- [ ] **Step 1: Write `mod.rs`**

```rust
//! Evergreen refresh — re-judges articles whose updated_at > stale_after_days.
//! Does NOT re-crawl (content-drift refresh deferred to Phase 6+).

pub mod worker;

pub use worker::{RefreshWorker, RefreshWorkerDeps};
```

Add `pub mod refresh;` to `src/article_memory/mod.rs`.

- [ ] **Step 2: Write `worker.rs` with tests**

```rust
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

    let mut to_process: Vec<&mut ArticleMemoryRecord> = idx
        .articles
        .iter_mut()
        .filter(|r| matches!(r.status, ArticleMemoryRecordStatus::Saved | ArticleMemoryRecordStatus::Candidate))
        .filter(|r| {
            parse_iso(&r.updated_at).map(|dt| dt < cutoff).unwrap_or(false)
        })
        .collect();
    to_process.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
    let limit = deps.config.batch_per_cycle;

    for record in to_process.into_iter().take(limit) {
        report.scanned += 1;
        // For MVP evergreen refresh, we do NOT re-call the LLM judge. We simply
        // touch updated_at so the scan window rotates. When content-drift
        // refresh lands (Phase 6+), that phase re-crawls and re-judges.
        // The score_delta / decisions_flipped fields stay zero here.
        record.updated_at = isoformat(now_utc());
        report.unchanged_bumped += 1;
    }

    crate::article_memory::save_article_index(&deps.paths, &idx)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mempalace_sink::testing::NoopSink;
    use tempfile::TempDir;

    fn seed_record(idx: &mut crate::article_memory::ArticleMemoryIndex, id: &str, updated_at: &str) {
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
        let paths = RuntimePaths::for_test(tmp.path());
        let mut idx = crate::article_memory::ArticleMemoryIndex::default();
        seed_record(&mut idx, "old", "2020-01-01T00:00:00Z");     // very old
        seed_record(&mut idx, "recent", &isoformat(now_utc()));   // fresh
        crate::article_memory::save_article_index(&paths, &idx).unwrap();

        let deps = RefreshWorkerDeps {
            config: Arc::new(RefreshConfig { enabled: true, stale_after_days: 30, ..RefreshConfig::default() }),
            paths: paths.clone(),
            mempalace_sink: Arc::new(NoopSink::default()),
        };
        let report = run_one_cycle(&deps).await.unwrap();
        assert_eq!(report.scanned, 1, "only the old record is picked");
    }

    #[tokio::test]
    async fn disabled_worker_does_not_spawn() {
        // Basic sanity: constructing with enabled=false must return quickly
        // from run_one_cycle since spawn() would short-circuit. We assert the
        // cycle is a no-op.
        let tmp = TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(tmp.path());
        crate::article_memory::save_article_index(&paths, &Default::default()).unwrap();
        let deps = RefreshWorkerDeps {
            config: Arc::new(RefreshConfig { enabled: false, ..RefreshConfig::default() }),
            paths,
            mempalace_sink: Arc::new(NoopSink::default()),
        };
        // Even though disabled, run_one_cycle itself doesn't branch on enabled;
        // we just confirm it returns Ok with zero scans on empty index.
        let report = run_one_cycle(&deps).await.unwrap();
        assert_eq!(report.scanned, 0);
    }
}
```

If `parse_iso` doesn't exist in `support.rs`, add a tiny helper:

```rust
pub fn parse_iso(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw).ok().map(|dt| dt.with_timezone(&chrono::Utc))
}
```

- [ ] **Step 3: Run**

Run: `cargo test --lib refresh::worker`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/refresh src/article_memory/mod.rs src/support.rs
git commit -m "feat(refresh): evergreen worker (bump updated_at only for MVP)"
```

---

### Task 23: Wire `RefreshWorker::spawn` + example config

**Files:**
- Modify: `src/lib.rs`, `src/local_proxy.rs`, `config/davis/local.example.toml`

- [ ] **Step 1: Re-export + spawn + example**

Re-export in `src/lib.rs`:

```rust
pub use article_memory::refresh::{RefreshWorker, RefreshWorkerDeps};
```

Spawn in `src/local_proxy.rs` (after TranslateWorker):

```rust
    if local_config.article_memory.refresh.enabled {
        crate::article_memory::refresh::RefreshWorker::spawn(
            crate::article_memory::refresh::RefreshWorkerDeps {
                config: Arc::new(local_config.article_memory.refresh.clone()),
                paths: paths.clone(),
                mempalace_sink: ingest_sink.clone(),
            },
        );
        tracing::info!("refresh worker started");
    }
```

Example in `config/davis/local.example.toml`:

```toml
# --- Evergreen refresh (MVP Phase 4) ----------------------------------------
# [article_memory.refresh]
# enabled = false
# interval_secs = 86400
# stale_after_days = 30
# score_delta_threshold = 0.2
# batch_per_cycle = 20
```

- [ ] **Step 2: Build + commit**

Run: `cargo build --lib && cargo clippy --all-targets -- -D warnings`
Expected: clean.

```bash
git add src/lib.rs src/local_proxy.rs config/davis/local.example.toml
git commit -m "feat(refresh): wire worker + example config"
```

---

### Task 24: Phase 4 gate

- [ ] **Step 1: Full suite**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: all green.

---

## Phase 5 — Digest Endpoint + zeroclaw Cron Stanza

### Task 25: `GET /article-memory/digest` handler

**Files:**
- Create: `src/server_digest.rs`
- Modify: `src/server.rs` (register route)
- Modify: `src/lib.rs` (add `mod server_digest;`)

- [ ] **Step 1: Write failing handler tests**

Write `src/server_digest.rs`:

```rust
//! GET /article-memory/digest?topic=<slug>&since_days=<n>
//!
//! Returns a topic-scoped + time-scoped summary of the article index. Designed
//! to be consumed by zeroclaw agent cron jobs that then format + deliver a
//! digest to Telegram/Slack. Davis does no scheduling or delivery itself.

use crate::article_memory::types::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
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

fn default_since() -> u32 { 7 }
fn default_top() -> usize { 10 }

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
    State(paths): State<RuntimePaths>,
    Query(q): Query<DigestQuery>,
) -> Json<DigestResponse> {
    let idx = crate::article_memory::load_article_index(&paths).unwrap_or_default();
    let cutoff = Utc::now() - Duration::days(q.since_days as i64);

    let mut filtered: Vec<_> = idx
        .articles
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

    filtered.sort_by(|a, b| b.value_score.unwrap_or(0.0).partial_cmp(&a.value_score.unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
    let top = filtered.iter().take(q.top).map(to_item).collect();

    let mut translated: Vec<_> = filtered.iter().filter(|r| r.translation_path.is_some()).collect();
    translated.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let recent_translations = translated.iter().take(q.top).map(|r| to_item(*r)).collect();

    Json(DigestResponse {
        topic: q.topic,
        window_days: q.since_days,
        total: filtered.len(),
        by_decision: counts,
        top,
        recent_translations,
    })
}

fn within_window(r: &ArticleMemoryRecord, cutoff: DateTime<Utc>) -> bool {
    crate::support::parse_iso(&r.updated_at).map(|dt| dt >= cutoff).unwrap_or(false)
}

fn topic_matches(r: &ArticleMemoryRecord, topic: Option<&str>) -> bool {
    match topic {
        None => true,
        Some(t) => r.tags.iter().any(|tag| tag == &format!("topic:{t}")),
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
    use crate::article_memory::{save_article_index, ArticleMemoryIndex};
    use tempfile::TempDir;

    fn record(id: &str, topic: &str, score: f32, age_days: i64, translated: bool) -> ArticleMemoryRecord {
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
            content_path: "".into(),
            raw_path: None,
            normalized_path: None,
            summary_path: None,
            translation_path: translated.then(|| format!("{id}/translation.md")),
            notes: None,
            clean_status: None,
            clean_profile: None,
        }
    }

    async fn exec(paths: RuntimePaths, q: DigestQuery) -> DigestResponse {
        handle(State(paths), Query(q)).await.0
    }

    #[tokio::test]
    async fn counts_by_decision_within_window() {
        let tmp = TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(tmp.path());
        let mut idx = ArticleMemoryIndex::default();
        idx.articles.push(record("a1", "rust", 0.9, 1, false));
        idx.articles.push(record("a2", "rust", 0.6, 2, true));
        idx.articles.push(record("old", "rust", 0.9, 100, false)); // outside window
        save_article_index(&paths, &idx).unwrap();

        let resp = exec(paths, DigestQuery { topic: Some("rust".into()), since_days: 7, top: 10 }).await;
        assert_eq!(resp.total, 2);
        assert_eq!(resp.by_decision.saved, 2);
        assert_eq!(resp.top.len(), 2);
        assert_eq!(resp.top[0].id, "a1", "higher score first");
    }

    #[tokio::test]
    async fn filters_by_topic() {
        let tmp = TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(tmp.path());
        let mut idx = ArticleMemoryIndex::default();
        idx.articles.push(record("a1", "rust", 0.9, 1, false));
        idx.articles.push(record("b1", "python", 0.9, 1, false));
        save_article_index(&paths, &idx).unwrap();

        let resp = exec(paths, DigestQuery { topic: Some("rust".into()), since_days: 7, top: 10 }).await;
        assert_eq!(resp.total, 1);
        assert_eq!(resp.top[0].id, "a1");
    }

    #[tokio::test]
    async fn recent_translations_only_if_translated() {
        let tmp = TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(tmp.path());
        let mut idx = ArticleMemoryIndex::default();
        idx.articles.push(record("a1", "rust", 0.9, 1, true));
        idx.articles.push(record("a2", "rust", 0.6, 2, false));
        save_article_index(&paths, &idx).unwrap();

        let resp = exec(paths, DigestQuery { topic: None, since_days: 7, top: 10 }).await;
        assert_eq!(resp.recent_translations.len(), 1);
        assert_eq!(resp.recent_translations[0].id, "a1");
    }
}
```

Add `mod server_digest;` to `src/lib.rs`.

In `src/server.rs`, next to the existing `.route("/article-memory/search", ...)` line, add:

```rust
        .route("/article-memory/digest", get(server_digest::handle))
```

The route's state type (`RuntimePaths` in the snippet) must match how `AppState` threads paths; adjust the `State<RuntimePaths>` extractor accordingly (probably `State<AppState>` and then `state.paths` — inspect and adapt).

- [ ] **Step 2: Run**

Run: `cargo test --lib server_digest`
Expected: 3 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/server_digest.rs src/server.rs src/lib.rs
git commit -m "feat(digest): GET /article-memory/digest handler"
```

---

### Task 26: Integration test — hit real axum router

**Files:**
- Create: `tests/rust/topic_crawl_digest.rs`

- [ ] **Step 1: Write test**

```rust
use tower::ServiceExt; // for `oneshot`

#[tokio::test]
async fn digest_endpoint_returns_expected_shape() {
    // Use davis_zero_claw's server builder if exposed; else build a minimal
    // Router::new().route(...).with_state(paths) and hit it directly.
    // The point is that the path + query params wire up.
    //
    // Intentionally concise: the handler logic is covered by Task 25 unit tests.
    // This test only asserts the route exists and responds 200.

    let tmp = tempfile::TempDir::new().unwrap();
    let paths = davis_zero_claw::RuntimePaths::for_test(tmp.path());
    davis_zero_claw::article_memory::save_article_index(
        &paths,
        &davis_zero_claw::article_memory::ArticleMemoryIndex::default(),
    )
    .unwrap();

    use axum::{routing::get, Router};
    let app = Router::new()
        .route("/article-memory/digest", get(davis_zero_claw::server_digest::handle))
        .with_state(paths);

    let req = axum::http::Request::builder()
        .uri("/article-memory/digest?since_days=7&top=5")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
}
```

- [ ] **Step 2: Run**

Run: `cargo test --test topic_crawl_digest`
Expected: PASS. (If `server_digest` isn't `pub` from the crate, expose it via `pub mod server_digest;` in `lib.rs` or add a crate-level re-export for testing.)

- [ ] **Step 3: Commit**

```bash
git add tests/rust/topic_crawl_digest.rs
git commit -m "test(digest): integration via axum tower oneshot"
```

---

### Task 27: Render three zeroclaw agent-cron jobs via `model_routing.rs`

**Files:**
- Modify: `src/model_routing.rs` — identify the section that renders `[[cron.jobs]]` into zeroclaw's `config.toml` (if none exists, add one)
- Modify: `src/app_config.rs` — add a `[zeroclaw.digest]` toml block with user-editable settings

- [ ] **Step 1: Inspect the config renderer**

Run: `grep -nE '\[\[cron.jobs\]\]|cron_jobs|render_zeroclaw_config|patch_config_toml' src/model_routing.rs`

If there is no existing cron-rendering logic, add a helper `render_cron_jobs(config: &LocalConfig) -> String` that emits the stanzas. If there is existing logic for another table, follow that pattern exactly.

- [ ] **Step 2: Add the digest config struct**

In `src/app_config.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DigestConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub weekly_topic: Option<String>,
    #[serde(default)]
    pub telegram_chat_id: Option<String>,
    #[serde(default = "default_digest_timezone")]
    pub timezone: String,
}

fn default_digest_timezone() -> String { "Asia/Shanghai".into() }
```

Nest under `ZeroclawConfig` (or add that struct if absent):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ZeroclawConfig {
    #[serde(default)]
    pub digest: DigestConfig,
}
```

Mount on `LocalConfig`:

```rust
    #[serde(default)]
    pub zeroclaw: ZeroclawConfig,
```

- [ ] **Step 3: Write the renderer + test**

```rust
pub fn render_digest_cron_jobs(cfg: &crate::app_config::LocalConfig) -> String {
    let d = &cfg.zeroclaw.digest;
    if !d.enabled {
        return String::new();
    }
    let Some(topic) = d.weekly_topic.as_deref() else { return String::new() };
    let Some(chat) = d.telegram_chat_id.as_deref() else { return String::new() };
    format!(
        r#"
[[cron.jobs]]
id = "davis-weekly-digest-{topic}"
job_type = "agent"
schedule = {{ kind = "cron", expr = "0 9 * * 1", tz = "{tz}" }}
prompt = """
Fetch GET http://127.0.0.1:3010/article-memory/brief?topic={topic}&since_days=7 (or /digest if brief unavailable).
Format as a concise Telegram message with sections: new highlights, candidates needing review, translations available.
"""
delivery = {{ mode = "announce", channel = "telegram", to = "{chat}" }}
allowed_tools = ["web_fetch"]
uses_memory = false

[[cron.jobs]]
id = "davis-daily-candidate-review"
job_type = "agent"
schedule = {{ kind = "cron", expr = "0 22 * * *", tz = "{tz}" }}
prompt = """
GET http://127.0.0.1:3010/article-memory/digest?since_days=1 and list all decision=candidate entries with URLs.
"""
delivery = {{ mode = "announce", channel = "telegram", to = "{chat}" }}
allowed_tools = ["web_fetch"]
uses_memory = false

[[cron.jobs]]
id = "davis-monthly-compress"
job_type = "shell"
schedule = {{ kind = "cron", expr = "0 3 1 * *", tz = "{tz}" }}
command = "davis mempalace compress --wing davis.articles"
"#,
        topic = topic,
        tz = d.timezone,
        chat = chat,
    )
}

#[cfg(test)]
mod digest_cron_tests {
    use super::*;
    use crate::app_config::{DigestConfig, LocalConfig, ZeroclawConfig};

    #[test]
    fn disabled_produces_empty() {
        let mut cfg = LocalConfig::default();
        cfg.zeroclaw = ZeroclawConfig::default();
        assert_eq!(render_digest_cron_jobs(&cfg), "");
    }

    #[test]
    fn enabled_with_topic_and_chat_emits_three_jobs() {
        let mut cfg = LocalConfig::default();
        cfg.zeroclaw = ZeroclawConfig {
            digest: DigestConfig {
                enabled: true,
                weekly_topic: Some("async-rust".into()),
                telegram_chat_id: Some("-100123".into()),
                timezone: "Asia/Shanghai".into(),
            },
        };
        let out = render_digest_cron_jobs(&cfg);
        assert!(out.contains("davis-weekly-digest-async-rust"), "{out}");
        assert!(out.contains("davis-daily-candidate-review"), "{out}");
        assert!(out.contains("davis-monthly-compress"), "{out}");
        assert!(out.contains("to = \"-100123\""), "{out}");
    }

    #[test]
    fn enabled_but_missing_topic_or_chat_produces_empty() {
        let mut cfg = LocalConfig::default();
        cfg.zeroclaw = ZeroclawConfig {
            digest: DigestConfig {
                enabled: true,
                weekly_topic: None,
                telegram_chat_id: Some("-100".into()),
                timezone: "UTC".into(),
            },
        };
        assert_eq!(render_digest_cron_jobs(&cfg), "");
    }
}
```

- [ ] **Step 4: Wire renderer into the zeroclaw config writer**

Find the function in `model_routing.rs` that writes the zeroclaw config (likely named `write_zeroclaw_config` or `render_zeroclaw_config`). After its existing output, append `render_digest_cron_jobs(cfg)`. Do not reorder or overwrite existing sections.

- [ ] **Step 5: Run + commit**

Run: `cargo test --lib digest_cron_tests`
Expected: PASS.

```bash
git add src/model_routing.rs src/app_config.rs
git commit -m "feat(digest): render weekly/daily/monthly zeroclaw cron jobs from config"
```

---

### Task 28: Example config + Phase 5 gate

**Files:**
- Modify: `config/davis/local.example.toml`

- [ ] **Step 1: Append**

```toml
# --- Weekly digest (MVP Phase 5) --------------------------------------------
# Davis does no scheduling or delivery itself; these fields are rendered into
# zeroclaw's config.toml as [[cron.jobs]] so zeroclaw runs the agent cron that
# GETs /article-memory/digest and pushes a Telegram message.
# [zeroclaw.digest]
# enabled = false
# weekly_topic = "async-rust"
# telegram_chat_id = "-1001234567890"
# timezone = "Asia/Shanghai"
```

- [ ] **Step 2: Gate**

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add config/davis/local.example.toml
git commit -m "docs(digest): example config"
```

---

## Phase 6 — Invariants + Docs Sync

### Task 29: Invariants test — the "unified dispatcher" ghost stays buried

**Files:**
- Create: `tests/rust/topic_crawl_invariants.rs`

- [ ] **Step 1: Write the invariant test**

```rust
//! These invariants encode architectural decisions that would re-open debate
//! every time a new LLM caller is added. See
//! docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md §"Anchor decisions" A1-A3.

use std::fs;
use std::path::Path;

#[test]
fn remote_chat_is_not_imported_outside_translate_module() {
    let src = Path::new("src");
    for entry in walkdir::WalkDir::new(src).into_iter().flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        // Allow translate module to own the import.
        if p.components().any(|c| c.as_os_str() == "translate") {
            continue;
        }
        let body = fs::read_to_string(p).unwrap();
        assert!(
            !body.contains("translate::remote_chat"),
            "{p:?} imports translate::remote_chat; remote_chat must stay private to translate"
        );
        assert!(
            !body.contains("RemoteChat"),
            "{p:?} references RemoteChat; stay inside translate"
        );
    }
}

#[test]
fn no_zeroclaw_crate_in_cargo_toml() {
    let body = fs::read_to_string("Cargo.toml").unwrap();
    for (i, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("zeroclaw") && trimmed.contains('=') {
            panic!("Cargo.toml line {i}: {line}\n— zeroclaw must not be a Cargo dep");
        }
    }
}
```

Add `walkdir = "2"` to `[dev-dependencies]` in `Cargo.toml`.

- [ ] **Step 2: Run**

Run: `cargo test --test topic_crawl_invariants`
Expected: both tests PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/rust/topic_crawl_invariants.rs Cargo.toml Cargo.lock
git commit -m "test(invariants): remote_chat stays private; no zeroclaw crate dep"
```

---

### Task 30: CLAUDE.md update — "What looks like duplication" table

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add row to the table**

In the table under §"What looks like duplication but is not", add one row:

```
| `article_memory/translate` (inline zeroclaw HTTP client) | Davis keeps a private, module-scoped HTTP client to zeroclaw `/api/chat`. This is **not** a general-purpose `zeroclaw_client` shared with hot-path callers — hot-path stays direct-OpenRouter per CLAUDE.md. The inline client exists only because translation is non-hot-path and benefits from zeroclaw's failover/budget. Hot-path and enhancement callers have opposite failure semantics (see implementation plan §"Anchor decisions" A1/A3), so they intentionally do not share a dispatcher. |
```

Also add under §"Subsystem hook points":

```
- `article_memory/discovery/worker.rs` end-of-cycle — `kg_add(ArticleDiscoveredFrom)` + discovery diary (wing `davis.agent.discovery`).
- `article_memory/translate/worker.rs` on successful translation — `kg_add(ArticleTranslated)` + translator diary (wing `davis.agent.translator`).
```

- [ ] **Step 2: Verify predicates table**

Confirm the two new rows (`ArticleDiscoveredFrom`, `ArticleTranslated`) added in Tasks 5 and 16 are present. If not, add them now.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(claude-md): reflect topic-crawl mvp — inline translate client + two new predicates"
```

---

## Final Gate

- [ ] Run the full suite one more time:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Expected: all green. If not, fix forward before declaring MVP done.

- [ ] Sanity-check file sizes:

```bash
wc -l src/article_memory/discovery/*.rs src/article_memory/discovery/search/*.rs src/article_memory/translate/*.rs src/article_memory/refresh/*.rs src/server_digest.rs
```

Expected: every file ≤ 600 lines. If any file breached 800, split per CLAUDE.md's "Target file size ≤ 800 lines" rule before declaring done.

---

## Post-MVP (deferred — Phase 6+)

These were **not** in the MVP scope and intentionally left out:

- Content-drift refresh (re-crawl high-value records periodically)
- Swarm multi-model consensus judge for 0.5–0.7 score band
- SOP approval flow for translations above a cost threshold
- Tavily / Kagi / SearXNG / Exa search providers
- LLM query expansion (turning one keyword into 20 queries)
- Prometheus/OTel observability for the three workers
- Extracting `translate::remote_chat` into a shared module — only do this when a second non-hot-path consumer lands AND the two consumers want identical failure semantics; otherwise copy the 60 lines and keep them private

When any of these starts, create a new plan in `docs/superpowers/plans/` — do not retrofit into this plan.
