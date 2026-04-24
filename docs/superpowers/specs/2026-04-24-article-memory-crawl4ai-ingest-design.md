# Article Memory × crawl4ai — Unified URL Ingest Design

Status: **Landed (2026-04-24)** — implementation across commits `89d763b..5c6a441` on branch `docs/crawl4ai-plan-slim`. 149/149 tests pass; clippy and fmt clean.
Date: 2026-04-24
Owner: Faillone Xie
Target branch: `docs/crawl4ai-plan-slim` (local-only per user workflow — never pushed)

---

## 1. Goal

Let any direct caller (CLI, cron, Shortcut, webhook) POST a URL to `/article-memory/ingest` over loopback HTTP and trigger async capture. The daemon fetches the page via crawl4ai, converts to Markdown, runs the existing `article_memory` pipeline (clean → value judge → polish → summary → embedding), and stores the result.

iMessage intake is **not** a direct caller. iMessage messages reach this daemon through ZeroClaw's LLM tool-calling layer via the `article-memory__ingest_*` skill tools defined in `project-skills/article-memory/`. See the Architecture Addendum at the bottom of this spec and the Phase 2 spec (`2026-04-24-article-memory-phase2-skill-and-dedup-design.md`) for the final contract.

**User quote**: "我只希望，今后我通过 imessage 或其他 channel，定时任务，shortcut，都可以只发 url，agent 就开始抓取并存储"

This consolidates crawl4ai as the project's single crawling subsystem and retires Shortcut-side Safari-Reader text extraction.

---

## 2. Non-goals

- Replacing `ArticleMemoryAddRequest` (content-in API). The existing CLI `articles add --content-file` stays — useful for offline input and tests.
- DNS rebinding defense at the HTTP client layer (deferred; out of scope for v1).
- Automatic retry of failed ingests (user explicitly wants failures recorded for postmortem, not retried).
- Cross-process / multi-daemon queue (single daemon, in-process).
- Migrating `shortcuts/叫下戴维斯.shortcut.json` to URL-only in this spec (separate follow-up once backend is proven).

---

## 3. Constraints taken from existing code

- `crawl4ai_crawl` already takes `Crawl4aiPageRequest { profile_name, url, wait_for, js_code }` (src/crawl4ai.rs:7) and returns `Crawl4aiPageResult { html, cleaned_html, raw, … }`.
- Per-profile serialization via `Crawl4aiProfileLocks` (`Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>`) already exists at src/express.rs:296. Reused verbatim for ingest.
- `Crawl4aiSupervisor::for_test` constructor lets us spin mock axum routers (Task 14).
- `add_article_memory` → `normalize_article_memory` → `upsert_article_memory_embedding` already assembled in src/cli/articles.rs:45-103 for the `articles add` CLI. Ingest reuses this sequence byte-for-byte — **zero fork**.
- `Crawl4aiError::issue_type()` exposes three stable strings: `crawl4ai_unavailable` / `auth_required` / `site_changed`. Ingest extends with two new ones (`empty_content`, `pipeline_error`).

---

## 4. Architecture overview

```
Direct HTTP callers (CLI / cron / Shortcut / webhook)        LLM tool-call path
   │                                                              (iMessage → ZeroClaw agent → skill)
   │                                                              │
   └──────────────────┬───────────────────────────────────────────┘
                      │  POST /article-memory/ingest { url, title?, tags?, source_hint?, reply_handle? }
   ▼
server.rs::ingest_handler
   │ validate_url_for_ingest(url, config)
   │ IngestQueue::submit()  [dedup check + persist + notify_one]
   │ ← 202 { job_id, status: "pending" }      (or 409 if duplicate, or 400 if invalid URL)
   ▼
IngestWorkerPool (default 3 workers, tokio::spawn × N)
   │ next_pending().await   (tokio::sync::Notify-backed)
   │ acquire_profile_lock(resolve_profile(url.host))   ← reuses Crawl4aiProfileLocks
   ▼
crawl4ai_crawl(profile, url, markdown=true, content_filter="pruning")
   │ → Crawl4aiPageResult { markdown: Some("..."), html, cleaned_html, raw }
   ▼
add_article_memory(content=markdown, url=Some(url), source=resolve_source(host))
   ▼
normalize_article_memory(paths, normalize_cfg, value_cfg, record.id)
   ▼
upsert_article_memory_embedding(paths, embedding_cfg, &record)
   │ (skipped if value_decision == "reject")
   ▼
queue.finish(job_id, Outcome::Saved | Rejected | Failed)
   [persist ingest_jobs.json]
```

Key design principles:

1. **Zero-fork pipeline.** New path only swaps content source. Downstream (`add_article_memory → normalize → embed`) is identical to today's CLI `articles add`. Manual file-based add and URL-based ingest produce byte-identical records.
2. **In-process queue, single JSON file.** No Redis, no SQLite. Same persistence pattern as `article_memory_index.json` and `crawl4ai.pid`.
3. **Worker pool = 3, cross-host parallel, same-host serial.** Same-host serialization falls out of the existing per-profile Mutex; no extra scheduler needed.
4. **Host-specific profiles via TOML.** `zhihu.com → articles-zhihu`, `mp.weixin.qq.com → articles-weixin`, miss → `articles-generic`. User edits `config/davis/article_memory.toml` to add sites. Cookies are scoped per site (login-gated content like Medium paywall or zhihu full answers work once `daviszeroclaw crawl profile login articles-zhihu` is done).
5. **Existing express path untouched.** `express-ali` / `express-jd` profiles and their Mutex tenancy keep working — ingest is just another tenant of the same Mutex map.

---

## 5. Module layout

```
src/
├── article_memory/
│   ├── ingest/                       ← new directory
│   │   ├── mod.rs                    # pub use re-exports
│   │   ├── types.rs                  # IngestJob, IngestJobStatus, IngestRequest, IngestResponse,
│   │   │                             # IngestJobError, IngestOutcome, ListFilter
│   │   ├── queue.rs                  # IngestQueue (in-memory state + JSON persistence + Notify)
│   │   ├── worker.rs                 # IngestWorkerPool + worker_loop
│   │   └── host_profile.rs           # resolve_profile(), resolve_source(),
│   │                                 # validate_url_for_ingest(), normalize_url()
│   ├── config.rs                     # + ArticleMemoryIngestConfig
│   └── mod.rs                        # mod ingest; pub use ingest::*;
├── server.rs                         # + POST /article-memory/ingest
│                                     # + GET  /article-memory/ingest/{job_id}
│                                     # + GET  /article-memory/ingest?status=&limit=
│                                     # AppState + ingest_queue: Arc<IngestQueue>
├── local_proxy.rs                    # daemon startup: IngestQueue::load_or_create()
│                                     #                 + IngestWorkerPool::spawn()
└── cli/articles.rs                   # + articles ingest <url> subcommand + history + show

crawl4ai_adapter/
└── server.py                         # CrawlRequest + markdown_generator + content_filter
                                      # CrawlResponse + markdown: Optional[str]

config/davis/
└── article_memory.toml               # + [article_memory.ingest] section + host_profiles array
```

File size budget: each new Rust file ≤ 300 lines. Total new Rust ≈ 520 lines.

---

## 6. Data shapes

### 6.1 `IngestRequest` (HTTP body)

```rust
pub struct IngestRequest {
    pub url: String,                       // required
    pub title: Option<String>,             // override detected title
    pub tags: Vec<String>,                 // forwarded to ArticleMemoryAddRequest
    pub source_hint: Option<String>,       // "imessage" | "shortcut" | "cron" | "cli" | "mcp"
                                           // purely observability; does not affect pipeline
}
```

### 6.2 `IngestJob` (persisted + API response)

```rust
pub struct IngestJob {
    pub id: String,                        // uuid v4
    pub url: String,                       // original, as submitted
    pub normalized_url: String,            // lower-cased scheme+host, fragment stripped, query kept
    pub title_override: Option<String>,
    pub tags: Vec<String>,
    pub source_hint: Option<String>,
    pub profile_name: String,              // resolved from host at submit time
    pub status: IngestJobStatus,
    pub article_id: Option<String>,        // set once add_article_memory succeeds
    pub outcome: Option<IngestOutcomeSummary>,  // clean_status / value_decision / value_score
    pub error: Option<IngestJobError>,
    pub warnings: Vec<String>,             // non-fatal (e.g. "embedding_failed")
    pub submitted_at: String,              // ISO-8601
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub attempts: u32,                     // always 1 in v1 (no auto-retry)
}

pub enum IngestJobStatus {
    Pending,        // in queue, waiting for a worker
    Fetching,       // crawl4ai in flight
    Cleaning,       // add_article_memory done, normalize running
    Judging,        // LLM value judge running (if configured)
    Embedding,      // upsert_embedding running
    Saved,          // terminal success
    Rejected,       // terminal, value judge said "reject"
    Failed,         // terminal failure
}

impl IngestJobStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Pending | Fetching | Cleaning | Judging | Embedding)
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, Saved | Rejected | Failed)
    }
}

pub struct IngestJobError {
    pub issue_type: String,   // see §7.2
    pub message: String,      // full detail for postmortem
    pub stage: String,        // which status the job was in when it failed
}

pub struct IngestOutcomeSummary {
    pub clean_status: String,            // "ok" | "polished" | "fallback_raw" | "rejected"
    pub clean_profile: String,
    pub value_decision: Option<String>,  // "save" | "candidate" | "reject"
    pub value_score: Option<f32>,
    pub normalized_chars: usize,
    pub polished: bool,
    pub summary_generated: bool,
    pub embedded: bool,
}
```

### 6.3 `IngestResponse` (202 body)

```rust
pub struct IngestResponse {
    pub job_id: String,
    pub status: IngestJobStatus,    // Pending for new, or current state for idempotent hit
    pub submitted_at: String,
    pub deduped: bool,              // true if this response refers to a pre-existing in-flight job
}
```

### 6.4 `ArticleMemoryIngestConfig` (TOML)

```toml
[article_memory.ingest]
enabled = true                      # default true; set false to disable worker pool + endpoints
max_concurrency = 3                 # worker count
default_profile = "articles-generic"
min_markdown_chars = 600            # below this, ingest fails with issue_type="empty_content"
dedup_window_hours = 24             # how long Saved jobs block same-URL resubmission
allow_private_hosts = []            # SSRF whitelist, e.g. ["wiki.internal.example"]

[[article_memory.ingest.host_profiles]]
match = "zhihu.com"                 # host suffix; first-hit wins, order matters
profile = "articles-zhihu"
source = "zhihu"                    # optional; sent as `source` field to add_article_memory

[[article_memory.ingest.host_profiles]]
match = "mp.weixin.qq.com"
profile = "articles-weixin"
source = "weixin"

[[article_memory.ingest.host_profiles]]
match = "medium.com"
profile = "articles-medium"
source = "medium"
```

Rust side:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryIngestConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_ingest_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_ingest_profile")]
    pub default_profile: String,
    #[serde(default = "default_min_markdown_chars")]
    pub min_markdown_chars: usize,
    #[serde(default = "default_dedup_window_hours")]
    pub dedup_window_hours: u64,
    #[serde(default)]
    pub allow_private_hosts: Vec<String>,
    #[serde(default)]
    pub host_profiles: Vec<ArticleMemoryHostProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryHostProfile {
    #[serde(rename = "match")]
    pub match_suffix: String,          // renamed to avoid Rust reserved keyword "match"
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

fn default_ingest_concurrency() -> usize { 3 }
fn default_ingest_profile() -> String { "articles-generic".into() }
fn default_min_markdown_chars() -> usize { 600 }
fn default_dedup_window_hours() -> u64 { 24 }
```

---

## 7. Data flow

### 7.1 Happy path

```
T+0ms    Channel POSTs /article-memory/ingest {url: "https://zhuanlan.zhihu.com/p/123"}
T+1ms    validate_url_for_ingest(url, config)  → Ok
T+2ms    IngestQueue::submit()
         ├─ normalize_url() → "https://zhuanlan.zhihu.com/p/123"
         ├─ dedup check → no active or recent-Saved match
         ├─ resolve_profile("zhuanlan.zhihu.com") → "articles-zhihu"
         ├─ create IngestJob { id: uuid, status: Pending, ... }
         ├─ jobs.insert(id, job); pending.push_back(id);
         ├─ persist_to_json() → runtime/article_memory/ingest_jobs.json
         └─ notify.notify_one()
T+3ms    HTTP 202 { job_id, status: "pending", deduped: false }
         Channel returns immediately.

── asynchronously in worker pool ──

T+5ms    IngestWorker-2 wakes from Notify
         ├─ next_pending() returns job
         ├─ acquire profile_lock("articles-zhihu")   (existing Crawl4aiProfileLocks)
         └─ mark_status(Fetching) + persist

T+12s    crawl4ai_crawl(profile="articles-zhihu", url, markdown=true, content_filter="pruning")
         │ → Crawl4aiPageResult { markdown: Some("# Title\n\n..."), html, raw: {metadata: {title: "..."}, ...} }
         ├─ assert markdown.len() >= config.min_markdown_chars, else Err(empty_content)
         ├─ title = job.title_override ?? raw.metadata.title ?? url
         └─ source = resolve_source(host) via config.host_profiles → "zhihu"

T+12.1s  mark_status(Cleaning) + persist
         add_article_memory(paths, ArticleMemoryAddRequest {
             title, url: Some(url), source, content: markdown,
             status: Candidate, language: None, tags: job.tags, ...
         }) → record { id: "abc123", ... }
         job.article_id = Some("abc123")

T+12.2s  mark_status(Judging) + persist
         normalize_article_memory(paths, normalize_cfg, value_cfg, record.id)
         → NormalizeResponse { clean_status: "polished", value_decision: Some("save"), ... }

T+43s    mark_status(Embedding) + persist
         upsert_article_memory_embedding(paths, embedding_cfg, &record)

T+44s    finish(id, Outcome::Saved { ... }) + persist
         profile_lock drops → next same-host job can proceed
```

### 7.2 Error taxonomy

| Failure point | Source | `issue_type` | Terminal state |
|---------------|--------|--------------|----------------|
| URL validation (scheme/private) | `validate_url_for_ingest` | (HTTP 400 pre-submit) | Not a job |
| Duplicate URL in dedup window | `IngestQueue::submit` | (HTTP 409 pre-submit) | Not a job |
| `Crawl4aiError::Disabled` | crawl4ai stage | `crawl4ai_unavailable` | Failed |
| `Crawl4aiError::ServerUnavailable` | crawl4ai stage | `crawl4ai_unavailable` | Failed |
| `Crawl4aiError::Timeout` | crawl4ai stage | `crawl4ai_unavailable` | Failed |
| `Crawl4aiError::AuthRequired` | crawl4ai stage | `auth_required` | Failed (message points at profile name) |
| `Crawl4aiError::CrawlFailed` | crawl4ai stage | `site_changed` | Failed |
| `Crawl4aiError::PayloadMalformed` | crawl4ai stage | `site_changed` | Failed |
| Markdown < `min_markdown_chars` | ingest | `empty_content` | Failed |
| `add_article_memory` error | pipeline | `pipeline_error` | Failed |
| `normalize_article_memory` error | pipeline | `pipeline_error` | Failed |
| Value judge returns "reject" | pipeline | N/A (not an error) | **Rejected** |
| `upsert_embedding` error | pipeline | N/A (warning only) | **Saved** + `warnings: ["embedding_failed: <msg>"]` |

Rationale for embedding-warning: the article is already committed to the index. `daviszeroclaw articles index` rebuilds vectors idempotently. Marking the whole job Failed would mislead the user into thinking nothing was saved.

### 7.3 URL normalization and dedup

```rust
pub fn normalize_url(url: &str) -> Result<String, InvalidUrl> {
    let mut parsed = Url::parse(url)?;
    parsed.set_fragment(None);                                      // strip #anchor
    if let Some(host) = parsed.host_str() {
        let lowered = host.to_ascii_lowercase();
        parsed.set_host(Some(&lowered)).ok();
    }
    // scheme is already lowercased by url crate
    Ok(parsed.to_string())
}
```

Dedup rules (in `IngestQueue::submit`):

1. If any existing job has `normalized_url == req_norm && status.is_active()` → return 202 with **existing job_id** and `deduped: true`. Idempotent; same channel retry is safe.
2. Else if any existing job has `normalized_url == req_norm && status == Saved && finished_at > now - dedup_window_hours` → return **409 Conflict** + `{ existing_article_id, finished_at }`.
3. Else create a new job.

Query string is **part** of the identity (`?page=2` ≠ `?page=3`). Path case **is** preserved.

### 7.4 Host suffix matching for profile resolution

```rust
pub fn resolve_profile(url: &str, config: &ArticleMemoryIngestConfig) -> (String, Option<String>) {
    let host = match Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)) {
        Some(h) => h.to_ascii_lowercase(),
        None => return (config.default_profile.clone(), None),
    };
    for entry in &config.host_profiles {
        let suffix = entry.match_suffix.to_ascii_lowercase();
        // match if host == suffix OR host ends with ".{suffix}"
        // this rejects "zhihubus.com" against a "zhihu.com" rule
        if host == suffix || host.ends_with(&format!(".{suffix}")) {
            return (entry.profile.clone(), entry.source.clone());
        }
    }
    (config.default_profile.clone(), None)
}
```

Boundary rule: `zhihu.com` matches `www.zhihu.com` / `zhuanlan.zhihu.com` / `zhihu.com` itself, but **not** `zhihubus.com` or `fakezhihu.com`. Order-sensitive; first entry wins.

### 7.5 SSRF guard

`validate_url_for_ingest(url, config)` runs **before** dedup and returns HTTP 400 on any rejection. Rules:

- Reject non-http(s) schemes (`file://`, `javascript:`, `data:`, `ftp://`, etc.)
- Reject loopback / private / link-local / broadcast / multicast / unspecified IPv4 and IPv6
- Reject IPv6 unique-local (`fc00::/7`) and link-local (`fe80::/10`)
- Reject domain literals `localhost`, `*.local`, `*.internal`, `*.localhost`
- Bypass for any host listed in `config.allow_private_hosts`
- **No DNS resolution** is performed (TOCTOU avoidance; DNS rebinding defense deferred)

Response format:
```json
HTTP 400
{ "error": "invalid_url", "reason": "private_address_blocked", "detail": "127.0.0.1 is a loopback address" }
```

---

## 8. HTTP and CLI surfaces

### 8.1 HTTP

| Method | Path | Body / Query | Status | Response |
|--------|------|--------------|--------|----------|
| POST | `/article-memory/ingest` | `IngestRequest` | 202 / 400 / 409 / 503 | `IngestResponse` / error object |
| GET | `/article-memory/ingest/{job_id}` | — | 200 / 404 | `IngestJob` |
| GET | `/article-memory/ingest` | `?status=&limit=&since=` | 200 | `{ jobs: [IngestJob], total: n }` |

503 is returned when `[article_memory.ingest].enabled = false`, with body `{ error: "ingest_disabled" }`.

### 8.2 CLI

```bash
# Fire-and-forget submit; prints job_id; optional --wait for polling
daviszeroclaw articles ingest <url> [--tag <t>]... [--title <t>] [--wait]

# Recent history (default 20)
daviszeroclaw articles ingest history [--failed] [--limit 20]

# Single job inspect
daviszeroclaw articles ingest show <job_id>
```

`--wait` polls `GET /article-memory/ingest/{job_id}` every 2 seconds and streams status transitions until terminal. Optional; default is fire-and-forget.

### 8.3 Python adapter (crawl4ai_adapter/server.py)

```python
class CrawlRequest(BaseModel):
    # existing fields unchanged
    markdown_generator: bool = False
    content_filter: Optional[str] = None   # "pruning" | "bm25" | None

class CrawlResponse(BaseModel):
    # existing fields unchanged
    markdown: Optional[str] = None

# /crawl handler:
if req.markdown_generator:
    from crawl4ai.content_filter_strategy import PruningContentFilter, BM25ContentFilter
    from crawl4ai.markdown_generation_strategy import DefaultMarkdownGenerator
    content_filter = {
        "pruning": PruningContentFilter(),
        "bm25": BM25ContentFilter(),
    }.get(req.content_filter)
    crawler_config.markdown_generator = DefaultMarkdownGenerator(content_filter=content_filter)

# After crawl:
response.markdown = (
    result.markdown_v2.fit_markdown
    if getattr(result, "markdown_v2", None)
    else (result.markdown or None)
)
```

Rust-side `Crawl4aiPageRequest` gains `markdown: bool` (default `false`). Express path does not set it → Python adapter returns `markdown = None` → zero behavior change for express.

---

## 9. Concurrency and persistence

### 9.1 Lock ordering (deadlock prevention)

```
Worker loop:
  1. queue.next_pending().await          ← takes QueueState lock briefly
  2. acquire profile_lock                ← takes outer Crawl4aiProfileLocks briefly, then holds inner profile_lock
  3. crawl + pipeline (profile_lock held)
  4. release profile_lock (Drop)
  5. queue.mark_status() / queue.finish()  ← takes QueueState lock briefly
```

`QueueState` lock and `profile_lock` are **never** held simultaneously. No deadlock possible.

### 9.2 Notify contract

```rust
impl IngestQueue {
    pub async fn submit(&self, req: IngestRequest) -> Result<IngestResponse, IngestError> {
        let mut state = self.inner.lock().await;
        // 1. push_back FIRST
        state.jobs.insert(job.id.clone(), job.clone());
        state.pending.push_back(job.id.clone());
        self.persist_locked(&state)?;
        drop(state);
        // 2. notify AFTER
        self.notify.notify_one();
        Ok(response)
    }

    pub async fn next_pending(&self) -> IngestJob {
        loop {
            {
                let mut state = self.inner.lock().await;
                if let Some(id) = state.pending.pop_front() {
                    if let Some(mut job) = state.jobs.get(&id).cloned() {
                        job.started_at = Some(now_iso());
                        job.status = IngestJobStatus::Fetching;
                        state.jobs.insert(id.clone(), job.clone());
                        self.persist_locked(&state).ok();
                        return job;
                    }
                }
            }
            self.notify.notified().await;
            // loop re-checks — survives the race where notify fires before a submit's push_back commits
        }
    }
}
```

### 9.3 Persistence

- File: `runtime/article_memory/ingest_jobs.json`
- Schema: `{ version: 1, updated_at: ISO, jobs: HashMap<String, IngestJob>, pending: Vec<String> }`
- Write every state change. 10 jobs/day × ~5 writes each × few-KB each = negligible on APFS.
- `load_or_create()` on startup:
  - File missing → create empty queue
  - File corrupt / version mismatch → `tracing::error` + create empty queue (do **not** panic); lost jobs still visible in log for manual replay
  - For any job loaded with `status.is_active()` → **reset to Failed** with `error.message = "daemon restarted mid-job, status was <X>"`, `error.issue_type = "daemon_restart"`. This is consistent with the "no auto-retry" rule: user can resubmit the URL manually via CLI.

---

## 10. Observability

- `#[tracing::instrument(name = "ingest.worker", skip_all, fields(job_id, url, profile))]` on the execute function
- Existing `runtime/crawl4ai.log` and `runtime/davis.log` pick up worker spans automatically
- CLI `articles ingest history --failed` is the primary postmortem tool
- `GET /article-memory/ingest?status=failed` is exposed to ZeroClaw's LLM as the `article-memory__ingest_list` skill tool; iMessage users ask for "recent failures" in natural language and the LLM calls this tool.

No new metrics system; this is a single-user local daemon.

---

## 11. Testing strategy

### 11.1 Unit — pure functions

- `resolve_profile_matches_host_suffix` — `zhuanlan.zhihu.com` → `articles-zhihu`
- `resolve_profile_first_hit_wins` — order-sensitive match table
- `resolve_profile_empty_config_defaults` — empty → `articles-generic`
- `resolve_profile_invalid_url_defaults` — malformed URL → default (no panic)
- `resolve_source_from_host` — `zhihu.com` → `Some("zhihu")`
- `normalize_url_strips_fragment` and `normalize_url_lowercases_host`
- `validate_rejects_file_scheme`, `validate_rejects_javascript_scheme`
- `validate_rejects_localhost_variants` — `127.0.0.1`, `localhost`, `::1`, `0.0.0.0`
- `validate_rejects_private_ipv4` — 10.x, 172.16-31.x, 192.168.x, 169.254.x
- `validate_rejects_ipv6_ula` — `fc00::/7`, `fe80::/10`
- `validate_allows_public_domain` — `zhihu.com`, `1.1.1.1`
- `validate_allowlist_bypass` — allow_private_hosts entries pass

### 11.2 Queue — Mutex + Notify, no crawl4ai

- `submit_creates_pending_job`
- `next_pending_blocks_until_notify` (assert elapsed > 100ms then wakes < 10ms after submit)
- `test_notify_race_safety` — 5 concurrent submits + 5 concurrent next_pending, no lost jobs
- `mark_status_persists_to_disk`
- `load_or_create_recovers_from_disk`
- `load_or_create_resets_running_to_failed` — active-status jobs on disk become Failed on reload
- `submit_dedup_returns_existing_for_in_flight`
- `submit_dedup_conflicts_for_recent_saved`
- `submit_dedup_allows_after_window_expires`

### 11.3 Worker — mocked crawl4ai via `Crawl4aiSupervisor::for_test`

- `ingest_happy_path_end_to_end` — mock 200 + markdown → job Saved, article_id set, markdown file on disk
- `ingest_empty_markdown_rejected` — mock returns 50-char markdown → Failed, `issue_type == "empty_content"`
- `ingest_crawl_timeout_surfaces_issue_type` — mock 504 → Failed, `crawl4ai_unavailable`
- `ingest_auth_required_surfaces_reauth_hint` — mock success=false, auth_required → Failed, `auth_required`, message names profile
- `ingest_rejected_by_value_judge` — off-topic content → Rejected (not Failed), embedding skipped
- `ingest_embedding_fails_job_still_saved` — embedding API unreachable → Saved + warning "embedding_failed"
- `ingest_same_host_serializes` — 3 zhihu URLs simultaneous, mock counter `max_in_flight_observed == 1`
- `ingest_different_hosts_parallelize` — 3 distinct hosts, mock counter `max_in_flight_observed >= 2`

Counter-based rather than time-based concurrency assertions to avoid CI flake.

### 11.4 HTTP/CLI smoke

- `post_ingest_returns_202_with_job_id`
- `post_ingest_invalid_url_returns_400`
- `post_ingest_duplicate_within_window_returns_409`
- `get_ingest_job_returns_current_status`
- `list_ingest_jobs_filters_by_status`

### 11.5 Python adapter manual smoke

Recorded as a validation step in the implementation plan (not automated): `curl -X POST /crawl` with `markdown_generator: true` against a known-public zhihu URL, assert `response.markdown` non-empty and > 600 chars.

### 11.6 Coverage target

New Rust surface ≈ 520 lines. Unit + queue + worker tests cover ~420 lines (80%+). Remainder is trivial re-exports / `Default` impls / tracing macros.

---

## 12. Backward compatibility

- `ArticleMemoryAddRequest` unchanged — existing `POST /article-memory/articles` and `articles add --content-file` paths continue to work byte-identically.
- `Crawl4aiPageRequest.markdown` added with `#[serde(default)]` → Python adapter sees `false`, returns existing shape, express.rs unaffected.
- `Crawl4aiPageResult.markdown` added as `Option<String>` → defaults to `None` everywhere except ingest callers.
- `crawl4ai_adapter/server.py` CrawlRequest fields default to `False` / `None` → old Rust clients work unchanged.
- New TOML section is entirely `#[serde(default)]` → old `article_memory.toml` still loads; defaults give ingest enabled, empty host_profiles (all traffic to `articles-generic`).

---

## 13. Rollback plan

In order of cost:

1. **Config toggle**: `[article_memory.ingest].enabled = false` + restart daemon. Worker pool not spawned, `/ingest` returns 503. Existing articles untouched.
2. **Drop queue state**: `trash runtime/article_memory/ingest_jobs.json`, restart. Loses in-flight job metadata, but already-Saved articles remain in index.
3. **Full revert**: entire change is additive (no schema migrations on existing files, no API breaks). `git revert <merge>` returns the codebase to pre-ingest state.

---

## 14. Follow-ups (out of scope)

- Shortcut migration (`shortcuts/叫下戴维斯.shortcut.json` → URL-only) once ingest is proven in production.
- iMessage integration is complete in Phase 2 via the `article-memory` skill tools and `reply_handle`-driven completion notifications; no separate bridge wiring is needed.
- Cron job examples in `docs/` for scheduled RSS → ingest.
- Proper article-level URL dedup (queryable `find_by_url()` in `article_memory`) — more robust than the 24h-window guard.
- DNS rebinding defense in crawl4ai adapter (resolve to IP before Chromium launch, reject private ranges).
- Per-host rate limiting inside the worker pool (e.g. "max 1 zhihu fetch per 10 seconds") to avoid triggering anti-scrape.

---

## 15. Open questions

None at spec-close; all decisions were made during brainstorming. The writing-plans session will translate this into ordered tasks.

## 16. Glossary

- **Profile** — a Chromium `user_data_dir` under `profiles/<name>/`. Carries cookies, localStorage, cached fingerprints. Scoped per site for ingest.
- **Host suffix match** — `zhihu.com` matches `www.zhihu.com`, `zhuanlan.zhihu.com`, `cn.zhihu.com`. Does not match `zhihubus.com` (boundary check: either exact host or `.{suffix}`).
- **Job** — one ingest attempt. 1:1 with a submitted URL (modulo dedup). Never retried automatically.
- **Active status** — any of Pending/Fetching/Cleaning/Judging/Embedding. Used for in-flight dedup.
- **Terminal status** — Saved / Rejected / Failed.
- **Zero-fork pipeline** — ingest does not duplicate or alter the `add_article_memory → normalize → embed` sequence. It reuses the exact same functions.

---

## Architecture Addendum (2026-04-24)

The original "Channels" framing in §1 and §3 incorrectly treated iMessage
as a direct HTTP caller on par with CLI/cron. In reality, iMessage is
handled by the open-source ZeroClaw process, which exposes its LLM agent
to user messages and discovers daemon capabilities through the skill-tool
contract at `project-skills/article-memory/SKILL.toml`. Phase 2 closes
this gap by updating the skill prompts and tool entries to match the real
`POST /article-memory/ingest` body schema, adding `force=true` semantics,
and wiring a `reply_handle`-driven completion notification via osascript
inside the daemon itself. See the Phase 2 spec
(`2026-04-24-article-memory-phase2-skill-and-dedup-design.md`) for the
new contract.

The `[article_memory.ingest]` config section lives in `config/davis/local.toml`
and is consumed by the daemon via `LocalConfig` only. ZeroClaw does not
reference these fields (verified 2026-04-24 via `rg` over
`/Users/faillonexie/Projects/zeroclaw/src`), so `render_runtime_config_str`
does not need to propagate the section.
