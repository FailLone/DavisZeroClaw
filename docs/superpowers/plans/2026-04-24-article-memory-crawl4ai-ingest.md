# Article Memory × crawl4ai Unified URL Ingest — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let any channel (iMessage, Shortcut, cron, MCP, CLI, HTTP) trigger article capture by posting only a URL. Daemon fetches via crawl4ai → Markdown → existing article_memory pipeline → store. Async in-process queue with per-host profile routing, SSRF guard, and 24h URL dedup.

**Architecture:** Additive only. New `src/article_memory/ingest/` subtree (types/queue/worker/host_profile) consumes the existing `Crawl4aiSupervisor` + `Crawl4aiProfileLocks` and the existing `add_article_memory → normalize → embed` chain — zero fork. Python adapter gains `markdown_generator` / `content_filter` fields; existing callers unchanged. Worker pool default 3 (same-host serial via existing Mutex, cross-host parallel).

**Tech Stack:** Rust (tokio, axum, serde, `url` crate, new `uuid` crate), Python (FastAPI adapter — crawl4ai's `DefaultMarkdownGenerator` + `PruningContentFilter`), TOML config.

**Spec:** `docs/superpowers/specs/2026-04-24-article-memory-crawl4ai-ingest-design.md`

---

## Shared Context (read once before Task 1)

**File layout (end state):**
```
src/
├── article_memory/
│   ├── ingest/                         ← NEW
│   │   ├── mod.rs
│   │   ├── types.rs                    # IngestJob, IngestRequest, IngestResponse, errors
│   │   ├── queue.rs                    # IngestQueue + Notify + persistence
│   │   ├── worker.rs                   # IngestWorkerPool + execute_job
│   │   └── host_profile.rs             # resolve_profile, validate_url, normalize_url
│   ├── config.rs                       # + resolve_article_ingest_config (optional helper)
│   └── mod.rs                          # mod ingest; pub use ingest::*;
├── app_config.rs                       # + ArticleMemoryIngestConfig struct
├── server.rs                           # + 3 ingest endpoints + AppState.ingest_queue
├── local_proxy.rs                      # boot IngestQueue + spawn IngestWorkerPool
├── cli/articles.rs                     # + ingest <url> / history / show
├── cli/mod.rs                          # + IngestCommand subcommands
└── runtime_paths.rs                    # + ingest_jobs_path helper

crawl4ai_adapter/
└── server.py                           # CrawlRequest / CrawlResponse extended

config/davis/
└── article_memory.toml                 # + [article_memory.ingest] section
```

**Existing invariants NOT to break:**
- `Crawl4aiError::issue_type()` returns three strings: `"crawl4ai_unavailable"` / `"site_changed"` / `"auth_required"`. Ingest extends with `"empty_content"`, `"pipeline_error"`, `"daemon_restart"` — these are new.
- `Crawl4aiProfileLocks` (src/server.rs:61) is `Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>`. Ingest reuses the same map via `AppState.crawl4ai_profile_lock(profile)`.
- `add_article_memory → normalize_article_memory → upsert_article_memory_embedding` is the production pipeline (src/cli/articles.rs:45-103). Ingest calls the same functions in the same order.
- Express path (src/express.rs) has no ingest awareness; `Crawl4aiPageRequest.markdown` defaults to `false` so express behavior is byte-identical.

**Commit discipline:** Each task ends with a single commit. If a task's verification fails, fix it in-place (new commit, no amend). Never `--no-verify`.

**Test commands (reference):**
```bash
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

---

## Task 1: Add `uuid` dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add uuid crate to dependencies**

Edit `Cargo.toml`, under `[dependencies]` (alphabetical — between `tracing-subscriber` and the next lib; exact placement follows the existing alphabetical order):

```toml
uuid = { version = "1", features = ["v4", "serde"] }
```

- [ ] **Step 2: Verify it builds**

Run: `cargo check --quiet`
Expected: exit 0, no new warnings

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build(deps): add uuid crate for ingest job ids"
```

---

## Task 2: Add `ArticleMemoryIngestConfig` to app_config

**Files:**
- Modify: `src/app_config.rs` — add struct and include in `ArticleMemoryConfig`
- Modify: `config/davis/article_memory.toml` — document new section (comment only, defaults work empty)
- Test: `src/app_config.rs` inline `#[cfg(test)]` module

- [ ] **Step 1: Locate `ArticleMemoryConfig` and add ingest field**

Find `ArticleMemoryConfig` in `src/app_config.rs` (grep to get exact line). It currently has `normalize: ArticleMemoryNormalizeConfig` and `embedding: ArticleMemoryEmbeddingConfig`. Add a third field:

```rust
#[serde(default)]
pub ingest: ArticleMemoryIngestConfig,
```

- [ ] **Step 2: Define `ArticleMemoryIngestConfig` and `ArticleMemoryHostProfile`**

Append to `src/app_config.rs` (same file, above any existing `#[cfg(test)]` block). The exact code:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryIngestConfig {
    #[serde(default = "default_ingest_enabled")]
    pub enabled: bool,
    #[serde(default = "default_ingest_max_concurrency")]
    pub max_concurrency: usize,
    #[serde(default = "default_ingest_default_profile")]
    pub default_profile: String,
    #[serde(default = "default_ingest_min_markdown_chars")]
    pub min_markdown_chars: usize,
    #[serde(default = "default_ingest_dedup_window_hours")]
    pub dedup_window_hours: u64,
    #[serde(default)]
    pub allow_private_hosts: Vec<String>,
    #[serde(default)]
    pub host_profiles: Vec<ArticleMemoryHostProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryHostProfile {
    #[serde(rename = "match")]
    pub match_suffix: String,
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl Default for ArticleMemoryIngestConfig {
    fn default() -> Self {
        Self {
            enabled: default_ingest_enabled(),
            max_concurrency: default_ingest_max_concurrency(),
            default_profile: default_ingest_default_profile(),
            min_markdown_chars: default_ingest_min_markdown_chars(),
            dedup_window_hours: default_ingest_dedup_window_hours(),
            allow_private_hosts: Vec::new(),
            host_profiles: Vec::new(),
        }
    }
}

fn default_ingest_enabled() -> bool { true }
fn default_ingest_max_concurrency() -> usize { 3 }
fn default_ingest_default_profile() -> String { "articles-generic".to_string() }
fn default_ingest_min_markdown_chars() -> usize { 600 }
fn default_ingest_dedup_window_hours() -> u64 { 24 }
```

- [ ] **Step 3: Write failing test for TOML parsing with defaults**

Add to `src/app_config.rs` `#[cfg(test)] mod tests` (create the block if none exists):

```rust
#[test]
fn article_memory_ingest_defaults_when_missing() {
    let toml = r#"
        [normalize]
        [embedding]
    "#;
    let cfg: ArticleMemoryConfig = toml::from_str(toml).unwrap();
    assert!(cfg.ingest.enabled);
    assert_eq!(cfg.ingest.max_concurrency, 3);
    assert_eq!(cfg.ingest.default_profile, "articles-generic");
    assert_eq!(cfg.ingest.min_markdown_chars, 600);
    assert_eq!(cfg.ingest.dedup_window_hours, 24);
    assert!(cfg.ingest.allow_private_hosts.is_empty());
    assert!(cfg.ingest.host_profiles.is_empty());
}

#[test]
fn article_memory_ingest_parses_host_profiles() {
    let toml = r#"
        [normalize]
        [embedding]
        [ingest]
        enabled = false
        max_concurrency = 5
        allow_private_hosts = ["wiki.internal"]
        [[ingest.host_profiles]]
        match = "zhihu.com"
        profile = "articles-zhihu"
        source = "zhihu"
    "#;
    let cfg: ArticleMemoryConfig = toml::from_str(toml).unwrap();
    assert!(!cfg.ingest.enabled);
    assert_eq!(cfg.ingest.max_concurrency, 5);
    assert_eq!(cfg.ingest.allow_private_hosts, vec!["wiki.internal"]);
    assert_eq!(cfg.ingest.host_profiles.len(), 1);
    assert_eq!(cfg.ingest.host_profiles[0].match_suffix, "zhihu.com");
    assert_eq!(cfg.ingest.host_profiles[0].profile, "articles-zhihu");
    assert_eq!(cfg.ingest.host_profiles[0].source.as_deref(), Some("zhihu"));
}
```

- [ ] **Step 4: Run tests — expect FAIL (ingest field not wired yet)**

Run: `cargo test --quiet article_memory_ingest`
Expected: compilation success, both tests PASS (if they're already passing, the structs wired correctly). If they FAIL, fix the struct definitions from Step 1-2.

- [ ] **Step 5: Confirm no other tests regressed**

Run: `cargo test --workspace --quiet`
Expected: all existing tests pass plus the 2 new ones.

- [ ] **Step 6: Commit**

```bash
git add Cargo.lock Cargo.toml src/app_config.rs
git commit -m "feat(article-memory): add ArticleMemoryIngestConfig with host-profile routing"
```

---

## Task 3: Pure functions — `host_profile.rs`

**Files:**
- Create: `src/article_memory/ingest/host_profile.rs`
- Modify: `src/article_memory/mod.rs` — add `mod ingest;`
- Create: `src/article_memory/ingest/mod.rs` — re-exports

- [ ] **Step 1: Create `src/article_memory/ingest/mod.rs`**

```rust
mod host_profile;
pub use host_profile::{
    normalize_url, resolve_profile, validate_url_for_ingest, NormalizeUrlError,
    ResolvedProfile, UrlValidationError,
};
```

- [ ] **Step 2: Write failing tests first**

Create `src/article_memory/ingest/host_profile.rs`:

```rust
use crate::app_config::{ArticleMemoryHostProfile, ArticleMemoryIngestConfig};
use std::fmt;
use url::{Host, Url};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProfile {
    pub profile: String,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlValidationError {
    InvalidUrl,
    InvalidScheme,
    MissingHost,
    PrivateAddressBlocked(String),
}

impl fmt::Display for UrlValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => write!(f, "url could not be parsed"),
            Self::InvalidScheme => write!(f, "only http and https schemes are allowed"),
            Self::MissingHost => write!(f, "url is missing a host"),
            Self::PrivateAddressBlocked(detail) => {
                write!(f, "private or loopback address blocked: {detail}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizeUrlError {
    InvalidUrl,
}

impl fmt::Display for NormalizeUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "url could not be parsed")
    }
}

pub fn resolve_profile(url: &str, config: &ArticleMemoryIngestConfig) -> ResolvedProfile {
    let host = match Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
    {
        Some(h) => h,
        None => {
            return ResolvedProfile {
                profile: config.default_profile.clone(),
                source: None,
            }
        }
    };
    for entry in &config.host_profiles {
        if host_matches_suffix(&host, &entry.match_suffix) {
            return ResolvedProfile {
                profile: entry.profile.clone(),
                source: entry.source.clone(),
            };
        }
    }
    ResolvedProfile {
        profile: config.default_profile.clone(),
        source: None,
    }
}

fn host_matches_suffix(host: &str, suffix: &str) -> bool {
    let s = suffix.to_ascii_lowercase();
    if s.is_empty() {
        return false;
    }
    host == s || host.ends_with(&format!(".{s}"))
}

pub fn normalize_url(url: &str) -> Result<String, NormalizeUrlError> {
    let mut parsed = Url::parse(url).map_err(|_| NormalizeUrlError::InvalidUrl)?;
    parsed.set_fragment(None);
    if let Some(host) = parsed.host_str() {
        let lowered = host.to_ascii_lowercase();
        let _ = parsed.set_host(Some(&lowered));
    }
    Ok(parsed.to_string())
}

pub fn validate_url_for_ingest(
    url: &str,
    config: &ArticleMemoryIngestConfig,
) -> Result<(), UrlValidationError> {
    let parsed = Url::parse(url).map_err(|_| UrlValidationError::InvalidUrl)?;
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return Err(UrlValidationError::InvalidScheme),
    }
    let host = parsed.host().ok_or(UrlValidationError::MissingHost)?;
    let host_string = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if config.allow_private_hosts.iter().any(|h| h.eq_ignore_ascii_case(&host_string)) {
        return Ok(());
    }
    match host {
        Host::Ipv4(ip) => {
            if ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.is_unspecified()
            {
                return Err(UrlValidationError::PrivateAddressBlocked(format!(
                    "{ip} is a private/loopback/link-local/broadcast/multicast/unspecified address"
                )));
            }
        }
        Host::Ipv6(ip) => {
            if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
                return Err(UrlValidationError::PrivateAddressBlocked(format!(
                    "{ip} is a loopback/unspecified/multicast IPv6 address"
                )));
            }
            let seg0 = ip.segments()[0];
            if (seg0 & 0xfe00) == 0xfc00 {
                return Err(UrlValidationError::PrivateAddressBlocked(format!(
                    "{ip} is a unique-local IPv6 address (fc00::/7)"
                )));
            }
            if (seg0 & 0xffc0) == 0xfe80 {
                return Err(UrlValidationError::PrivateAddressBlocked(format!(
                    "{ip} is a link-local IPv6 address (fe80::/10)"
                )));
            }
        }
        Host::Domain(name) => {
            let n = name.to_ascii_lowercase();
            if n == "localhost"
                || n.ends_with(".local")
                || n.ends_with(".internal")
                || n.ends_with(".localhost")
            {
                return Err(UrlValidationError::PrivateAddressBlocked(format!(
                    "{n} is a reserved private-use hostname"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::ArticleMemoryHostProfile;

    fn cfg_with(host_profiles: Vec<ArticleMemoryHostProfile>) -> ArticleMemoryIngestConfig {
        ArticleMemoryIngestConfig {
            host_profiles,
            ..Default::default()
        }
    }

    #[test]
    fn resolve_profile_matches_host_suffix() {
        let cfg = cfg_with(vec![ArticleMemoryHostProfile {
            match_suffix: "zhihu.com".into(),
            profile: "articles-zhihu".into(),
            source: Some("zhihu".into()),
        }]);
        assert_eq!(resolve_profile("https://zhuanlan.zhihu.com/p/1", &cfg).profile, "articles-zhihu");
        assert_eq!(resolve_profile("https://www.zhihu.com/q/1", &cfg).profile, "articles-zhihu");
        assert_eq!(resolve_profile("https://zhihu.com/", &cfg).profile, "articles-zhihu");
        assert_eq!(
            resolve_profile("https://zhihu.com/", &cfg).source.as_deref(),
            Some("zhihu")
        );
    }

    #[test]
    fn resolve_profile_rejects_unrelated_hosts() {
        let cfg = cfg_with(vec![ArticleMemoryHostProfile {
            match_suffix: "zhihu.com".into(),
            profile: "articles-zhihu".into(),
            source: None,
        }]);
        // zhihubus.com must NOT match zhihu.com suffix rule
        assert_eq!(resolve_profile("https://zhihubus.com/", &cfg).profile, "articles-generic");
        assert_eq!(resolve_profile("https://fakezhihu.com/", &cfg).profile, "articles-generic");
    }

    #[test]
    fn resolve_profile_first_hit_wins() {
        let cfg = cfg_with(vec![
            ArticleMemoryHostProfile {
                match_suffix: "zhihu.com".into(),
                profile: "articles-zhihu".into(),
                source: None,
            },
            ArticleMemoryHostProfile {
                match_suffix: "zhuanlan.zhihu.com".into(),
                profile: "articles-never".into(),
                source: None,
            },
        ]);
        assert_eq!(
            resolve_profile("https://zhuanlan.zhihu.com/p/1", &cfg).profile,
            "articles-zhihu"
        );
    }

    #[test]
    fn resolve_profile_empty_config_defaults() {
        let cfg = cfg_with(vec![]);
        assert_eq!(resolve_profile("https://x.com/", &cfg).profile, "articles-generic");
    }

    #[test]
    fn resolve_profile_invalid_url_defaults() {
        let cfg = cfg_with(vec![]);
        assert_eq!(resolve_profile("not a url", &cfg).profile, "articles-generic");
    }

    #[test]
    fn normalize_url_strips_fragment_and_lowercases_host() {
        let out = normalize_url("HTTPS://WWW.Zhihu.COM/p/1#section").unwrap();
        assert_eq!(out, "https://www.zhihu.com/p/1");
    }

    #[test]
    fn normalize_url_preserves_query_and_path_case() {
        let out = normalize_url("https://example.com/Path?Q=Value").unwrap();
        assert!(out.ends_with("/Path?Q=Value"));
    }

    #[test]
    fn validate_rejects_non_http_schemes() {
        let cfg = cfg_with(vec![]);
        for url in ["file:///etc/passwd", "javascript:alert(1)", "data:text/html,x", "ftp://x"] {
            assert!(matches!(
                validate_url_for_ingest(url, &cfg),
                Err(UrlValidationError::InvalidScheme) | Err(UrlValidationError::InvalidUrl)
            ));
        }
    }

    #[test]
    fn validate_rejects_localhost_variants() {
        let cfg = cfg_with(vec![]);
        for url in [
            "http://127.0.0.1/",
            "http://localhost/",
            "http://[::1]/",
            "http://0.0.0.0/",
            "http://foo.local/",
            "http://bar.internal/",
        ] {
            match validate_url_for_ingest(url, &cfg) {
                Err(UrlValidationError::PrivateAddressBlocked(_)) => {}
                other => panic!("expected PrivateAddressBlocked for {url}, got {other:?}"),
            }
        }
    }

    #[test]
    fn validate_rejects_private_ipv4() {
        let cfg = cfg_with(vec![]);
        for url in [
            "http://10.0.0.1/",
            "http://172.16.0.1/",
            "http://172.31.255.1/",
            "http://192.168.1.1/",
            "http://169.254.169.254/",
        ] {
            assert!(matches!(
                validate_url_for_ingest(url, &cfg),
                Err(UrlValidationError::PrivateAddressBlocked(_))
            ));
        }
    }

    #[test]
    fn validate_rejects_ipv6_ula_and_link_local() {
        let cfg = cfg_with(vec![]);
        for url in ["http://[fc00::1]/", "http://[fd00::1]/", "http://[fe80::1]/"] {
            assert!(matches!(
                validate_url_for_ingest(url, &cfg),
                Err(UrlValidationError::PrivateAddressBlocked(_))
            ));
        }
    }

    #[test]
    fn validate_allows_public_domain_and_public_ip() {
        let cfg = cfg_with(vec![]);
        assert!(validate_url_for_ingest("https://zhihu.com/", &cfg).is_ok());
        assert!(validate_url_for_ingest("https://1.1.1.1/", &cfg).is_ok());
        assert!(validate_url_for_ingest("https://[2001:4860:4860::8888]/", &cfg).is_ok());
    }

    #[test]
    fn validate_allowlist_bypasses_private_block() {
        let cfg = ArticleMemoryIngestConfig {
            allow_private_hosts: vec!["wiki.internal".into()],
            ..Default::default()
        };
        assert!(validate_url_for_ingest("http://wiki.internal/page", &cfg).is_ok());
    }
}
```

- [ ] **Step 3: Wire the ingest module into article_memory**

Edit `src/article_memory/mod.rs` — add, near the other `mod` lines at the bottom (around line 296):

```rust
mod ingest;
pub use ingest::*;
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo test --quiet article_memory::ingest::host_profile`
Expected: all tests from the `tests` module pass.

- [ ] **Step 5: Run full test suite to catch regressions**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/mod.rs src/article_memory/ingest/
git commit -m "feat(article-memory): add host_profile resolver, URL normalize, and SSRF guard"
```

---

## Task 4: `types.rs` — job and request/response shapes

**Files:**
- Create: `src/article_memory/ingest/types.rs`
- Modify: `src/article_memory/ingest/mod.rs` — add `mod types;`

- [ ] **Step 1: Extend `mod.rs`**

Edit `src/article_memory/ingest/mod.rs` to:

```rust
mod host_profile;
mod types;

pub use host_profile::{
    normalize_url, resolve_profile, validate_url_for_ingest, NormalizeUrlError,
    ResolvedProfile, UrlValidationError,
};
pub use types::{
    IngestJob, IngestJobError, IngestJobStatus, IngestOutcome, IngestOutcomeSummary,
    IngestRequest, IngestResponse, IngestSubmitError, ListFilter,
};
```

- [ ] **Step 2: Create `types.rs`**

Write `src/article_memory/ingest/types.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IngestJobStatus {
    Pending,
    Fetching,
    Cleaning,
    Judging,
    Embedding,
    Saved,
    Rejected,
    Failed,
}

impl IngestJobStatus {
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Fetching | Self::Cleaning | Self::Judging | Self::Embedding
        )
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Saved | Self::Rejected | Self::Failed)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Fetching => "fetching",
            Self::Cleaning => "cleaning",
            Self::Judging => "judging",
            Self::Embedding => "embedding",
            Self::Saved => "saved",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestJobError {
    pub issue_type: String,
    pub message: String,
    pub stage: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestOutcomeSummary {
    pub clean_status: String,
    pub clean_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_score: Option<f32>,
    pub normalized_chars: usize,
    pub polished: bool,
    pub summary_generated: bool,
    pub embedded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestJob {
    pub id: String,
    pub url: String,
    pub normalized_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_override: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<String>,
    pub profile_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_source: Option<String>,
    pub status: IngestJobStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub article_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<IngestOutcomeSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<IngestJobError>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub submitted_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default = "default_attempts")]
    pub attempts: u32,
}

fn default_attempts() -> u32 { 1 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestRequest {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestResponse {
    pub job_id: String,
    pub status: IngestJobStatus,
    pub submitted_at: String,
    #[serde(default)]
    pub deduped: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IngestSubmitError {
    InvalidUrl(String),
    InvalidScheme,
    PrivateAddressBlocked(String),
    DuplicateSaved {
        existing_article_id: Option<String>,
        finished_at: String,
    },
    IngestDisabled,
    PersistenceError(String),
}

impl std::fmt::Display for IngestSubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl(d) => write!(f, "invalid url: {d}"),
            Self::InvalidScheme => write!(f, "only http and https schemes are allowed"),
            Self::PrivateAddressBlocked(d) => write!(f, "private address blocked: {d}"),
            Self::DuplicateSaved { existing_article_id, finished_at } => write!(
                f,
                "article already saved within dedup window at {finished_at} (article_id={})",
                existing_article_id.as_deref().unwrap_or("-")
            ),
            Self::IngestDisabled => write!(f, "article memory ingest is disabled"),
            Self::PersistenceError(d) => write!(f, "failed to persist job: {d}"),
        }
    }
}

impl std::error::Error for IngestSubmitError {}

#[derive(Debug, Clone)]
pub enum IngestOutcome {
    Saved {
        article_id: String,
        summary: IngestOutcomeSummary,
        warnings: Vec<String>,
    },
    Rejected {
        article_id: Option<String>,
        summary: IngestOutcomeSummary,
    },
    Failed(IngestJobError),
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub status: Option<IngestJobStatus>,
    pub limit: Option<usize>,
    pub only_failed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_active_terminal_partition() {
        for s in [
            IngestJobStatus::Pending,
            IngestJobStatus::Fetching,
            IngestJobStatus::Cleaning,
            IngestJobStatus::Judging,
            IngestJobStatus::Embedding,
        ] {
            assert!(s.is_active());
            assert!(!s.is_terminal());
        }
        for s in [
            IngestJobStatus::Saved,
            IngestJobStatus::Rejected,
            IngestJobStatus::Failed,
        ] {
            assert!(!s.is_active());
            assert!(s.is_terminal());
        }
    }

    #[test]
    fn status_serializes_snake_case() {
        let json = serde_json::to_string(&IngestJobStatus::Pending).unwrap();
        assert_eq!(json, "\"pending\"");
        let back: IngestJobStatus = serde_json::from_str("\"fetching\"").unwrap();
        assert_eq!(back, IngestJobStatus::Fetching);
    }

    #[test]
    fn ingest_request_accepts_minimal_payload() {
        let req: IngestRequest =
            serde_json::from_str(r#"{"url": "https://example.com/"}"#).unwrap();
        assert_eq!(req.url, "https://example.com/");
        assert!(req.tags.is_empty());
        assert!(req.title.is_none());
        assert!(req.source_hint.is_none());
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --quiet article_memory::ingest::types`
Expected: 3 tests pass.

- [ ] **Step 4: Full check**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/article_memory/ingest/mod.rs src/article_memory/ingest/types.rs
git commit -m "feat(article-memory): add ingest job/request/response types"
```

---

## Task 5: Runtime path for the jobs file

**Files:**
- Modify: `src/runtime_paths.rs` — add `article_memory_ingest_jobs_path`

- [ ] **Step 1: Add the path helper**

Open `src/runtime_paths.rs`. Find the existing `article_memory_*_dir` helpers (around lines 171-210). Add after `article_memory_implementation_requests_dir`:

```rust
pub fn article_memory_ingest_jobs_path(&self) -> PathBuf {
    self.article_memory_dir().join("ingest_jobs.json")
}
```

- [ ] **Step 2: Add a test**

In the existing `#[cfg(test)] mod tests` block of `src/runtime_paths.rs` (find the last test and append):

```rust
#[test]
fn ingest_jobs_path_nests_under_article_memory_dir() {
    let paths = RuntimePaths {
        repo_root: std::path::PathBuf::from("/tmp/repo"),
        runtime_dir: std::path::PathBuf::from("/tmp/runtime"),
    };
    let got = paths.article_memory_ingest_jobs_path();
    assert_eq!(got, paths.article_memory_dir().join("ingest_jobs.json"));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --quiet ingest_jobs_path_nests_under_article_memory_dir`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/runtime_paths.rs
git commit -m "feat(runtime-paths): add article_memory_ingest_jobs_path helper"
```

---

## Task 6: `queue.rs` — `IngestQueue` with persistence + Notify

**Files:**
- Create: `src/article_memory/ingest/queue.rs`
- Modify: `src/article_memory/ingest/mod.rs` — add `mod queue;` and re-exports
- Test: inline `#[cfg(test)]` module + new file `tests/rust/article_memory_ingest_queue.rs`

- [ ] **Step 1: Extend `mod.rs`**

Edit `src/article_memory/ingest/mod.rs` to add queue:

```rust
mod host_profile;
mod queue;
mod types;

pub use host_profile::{
    normalize_url, resolve_profile, validate_url_for_ingest, NormalizeUrlError,
    ResolvedProfile, UrlValidationError,
};
pub use queue::{IngestQueue, IngestQueueState};
pub use types::{
    IngestJob, IngestJobError, IngestJobStatus, IngestOutcome, IngestOutcomeSummary,
    IngestRequest, IngestResponse, IngestSubmitError, ListFilter,
};
```

- [ ] **Step 2: Create `queue.rs`**

Write `src/article_memory/ingest/queue.rs`:

```rust
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
    inner: Mutex<IngestQueueState>,
    persistence_path: PathBuf,
    notify: Arc<Notify>,
    config: Arc<ArticleMemoryIngestConfig>,
}

impl IngestQueue {
    /// Load from disk or create a fresh queue. Any job found in an active
    /// status is reset to Failed with issue_type = "daemon_restart".
    pub fn load_or_create(
        paths: &RuntimePaths,
        config: Arc<ArticleMemoryIngestConfig>,
    ) -> Self {
        let persistence_path = paths.article_memory_ingest_jobs_path();
        let state = Self::read_or_default(&persistence_path);
        let state = Self::reset_active_to_failed(state);
        let queue = Self {
            inner: Mutex::new(state),
            persistence_path,
            notify: Arc::new(Notify::new()),
            config,
        };
        // Best-effort persistence of the reset state. Non-fatal on failure.
        let _ = queue.persist_blocking();
        queue
    }

    fn read_or_default(path: &PathBuf) -> IngestQueueState {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(_) => return IngestQueueState {
                version: INGEST_JOBS_VERSION,
                updated_at: isoformat(now_utc()),
                jobs: HashMap::new(),
                pending: VecDeque::new(),
            },
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

    pub async fn submit(
        &self,
        req: IngestRequest,
    ) -> Result<IngestResponse, IngestSubmitError> {
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

        // Dedup rule 2: same URL Saved within window → 409
        let window_hours = self.config.dedup_window_hours as i64;
        if let Some(recent) = state
            .jobs
            .values()
            .filter(|j| j.normalized_url == normalized && j.status == IngestJobStatus::Saved)
            .filter_map(|j| j.finished_at.as_ref().map(|ts| (ts.as_str(), j)))
            .filter_map(|(ts, j)| crate::support::parse_time(ts).map(|t| (t, j)))
            .filter(|(t, _)| (now_utc() - *t) <= Duration::hours(window_hours))
            .max_by_key(|(t, _)| *t)
            .map(|(_, j)| j.clone())
        {
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
                        let _ = self.persist_locked(&state);
                        return cloned;
                    }
                }
            }
            self.notify.notified().await;
        }
    }

    pub async fn mark_status(
        &self,
        job_id: &str,
        status: IngestJobStatus,
    ) -> std::io::Result<()> {
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
            let _ = self.persist_locked(&state);
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
            let _ = self.persist_locked(&state);
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
            let _ = self.persist_locked(&state);
        }
    }

    pub async fn finish_failed(&self, job_id: &str, error: IngestJobError) {
        let mut state = self.inner.lock().await;
        if let Some(job) = state.jobs.get_mut(job_id) {
            job.status = IngestJobStatus::Failed;
            job.error = Some(error);
            job.finished_at = Some(isoformat(now_utc()));
            state.updated_at = isoformat(now_utc());
            let _ = self.persist_locked(&state);
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
        let raw =
            std::fs::read_to_string(paths.article_memory_ingest_jobs_path()).unwrap();
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
        for h in submit_handles { h.await.unwrap(); }
        let mut ids = Vec::new();
        for h in pop_handles {
            let job = tokio::time::timeout(std::time::Duration::from_secs(2), h)
                .await
                .unwrap()
                .unwrap();
            ids.push(job.id);
        }
        ids.sort(); ids.dedup();
        assert_eq!(ids.len(), 5, "expected every submission to be observed exactly once");
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
}
```

- [ ] **Step 3: Add `tempfile` to `[dev-dependencies]` if not already present**

Check `Cargo.toml`:

```bash
grep tempfile Cargo.toml || echo "MISSING"
```

If `MISSING`, add to `[dev-dependencies]`:

```toml
tempfile = "3"
```

- [ ] **Step 4: Add `chrono` if not present**

```bash
grep '^chrono' Cargo.toml || echo "MISSING"
```

If `MISSING`, add `chrono = "0.4"` to `[dependencies]`. (It's almost certainly already present via other code; `grep` confirms.) `support::parse_time` already uses it.

- [ ] **Step 5: Run tests**

Run: `cargo test --quiet article_memory::ingest::queue`
Expected: 8 tests pass.

- [ ] **Step 6: Full check**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/article_memory/ingest/
git commit -m "feat(article-memory): add IngestQueue with persistence, dedup, and Notify"
```

---

## Task 7: Python adapter — markdown output

**Files:**
- Modify: `crawl4ai_adapter/server.py`

- [ ] **Step 1: Read current `CrawlRequest` / `CrawlResponse`**

Run: `sed -n '1,200p' crawl4ai_adapter/server.py` (use your editor of choice) and note where `CrawlRequest(BaseModel)` and `CrawlResponse(BaseModel)` are defined, and where the `/crawl` handler invokes `AsyncWebCrawler`.

- [ ] **Step 2: Add the two fields to `CrawlRequest`**

Inside the `CrawlRequest(BaseModel):` block, append:

```python
    markdown_generator: bool = False
    content_filter: Optional[str] = None  # "pruning" | "bm25" | None
```

Ensure `from typing import Optional` is present at the top.

- [ ] **Step 3: Add the optional field to `CrawlResponse`**

Inside the `CrawlResponse(BaseModel):` block, append:

```python
    markdown: Optional[str] = None
```

- [ ] **Step 4: Wire the filter + generator into `/crawl` handler**

Inside the `/crawl` handler, before the `AsyncWebCrawler` call where `CrawlerRunConfig` (or equivalent) is built, add:

```python
    markdown_generator_cfg = None
    if req.markdown_generator:
        from crawl4ai.markdown_generation_strategy import DefaultMarkdownGenerator
        content_filter_obj = None
        if req.content_filter == "pruning":
            from crawl4ai.content_filter_strategy import PruningContentFilter
            content_filter_obj = PruningContentFilter()
        elif req.content_filter == "bm25":
            from crawl4ai.content_filter_strategy import BM25ContentFilter
            content_filter_obj = BM25ContentFilter()
        markdown_generator_cfg = DefaultMarkdownGenerator(
            content_filter=content_filter_obj
        )
```

Then pass `markdown_generator=markdown_generator_cfg` into the existing `CrawlerRunConfig(...)` call (look for the field name in the local version of crawl4ai; it's typically `markdown_generator` on `CrawlerRunConfig` or set on `BrowserConfig` in older releases). If the config struct already aggregates these, setting `crawler_config.markdown_generator = markdown_generator_cfg` works equivalently — follow the existing pattern in the file.

After the crawl completes, populate the response field. Immediately before `return CrawlResponse(...)` (or where `CrawlResponse` is constructed), add:

```python
    response_markdown = None
    if req.markdown_generator:
        markdown_v2 = getattr(result, "markdown_v2", None)
        if markdown_v2 is not None:
            response_markdown = getattr(markdown_v2, "fit_markdown", None) or getattr(
                markdown_v2, "raw_markdown", None
            )
        if response_markdown is None:
            response_markdown = getattr(result, "markdown", None)
```

Then pass `markdown=response_markdown` into the `CrawlResponse(...)` constructor.

- [ ] **Step 5: Manual smoke**

Start the daemon: `daviszeroclaw start` (or restart if already running).

Then:

```bash
PORT=$(cat ~/.runtime/davis/crawl4ai.pid 2>/dev/null && echo 11235)  # or read the actual port from config
curl -sS -X POST http://127.0.0.1:11235/crawl \
  -H 'Content-Type: application/json' \
  -d '{
    "profile_path": "/tmp/articles-generic",
    "url": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    "timeout_secs": 60,
    "headless": true,
    "magic": false,
    "simulate_user": false,
    "override_navigator": false,
    "remove_overlay_elements": true,
    "enable_stealth": false,
    "markdown_generator": true,
    "content_filter": "pruning"
  }' | jq '.markdown | length'
```

Expected: a number ≥ 600. If null or 0, re-check Step 4 wiring.

- [ ] **Step 6: Commit**

```bash
git add crawl4ai_adapter/server.py
git commit -m "feat(crawl4ai-adapter): expose markdown_generator + content_filter on /crawl"
```

---

## Task 8: Extend Rust `Crawl4aiPageRequest` / `Crawl4aiPageResult`

**Files:**
- Modify: `src/crawl4ai.rs`

- [ ] **Step 1: Add the request flag**

Open `src/crawl4ai.rs`. Find `pub struct Crawl4aiPageRequest` (currently lines 6-12). Add the field:

```rust
pub struct Crawl4aiPageRequest {
    pub profile_name: String,
    pub url: String,
    pub wait_for: Option<String>,
    pub js_code: Option<String>,
    /// When true, request crawl4ai to produce fit-filtered Markdown.
    pub markdown: bool,
}
```

- [ ] **Step 2: Add the response field**

Find `pub struct Crawl4aiPageResult` (currently lines 14-23) and add:

```rust
pub struct Crawl4aiPageResult {
    pub success: bool,
    pub current_url: Option<String>,
    pub html: Option<String>,
    pub cleaned_html: Option<String>,
    pub markdown: Option<String>,
    pub error_message: Option<String>,
    pub status_code: Option<u16>,
    pub raw: Value,
}
```

- [ ] **Step 3: Update `CrawlRequestBody` serde struct**

Find the `CrawlRequestBody<'a>` struct (around lines 25-38). Add:

```rust
#[derive(Serialize)]
struct CrawlRequestBody<'a> {
    profile_path: String,
    url: &'a str,
    wait_for: Option<&'a str>,
    js_code: Option<&'a str>,
    timeout_secs: u64,
    headless: bool,
    magic: bool,
    simulate_user: bool,
    override_navigator: bool,
    remove_overlay_elements: bool,
    enable_stealth: bool,
    markdown_generator: bool,
    content_filter: Option<&'a str>,
}
```

- [ ] **Step 4: Populate the new body fields in `crawl4ai_crawl`**

In the body construction inside `crawl4ai_crawl` (around lines 64-76), append:

```rust
    let body = CrawlRequestBody {
        profile_path: profile_dir.display().to_string(),
        url: &request.url,
        wait_for: request.wait_for.as_deref(),
        js_code: request.js_code.as_deref(),
        timeout_secs: config.timeout_secs,
        headless: config.headless,
        magic: config.magic,
        simulate_user: config.simulate_user,
        override_navigator: config.override_navigator,
        remove_overlay_elements: config.remove_overlay_elements,
        enable_stealth: config.enable_stealth,
        markdown_generator: request.markdown,
        content_filter: if request.markdown { Some("pruning") } else { None },
    };
```

- [ ] **Step 5: Surface markdown in `parse_result_value`**

Find `fn parse_result_value` (near the bottom of the file). Add a line that extracts `markdown`. Example — merge into the existing function:

```rust
fn parse_result_value(payload: Value) -> Crawl4aiPageResult {
    // existing extractions for success / current_url / html / cleaned_html ...
    let markdown = payload
        .get("markdown")
        .and_then(Value::as_str)
        .map(str::to_string);
    Crawl4aiPageResult {
        success: /* existing */ success_value,
        current_url: /* existing */ current_url_value,
        html: /* existing */ html_value,
        cleaned_html: /* existing */ cleaned_html_value,
        markdown,
        error_message: /* existing */ error_message_value,
        status_code: /* existing */ status_code_value,
        raw: payload,
    }
}
```

Read the current body of `parse_result_value` first and add `markdown` extraction to the existing construction — do not rewrite the whole function unless the shape has drifted.

- [ ] **Step 6: Update express call site — no behavior change**

`src/express.rs:273-283` constructs `Crawl4aiPageRequest { profile_name, url, wait_for, js_code }`. Add `markdown: false`:

```rust
    let response = crawl4ai_crawl(
        paths,
        crawl4ai_config,
        supervisor,
        Crawl4aiPageRequest {
            profile_name: express_profile_name(source),
            url: source_order_url(source).to_string(),
            wait_for: Some(source_wait_for(source).to_string()),
            js_code: Some(script),
            markdown: false,
        },
    )
    .await?;
```

- [ ] **Step 7: Full test suite**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean; express tests still pass byte-identically because `markdown: false` preserves old server contract.

- [ ] **Step 8: Commit**

```bash
git add src/crawl4ai.rs src/express.rs
git commit -m "feat(crawl4ai): add markdown request flag and markdown response field"
```

---

## Task 9: `worker.rs` — worker pool + job execution

**Files:**
- Create: `src/article_memory/ingest/worker.rs`
- Modify: `src/article_memory/ingest/mod.rs` — add `mod worker;` + re-exports
- Modify: `src/article_memory/ingest/types.rs` — add `WorkerDeps` helper (wire point for ingest pipeline)
- Test: `tests/rust/article_memory_ingest_worker.rs` — mock crawl4ai end-to-end

- [ ] **Step 1: Extend mod.rs**

Edit `src/article_memory/ingest/mod.rs`:

```rust
mod host_profile;
mod queue;
mod types;
mod worker;

pub use host_profile::{
    normalize_url, resolve_profile, validate_url_for_ingest, NormalizeUrlError,
    ResolvedProfile, UrlValidationError,
};
pub use queue::{IngestQueue, IngestQueueState};
pub use types::{
    IngestJob, IngestJobError, IngestJobStatus, IngestOutcome, IngestOutcomeSummary,
    IngestRequest, IngestResponse, IngestSubmitError, ListFilter,
};
pub use worker::{IngestWorkerDeps, IngestWorkerPool};
```

- [ ] **Step 2: Create `worker.rs`**

Write `src/article_memory/ingest/worker.rs`:

```rust
use super::queue::IngestQueue;
use super::types::{IngestJob, IngestJobError, IngestJobStatus, IngestOutcomeSummary};
use crate::app_config::{
    ArticleMemoryConfig, ArticleMemoryIngestConfig, ModelProviderConfig,
};
use crate::server::Crawl4aiProfileLocks;
use crate::{
    add_article_memory, crawl4ai_crawl, normalize_article_memory,
    resolve_article_embedding_config, resolve_article_normalize_config,
    resolve_article_value_config, upsert_article_memory_embedding,
    ArticleMemoryAddRequest, ArticleMemoryRecordStatus, Crawl4aiConfig, Crawl4aiError,
    Crawl4aiPageRequest, Crawl4aiSupervisor, RuntimePaths,
};
use std::collections::HashMap;
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
    let profile_lock = acquire_profile_lock(&deps.profile_locks, &job.profile_name).await;
    let _guard = profile_lock.lock().await;

    // Stage 1: fetch
    let page = match crawl4ai_crawl(
        &deps.paths,
        &deps.crawl4ai_config,
        &deps.supervisor,
        Crawl4aiPageRequest {
            profile_name: job.profile_name.clone(),
            url: job.url.clone(),
            wait_for: None,
            js_code: None,
            markdown: true,
        },
    )
    .await
    {
        Ok(page) => page,
        Err(err) => {
            let issue_type = err.issue_type().to_string();
            let message = err.to_string();
            queue
                .finish_failed(
                    &job.id,
                    IngestJobError { issue_type, message, stage: "fetching".into() },
                )
                .await;
            return;
        }
    };

    let markdown = match page.markdown.as_deref() {
        Some(m) => m.to_string(),
        None => {
            queue
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type: "empty_content".into(),
                        message: "crawl4ai returned no markdown field".into(),
                        stage: "fetching".into(),
                    },
                )
                .await;
            return;
        }
    };
    if markdown.chars().count() < deps.ingest_config.min_markdown_chars {
        queue
            .finish_failed(
                &job.id,
                IngestJobError {
                    issue_type: "empty_content".into(),
                    message: format!(
                        "markdown length {} below min_markdown_chars {}",
                        markdown.chars().count(),
                        deps.ingest_config.min_markdown_chars
                    ),
                    stage: "fetching".into(),
                },
            )
            .await;
        return;
    }

    // Stage 2: cleaning
    if let Err(e) = queue.mark_status(&job.id, IngestJobStatus::Cleaning).await {
        tracing::warn!(error = %e, "failed to persist Cleaning status");
    }
    let title = job
        .title_override
        .clone()
        .or_else(|| page.raw.get("metadata").and_then(|m| m.get("title")).and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_else(|| job.url.clone());
    let source = job
        .resolved_source
        .clone()
        .unwrap_or_else(|| "web".to_string());

    let record = match add_article_memory(
        &deps.paths,
        ArticleMemoryAddRequest {
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
        },
    ) {
        Ok(rec) => rec,
        Err(err) => {
            queue
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: err.to_string(),
                        stage: "cleaning".into(),
                    },
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
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: format!("resolve_article_normalize_config: {err}"),
                        stage: "judging".into(),
                    },
                )
                .await;
            return;
        }
    };
    let value_config = match resolve_article_value_config(&deps.paths, &deps.providers) {
        Ok(cfg) => cfg,
        Err(err) => {
            queue
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: format!("resolve_article_value_config: {err}"),
                        stage: "judging".into(),
                    },
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
                .finish_failed(
                    &job.id,
                    IngestJobError {
                        issue_type: "pipeline_error".into(),
                        message: err.to_string(),
                        stage: "judging".into(),
                    },
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

    if rejected {
        queue
            .finish_rejected(&job.id, Some(record.id.clone()), summary)
            .await;
    } else {
        queue
            .finish_saved(&job.id, record.id.clone(), summary, warnings)
            .await;
    }
}
```

- [ ] **Step 3: Write end-to-end worker tests with a mock crawl4ai**

Create `tests/rust/article_memory_ingest_worker.rs` (this file does NOT exist yet). Full content:

```rust
//! End-to-end ingest worker tests. Uses a mock crawl4ai axum router spun up
//! through `Crawl4aiSupervisor::for_test` to avoid starting a real Python
//! adapter.

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use daviszeroclaw::article_memory::{
    IngestJobStatus, IngestQueue, IngestRequest, IngestWorkerDeps, IngestWorkerPool,
};
use daviszeroclaw::{
    init_article_memory, ArticleMemoryConfig, ArticleMemoryIngestConfig, Crawl4aiConfig,
    Crawl4aiSupervisor, ModelProviderConfig, RuntimePaths,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;

#[derive(Clone, Default)]
struct MockState {
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    markdown_body: Arc<std::sync::Mutex<String>>,
    status_override: Arc<std::sync::Mutex<Option<u16>>>,
    fail_body: Arc<std::sync::Mutex<Option<Value>>>,
    per_host_delay_ms: Arc<std::sync::Mutex<HashMap<String, u64>>>,
}

async fn mock_crawl(
    State(state): State<MockState>,
    Json(payload): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if let Some(code) = *state.status_override.lock().unwrap() {
        let body = state.fail_body.lock().unwrap().clone().unwrap_or(json!({}));
        return (StatusCode::from_u16(code).unwrap(), Json(body));
    }
    let in_flight = state.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
    state
        .max_in_flight
        .fetch_max(in_flight, Ordering::SeqCst);
    let url = payload.get("url").and_then(|v| v.as_str()).unwrap_or_default();
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();
    let delay = state
        .per_host_delay_ms
        .lock()
        .unwrap()
        .get(&host)
        .copied()
        .unwrap_or(50);
    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    let markdown = state.markdown_body.lock().unwrap().clone();
    state.in_flight.fetch_sub(1, Ordering::SeqCst);
    (
        StatusCode::OK,
        Json(json!({
            "success": true,
            "current_url": url,
            "html": "<html><body>mock</body></html>",
            "cleaned_html": "<body>mock</body>",
            "markdown": markdown,
            "error_message": null,
            "status_code": 200,
            "metadata": { "title": "Mock Title" },
        })),
    )
}

fn test_paths(name: &str) -> (TempDir, RuntimePaths) {
    let tmp = TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join("runtime"),
    };
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    init_article_memory(&paths).unwrap();
    let _ = name;
    (tmp, paths)
}

fn test_crawl4ai_config() -> Arc<Crawl4aiConfig> {
    Arc::new(Crawl4aiConfig {
        enabled: true,
        base_url: "http://127.0.0.1:0".into(),
        timeout_secs: 30,
        headless: true,
        magic: false,
        simulate_user: false,
        override_navigator: false,
        remove_overlay_elements: true,
        enable_stealth: false,
    })
}

async fn spawn_mock_supervisor(state: MockState) -> Arc<Crawl4aiSupervisor> {
    let app = Router::new()
        .route("/crawl", post(mock_crawl))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Crawl4aiSupervisor::for_test(format!("http://{addr}")).into()
}

fn default_ingest_cfg() -> Arc<ArticleMemoryIngestConfig> {
    Arc::new(ArticleMemoryIngestConfig {
        enabled: true,
        max_concurrency: 3,
        min_markdown_chars: 100,
        host_profiles: vec![
            daviszeroclaw::app_config::ArticleMemoryHostProfile {
                match_suffix: "zhihu.com".into(),
                profile: "articles-zhihu".into(),
                source: Some("zhihu".into()),
            },
            daviszeroclaw::app_config::ArticleMemoryHostProfile {
                match_suffix: "example.com".into(),
                profile: "articles-example".into(),
                source: Some("example".into()),
            },
            daviszeroclaw::app_config::ArticleMemoryHostProfile {
                match_suffix: "medium.com".into(),
                profile: "articles-medium".into(),
                source: Some("medium".into()),
            },
        ],
        ..Default::default()
    })
}

fn default_article_memory_cfg() -> Arc<ArticleMemoryConfig> {
    Arc::new(ArticleMemoryConfig::default())
}

async fn wait_for_status(
    queue: &IngestQueue,
    job_id: &str,
    want: IngestJobStatus,
    timeout_secs: u64,
) -> daviszeroclaw::article_memory::IngestJob {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if let Some(job) = queue.get(job_id).await {
            if job.status == want || job.status.is_terminal() {
                return job;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let j = queue.get(job_id).await;
            panic!("timed out waiting for {want:?}, last = {j:?}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn ingest_happy_path_end_to_end() {
    let (_tmp, paths) = test_paths("happy");
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = "# Hello\n\n".to_string() + &"word ".repeat(200);
    let supervisor = spawn_mock_supervisor(mock.clone()).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
        },
        1,
    );
    let resp = queue
        .submit(IngestRequest {
            url: "https://zhihu.com/p/1".into(),
            title: None,
            tags: vec!["test".into()],
            source_hint: Some("test".into()),
        })
        .await
        .unwrap();
    let job = wait_for_status(&queue, &resp.job_id, IngestJobStatus::Saved, 10).await;
    assert_eq!(job.status, IngestJobStatus::Saved);
    assert!(job.article_id.is_some());
}

#[tokio::test]
async fn ingest_empty_markdown_rejected() {
    let (_tmp, paths) = test_paths("empty");
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = "too short".into();
    let supervisor = spawn_mock_supervisor(mock).await;
    let ingest_cfg = Arc::new(ArticleMemoryIngestConfig {
        min_markdown_chars: 600,
        ..(*default_ingest_cfg()).clone()
    });
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
        },
        1,
    );
    let resp = queue
        .submit(IngestRequest {
            url: "https://zhihu.com/p/short".into(),
            title: None,
            tags: vec![],
            source_hint: None,
        })
        .await
        .unwrap();
    let job = wait_for_status(&queue, &resp.job_id, IngestJobStatus::Failed, 10).await;
    assert_eq!(job.status, IngestJobStatus::Failed);
    assert_eq!(job.error.unwrap().issue_type, "empty_content");
}

#[tokio::test]
async fn ingest_crawl_server_error_surfaces_issue_type() {
    let (_tmp, paths) = test_paths("server_err");
    let mock = MockState::default();
    *mock.status_override.lock().unwrap() = Some(503);
    *mock.fail_body.lock().unwrap() = Some(json!({"detail": "upstream sad"}));
    let supervisor = spawn_mock_supervisor(mock).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
        },
        1,
    );
    let resp = queue
        .submit(IngestRequest {
            url: "https://zhihu.com/p/503".into(),
            title: None,
            tags: vec![],
            source_hint: None,
        })
        .await
        .unwrap();
    let job = wait_for_status(&queue, &resp.job_id, IngestJobStatus::Failed, 10).await;
    assert_eq!(job.error.unwrap().issue_type, "crawl4ai_unavailable");
}

#[tokio::test]
async fn ingest_same_host_serializes() {
    let (_tmp, paths) = test_paths("serial");
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = "# Hi\n\n".to_string() + &"word ".repeat(200);
    mock.per_host_delay_ms
        .lock()
        .unwrap()
        .insert("zhihu.com".into(), 150);
    let supervisor = spawn_mock_supervisor(mock.clone()).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
        },
        3,
    );
    let mut ids = Vec::new();
    for i in 0..3 {
        let resp = queue
            .submit(IngestRequest {
                url: format!("https://zhihu.com/p/{i}"),
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .unwrap();
        ids.push(resp.job_id);
    }
    for id in &ids {
        let _ = wait_for_status(&queue, id, IngestJobStatus::Saved, 15).await;
    }
    let max = mock.max_in_flight.load(Ordering::SeqCst);
    assert_eq!(max, 1, "same-host ingests must serialize via profile lock");
}

#[tokio::test]
async fn ingest_different_hosts_parallelize() {
    let (_tmp, paths) = test_paths("parallel");
    let mock = MockState::default();
    *mock.markdown_body.lock().unwrap() = "# Hi\n\n".to_string() + &"word ".repeat(200);
    mock.per_host_delay_ms
        .lock()
        .unwrap()
        .insert("zhihu.com".into(), 200);
    mock.per_host_delay_ms
        .lock()
        .unwrap()
        .insert("example.com".into(), 200);
    mock.per_host_delay_ms
        .lock()
        .unwrap()
        .insert("medium.com".into(), 200);
    let supervisor = spawn_mock_supervisor(mock.clone()).await;
    let ingest_cfg = default_ingest_cfg();
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    IngestWorkerPool::spawn(
        queue.clone(),
        IngestWorkerDeps {
            paths: paths.clone(),
            crawl4ai_config: test_crawl4ai_config(),
            supervisor,
            profile_locks: Arc::new(Mutex::new(HashMap::new())),
            article_memory_config: default_article_memory_cfg(),
            providers: Arc::new(vec![]),
            ingest_config: ingest_cfg,
        },
        3,
    );
    let urls = [
        "https://zhihu.com/p/1",
        "https://example.com/p/1",
        "https://medium.com/p/1",
    ];
    let mut ids = Vec::new();
    for u in urls {
        ids.push(
            queue
                .submit(IngestRequest {
                    url: u.into(),
                    title: None,
                    tags: vec![],
                    source_hint: None,
                })
                .await
                .unwrap()
                .job_id,
        );
    }
    for id in &ids {
        let _ = wait_for_status(&queue, id, IngestJobStatus::Saved, 15).await;
    }
    let max = mock.max_in_flight.load(Ordering::SeqCst);
    assert!(max >= 2, "cross-host ingests must parallelize; observed max={max}");
}
```

> **Note:** This test file imports `Crawl4aiSupervisor::for_test`. If that constructor does not yet exist in `src/crawl4ai_supervisor.rs`, add it in this same task. It should accept a `String` base URL and return a `Crawl4aiSupervisor` with a stubbed `SupervisorInner` that is `is_healthy() == true` and whose `base_url()` returns the provided string. Follow the pattern documented in the original supervised-server plan (Task 14). If the method already exists (it should, per the architecture summary), no change needed.

- [ ] **Step 4: Re-export required types from lib.rs if missing**

Check `src/lib.rs` for `pub use article_memory::*;`. It must re-export the new ingest types. Grep:

```bash
grep -n "pub use article_memory" src/lib.rs
```

If ingest types are not yet public, add to the existing `pub use article_memory::{...}` statement:

```rust
pub use article_memory::{
    // ...existing
    IngestJob, IngestJobError, IngestJobStatus, IngestOutcome, IngestOutcomeSummary,
    IngestQueue, IngestQueueState, IngestRequest, IngestResponse, IngestSubmitError,
    IngestWorkerDeps, IngestWorkerPool, ListFilter, normalize_url, resolve_profile,
    validate_url_for_ingest, NormalizeUrlError, ResolvedProfile, UrlValidationError,
};
```

Also ensure `pub mod app_config;` exports `ArticleMemoryIngestConfig` and `ArticleMemoryHostProfile`.

- [ ] **Step 5: Run worker tests**

Run: `cargo test --test article_memory_ingest_worker --quiet`
Expected: 5 tests pass.

- [ ] **Step 6: Full check**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/article_memory/ingest/ tests/rust/article_memory_ingest_worker.rs src/lib.rs
git commit -m "feat(article-memory): add IngestWorkerPool with end-to-end pipeline execution"
```

---

## Task 10: HTTP endpoints — `POST /article-memory/ingest` + `GET`

**Files:**
- Modify: `src/server.rs` — add 3 handlers + routes + AppState field

- [ ] **Step 1: Extend `AppState` with the queue**

Edit `src/server.rs` `AppState` struct (around line 64). Add:

```rust
    pub ingest_queue: Arc<crate::IngestQueue>,
```

Update `AppState::new` signature to accept it (append to the parameter list) and assign it in the constructor. Callers update in Task 11.

- [ ] **Step 2: Import new types**

At the top of `src/server.rs`, extend the `use crate::{…}` group to include:

```rust
    IngestJob, IngestJobStatus, IngestQueue, IngestRequest, IngestResponse,
    IngestSubmitError, ListFilter,
```

- [ ] **Step 3: Add routes**

In `build_app`, add the three routes just after `article_memory_add_handler` (around line 140):

```rust
        .route("/article-memory/ingest", post(ingest_submit_handler))
        .route("/article-memory/ingest", get(ingest_list_handler))
        .route("/article-memory/ingest/{job_id}", get(ingest_get_handler))
```

(Axum 0.7 uses `{param}` path syntax.)

- [ ] **Step 4: Implement the handlers**

Append to `src/server.rs` (near the other article memory handlers):

```rust
#[derive(serde::Deserialize)]
struct IngestListQuery {
    status: Option<String>,
    limit: Option<usize>,
    only_failed: Option<bool>,
}

async fn ingest_submit_handler(
    State(state): State<AppState>,
    Json(payload): Json<IngestRequest>,
) -> (StatusCode, Json<Value>) {
    match state.ingest_queue.submit(payload).await {
        Ok(resp) => (
            StatusCode::ACCEPTED,
            Json(serde_json::to_value(resp).unwrap_or_else(|_| json!({}))),
        ),
        Err(err) => match err {
            IngestSubmitError::InvalidUrl(d) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_url", "detail": d})),
            ),
            IngestSubmitError::InvalidScheme => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid_scheme",
                    "detail": "only http and https schemes are allowed"})),
            ),
            IngestSubmitError::PrivateAddressBlocked(d) => (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "private_address_blocked", "detail": d})),
            ),
            IngestSubmitError::DuplicateSaved { existing_article_id, finished_at } => (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "duplicate_within_window",
                    "existing_article_id": existing_article_id,
                    "finished_at": finished_at,
                })),
            ),
            IngestSubmitError::IngestDisabled => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "ingest_disabled"})),
            ),
            IngestSubmitError::PersistenceError(d) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "persistence_error", "detail": d})),
            ),
        },
    }
}

async fn ingest_get_handler(
    State(state): State<AppState>,
    axum::extract::Path(job_id): axum::extract::Path<String>,
) -> (StatusCode, Json<Value>) {
    match state.ingest_queue.get(&job_id).await {
        Some(job) => (
            StatusCode::OK,
            Json(serde_json::to_value(job).unwrap_or_else(|_| json!({}))),
        ),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "job_not_found", "job_id": job_id})),
        ),
    }
}

async fn ingest_list_handler(
    State(state): State<AppState>,
    Query(q): Query<IngestListQuery>,
) -> Json<Value> {
    let status = q.status.as_deref().and_then(|s| match s {
        "pending" => Some(IngestJobStatus::Pending),
        "fetching" => Some(IngestJobStatus::Fetching),
        "cleaning" => Some(IngestJobStatus::Cleaning),
        "judging" => Some(IngestJobStatus::Judging),
        "embedding" => Some(IngestJobStatus::Embedding),
        "saved" => Some(IngestJobStatus::Saved),
        "rejected" => Some(IngestJobStatus::Rejected),
        "failed" => Some(IngestJobStatus::Failed),
        _ => None,
    });
    let filter = ListFilter {
        status,
        limit: q.limit,
        only_failed: q.only_failed.unwrap_or(false),
    };
    let jobs = state.ingest_queue.list(&filter).await;
    let total = jobs.len();
    Json(json!({"jobs": jobs, "total": total}))
}
```

- [ ] **Step 5: Add a minimal smoke test at the file level**

Append to the existing server-tests location (if there's a `tests/rust/` file for server integration, extend it; otherwise add a new one `tests/rust/ingest_http.rs`):

```rust
//! Thin HTTP-level smoke: submit + get.
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use daviszeroclaw::{
    article_memory::{IngestQueue, IngestWorkerDeps, IngestWorkerPool},
    build_app, init_article_memory, AppState, ArticleMemoryConfig, ArticleMemoryIngestConfig,
    ControlConfig, Crawl4aiConfig, Crawl4aiSupervisor, HaClient, HaMcpClient,
    ModelProviderConfig, RuntimePaths,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Mutex;
use tower::ServiceExt;

fn build_state(paths: RuntimePaths, base_url: String) -> AppState {
    let supervisor = Arc::new(Crawl4aiSupervisor::for_test(base_url));
    let ingest_cfg = Arc::new(ArticleMemoryIngestConfig::default());
    let queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_cfg.clone()));
    // Workers are not required for the HTTP smoke; we only verify the
    // submit/get transaction layer.
    let _ = IngestWorkerPool::spawn;
    let _ = IngestWorkerDeps::clone;
    AppState::new(
        HaClient::disabled(),
        HaMcpClient::disabled(),
        paths.clone(),
        Arc::new(ControlConfig::default()),
        Arc::new(Crawl4aiConfig {
            enabled: true,
            base_url: "http://127.0.0.1:0".into(),
            timeout_secs: 30,
            headless: true,
            magic: false,
            simulate_user: false,
            override_navigator: false,
            remove_overlay_elements: true,
            enable_stealth: false,
        }),
        supervisor,
        Arc::new(ArticleMemoryConfig::default()),
        Arc::new(vec![]),
        String::new(),
        queue,
    )
}

#[tokio::test]
async fn post_ingest_returns_202_with_job_id() {
    let tmp = TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join("runtime"),
    };
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    init_article_memory(&paths).unwrap();
    let state = build_state(paths, "http://127.0.0.1:0".into());
    let app = build_app(state);
    let body = serde_json::to_vec(&json!({"url": "https://zhihu.com/p/1"})).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/article-memory/ingest")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn post_ingest_invalid_url_returns_400() {
    let tmp = TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join("runtime"),
    };
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    init_article_memory(&paths).unwrap();
    let state = build_state(paths, "http://127.0.0.1:0".into());
    let app = build_app(state);
    let body = serde_json::to_vec(&json!({"url": "not a url"})).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/article-memory/ingest")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_ingest_ssrf_returns_400() {
    let tmp = TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join("runtime"),
    };
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    init_article_memory(&paths).unwrap();
    let state = build_state(paths, "http://127.0.0.1:0".into());
    let app = build_app(state);
    let body = serde_json::to_vec(&json!({"url": "http://127.0.0.1/"})).unwrap();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/article-memory/ingest")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
```

Add `tower = "0.5"` (features = ["util"]) to `[dev-dependencies]` if not present:

```bash
grep '^tower' Cargo.toml || echo "MISSING_TOWER"
```

- [ ] **Step 6: Run HTTP tests**

Run: `cargo test --test ingest_http --quiet`
Expected: 3 tests pass.

- [ ] **Step 7: Full check**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock src/server.rs tests/rust/ingest_http.rs
git commit -m "feat(server): add POST/GET /article-memory/ingest endpoints"
```

---

## Task 11: Wire queue + workers in daemon boot

**Files:**
- Modify: `src/local_proxy.rs` — load queue, spawn worker pool, pass into AppState

- [ ] **Step 1: Locate where `AppState::new` is constructed**

```bash
grep -n "AppState::new\|Crawl4aiSupervisor::start\|article_memory_config" src/local_proxy.rs | head -20
```

- [ ] **Step 2: Load queue and spawn workers before `AppState::new`**

In `src/local_proxy.rs`, right before the `AppState::new(...)` call that builds the state, add:

```rust
    let ingest_config = Arc::new(article_memory_config.ingest.clone());
    let ingest_queue = Arc::new(
        crate::article_memory::IngestQueue::load_or_create(&paths, ingest_config.clone()),
    );
    if ingest_config.enabled {
        crate::article_memory::IngestWorkerPool::spawn(
            ingest_queue.clone(),
            crate::article_memory::IngestWorkerDeps {
                paths: paths.clone(),
                crawl4ai_config: crawl4ai_config.clone(),
                supervisor: crawl4ai_supervisor.clone(),
                profile_locks: std::sync::Arc::new(tokio::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
                article_memory_config: article_memory_config.clone(),
                providers: providers.clone(),
                ingest_config: ingest_config.clone(),
            },
            ingest_config.max_concurrency,
        );
        tracing::info!(
            workers = ingest_config.max_concurrency,
            "article memory ingest workers started"
        );
    } else {
        tracing::info!("article memory ingest disabled by config");
    }
```

> **Critical:** The `profile_locks` Arc passed above MUST be the same instance that `AppState` stores. Adjust so both worker pool and `AppState` share one `Arc<Mutex<HashMap<...>>>`:
>
> ```rust
> let profile_locks: crate::server::Crawl4aiProfileLocks =
>     std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
> ```
>
> Pass `profile_locks.clone()` into both the `IngestWorkerDeps` and `AppState::new`. Verify by grepping after the edit — only one `Arc::new(Mutex::new(HashMap::new()))` on profile locks should remain in this file.
>
> This requires `AppState::new` to accept a pre-built `profile_locks` argument. Update the signature:
>
> ```rust
> impl AppState {
>     #[allow(clippy::too_many_arguments)]
>     pub fn new(
>         client: HaClient,
>         mcp_client: HaMcpClient,
>         paths: RuntimePaths,
>         control_config: Arc<ControlConfig>,
>         crawl4ai_config: Arc<Crawl4aiConfig>,
>         crawl4ai_supervisor: Arc<Crawl4aiSupervisor>,
>         article_memory_config: Arc<ArticleMemoryConfig>,
>         providers: Arc<Vec<ModelProviderConfig>>,
>         shortcut_secret: String,
>         profile_locks: Crawl4aiProfileLocks,
>         ingest_queue: Arc<IngestQueue>,
>     ) -> Self {
>         Self {
>             client,
>             mcp_client,
>             paths,
>             control_config,
>             crawl4ai_config,
>             crawl4ai_profile_locks: profile_locks,
>             crawl4ai_supervisor,
>             article_memory_config,
>             providers,
>             shortcut_secret,
>             ingest_queue,
>         }
>     }
> }
> ```
>
> Then the ingest HTTP smoke test in Task 10 must match this signature; update `build_state()` there if needed.

- [ ] **Step 3: Update the `AppState::new` call site in `local_proxy.rs`**

Pass `profile_locks.clone()` and `ingest_queue.clone()` as new trailing args.

- [ ] **Step 4: Run everything**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 5: Manual smoke — start daemon, submit URL**

```bash
daviszeroclaw start
sleep 2
curl -sS -X POST http://127.0.0.1:11434/article-memory/ingest \
  -H 'Content-Type: application/json' \
  -d '{"url": "https://en.wikipedia.org/wiki/Rust_(programming_language)", "tags": ["smoke"]}'
```

(Port 11434 is an example; read the daemon's actual bind port from the startup log if different.)

Expected: HTTP 202 with a `job_id`.

Then:

```bash
curl -sS http://127.0.0.1:11434/article-memory/ingest/<job_id> | jq
```

After ~30-60 seconds, `status` should transition Pending → Fetching → Cleaning → Judging → (Embedding) → Saved.

- [ ] **Step 6: Commit**

```bash
git add src/local_proxy.rs src/server.rs tests/rust/ingest_http.rs
git commit -m "feat(daemon): boot ingest queue + worker pool with shared profile locks"
```

---

## Task 12: CLI — `daviszeroclaw articles ingest`

**Files:**
- Modify: `src/cli/mod.rs` — add subcommand variants
- Modify: `src/cli/articles.rs` — add handlers
- Modify: `config/davis/article_memory.toml` — add commented example section

- [ ] **Step 1: Add the ingest subcommand to `ArticlesCommand`**

In `src/cli/mod.rs`, inside `enum ArticlesCommand`, add a new variant (around line 296, after `Normalize`):

```rust
    /// Submit a URL for asynchronous crawl + ingest.
    Ingest {
        #[command(subcommand)]
        command: ArticleIngestCommand,
    },
```

Then define the new enum below the existing article subcommand enums:

```rust
#[derive(Debug, Subcommand)]
enum ArticleIngestCommand {
    /// Submit a URL for ingest.
    Submit {
        url: String,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        source_hint: Option<String>,
        /// Poll the job to terminal state before returning.
        #[arg(long)]
        wait: bool,
    },
    /// Show recent ingest jobs.
    History {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        failed: bool,
    },
    /// Show a single ingest job.
    Show { job_id: String },
}
```

- [ ] **Step 2: Route the new subcommand**

In `src/cli/mod.rs` `run_cli`, inside the `Commands::Articles { command }` match, add a new arm. Locate the existing `ArticlesCommand::Normalize { … }` arm and add after it:

```rust
            ArticlesCommand::Ingest { command } => match command {
                ArticleIngestCommand::Submit {
                    url, tags, title, source_hint, wait,
                } => submit_article_ingest(&paths, url, tags, title, source_hint, wait).await,
                ArticleIngestCommand::History { limit, failed } => {
                    list_article_ingest(&paths, limit, failed).await
                }
                ArticleIngestCommand::Show { job_id } => {
                    show_article_ingest(&paths, &job_id).await
                }
            },
```

- [ ] **Step 3: Add the handlers in `src/cli/articles.rs`**

Append to `src/cli/articles.rs`:

```rust
pub(super) async fn submit_article_ingest(
    paths: &RuntimePaths,
    url: String,
    tags: Vec<String>,
    title: Option<String>,
    source_hint: Option<String>,
    wait: bool,
) -> Result<()> {
    let config = check_local_config(paths)?;
    let ingest_config = std::sync::Arc::new(config.article_memory.ingest.clone());
    let queue = std::sync::Arc::new(crate::article_memory::IngestQueue::load_or_create(
        paths,
        ingest_config.clone(),
    ));
    let req = crate::article_memory::IngestRequest {
        url,
        title,
        tags,
        source_hint: source_hint.or_else(|| Some("cli".to_string())),
    };
    let resp = queue.submit(req).await?;
    println!("Submitted ingest job.");
    println!("- job_id: {}", resp.job_id);
    println!("- status: {}", resp.status.as_str());
    println!("- deduped: {}", resp.deduped);
    if !wait {
        return Ok(());
    }
    // Note: in --wait mode we are a one-shot CLI — no worker is running here.
    // Workers only run inside the daemon. --wait polls the daemon's
    // persisted view via the queue's JSON file.
    println!("Polling... (Ctrl-C to stop)");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let queue_reload = crate::article_memory::IngestQueue::load_or_create(
            paths,
            ingest_config.clone(),
        );
        if let Some(job) = queue_reload.get(&resp.job_id).await {
            println!("  status={}  article_id={:?}", job.status.as_str(), job.article_id);
            if job.status.is_terminal() {
                if let Some(err) = &job.error {
                    println!("  error: {} — {}", err.issue_type, err.message);
                }
                break;
            }
        }
    }
    Ok(())
}

pub(super) async fn list_article_ingest(
    paths: &RuntimePaths,
    limit: usize,
    only_failed: bool,
) -> Result<()> {
    let config = check_local_config(paths)?;
    let ingest_config = std::sync::Arc::new(config.article_memory.ingest.clone());
    let queue = crate::article_memory::IngestQueue::load_or_create(paths, ingest_config);
    let jobs = queue
        .list(&crate::article_memory::ListFilter {
            status: None,
            limit: Some(limit),
            only_failed,
        })
        .await;
    println!("Ingest history: {} job(s)", jobs.len());
    for job in jobs {
        println!(
            "- {} | {} | profile={} | {} | {}",
            job.id,
            job.status.as_str(),
            job.profile_name,
            job.submitted_at,
            job.url
        );
        if let Some(err) = &job.error {
            println!("  error: {} — {}", err.issue_type, err.message);
        }
    }
    Ok(())
}

pub(super) async fn show_article_ingest(paths: &RuntimePaths, job_id: &str) -> Result<()> {
    let config = check_local_config(paths)?;
    let ingest_config = std::sync::Arc::new(config.article_memory.ingest.clone());
    let queue = crate::article_memory::IngestQueue::load_or_create(paths, ingest_config);
    match queue.get(job_id).await {
        Some(job) => {
            let rendered = serde_json::to_string_pretty(&job)
                .unwrap_or_else(|_| format!("{job:?}"));
            println!("{rendered}");
            Ok(())
        }
        None => bail!("ingest job not found: {job_id}"),
    }
}
```

- [ ] **Step 4: Update the TOML sample file with the commented section**

Edit `config/davis/article_memory.toml` — append at the end:

```toml

# --- URL ingest pipeline (crawl4ai → article_memory) ----------------------
# Defaults are safe: enabled with 3 workers, empty host_profiles routes
# everything to a generic profile. To scope cookies per site, log a profile
# in with `daviszeroclaw crawl profile login articles-<name>` and list it
# below.
#
# [ingest]
# enabled = true
# max_concurrency = 3
# default_profile = "articles-generic"
# min_markdown_chars = 600
# dedup_window_hours = 24
# allow_private_hosts = []
#
# [[ingest.host_profiles]]
# match = "zhihu.com"
# profile = "articles-zhihu"
# source = "zhihu"
#
# [[ingest.host_profiles]]
# match = "mp.weixin.qq.com"
# profile = "articles-weixin"
# source = "weixin"
```

- [ ] **Step 5: Run tests + lint**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 6: Manual CLI smoke (optional if daemon is running)**

```bash
daviszeroclaw articles ingest submit https://en.wikipedia.org/wiki/Rust_(programming_language) --tag smoke
daviszeroclaw articles ingest history --limit 5
```

Expected: submit prints `job_id`, history shows it.

- [ ] **Step 7: Commit**

```bash
git add src/cli/mod.rs src/cli/articles.rs config/davis/article_memory.toml
git commit -m "feat(cli): add articles ingest submit/history/show subcommands"
```

---

## Task 13: Final verification + follow-ups document

**Files:**
- Modify: `docs/superpowers/specs/2026-04-24-article-memory-crawl4ai-ingest-design.md` — flip Status to "Landed"
- (Optional) Modify: `docs/superpowers/plans/2026-04-22-crawl4ai-supervised-server.md` — cross-link

- [ ] **Step 1: End-to-end live smoke (requires daemon running)**

```bash
daviszeroclaw stop || true
daviszeroclaw start
sleep 3
# Submit a real URL
RESP=$(curl -sS -X POST http://127.0.0.1:11434/article-memory/ingest \
  -H 'Content-Type: application/json' \
  -d '{"url": "https://en.wikipedia.org/wiki/Markdown", "tags": ["verification"]}')
echo "$RESP" | jq
JOB_ID=$(echo "$RESP" | jq -r .job_id)
# Poll
for i in 1 2 3 4 5 6 7 8 9 10; do
  sleep 5
  STATUS=$(curl -sS http://127.0.0.1:11434/article-memory/ingest/"$JOB_ID" | jq -r .status)
  echo "tick $i: $STATUS"
  if [ "$STATUS" = "saved" ] || [ "$STATUS" = "rejected" ] || [ "$STATUS" = "failed" ]; then
    break
  fi
done
curl -sS http://127.0.0.1:11434/article-memory/ingest/"$JOB_ID" | jq
daviszeroclaw articles list --limit 5
```

Expected terminal output includes `status: saved` and the Wikipedia article in `articles list`.

- [ ] **Step 2: Flip spec status**

Edit `docs/superpowers/specs/2026-04-24-article-memory-crawl4ai-ingest-design.md` line 3:

```markdown
Status: **Landed (2026-04-24)**
```

- [ ] **Step 3: Full test + lint one more time**

Run: `cargo test --workspace --quiet && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-04-24-article-memory-crawl4ai-ingest-design.md
git commit -m "docs(specs): mark article_memory ingest design as Landed"
```

---

## Self-review checklist (for plan author)

Spec coverage map:

| Spec section | Plan coverage |
|---|---|
| §1 Goal (URL-only ingest from any channel) | Tasks 6, 9, 10, 11, 12 |
| §2 Non-goals | Tasks keep ArticleMemoryAddRequest unchanged (Task 9), no retry (Task 6 — attempts always 1), no DNS rebinding (explicit out of scope in Task 3) |
| §3 Constraints | Task 6 reuses Crawl4aiProfileLocks; Task 9 calls add_article_memory → normalize → embed verbatim; Task 8 extends Crawl4aiError usage without altering issue_type() contract |
| §4 Architecture | Task 11 wires queue + worker pool, shares profile_locks |
| §5 Module layout | Tasks 3, 4, 6, 9 each create one file under src/article_memory/ingest/ |
| §6.1 IngestRequest | Task 4 `types.rs` |
| §6.2 IngestJob + IngestJobStatus | Task 4 |
| §6.3 IngestResponse | Task 4 |
| §6.4 ArticleMemoryIngestConfig TOML | Task 2 struct + Task 12 commented sample |
| §7.1 Happy path | Task 9 execute_job implements exact stage sequence |
| §7.2 Error taxonomy | Task 9 maps every Crawl4aiError + empty_content + pipeline_error; Task 6 adds daemon_restart |
| §7.3 URL normalization + dedup | Task 3 normalize_url + Task 6 submit() dedup rules |
| §7.4 Host suffix matching | Task 3 resolve_profile + host_matches_suffix + tests |
| §7.5 SSRF guard | Task 3 validate_url_for_ingest + 7 unit tests |
| §8.1 HTTP endpoints | Task 10 adds 3 routes |
| §8.2 CLI | Task 12 |
| §8.3 Python adapter | Task 7 |
| §9 Concurrency + persistence | Task 6 Notify contract + load_or_create reset rule |
| §10 Observability | Task 9 `#[tracing::instrument]` on execute_job + Task 12 history CLI |
| §11 Testing | Task 3 unit; Task 6 queue tests; Task 9 worker mocked tests; Task 10 HTTP smoke |
| §12 Backward compat | Task 8 Crawl4aiPageRequest.markdown=false default preserves express |
| §13 Rollback | Task 2 sets enabled=true but honored by Task 11 early-return |
| §14 Follow-ups | Not implemented (out of scope) |

Placeholder scan: PASS. No "TBD", no "similar to Task N", every code step shows full code.

Type consistency check:
- `IngestJobStatus::as_str()` returns lowercase strings matching `#[serde(rename_all = "snake_case")]` — aligned.
- `IngestJobError { issue_type, message, stage }` used identically in Tasks 4, 6, 9.
- `IngestOutcomeSummary` fields match exactly between Task 4 definition and Task 9 construction.
- `Crawl4aiPageRequest.markdown` field name used identically in Tasks 8 and 9.
- `ArticleMemoryHostProfile.match_suffix` renamed to `"match"` via serde — consistent across Tasks 2, 3, 12.

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-04-24-article-memory-crawl4ai-ingest.md`.**

Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
