# Article Memory Phase 2 — Skill + Dedup + iMessage Notify Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the Phase 1 async ingest pipeline into ZeroClaw's LLM via skill tools, add article-level URL dedup with `force=true` update-in-place, and proactively notify iMessage users when their URL ingest finishes.

**Architecture:** Three orthogonal units on top of Phase 1: T1 adds `ArticleExists` dedup + `force` flag + startup migration/merge in `src/article_memory/`; T2 adds `src/imessage_send.rs` + reply-text builder hooked into worker terminal state; T3 rewrites `project-skills/article-memory/` so ZeroClaw LLM speaks the real crawl4ai-backed contract; T4 verifies config propagation and updates the Phase 1 spec narrative.

**Tech Stack:** Rust (axum 0.7, tokio, serde, uuid, tempfile, tracing), osascript (macOS iMessage), TOML (skill definitions), markdown (skill references).

**Spec:** `docs/superpowers/specs/2026-04-24-article-memory-phase2-skill-and-dedup-design.md`

**Branch:** `docs/crawl4ai-plan-slim` (local-only, no push/PR per standing user directive)

**Baseline:** head `365e354` (after spec commit), 154/154 tests pass, clippy clean, fmt clean.

---

## Task Ordering Rationale

| # | Task | Why this order |
|---|---|---|
| T1a | `find_article_by_normalized_url` + unit tests | Pure function, zero deps, foundation for later tasks |
| T1b | `IngestRequest.force` + `IngestSubmitError::ArticleExists` + serde defaults | Types before consumers |
| T1c | `submit()` Rule 0 gate | Consumes T1a + T1b |
| T1d | `server.rs` 409 mapping for `ArticleExists` | Consumes T1b |
| T1e | `add_article_memory_override` + unit tests | Foundation for T1f |
| T1f | Worker `force` branch — reuse existing article_id | Consumes T1a + T1e |
| T1g | Startup URL normalization migration | Standalone, foundational for T1h |
| T1h | Startup duplicate merge (Q11 D+A) | Consumes T1g |
| T2a | `src/imessage_send.rs` target validator + escape | Pure functions, no deps |
| T2b | `IngestRequest.reply_handle` + `IngestJob.reply_handle` | Types before consumers |
| T2c | `build_reply_text` + `humanize_issue_type` | Pure functions |
| T2d | Worker terminal-state notify hook + `IngestWorkerDeps.imessage_config` | Consumes T2a/c + T2b |
| T2e | `local_proxy.rs` wiring — pass `imessage_config` to worker pool | Consumes T2d |
| T3a | `project-skills/article-memory/SKILL.toml` rewrite | Doc-only, after all Rust lands |
| T3b | `references/article_memory_api.md` rewrite | Doc-only, pairs with T3a |
| T4a | Verify `render_runtime_config_str` propagation + Phase 1 spec revisions | Verification + doc edits |

---

## Task 1a: `find_article_by_normalized_url` + unit tests

**Files:**
- Modify: `src/article_memory/mod.rs`
- Test (co-located): append `#[cfg(test)] mod tests { ... }` to `src/article_memory/mod.rs` if not already present, else add module

**Context:** The `ArticleMemoryRecord.url: Option<String>` at `src/article_memory/types.rs:29` already exists. We need a linear-scan lookup by URL for Phase 2's article-level dedup. Phase 1's `normalize_url` at `src/article_memory/ingest/host_profile.rs` is the source of truth for URL canonicalization. This task adds a pure read helper.

- [ ] **Step 1: Write the failing tests**

Append to `src/article_memory/mod.rs`:

```rust
#[cfg(test)]
mod find_by_url_tests {
    use super::*;
    use crate::article_memory::types::{ArticleMemoryIndex, ArticleMemoryRecord, ArticleMemoryRecordStatus};
    use tempfile::TempDir;

    fn mk_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn mk_record(id: &str, url: Option<&str>) -> ArticleMemoryRecord {
        ArticleMemoryRecord {
            id: id.into(),
            title: format!("T {id}"),
            url: url.map(String::from),
            source: "test".into(),
            language: None,
            tags: vec![],
            status: ArticleMemoryRecordStatus::Saved,
            value_score: Some(0.9),
            captured_at: "2026-04-24T00:00:00Z".into(),
            updated_at: "2026-04-24T00:00:00Z".into(),
            content_path: format!("articles/{id}.md"),
            raw_path: None,
            normalized_path: None,
            summary_path: None,
            translation_path: None,
            notes: None,
            clean_status: None,
            clean_profile: None,
        }
    }

    fn seed(paths: &RuntimePaths, records: Vec<ArticleMemoryRecord>) {
        init_article_memory(paths).unwrap();
        let mut index = crate::article_memory::internals::load_index(paths).unwrap();
        index.articles = records;
        crate::article_memory::internals::write_index(paths, &index).unwrap();
    }

    #[test]
    fn returns_none_when_index_empty() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        init_article_memory(&paths).unwrap();
        let hit = find_article_by_normalized_url(&paths, "https://example.com/").unwrap();
        assert!(hit.is_none());
    }

    #[test]
    fn returns_record_when_normalized_url_matches() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![mk_record("aaa", Some("https://example.com/"))]);
        let hit = find_article_by_normalized_url(&paths, "https://example.com/")
            .unwrap()
            .expect("record should be found");
        assert_eq!(hit.id, "aaa");
    }

    #[test]
    fn returns_none_when_only_non_matching_urls() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![mk_record("aaa", Some("https://other.com/"))]);
        let hit = find_article_by_normalized_url(&paths, "https://example.com/").unwrap();
        assert!(hit.is_none());
    }

    #[test]
    fn skips_records_with_missing_url() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![
            mk_record("aaa", None),
            mk_record("bbb", Some("https://example.com/")),
        ]);
        let hit = find_article_by_normalized_url(&paths, "https://example.com/")
            .unwrap()
            .expect("bbb should be found");
        assert_eq!(hit.id, "bbb");
    }

    #[test]
    fn returns_first_hit_when_multiple_match() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![
            mk_record("aaa", Some("https://example.com/")),
            mk_record("bbb", Some("https://example.com/")),
        ]);
        let hit = find_article_by_normalized_url(&paths, "https://example.com/")
            .unwrap()
            .expect("should find first");
        assert_eq!(hit.id, "aaa");
    }
}
```

Also ensure `internals` is reachable from this test scope. The module is declared `mod internals;` in `mod.rs`; tests inside `mod.rs` can reach it as `crate::article_memory::internals::...` or `super::internals::...` depending on positioning. Use `super::internals::load_index` / `super::internals::write_index` and make those functions accessible from the test module by adding `#[cfg(test)] pub` visibility or importing via `crate::article_memory::internals` with a `pub(crate)` on the functions. Since the existing file declares `pub(super) fn load_index` and `pub(super) fn write_index` (at `src/article_memory/internals.rs:53, 98`), the tests inside `mod.rs` can call `internals::load_index` and `internals::write_index` directly.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib article_memory::find_by_url_tests 2>&1 | tail -20`
Expected: compile error `cannot find function 'find_article_by_normalized_url'`.

- [ ] **Step 3: Write minimal implementation**

Add to `src/article_memory/mod.rs`, near other public functions (after `add_article_memory`):

```rust
/// Linear-scan lookup by canonical URL. Returns the first record whose
/// `url` field equals `normalized_url`. Callers must normalize the URL
/// themselves via `crate::article_memory::ingest::host_profile::normalize_url`
/// before calling — this function does no normalization.
pub fn find_article_by_normalized_url(
    paths: &RuntimePaths,
    normalized_url: &str,
) -> Result<Option<ArticleMemoryRecord>> {
    let index = internals::load_index(paths)?;
    Ok(index
        .articles
        .into_iter()
        .find(|r| r.url.as_deref() == Some(normalized_url)))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib article_memory::find_by_url_tests 2>&1 | tail -10`
Expected: `test result: ok. 5 passed`.

- [ ] **Step 5: Run clippy + fmt gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/mod.rs
git commit -m "feat(article-memory): add find_article_by_normalized_url for URL dedup"
```

---

## Task 1b: `IngestRequest.force` + `IngestSubmitError::ArticleExists` + default tests

**Files:**
- Modify: `src/article_memory/ingest/types.rs:100-108` (`IngestRequest`), `:120-134` (`IngestSubmitError`), `:136-163` (`Display` impl)

**Context:** Add the new request field and error variant. Serde defaults ensure Phase 1 clients (CLI, existing tests) keep working without emitting the new fields.

- [ ] **Step 1: Write failing tests**

Append to `src/article_memory/ingest/types.rs` inside the existing `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn ingest_request_defaults_force_to_false() {
        let req: IngestRequest =
            serde_json::from_str(r#"{"url": "https://example.com/"}"#).unwrap();
        assert!(!req.force);
    }

    #[test]
    fn ingest_request_accepts_force_true() {
        let req: IngestRequest =
            serde_json::from_str(r#"{"url": "https://example.com/", "force": true}"#).unwrap();
        assert!(req.force);
    }

    #[test]
    fn article_exists_error_displays_with_article_id_and_url() {
        let e = IngestSubmitError::ArticleExists {
            existing_article_id: "aaa".into(),
            title: "T".into(),
            url: "https://example.com/".into(),
        };
        let s = format!("{e}");
        assert!(s.contains("aaa"));
        assert!(s.contains("https://example.com/"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib article_memory::ingest::types::tests::ingest_request_defaults_force 2>&1 | tail -10`
Expected: compile error — `force` does not exist on `IngestRequest`.

- [ ] **Step 3: Add `force` to `IngestRequest`**

Edit `src/article_memory/ingest/types.rs:100-108` — change the struct from:

```rust
pub struct IngestRequest {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<String>,
}
```

to:

```rust
pub struct IngestRequest {
    pub url: String,
    #[serde(default)]
    pub force: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<String>,
}
```

- [ ] **Step 4: Add `ArticleExists` variant to `IngestSubmitError`**

Edit `src/article_memory/ingest/types.rs:120-134` — add variant `ArticleExists { existing_article_id: String, title: String, url: String }` after `DuplicateSaved`. Resulting enum:

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum IngestSubmitError {
    InvalidUrl(String),
    InvalidScheme,
    PrivateAddressBlocked(String),
    DuplicateSaved {
        existing_article_id: Option<String>,
        finished_at: String,
    },
    ArticleExists {
        existing_article_id: String,
        title: String,
        url: String,
    },
    IngestDisabled,
    PersistenceError(String),
    PersistenceDegraded {
        consecutive_failures: usize,
        last_error: String,
    },
}
```

- [ ] **Step 5: Extend `Display` impl**

Edit `src/article_memory/ingest/types.rs:136-161` — add a match arm for `ArticleExists` inside the existing `impl Display for IngestSubmitError`:

```rust
            Self::ArticleExists {
                existing_article_id,
                url,
                ..
            } => write!(
                f,
                "article already saved for {url} (article_id={existing_article_id})"
            ),
```

Place it between the existing `DuplicateSaved` arm and the `IngestDisabled` arm.

- [ ] **Step 6: Run all types tests — they should pass**

Run: `cargo test --lib article_memory::ingest::types 2>&1 | tail -10`
Expected: `test result: ok. N passed`, with N including the 3 new cases.

- [ ] **Step 7: Run full test suite to catch exhaustiveness warnings**

Run: `cargo build --lib 2>&1 | grep -E 'warning|error' | head -20 || echo 'clean'`
Expected: 1 or more `non-exhaustive patterns: ArticleExists not covered` errors from `src/server.rs:922` (the submit handler's `match err`). That is handled in Task 1d. Clippy will also flag it; ignore for this task.

- [ ] **Step 8: Commit (allow the broken match temporarily — Task 1d closes it)**

For this commit only, add a `_ => unreachable!("handled in task 1d")` arm at the tail of `src/server.rs:922-969` match so the build compiles:

Edit `src/server.rs:969` — change the closing of the inner match block. The current layout ends with `},\n    }\n}`. Insert before the final closing:

```rust
            IngestSubmitError::ArticleExists { .. } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "not_yet_mapped", "detail": "pending task 1d"})),
            ),
```

This keeps HEAD green at every commit. Task 1d replaces this placeholder with the real 409.

Run: `cargo build --lib 2>&1 | tail -5`
Expected: `Finished dev`.

Commit:

```bash
git add src/article_memory/ingest/types.rs src/server.rs
git commit -m "feat(article-memory): add IngestRequest.force and IngestSubmitError::ArticleExists"
```

---

## Task 1c: `submit()` Rule 0 article-level dedup gate

**Files:**
- Modify: `src/article_memory/ingest/queue.rs:162-301` — `submit()` method

**Context:** With `find_article_by_normalized_url` and `ArticleExists` in place, add the gate in `submit()`. Rule order: validate → normalize → **(new) Rule 0 if !force: article-level dedup** → Rule 1 active job → Rule 2 saved window. `force=true` skips only Rule 0, not Rules 1–2.

- [ ] **Step 1: Write failing tests**

Append to `src/article_memory/ingest/queue.rs` inside the existing `#[cfg(test)] mod tests` block (near the other `submit_*` tests):

```rust
    #[tokio::test]
    async fn submit_without_force_rejects_when_article_exists_in_store() {
        let tmp = TempDir::new().unwrap();
        let paths = RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        };
        crate::init_article_memory(&paths).unwrap();
        // Seed article_memory index with a record at the target URL.
        let mut index = crate::article_memory::internals::load_index(&paths).unwrap();
        index.articles.push(crate::article_memory::types::ArticleMemoryRecord {
            id: "existing".into(),
            title: "Existing Article".into(),
            url: Some("https://example.com/p/1".into()),
            source: "test".into(),
            language: None,
            tags: vec![],
            status: crate::article_memory::types::ArticleMemoryRecordStatus::Saved,
            value_score: Some(0.9),
            captured_at: "2026-04-20T00:00:00Z".into(),
            updated_at: "2026-04-20T00:00:00Z".into(),
            content_path: "articles/existing.md".into(),
            raw_path: None,
            normalized_path: None,
            summary_path: None,
            translation_path: None,
            notes: None,
            clean_status: None,
            clean_profile: None,
        });
        crate::article_memory::internals::write_index(&paths, &index).unwrap();

        let queue = IngestQueue::load_or_create(&paths, default_config());

        let err = queue
            .submit(IngestRequest {
                url: "https://example.com/p/1".into(),
                force: false,
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .expect_err("should reject with ArticleExists");

        match err {
            IngestSubmitError::ArticleExists { existing_article_id, title, url } => {
                assert_eq!(existing_article_id, "existing");
                assert_eq!(title, "Existing Article");
                assert_eq!(url, "https://example.com/p/1");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_with_force_bypasses_article_dedup() {
        let tmp = TempDir::new().unwrap();
        let paths = RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        };
        crate::init_article_memory(&paths).unwrap();
        let mut index = crate::article_memory::internals::load_index(&paths).unwrap();
        index.articles.push(crate::article_memory::types::ArticleMemoryRecord {
            id: "existing".into(),
            title: "Existing".into(),
            url: Some("https://example.com/p/1".into()),
            source: "test".into(),
            language: None,
            tags: vec![],
            status: crate::article_memory::types::ArticleMemoryRecordStatus::Saved,
            value_score: Some(0.9),
            captured_at: "2026-04-20T00:00:00Z".into(),
            updated_at: "2026-04-20T00:00:00Z".into(),
            content_path: "articles/existing.md".into(),
            raw_path: None,
            normalized_path: None,
            summary_path: None,
            translation_path: None,
            notes: None,
            clean_status: None,
            clean_profile: None,
        });
        crate::article_memory::internals::write_index(&paths, &index).unwrap();

        let queue = IngestQueue::load_or_create(&paths, default_config());

        let resp = queue
            .submit(IngestRequest {
                url: "https://example.com/p/1".into(),
                force: true,
                title: None,
                tags: vec![],
                source_hint: None,
            })
            .await
            .expect("force=true should bypass article dedup");

        assert_eq!(resp.status, IngestJobStatus::Pending);
        assert!(!resp.deduped);
    }
```

A `default_config()` helper already exists in the test module (used by Phase 1 tests). If not, define it near the top of the test module as:

```rust
fn default_config() -> Arc<ArticleMemoryIngestConfig> {
    Arc::new(ArticleMemoryIngestConfig {
        enabled: true,
        max_concurrency: 3,
        default_profile: "articles-generic".into(),
        min_markdown_chars: 600,
        dedup_window_hours: 24,
        allow_private_hosts: vec![],
        host_profiles: vec![],
    })
}
```

(Check existing test module first; reuse if present.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib article_memory::ingest::queue::tests::submit_without_force_rejects 2>&1 | tail -10`
Expected: FAIL — current `submit()` doesn't check article store, so the first test either returns `Pending` or `DuplicateSaved`.

- [ ] **Step 3: Add Rule 0 to `submit()`**

Edit `src/article_memory/ingest/queue.rs` inside `submit()`, after the `normalize_url` call at line 194-195 and BEFORE `let mut state = self.inner.lock().await;` at line 197:

```rust
        // Dedup rule 0 (article-level): if !force and the URL already has a
        // record in ArticleMemoryIndex, reject with ArticleExists. Respects
        // user intent to refresh via force=true.
        if !req.force {
            if let Ok(Some(existing)) = crate::find_article_by_normalized_url(
                &self.paths,
                &normalized,
            ) {
                return Err(IngestSubmitError::ArticleExists {
                    existing_article_id: existing.id,
                    title: existing.title,
                    url: existing.url.unwrap_or(normalized),
                });
            }
        }
```

This requires `IngestQueue` to carry a `paths: RuntimePaths` field. Check the existing struct at `src/article_memory/ingest/queue.rs:41-52`. If `paths` is not there (it likely isn't — Phase 1 stored only `persistence_path`), add it:

```rust
pub struct IngestQueue {
    pub(super) inner: Mutex<IngestQueueState>,
    persistence_path: PathBuf,
    paths: RuntimePaths,              // NEW — for find_article_by_normalized_url
    notify: Arc<Notify>,
    config: Arc<ArticleMemoryIngestConfig>,
    pub(super) persist_failures: std::sync::atomic::AtomicUsize,
    pub(super) last_persist_error: std::sync::Mutex<Option<String>>,
}
```

And update `IngestQueue::load_or_create(paths: &RuntimePaths, ...)` constructor to clone `paths` into the struct. Locate the constructor (grep `fn load_or_create` in `queue.rs`) and add `paths: paths.clone(),` into the struct init.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib article_memory::ingest::queue::tests::submit_without_force_rejects article_memory::ingest::queue::tests::submit_with_force_bypasses 2>&1 | tail -10`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Run full ingest queue test suite to verify no regressions**

Run: `cargo test --lib article_memory::ingest::queue 2>&1 | tail -10`
Expected: all pre-existing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/ingest/queue.rs
git commit -m "feat(article-memory): add Rule 0 article-level dedup in IngestQueue::submit"
```

---

## Task 1d: Map `ArticleExists` to HTTP 409 with action payload

**Files:**
- Modify: `src/server.rs:913-971` — `ingest_submit_handler`

**Context:** Replace the Task 1b placeholder arm with the real 409 response.

- [ ] **Step 1: Write failing test**

Append to `tests/rust/ingest_http.rs` the following test. It requires seeding the article_memory index with an existing record; reuse helpers from Task 1c's test pattern.

```rust
#[tokio::test]
async fn post_ingest_returns_409_article_exists_when_url_already_saved() {
    let (state, _tmp) = build_state_for_test().await;
    // Seed an existing article at the target URL.
    let mut index = crate::article_memory::internals::load_index(&state.paths()).unwrap();
    index.articles.push(crate::article_memory::types::ArticleMemoryRecord {
        id: "existing".into(),
        title: "Already Saved".into(),
        url: Some("https://example.com/p/1".into()),
        source: "test".into(),
        language: None,
        tags: vec![],
        status: crate::article_memory::types::ArticleMemoryRecordStatus::Saved,
        value_score: Some(0.9),
        captured_at: "2026-04-20T00:00:00Z".into(),
        updated_at: "2026-04-20T00:00:00Z".into(),
        content_path: "articles/existing.md".into(),
        raw_path: None,
        normalized_path: None,
        summary_path: None,
        translation_path: None,
        notes: None,
        clean_status: None,
        clean_profile: None,
    });
    crate::article_memory::internals::write_index(&state.paths(), &index).unwrap();

    let app = build_app(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/article-memory/ingest")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "url": "https://example.com/p/1"
                    })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body_bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(body["error"], "article_exists");
    assert_eq!(body["existing_article_id"], "existing");
    assert_eq!(body["title"], "Already Saved");
    assert_eq!(body["url"], "https://example.com/p/1");
    assert!(body["action"].as_str().unwrap().contains("force"));
}
```

Note: `state.paths()` — `AppState` must expose `paths` cheaply. Check `src/server.rs` for `AppState` fields; if `paths` is a named field, use `state.paths.clone()` or similar. Adjust the test to the actual accessor.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib post_ingest_returns_409_article_exists 2>&1 | tail -15`
Expected: FAIL — either "not_yet_mapped" in response body (from Task 1b placeholder) or a 500.

- [ ] **Step 3: Replace placeholder with real 409 mapping**

Edit `src/server.rs:969` (the placeholder added in Task 1b step 8). Replace:

```rust
            IngestSubmitError::ArticleExists { .. } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "not_yet_mapped", "detail": "pending task 1d"})),
            ),
```

with:

```rust
            IngestSubmitError::ArticleExists {
                existing_article_id,
                title,
                url,
            } => (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "article_exists",
                    "existing_article_id": existing_article_id,
                    "title": title,
                    "url": url,
                    "action": "resubmit with \"force\": true to re-crawl and update"
                })),
            ),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib post_ingest_returns_409_article_exists 2>&1 | tail -10`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Run full test suite to catch regressions**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/server.rs tests/rust/ingest_http.rs
git commit -m "feat(server): map IngestSubmitError::ArticleExists to HTTP 409 with action hint"
```

---

## Task 1e: `add_article_memory_override` — update-in-place by existing `article_id`

**Files:**
- Modify: `src/article_memory/mod.rs` (new pub fn)

**Context:** When `force=true` and the URL already has a record, the worker should reuse its `article_id` and overwrite title/captured_at/content/summary. Phase 1's `add_article_memory` is at `src/article_memory/mod.rs:105-176`. This task adds a sibling that takes an `override_id` argument and replaces in-place instead of appending.

- [ ] **Step 1: Write failing tests**

Append to `src/article_memory/mod.rs` inside a new test module (or extend the test module added in Task 1a):

```rust
#[cfg(test)]
mod override_tests {
    use super::*;
    use crate::article_memory::types::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
    use tempfile::TempDir;

    fn mk_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn seed_one(paths: &RuntimePaths, id: &str, url: &str) {
        init_article_memory(paths).unwrap();
        let mut index = internals::load_index(paths).unwrap();
        index.articles.push(ArticleMemoryRecord {
            id: id.into(),
            title: "OLD TITLE".into(),
            url: Some(url.into()),
            source: "test".into(),
            language: None,
            tags: vec![],
            status: ArticleMemoryRecordStatus::Saved,
            value_score: Some(0.5),
            captured_at: "2026-04-01T00:00:00Z".into(),
            updated_at: "2026-04-01T00:00:00Z".into(),
            content_path: format!("articles/{id}.md"),
            raw_path: Some(format!("articles/{id}.raw.txt")),
            normalized_path: Some(format!("articles/{id}.normalized.md")),
            summary_path: None,
            translation_path: None,
            notes: None,
            clean_status: Some("raw".into()),
            clean_profile: None,
        });
        internals::write_index(paths, &index).unwrap();
        // Write the old content files so we can observe overwrites.
        let articles_dir = paths.runtime_dir.join("article-memory").join("articles");
        std::fs::create_dir_all(&articles_dir).unwrap();
        std::fs::write(articles_dir.join(format!("{id}.md")), "OLD CONTENT").unwrap();
        std::fs::write(articles_dir.join(format!("{id}.raw.txt")), "OLD CONTENT").unwrap();
        std::fs::write(articles_dir.join(format!("{id}.normalized.md")), "OLD CONTENT").unwrap();
    }

    #[test]
    fn override_reuses_id_and_does_not_append() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed_one(&paths, "aaa", "https://example.com/");

        let _updated = add_article_memory_override(
            &paths,
            ArticleMemoryAddRequest {
                title: "NEW TITLE".into(),
                url: Some("https://example.com/".into()),
                source: "web".into(),
                language: None,
                tags: vec![],
                content: "NEW CONTENT".into(),
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Saved,
                value_score: Some(0.95),
                notes: None,
            },
            "aaa",
        )
        .unwrap();

        let index = internals::load_index(&paths).unwrap();
        assert_eq!(index.articles.len(), 1, "no append");
        assert_eq!(index.articles[0].id, "aaa");
        assert_eq!(index.articles[0].title, "NEW TITLE");
        assert_eq!(index.articles[0].value_score, Some(0.95));
    }

    #[test]
    fn override_overwrites_on_disk_content_files() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed_one(&paths, "aaa", "https://example.com/");

        add_article_memory_override(
            &paths,
            ArticleMemoryAddRequest {
                title: "T".into(),
                url: Some("https://example.com/".into()),
                source: "web".into(),
                language: None,
                tags: vec![],
                content: "NEW CONTENT".into(),
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Saved,
                value_score: None,
                notes: None,
            },
            "aaa",
        )
        .unwrap();

        let articles_dir = paths.runtime_dir.join("article-memory").join("articles");
        let md = std::fs::read_to_string(articles_dir.join("aaa.md")).unwrap();
        assert_eq!(md, "NEW CONTENT");
    }

    #[test]
    fn override_errors_when_id_not_in_index() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        init_article_memory(&paths).unwrap();

        let err = add_article_memory_override(
            &paths,
            ArticleMemoryAddRequest {
                title: "T".into(),
                url: Some("https://example.com/".into()),
                source: "web".into(),
                language: None,
                tags: vec![],
                content: "X".into(),
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Saved,
                value_score: None,
                notes: None,
            },
            "missing",
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib article_memory::override_tests 2>&1 | tail -10`
Expected: compile error `cannot find function add_article_memory_override`.

- [ ] **Step 3: Implement `add_article_memory_override`**

Add to `src/article_memory/mod.rs` immediately after `add_article_memory` (after line 176):

```rust
/// Update an existing article record in place, reusing `override_id`.
/// Writes new content/summary/translation files under the same id, then
/// atomically rewrites the index. Fails if `override_id` is not present
/// in the index.
pub fn add_article_memory_override(
    paths: &RuntimePaths,
    request: ArticleMemoryAddRequest,
    override_id: &str,
) -> Result<ArticleMemoryRecord> {
    ensure_article_memory_dirs(paths)?;

    let title = clean_required("title", &request.title)?;
    let content = clean_required("content", &request.content)?;
    let source = clean_optional(&request.source).unwrap_or_else(|| "manual".to_string());
    let now = isoformat(now_utc());

    let content_path = format!("articles/{override_id}.md");
    let raw_path = format!("articles/{override_id}.raw.txt");
    let normalized_path = format!("articles/{override_id}.normalized.md");
    let summary_path = request
        .summary
        .as_deref()
        .and_then(clean_optional)
        .map(|_| format!("articles/{override_id}.summary.md"));
    let translation_path = request
        .translation
        .as_deref()
        .and_then(clean_optional)
        .map(|_| format!("articles/{override_id}.translation.md"));

    fs::write(resolve_article_path(paths, &raw_path), &content)
        .with_context(|| format!("failed to write article raw content for {override_id}"))?;
    fs::write(resolve_article_path(paths, &normalized_path), &content)
        .with_context(|| format!("failed to write article normalized content for {override_id}"))?;
    fs::write(resolve_article_path(paths, &content_path), &content)
        .with_context(|| format!("failed to write article content for {override_id}"))?;
    if let (Some(summary), Some(path)) = (request.summary.as_deref(), summary_path.as_deref()) {
        fs::write(resolve_article_path(paths, path), summary.trim())
            .with_context(|| format!("failed to write article summary for {override_id}"))?;
    }
    if let (Some(translation), Some(path)) =
        (request.translation.as_deref(), translation_path.as_deref())
    {
        fs::write(resolve_article_path(paths, path), translation.trim())
            .with_context(|| format!("failed to write article translation for {override_id}"))?;
    }

    let mut index = internals::load_index(paths)?;
    let idx = index
        .articles
        .iter()
        .position(|r| r.id == override_id)
        .ok_or_else(|| anyhow::anyhow!("article_id {override_id} not in index"))?;

    let replacement = ArticleMemoryRecord {
        id: override_id.to_string(),
        title,
        url: request.url.and_then(|value| clean_optional(&value)),
        source,
        language: request.language.and_then(|value| clean_optional(&value)),
        tags: normalize_tags(request.tags),
        status: request.status,
        value_score: normalize_score(request.value_score)?,
        captured_at: now.clone(),
        updated_at: now,
        content_path,
        raw_path: Some(raw_path),
        normalized_path: Some(normalized_path),
        summary_path,
        translation_path,
        notes: request.notes.and_then(|value| clean_optional(&value)),
        clean_status: Some("raw".to_string()),
        clean_profile: None,
    };
    index.articles[idx] = replacement.clone();
    index.updated_at = isoformat(now_utc());
    internals::write_index(paths, &index)?;
    Ok(replacement)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib article_memory::override_tests 2>&1 | tail -10`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Run clippy + fmt gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/mod.rs
git commit -m "feat(article-memory): add add_article_memory_override for force update-in-place"
```

---

## Task 1f: Worker `force` branch — reuse existing `article_id`

**Files:**
- Modify: `src/article_memory/ingest/worker.rs:159-190` — Stage 2 cleaning block (around `add_article_memory` call)
- Modify: `src/article_memory/ingest/queue.rs` — add helper to read `force` off the persisted job (since `IngestJob` does NOT currently carry `force`; the flag is request-only)

**Context:** The worker pulls `IngestJob` from the queue. Currently `IngestJob` does not persist `force` from the request. We need to plumb it so the worker can branch on it. Add `force: bool` to `IngestJob` (serde default false for Phase 1 on-disk state). In Stage 2 (after crawl + markdown extraction), if `job.force`, look up existing record by normalized URL; if hit, call `add_article_memory_override`; else fall through to `add_article_memory`.

- [ ] **Step 1: Persist `force` on `IngestJob`**

Edit `src/article_memory/ingest/types.rs:64-93` — add a `force` field to `IngestJob`:

```rust
    #[serde(default)]
    pub force: bool,
```

Place it after `tags: Vec<String>,` (around line 71).

- [ ] **Step 2: Plumb `force` through `submit()`**

Edit `src/article_memory/ingest/queue.rs` inside `submit()`, where the `IngestJob { ... }` struct literal is built (around line 269-287). Add:

```rust
            force: req.force,
```

Place it after `tags: req.tags.clone(),`.

- [ ] **Step 3: Run tests to confirm types still compile**

Run: `cargo build --lib 2>&1 | tail -5`
Expected: `Finished dev`.

- [ ] **Step 4: Write failing worker test**

Add to `tests/rust/article_memory_ingest_worker.rs` a new test that drives the worker with a mocked crawl4ai supervisor and a pre-seeded article record, then asserts the post-worker index still has exactly one article with the original id but with new content.

The existing test file has `Crawl4aiSupervisor::for_test(...)` helpers. Pattern after the existing `ingest_saves_article_end_to_end`-style test:

```rust
#[tokio::test]
async fn worker_force_path_reuses_existing_article_id() {
    let tmp = TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join("runtime"),
    };
    crate::init_article_memory(&paths).unwrap();

    // Seed an existing record with id "original" at the target URL.
    let mut index = crate::article_memory::internals::load_index(&paths).unwrap();
    index.articles.push(crate::article_memory::types::ArticleMemoryRecord {
        id: "original".into(),
        title: "OLD".into(),
        url: Some("https://example.com/p/1".into()),
        source: "test".into(),
        language: None,
        tags: vec![],
        status: crate::article_memory::types::ArticleMemoryRecordStatus::Saved,
        value_score: Some(0.5),
        captured_at: "2026-04-01T00:00:00Z".into(),
        updated_at: "2026-04-01T00:00:00Z".into(),
        content_path: "articles/original.md".into(),
        raw_path: None,
        normalized_path: None,
        summary_path: None,
        translation_path: None,
        notes: None,
        clean_status: None,
        clean_profile: None,
    });
    crate::article_memory::internals::write_index(&paths, &index).unwrap();

    let queue = build_queue(&paths);
    let supervisor = Arc::new(Crawl4aiSupervisor::for_test(rich_markdown_response(
        "NEW CONTENT WITH ENOUGH TOPICS: agent memory MCP Claude Code",
    )));
    let deps = build_deps(&paths, supervisor);
    IngestWorkerPool::spawn(queue.clone(), deps, 1);

    let resp = queue
        .submit(IngestRequest {
            url: "https://example.com/p/1".into(),
            force: true,
            title: None,
            tags: vec![],
            source_hint: None,
        })
        .await
        .unwrap();

    wait_for_terminal(&queue, &resp.job_id, Duration::from_secs(10)).await;

    let index = crate::article_memory::internals::load_index(&paths).unwrap();
    assert_eq!(index.articles.len(), 1, "no duplicate");
    assert_eq!(index.articles[0].id, "original", "id stable");
    let content = std::fs::read_to_string(
        paths.runtime_dir.join("article-memory/articles/original.md"),
    ).unwrap();
    assert!(content.contains("NEW CONTENT"));
}
```

`build_queue`, `build_deps`, `rich_markdown_response`, `wait_for_terminal` should follow patterns already in `tests/rust/article_memory_ingest_worker.rs`. Read the existing file first and mirror the helpers.

- [ ] **Step 5: Run test to verify it fails**

Run: `cargo test --lib worker_force_path_reuses_existing_article_id 2>&1 | tail -15`
Expected: FAIL — either `index.articles.len()` is 2 (worker appended instead of overriding) or id differs.

- [ ] **Step 6: Implement worker force branch**

Edit `src/article_memory/ingest/worker.rs:159-190`. Replace the `let record = match add_article_memory(...)` block with a branch:

```rust
    let add_req = ArticleMemoryAddRequest {
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
    };

    let existing_for_force = if job.force {
        crate::find_article_by_normalized_url(&deps.paths, &job.normalized_url)
            .ok()
            .flatten()
    } else {
        None
    };

    let record_result = match existing_for_force {
        Some(existing) => crate::add_article_memory_override(&deps.paths, add_req, &existing.id),
        None => add_article_memory(&deps.paths, add_req),
    };

    let record = match record_result {
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
```

Also add `crate::add_article_memory_override` to the `use crate::{...}` block at the top of `worker.rs`. And ensure `find_article_by_normalized_url` is re-exported via `src/lib.rs` (check with `grep 'pub use' src/lib.rs`; the existing `pub use` from `article_memory` likely already re-exports via `pub use article_memory::*` — verify).

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test --lib worker_force_path_reuses_existing_article_id 2>&1 | tail -10`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 8: Run full suite**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add src/article_memory/ingest/types.rs src/article_memory/ingest/queue.rs src/article_memory/ingest/worker.rs
git commit -m "feat(article-memory): worker reuses article_id when force=true hits existing record"
```

---

## Task 1g: Startup URL normalization migration

**Files:**
- Modify: `src/article_memory/mod.rs` — add `migrate_urls_to_normalized` + call site in `init_article_memory`

**Context:** Phase 1 stored raw `record.url` without running `normalize_url`. Article-level dedup only works if stored URLs match the normalized form queries use. One-time startup migration rewrites them. Idempotent: `normalize(normalize(x)) == normalize(x)`.

- [ ] **Step 1: Write failing test**

Append to `src/article_memory/mod.rs` test module:

```rust
#[cfg(test)]
mod migrate_tests {
    use super::*;
    use crate::article_memory::types::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
    use tempfile::TempDir;

    fn mk_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn seed(paths: &RuntimePaths, records: Vec<(&str, Option<&str>)>) {
        init_article_memory(paths).unwrap();
        let mut index = internals::load_index(paths).unwrap();
        index.articles = records
            .into_iter()
            .map(|(id, url)| ArticleMemoryRecord {
                id: id.into(),
                title: format!("T {id}"),
                url: url.map(String::from),
                source: "test".into(),
                language: None,
                tags: vec![],
                status: ArticleMemoryRecordStatus::Saved,
                value_score: Some(0.5),
                captured_at: "2026-04-01T00:00:00Z".into(),
                updated_at: "2026-04-01T00:00:00Z".into(),
                content_path: format!("articles/{id}.md"),
                raw_path: None,
                normalized_path: None,
                summary_path: None,
                translation_path: None,
                notes: None,
                clean_status: None,
                clean_profile: None,
            })
            .collect();
        internals::write_index(paths, &index).unwrap();
    }

    #[test]
    fn migration_normalizes_trailing_slash_and_fragment() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        // Seed with URLs that normalize_url will canonicalize (e.g., strip "#frag").
        seed(&paths, vec![("aaa", Some("https://example.com/p#frag"))]);
        let changed = migrate_urls_to_normalized(&paths).unwrap();
        assert!(changed >= 1, "expected at least one URL rewritten");
        let index = internals::load_index(&paths).unwrap();
        assert!(
            !index.articles[0].url.as_ref().unwrap().contains('#'),
            "fragment should be stripped"
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![("aaa", Some("https://example.com/p#frag"))]);
        migrate_urls_to_normalized(&paths).unwrap();
        let changed_second = migrate_urls_to_normalized(&paths).unwrap();
        assert_eq!(changed_second, 0, "second run should be no-op");
    }

    #[test]
    fn migration_skips_records_with_no_url() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![("aaa", None)]);
        let changed = migrate_urls_to_normalized(&paths).unwrap();
        assert_eq!(changed, 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib article_memory::migrate_tests 2>&1 | tail -10`
Expected: compile error `cannot find function migrate_urls_to_normalized`.

- [ ] **Step 3: Implement `migrate_urls_to_normalized`**

Add to `src/article_memory/mod.rs` after `add_article_memory_override`:

```rust
/// One-time startup pass: rewrites `record.url` to its normalized form so
/// `find_article_by_normalized_url` can match it. Idempotent — running it
/// twice is a no-op because `normalize_url` is a fixpoint. Returns the
/// number of records that changed.
pub fn migrate_urls_to_normalized(paths: &RuntimePaths) -> Result<usize> {
    let mut index = internals::load_index(paths)?;
    let mut changed = 0usize;
    for article in &mut index.articles {
        let Some(url) = article.url.as_ref() else {
            continue;
        };
        let normalized = crate::article_memory::ingest::host_profile::normalize_url(url)
            .unwrap_or_else(|_| url.clone());
        if normalized != *url {
            article.url = Some(normalized);
            changed += 1;
        }
    }
    if changed > 0 {
        index.updated_at = isoformat(now_utc());
        internals::write_index(paths, &index)?;
        tracing::info!(count = changed, "migrated article URLs to normalized form");
    }
    Ok(changed)
}
```

- [ ] **Step 4: Call from `init_article_memory`**

Edit `src/article_memory/mod.rs:18-24` — extend:

```rust
pub fn init_article_memory(paths: &RuntimePaths) -> Result<ArticleMemoryStatusResponse> {
    ensure_article_memory_dirs(paths)?;
    if !paths.article_memory_index_path().is_file() {
        write_index(paths, &ArticleMemoryIndex::new())?;
    }
    migrate_urls_to_normalized(paths)?;
    check_article_memory(paths)
}
```

Note the existing `write_index(paths, &ArticleMemoryIndex::new())?;` on line 21 uses the re-exported `write_index`; change that to `internals::write_index(paths, &ArticleMemoryIndex::new())?;` if it's not already. (Check: if the current line compiles, leave it; just add the new line after it.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib article_memory::migrate_tests 2>&1 | tail -10`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/mod.rs
git commit -m "feat(article-memory): migrate stored URLs to normalized form at startup"
```

---

## Task 1h: Startup duplicate merge (Q11 D+A — value_score priority, captured_at tiebreak, delete loser files)

**Files:**
- Modify: `src/article_memory/mod.rs` — add `merge_duplicate_urls` + call site in `init_article_memory`

**Context:** After Task 1g normalizes URLs, duplicates become detectable. Policy (locked in brainstorm):
- Winner: higher `value_score`; tie → later `captured_at`; tie → first-seen.
- Loser: removed from index + `{id}.md`, `{id}.summary.md`, `{id}.bin`, `{id}.raw.txt`, `{id}.normalized.md`, `{id}.translation.md` deleted from disk. No backup.

- [ ] **Step 1: Write failing tests**

Append to `src/article_memory/mod.rs` test module:

```rust
#[cfg(test)]
mod merge_tests {
    use super::*;
    use crate::article_memory::types::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
    use tempfile::TempDir;

    fn mk_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn mk_record(id: &str, url: &str, score: f32, captured_at: &str) -> ArticleMemoryRecord {
        ArticleMemoryRecord {
            id: id.into(),
            title: format!("T {id}"),
            url: Some(url.into()),
            source: "test".into(),
            language: None,
            tags: vec![],
            status: ArticleMemoryRecordStatus::Saved,
            value_score: Some(score),
            captured_at: captured_at.into(),
            updated_at: captured_at.into(),
            content_path: format!("articles/{id}.md"),
            raw_path: Some(format!("articles/{id}.raw.txt")),
            normalized_path: Some(format!("articles/{id}.normalized.md")),
            summary_path: None,
            translation_path: None,
            notes: None,
            clean_status: None,
            clean_profile: None,
        }
    }

    fn seed(paths: &RuntimePaths, records: Vec<ArticleMemoryRecord>) {
        init_article_memory(paths).unwrap();
        // Skip migration in tests that want raw duplicates — write index directly.
        let mut index = internals::load_index(paths).unwrap();
        index.articles = records;
        internals::write_index(paths, &index).unwrap();
        let articles_dir = paths.runtime_dir.join("article-memory").join("articles");
        std::fs::create_dir_all(&articles_dir).unwrap();
        for record in &index.articles {
            std::fs::write(articles_dir.join(format!("{}.md", record.id)), "c").unwrap();
            std::fs::write(articles_dir.join(format!("{}.raw.txt", record.id)), "r").unwrap();
            std::fs::write(articles_dir.join(format!("{}.normalized.md", record.id)), "n").unwrap();
        }
    }

    #[test]
    fn merge_keeps_higher_value_score_and_deletes_loser_files() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(
            &paths,
            vec![
                mk_record("lower", "https://example.com/p", 0.7, "2026-04-20T00:00:00Z"),
                mk_record("higher", "https://example.com/p", 0.9, "2026-04-10T00:00:00Z"),
            ],
        );

        let merged = merge_duplicate_urls(&paths).unwrap();
        assert_eq!(merged, 1);

        let index = internals::load_index(&paths).unwrap();
        assert_eq!(index.articles.len(), 1);
        assert_eq!(index.articles[0].id, "higher");

        let articles_dir = paths.runtime_dir.join("article-memory").join("articles");
        assert!(!articles_dir.join("lower.md").exists());
        assert!(!articles_dir.join("lower.raw.txt").exists());
        assert!(!articles_dir.join("lower.normalized.md").exists());
        assert!(articles_dir.join("higher.md").exists());
    }

    #[test]
    fn merge_tiebreaks_by_captured_at_when_scores_equal() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(
            &paths,
            vec![
                mk_record("older", "https://example.com/p", 0.5, "2026-04-01T00:00:00Z"),
                mk_record("newer", "https://example.com/p", 0.5, "2026-04-20T00:00:00Z"),
            ],
        );

        merge_duplicate_urls(&paths).unwrap();

        let index = internals::load_index(&paths).unwrap();
        assert_eq!(index.articles.len(), 1);
        assert_eq!(index.articles[0].id, "newer");
    }

    #[test]
    fn merge_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(
            &paths,
            vec![mk_record("a", "https://example.com/p", 0.5, "2026-04-01T00:00:00Z")],
        );
        merge_duplicate_urls(&paths).unwrap();
        let second = merge_duplicate_urls(&paths).unwrap();
        assert_eq!(second, 0);
    }

    #[test]
    fn merge_leaves_records_without_url_untouched() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        let mut r1 = mk_record("a", "https://example.com/p", 0.5, "2026-04-01T00:00:00Z");
        r1.url = None;
        let mut r2 = mk_record("b", "https://example.com/p", 0.5, "2026-04-01T00:00:00Z");
        r2.url = None;
        seed(&paths, vec![r1, r2]);
        let merged = merge_duplicate_urls(&paths).unwrap();
        assert_eq!(merged, 0);
        let index = internals::load_index(&paths).unwrap();
        assert_eq!(index.articles.len(), 2);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib article_memory::merge_tests 2>&1 | tail -10`
Expected: compile error `cannot find function merge_duplicate_urls`.

- [ ] **Step 3: Implement `merge_duplicate_urls`**

Add to `src/article_memory/mod.rs` after `migrate_urls_to_normalized`:

```rust
/// Merge duplicate article records sharing the same `url`. Winner selection:
/// (1) higher `value_score`, (2) later `captured_at`, (3) first-seen. Loser
/// record is removed from the index and its on-disk content/summary/raw/
/// normalized/translation/embedding files are deleted. No backup is kept —
/// this is a one-way cleanup per user decision.
pub fn merge_duplicate_urls(paths: &RuntimePaths) -> Result<usize> {
    use std::collections::HashMap;

    let mut index = internals::load_index(paths)?;
    if index.articles.is_empty() {
        return Ok(0);
    }

    // Group record indices by url. Records without url are skipped.
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, article) in index.articles.iter().enumerate() {
        if let Some(url) = &article.url {
            groups.entry(url.clone()).or_default().push(i);
        }
    }

    let mut losers: Vec<usize> = Vec::new();
    for (_url, indices) in groups.iter() {
        if indices.len() < 2 {
            continue;
        }
        // Find winner: highest value_score, tiebreak on captured_at desc,
        // final tiebreak: earliest vec position (first-seen).
        let mut best = indices[0];
        for &candidate in &indices[1..] {
            let best_rec = &index.articles[best];
            let cand_rec = &index.articles[candidate];
            let best_score = best_rec.value_score.unwrap_or(0.0);
            let cand_score = cand_rec.value_score.unwrap_or(0.0);
            let pick_candidate = if cand_score > best_score {
                true
            } else if cand_score < best_score {
                false
            } else {
                cand_rec.captured_at > best_rec.captured_at
            };
            if pick_candidate {
                best = candidate;
            }
        }
        for &i in indices {
            if i != best {
                losers.push(i);
            }
        }
    }

    if losers.is_empty() {
        return Ok(0);
    }

    let articles_dir = paths.runtime_dir.join("article-memory").join("articles");
    let embeddings_dir = paths.runtime_dir.join("article-memory").join("embeddings");

    // Collect loser IDs before we mutate the vec.
    let loser_ids: Vec<String> = losers
        .iter()
        .map(|&i| index.articles[i].id.clone())
        .collect();

    // Remove from index (desc order so indices stay valid).
    let mut sorted_losers = losers;
    sorted_losers.sort_unstable_by(|a, b| b.cmp(a));
    for i in sorted_losers {
        let dropped = index.articles.remove(i);
        tracing::info!(
            dropped_id = %dropped.id,
            url = %dropped.url.unwrap_or_default(),
            "merging duplicate article: dropping loser"
        );
    }

    index.updated_at = isoformat(now_utc());
    internals::write_index(paths, &index)?;

    // Best-effort delete disk files (index write already succeeded).
    for id in &loser_ids {
        for ext in ["md", "raw.txt", "normalized.md", "summary.md", "translation.md"] {
            let p = articles_dir.join(format!("{id}.{ext}"));
            let _ = fs::remove_file(&p);
        }
        let _ = fs::remove_file(embeddings_dir.join(format!("{id}.bin")));
    }

    Ok(loser_ids.len())
}
```

- [ ] **Step 4: Call from `init_article_memory`**

Edit `src/article_memory/mod.rs` — update `init_article_memory` to call `merge_duplicate_urls` AFTER `migrate_urls_to_normalized`:

```rust
pub fn init_article_memory(paths: &RuntimePaths) -> Result<ArticleMemoryStatusResponse> {
    ensure_article_memory_dirs(paths)?;
    if !paths.article_memory_index_path().is_file() {
        internals::write_index(paths, &ArticleMemoryIndex::new())?;
    }
    migrate_urls_to_normalized(paths)?;
    merge_duplicate_urls(paths)?;
    check_article_memory(paths)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib article_memory::merge_tests 2>&1 | tail -10`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 6: Run full test suite to catch regressions**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: all green.

- [ ] **Step 7: Run clippy + fmt gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/article_memory/mod.rs
git commit -m "feat(article-memory): merge duplicate-URL records at startup (score + captured_at)"
```

---

## Task 2a: `src/imessage_send.rs` — target validator, AppleScript escape, stub for non-macOS

**Files:**
- Create: `src/imessage_send.rs`
- Create: `tests/rust/imessage_send.rs`
- Modify: `src/lib.rs` — `pub mod imessage_send;`
- Modify: `tests/rust/mod.rs` — `mod imessage_send;`

**Context:** Foundation of T2. Pure functions (validator + escaper) + cfg-gated send function. No dependency on `regex` crate — we use manual char-class checks to avoid a new dep.

- [ ] **Step 1: Create the module skeleton with pub API**

Create `src/imessage_send.rs`:

```rust
//! iMessage notification for async ingest completion. macOS-only real
//! implementation shells out to `osascript`; non-macOS builds get a stub
//! so Linux CI and unit tests on dev machines still compile.

/// Accept `+E.164` phone (7..=15 digits after `+`) or a basic email
/// (`foo@bar.baz`). Matches the target shape ZeroClaw's IMessageChannel
/// uses, so behavior stays consistent across the stack.
pub fn is_valid_target(handle: &str) -> bool {
    if let Some(rest) = handle.strip_prefix('+') {
        return rest.len() >= 7
            && rest.len() <= 15
            && rest.chars().all(|c| c.is_ascii_digit());
    }
    // Basic email: exactly one '@', non-empty local + domain, domain has a dot.
    let mut parts = handle.splitn(2, '@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    if local.is_empty() || domain.is_empty() || handle.contains(char::is_whitespace) {
        return false;
    }
    if handle.matches('@').count() != 1 {
        return false;
    }
    domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && local.chars().all(|c| !c.is_whitespace())
}

/// Escape a string for safe embedding between double quotes in an
/// AppleScript literal. Only backslash and double-quote need escaping;
/// CJK and emoji pass through since AppleScript source is Unicode-safe.
pub fn escape_applescript(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', r#"\""#)
}

/// Send `text` to `handle` via osascript. Errors on invalid target,
/// osascript non-zero exit, or IO failure.
#[cfg(target_os = "macos")]
pub async fn send_imessage(handle: &str, text: &str) -> anyhow::Result<()> {
    use anyhow::{anyhow, bail, Context};
    if !is_valid_target(handle) {
        bail!("invalid imessage target: {handle}");
    }
    let script = format!(
        "tell application \"Messages\"\n\
         \tset targetService to 1st account whose service type = iMessage\n\
         \tset targetBuddy to buddy \"{handle}\" of targetService\n\
         \tsend \"{text}\" to targetBuddy\n\
         end tell",
        handle = escape_applescript(handle),
        text = escape_applescript(text),
    );
    let output = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .await
        .context("spawn osascript")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!("osascript failed: {stderr}"));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub async fn send_imessage(_handle: &str, _text: &str) -> anyhow::Result<()> {
    tracing::debug!("imessage_send stub (non-macos): no-op");
    Ok(())
}

/// Notify the user via iMessage, guarding against any handle not in
/// `allowed`. Defense-in-depth against upstream bypass: even if the
/// reply_handle came from a bad source, we re-check here. Not-in-allowlist
/// downgrades to a WARN log and returns Ok (the article is already saved,
/// so a missing notification should not fail the job).
pub async fn notify_user(
    handle: &str,
    text: &str,
    allowed: &[String],
) -> anyhow::Result<()> {
    if !allowed.iter().any(|c| c == handle) {
        tracing::warn!(
            handle = %handle,
            "reply_handle not in allowed_contacts; skipping iMessage notification",
        );
        return Ok(());
    }
    send_imessage(handle, text).await
}
```

- [ ] **Step 2: Register in `src/lib.rs`**

Edit `src/lib.rs` — add near other `pub mod` declarations:

```rust
pub mod imessage_send;
```

- [ ] **Step 3: Write failing tests**

Create `tests/rust/imessage_send.rs`:

```rust
//! Tests for the iMessage send module. Real osascript call is not
//! exercised (needs real iMessage account); we cover pure functions.

use crate::imessage_send::{escape_applescript, is_valid_target, notify_user};

#[test]
fn is_valid_target_accepts_e164_phone() {
    assert!(is_valid_target("+8618672954807"));
    assert!(is_valid_target("+1234567"));
    assert!(is_valid_target("+123456789012345"));
}

#[test]
fn is_valid_target_rejects_too_short_or_too_long_phone() {
    assert!(!is_valid_target("+123456"));          // 6 digits
    assert!(!is_valid_target("+1234567890123456")); // 16 digits
    assert!(!is_valid_target("+"));
    assert!(!is_valid_target("+abcdefg"));
}

#[test]
fn is_valid_target_accepts_email() {
    assert!(is_valid_target("user@icloud.com"));
    assert!(is_valid_target("a@b.co"));
}

#[test]
fn is_valid_target_rejects_malformed_email() {
    assert!(!is_valid_target("user@icloud"));   // no dot in domain
    assert!(!is_valid_target("@icloud.com"));   // empty local
    assert!(!is_valid_target("user@"));         // empty domain
    assert!(!is_valid_target("a@b@c.com"));     // two @
    assert!(!is_valid_target("user @icloud.com")); // whitespace
}

#[test]
fn is_valid_target_rejects_group_thread_id() {
    // chat000... style IDs from the Messages DB are NOT valid send targets.
    assert!(!is_valid_target("chat000000123456"));
    assert!(!is_valid_target("iMessage;-;chat000"));
}

#[test]
fn escape_applescript_handles_quotes() {
    assert_eq!(escape_applescript(r#"He said "hi""#), r#"He said \"hi\""#);
}

#[test]
fn escape_applescript_handles_backslash() {
    assert_eq!(escape_applescript(r"a\b"), r"a\\b");
}

#[test]
fn escape_applescript_preserves_cjk_and_emoji() {
    assert_eq!(escape_applescript("已保存《标题》🎉"), "已保存《标题》🎉");
}

#[tokio::test]
async fn notify_user_skips_when_not_in_allowlist() {
    // handle not in list → Ok + warn log, no osascript attempt.
    let allowed = vec!["+8618672954807".to_string()];
    notify_user("+8613800000000", "hi", &allowed).await.unwrap();
}

#[tokio::test]
async fn notify_user_stub_on_non_macos_returns_ok() {
    // Only meaningful on non-macOS, but on macOS this will actually try
    // to send; however, the handle IS in allowed list so it will go through
    // to send_imessage — and send_imessage would fail if no Messages.app
    // permission. We skip that assertion on macOS.
    #[cfg(not(target_os = "macos"))]
    {
        let allowed = vec!["user@example.com".to_string()];
        notify_user("user@example.com", "hi", &allowed).await.unwrap();
    }
}
```

- [ ] **Step 4: Register the test module**

Edit `tests/rust/mod.rs` — add `mod imessage_send;` to the list of submodules.

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test --lib imessage_send 2>&1 | tail -10`
Expected: compile error `module 'imessage_send' not found` or `function not defined`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib imessage_send 2>&1 | tail -15`
Expected: all tests pass.

- [ ] **Step 7: Run clippy + fmt gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/imessage_send.rs src/lib.rs tests/rust/imessage_send.rs tests/rust/mod.rs
git commit -m "feat(imessage-send): add osascript sender with validator, escaper, and allowlist gate"
```

---

## Task 2b: `IngestRequest.reply_handle` + `IngestJob.reply_handle`

**Files:**
- Modify: `src/article_memory/ingest/types.rs` — add field to both structs
- Modify: `src/article_memory/ingest/queue.rs:269-287` — propagate field in `submit()`

**Context:** The handle is set by the caller (LLM via skill prompt) and carried on the job across restarts. Missing → no notification (unit test verifies CLI/cron path).

- [ ] **Step 1: Write failing test**

Append to `src/article_memory/ingest/types.rs` tests module:

```rust
    #[test]
    fn ingest_request_default_reply_handle_is_none() {
        let req: IngestRequest =
            serde_json::from_str(r#"{"url": "https://example.com/"}"#).unwrap();
        assert!(req.reply_handle.is_none());
    }

    #[test]
    fn ingest_request_accepts_reply_handle() {
        let req: IngestRequest = serde_json::from_str(
            r#"{"url": "https://example.com/", "reply_handle": "+8618672954807"}"#,
        )
        .unwrap();
        assert_eq!(req.reply_handle.as_deref(), Some("+8618672954807"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib article_memory::ingest::types::tests::ingest_request_default_reply_handle 2>&1 | tail -10`
Expected: compile error `reply_handle not on IngestRequest`.

- [ ] **Step 3: Add `reply_handle` to `IngestRequest`**

Edit the struct definition at `src/article_memory/ingest/types.rs` (after Task 1b's shape):

```rust
pub struct IngestRequest {
    pub url: String,
    #[serde(default)]
    pub force: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_handle: Option<String>,
}
```

- [ ] **Step 4: Add `reply_handle` to `IngestJob`**

Edit the struct at `src/article_memory/ingest/types.rs:64-93`:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_handle: Option<String>,
```

Place it after `tags: Vec<String>,`.

- [ ] **Step 5: Propagate in `submit()`**

Edit `src/article_memory/ingest/queue.rs` inside `submit()` — the `IngestJob { ... }` struct literal. Add:

```rust
            reply_handle: req.reply_handle.clone(),
```

- [ ] **Step 6: Run tests**

Run: `cargo test --lib article_memory::ingest::types 2>&1 | tail -10`
Expected: all tests pass including 2 new ones.

Run: `cargo build --lib 2>&1 | tail -5` — expected `Finished dev` (no exhaustiveness issues, field is optional with default).

- [ ] **Step 7: Commit**

```bash
git add src/article_memory/ingest/types.rs src/article_memory/ingest/queue.rs
git commit -m "feat(article-memory): add reply_handle to IngestRequest and IngestJob"
```

---

## Task 2c: `build_reply_text` + `humanize_issue_type`

**Files:**
- Create: `src/article_memory/ingest/reply_text.rs`
- Modify: `src/article_memory/ingest/mod.rs` — `pub(super) mod reply_text;` (or `pub mod` if worker needs it)

**Context:** Pure text synthesis from `IngestJob` terminal state + article title lookup. Kept out of `worker.rs` to keep worker focused on orchestration.

- [ ] **Step 1: Write failing tests**

Create `src/article_memory/ingest/reply_text.rs` with a test-only skeleton first:

```rust
use super::types::{IngestJob, IngestJobError, IngestJobStatus, IngestOutcomeSummary};

/// Returns an empty string for non-terminal job states (caller should
/// treat empty as "don't send").
pub fn build_reply_text(job: &IngestJob, resolved_title: Option<&str>) -> String {
    match job.status {
        IngestJobStatus::Saved => {
            let title = resolved_title.unwrap_or(job.url.as_str());
            format!("已保存《{title}》")
        }
        IngestJobStatus::Rejected => "内容价值不高，已略过".to_string(),
        IngestJobStatus::Failed => {
            let reason = humanize_issue_type(
                job.error
                    .as_ref()
                    .map(|e| e.issue_type.as_str())
                    .unwrap_or(""),
            );
            format!("抓取失败：{reason}\n{url}", url = job.url)
        }
        _ => String::new(),
    }
}

/// Map stable `issue_type` strings to user-facing Chinese phrases.
/// Unknown types fall through to a generic "查看详情" hint.
pub fn humanize_issue_type(issue_type: &str) -> &'static str {
    match issue_type {
        "crawl4ai_unavailable" => "抓取服务暂时不可用，请稍后再试",
        "auth_required" => "需要登录才能访问，请登录后再发",
        "site_changed" => "页面结构无法识别，可能需要更新策略",
        "empty_content" => "抓到的内容太短（可能是登录墙或 404）",
        "pipeline_error" => "内部处理出错",
        _ => "未知错误：请查看 articles ingest show",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::IngestOutcomeSummary;

    fn base_job() -> IngestJob {
        IngestJob {
            id: "job1".into(),
            url: "https://example.com/a".into(),
            normalized_url: "https://example.com/a".into(),
            title_override: None,
            tags: vec![],
            source_hint: None,
            profile_name: "articles-generic".into(),
            resolved_source: None,
            status: IngestJobStatus::Pending,
            article_id: None,
            outcome: None,
            error: None,
            warnings: vec![],
            submitted_at: "t".into(),
            started_at: None,
            finished_at: None,
            attempts: 1,
            force: false,
            reply_handle: None,
        }
    }

    #[test]
    fn saved_uses_resolved_title() {
        let mut job = base_job();
        job.status = IngestJobStatus::Saved;
        let txt = build_reply_text(&job, Some("Real Title"));
        assert_eq!(txt, "已保存《Real Title》");
    }

    #[test]
    fn saved_falls_back_to_url_when_no_title() {
        let mut job = base_job();
        job.status = IngestJobStatus::Saved;
        let txt = build_reply_text(&job, None);
        assert_eq!(txt, "已保存《https://example.com/a》");
    }

    #[test]
    fn rejected_has_fixed_phrase() {
        let mut job = base_job();
        job.status = IngestJobStatus::Rejected;
        assert_eq!(build_reply_text(&job, None), "内容价值不高，已略过");
    }

    #[test]
    fn failed_includes_url_on_second_line() {
        let mut job = base_job();
        job.status = IngestJobStatus::Failed;
        job.error = Some(IngestJobError {
            issue_type: "auth_required".into(),
            message: "login wall".into(),
            stage: "fetching".into(),
        });
        let txt = build_reply_text(&job, None);
        assert!(txt.starts_with("抓取失败：需要登录才能访问"));
        assert!(txt.ends_with("\nhttps://example.com/a"));
    }

    #[test]
    fn failed_unknown_issue_type_uses_fallback() {
        let mut job = base_job();
        job.status = IngestJobStatus::Failed;
        job.error = Some(IngestJobError {
            issue_type: "something_new".into(),
            message: "x".into(),
            stage: "fetching".into(),
        });
        let txt = build_reply_text(&job, None);
        assert!(txt.contains("未知错误"));
    }

    #[test]
    fn non_terminal_returns_empty_string() {
        let mut job = base_job();
        for status in [
            IngestJobStatus::Pending,
            IngestJobStatus::Fetching,
            IngestJobStatus::Cleaning,
            IngestJobStatus::Judging,
            IngestJobStatus::Embedding,
        ] {
            job.status = status;
            assert!(build_reply_text(&job, None).is_empty());
        }
    }

    #[test]
    fn humanize_covers_all_stable_types() {
        for t in ["crawl4ai_unavailable", "auth_required", "site_changed", "empty_content", "pipeline_error"] {
            assert_ne!(humanize_issue_type(t), "未知错误：请查看 articles ingest show");
        }
    }
}
```

- [ ] **Step 2: Register the module**

Edit `src/article_memory/ingest/mod.rs` — add:

```rust
pub(super) mod reply_text;
```

(Check if it needs to be `pub` for access from `worker.rs` — in the same crate it can be `pub(super)` since `worker.rs` is a sibling.)

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib article_memory::ingest::reply_text 2>&1 | tail -15`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/ingest/reply_text.rs src/article_memory/ingest/mod.rs
git commit -m "feat(article-memory): add reply_text builder with stable issue_type humanizer"
```

---

## Task 2d: Worker terminal-state notify hook + `IngestWorkerDeps.imessage_config`

**Files:**
- Modify: `src/article_memory/ingest/worker.rs` — add `imessage_config` to `IngestWorkerDeps`, call `notify_user` at `execute_job` end

**Context:** After `queue.finish(...)` completes, re-read the job from the queue (final state + resolved `article_id`), look up title if Saved, then fire `notify_user`. Failures logged, never propagate.

- [ ] **Step 1: Extend `IngestWorkerDeps`**

Edit `src/article_memory/ingest/worker.rs:17-25` — add field:

```rust
#[derive(Clone)]
pub struct IngestWorkerDeps {
    pub paths: RuntimePaths,
    pub crawl4ai_config: Arc<Crawl4aiConfig>,
    pub supervisor: Arc<Crawl4aiSupervisor>,
    pub profile_locks: Crawl4aiProfileLocks,
    pub article_memory_config: Arc<ArticleMemoryConfig>,
    pub providers: Arc<Vec<ModelProviderConfig>>,
    pub ingest_config: Arc<ArticleMemoryIngestConfig>,
    pub imessage_config: Arc<crate::app_config::ImessageConfig>,
}
```

- [ ] **Step 2: Add notify hook at end of `execute_job`**

Edit `src/article_memory/ingest/worker.rs` — find the last line of `execute_job` (currently `queue.finish(&job.id, outcome).await;` around line 305). Immediately after it, add:

```rust
    // Terminal notification: only if job has a reply_handle. Article is
    // already saved by this point; notification failure never changes
    // job state.
    if let Some(handle) = job.reply_handle.as_deref() {
        let finished_job = queue.get(&job.id).await;
        let Some(finished_job) = finished_job else {
            return;
        };
        let resolved_title = finished_job.article_id.as_ref().and_then(|id| {
            crate::article_memory::internals::load_index(&deps.paths)
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
        if let Err(err) = crate::imessage_send::notify_user(
            handle,
            &text,
            &deps.imessage_config.allowed_contacts,
        )
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
```

This requires `internals` to be accessible from `worker.rs`. Since `worker.rs` lives at `src/article_memory/ingest/worker.rs` and `internals.rs` at `src/article_memory/internals.rs`, the path is `crate::article_memory::internals::load_index`. But `internals` is declared `mod internals;` (private to `article_memory`); `load_index` is `pub(super) fn` which only grants visibility to the `article_memory` module. To call it from `article_memory::ingest::worker`, it is already nested *within* `article_memory`, so `super::super::internals::load_index(&deps.paths)` is visible. Use that form for clarity:

```rust
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
```

- [ ] **Step 3: Write a mock-based worker integration test**

Add to `tests/rust/article_memory_ingest_worker.rs`:

```rust
#[tokio::test]
async fn worker_skips_notify_when_reply_handle_missing() {
    // CLI/cron path: no reply_handle → no notify attempt. We verify by
    // setting allowed_contacts to something that would WARN-log on miss,
    // and ensure normal termination regardless. Behavioral check is
    // indirect; primary guarantee is "doesn't panic + job reaches Saved".
    let tmp = TempDir::new().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join("runtime"),
    };
    crate::init_article_memory(&paths).unwrap();
    let queue = build_queue(&paths);
    let supervisor = Arc::new(Crawl4aiSupervisor::for_test(rich_markdown_response(
        "agent memory MCP Claude Code a lot of relevant content to pass value judge",
    )));
    let deps = build_deps(&paths, supervisor);
    IngestWorkerPool::spawn(queue.clone(), deps, 1);

    let resp = queue
        .submit(IngestRequest {
            url: "https://example.com/p/1".into(),
            force: false,
            title: None,
            tags: vec![],
            source_hint: None,
            reply_handle: None,
        })
        .await
        .unwrap();

    wait_for_terminal(&queue, &resp.job_id, Duration::from_secs(10)).await;
    let job = queue.get(&resp.job_id).await.unwrap();
    assert!(job.reply_handle.is_none());
    assert_eq!(job.status, IngestJobStatus::Saved);
}
```

`build_deps` must construct `IngestWorkerDeps` with a fresh `imessage_config`. Update the helper in the test file to include:

```rust
imessage_config: Arc::new(crate::app_config::ImessageConfig {
    allowed_contacts: vec!["+8618672954807".into()],
}),
```

Find the existing `build_deps` helper and add the field.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib worker_skips_notify_when_reply_handle_missing 2>&1 | tail -15`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Run full test suite**

Run: `cargo test --lib 2>&1 | tail -5`
Expected: all green. If pre-existing worker tests fail due to `imessage_config` missing on `IngestWorkerDeps`, update each `build_deps`/`IngestWorkerDeps { ... }` site in tests to add the field.

- [ ] **Step 6: Clippy + fmt gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/article_memory/ingest/worker.rs tests/rust/article_memory_ingest_worker.rs
git commit -m "feat(article-memory): worker posts iMessage on terminal state when reply_handle set"
```

---

## Task 2e: `local_proxy.rs` wiring — pass `imessage_config` to worker pool

**Files:**
- Modify: `src/local_proxy.rs:129-149` — IngestWorkerPool spawn args

**Context:** Close the wiring loop. `LocalConfig.imessage: ImessageConfig` is already parsed by `check_local_config`. Just clone it into `IngestWorkerDeps`.

- [ ] **Step 1: Update worker spawn block**

Edit `src/local_proxy.rs` — inside the `if ingest_config.enabled { IngestWorkerPool::spawn(...) }` block, add a field to the `IngestWorkerDeps` struct literal:

```rust
                imessage_config: Arc::new(local_config.imessage.clone()),
```

Place it after `ingest_config: ingest_config.clone(),`.

- [ ] **Step 2: Build and run the full test suite to verify wiring**

Run: `cargo build --release --bin davis-local-proxy 2>&1 | tail -5`
Expected: `Finished release`.

Run: `cargo test --lib 2>&1 | tail -5`
Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add src/local_proxy.rs
git commit -m "feat(local-proxy): wire imessage_config into ingest worker pool"
```

---

## Task 3a: `SKILL.toml` rewrite — add 2 ingest tools + rewrite prompts

**Files:**
- Modify: `project-skills/article-memory/SKILL.toml`

**Context:** Close the Phase 1 silent-contract break. Existing tools (`status`, `list`, `search`) stay. Add `ingest_status` + `ingest_list` as GET `kind="http"` tools (follow the existing `[tools.args]` query-param template pattern). Rewrite `[skill].prompts` with four intents: save / refresh / query / retrieval.

- [ ] **Step 1: Read the current SKILL.toml to see existing shape**

Run: `cat project-skills/article-memory/SKILL.toml`
Expected: a `[skill]` block with `name`, `description`, `prompts`, followed by `[[tools]]` blocks. Note the exact current shape so the rewrite matches conventions (quoting style, line breaks, `[tools.args]` vs inline).

- [ ] **Step 2: Replace the entire file**

Overwrite `project-skills/article-memory/SKILL.toml` with:

```toml
[skill]
name = "article-memory"
description = "抓取、存储、检索、回忆长文章与技术文档"
prompts = [
  """
  用户发来一段纯 URL（或"存一下 <url>"、"收藏 <url>"等保存意图）时：
  用 http_request 工具 POST 到 http://127.0.0.1:3010/article-memory/ingest
  body: {"url": "<url>", "source_hint": "<channel>", "reply_handle": "<sender 或 null>", "tags": []}
  channel 取值：iMessage → "imessage"；webhook/Shortcut → "shortcut"；其他 → 按实际来源
  iMessage 场景下 reply_handle 必须填为 sender；daemon 完成后会通过 osascript 主动给用户发完成通知。其他 channel 不要填 reply_handle（保持为 null）。
  返回 202 + {job_id, status: "pending"} → 告诉用户"已收到，开始抓取"
  返回 409 + {error: "article_exists", title, existing_article_id} → 告诉用户"《title》已经收藏过了，需要刷新吗"
  返回 409 + {error: "duplicate_within_window", ...} → 告诉用户"最近已经存过，请稍后再试"
  返回 503 + {error: "persistence_degraded", ...} → 告诉用户"daemon 存储出问题，请联系管理员处理磁盘"
  """,
  """
  用户表达"帮我刷新 <url>"、"这篇文档有没有更新"、"重抓一下 <url>"、"再抓一遍"等重新拉取意图时：
  同上，body 里追加 "force": true
  force=true 跳过 article 级去重，直接重抓并覆盖原记录（article_id 保持不变）。
  409 article_exists 不会在 force=true 时出现；但如果同一 URL 有 active job 或刚在 24h 窗口内存过，依然会 409（duplicate_active_job / duplicate_within_window）。
  """,
  """
  用户问"那篇存好了吗"、"某个 job 现在什么状态"、"最近失败了几篇"等复盘类问题时：
  - 查具体 job：用 article-memory__ingest_status，传入 job_id
  - 列最近任务 / 失败项：用 article-memory__ingest_list；传 status=failed 只看失败
  """,
  """
  用户问已存文章相关问题（检索 / 列最近 / 健康检查）时：
  - article-memory__search：语义检索
  - article-memory__list：列最近几篇
  - article-memory__status：系统状态
  """,
]

[[tools]]
name = "status"
description = "article memory system health check"
kind = "http"
command = "GET http://127.0.0.1:3010/article-memory/status"

[[tools]]
name = "list"
description = "list recent saved articles"
kind = "http"
command = "GET http://127.0.0.1:3010/article-memory/articles?limit={{limit}}"
[tools.args]
limit = { type = "integer", required = false, description = "default 20, max 200" }

[[tools]]
name = "search"
description = "semantic + keyword hybrid search over saved articles"
kind = "http"
command = "GET http://127.0.0.1:3010/article-memory/search?q={{q}}&limit={{limit}}"
[tools.args]
q = { type = "string", required = true, description = "query text" }
limit = { type = "integer", required = false, description = "default 10, max 50" }

[[tools]]
name = "ingest_status"
description = "查询一个 URL 抓取 job 的当前状态。返回 status ∈ {pending, fetching, cleaning, judging, embedding, saved, rejected, failed}，以及 article_id (若已存)、error (若失败)、outcome 字段。用户问'那篇存好了吗'时调用。"
kind = "http"
command = "GET http://127.0.0.1:3010/article-memory/ingest/{{job_id}}"
[tools.args]
job_id = { type = "string", required = true, description = "submit 时返回的 UUID" }

[[tools]]
name = "ingest_list"
description = "列出最近的抓取任务。可按状态过滤（status=failed 等）。用于用户问'最近哪些抓取失败了'、'今天存了几篇'等复盘类问题。"
kind = "http"
command = "GET http://127.0.0.1:3010/article-memory/ingest?status={{status}}&limit={{limit}}"
[tools.args]
status = { type = "string", required = false, description = "pending|saved|failed 等，留空返回全部" }
limit = { type = "integer", required = false, description = "default 20, max 200" }
```

- [ ] **Step 3: Validate TOML parses**

Run: `cargo run --bin daviszeroclaw -- check-config 2>&1 | tail -5 || echo 'check-config may not exist; fallback'`

Alternatively, load it with `toml` crate:

Run: `python3 -c "import tomllib; print(list(tomllib.loads(open('project-skills/article-memory/SKILL.toml').read()).keys()))"`
Expected: `['skill', 'tools']` or similar confirming parse success.

- [ ] **Step 4: Restart daemon + manually verify ZeroClaw picks up the new tools**

Run: `./target/release/daviszeroclaw start`
Wait for startup. Then:
Run: `ls -la .runtime/davis/workspace/skills/article-memory/SKILL.toml`
Expected: the synced file exists with recent mtime.

Run: `grep -c '\[\[tools\]\]' .runtime/davis/workspace/skills/article-memory/SKILL.toml`
Expected: `5` (status, list, search, ingest_status, ingest_list).

- [ ] **Step 5: Commit**

```bash
git add project-skills/article-memory/SKILL.toml
git commit -m "feat(skills): rewrite article-memory skill for crawl4ai-backed ingest contract"
```

---

## Task 3b: `references/article_memory_api.md` rewrite

**Files:**
- Modify: `project-skills/article-memory/references/article_memory_api.md`

**Context:** Replace the browser-bridge "Ingest Browser Page" section (lines 60-89 of the current file) with the real crawl4ai-backed contract. Keep status/list/search sections. Document 202/400/409/503 responses + async contract + iMessage notify trigger + force semantics.

- [ ] **Step 1: Read the current file**

Run: `wc -l project-skills/article-memory/references/article_memory_api.md && sed -n '50,95p' project-skills/article-memory/references/article_memory_api.md`

- [ ] **Step 2: Replace the "Ingest Browser Page" section (lines ~60-89)**

Replace the block that starts with "Ingest Browser Page" (or equivalent heading that describes `profile/tab_id/new_tab`) with:

```markdown
## URL Ingest (crawl4ai-backed, async)

`POST http://127.0.0.1:3010/article-memory/ingest`

Submit a URL for asynchronous crawling and storage. The daemon spawns a
Chromium profile, extracts Markdown via crawl4ai, runs the cleaning
pipeline (value judge, LLM summary, embedding), and stores the article.
Returns 202 with a `job_id` immediately; real completion is observable
via `ingest_status`.

### Request body

```json
{
  "url": "https://example.com/post/1",
  "force": false,
  "tags": ["smoke"],
  "title": "optional override",
  "source_hint": "imessage | shortcut | cli | cron",
  "reply_handle": "+8618672954807 or user@icloud.com or null"
}
```

| Field | Required | Notes |
|---|---|---|
| `url` | yes | http/https only; SSRF guard rejects private + loopback |
| `force` | no | Default false. If true, bypass article-level dedup and overwrite existing record in place (same `article_id`) |
| `tags` | no | Array of strings, default empty |
| `title` | no | Optional title override; defaults to page metadata or URL |
| `source_hint` | no | Informational; suggested: `imessage`, `shortcut`, `cli`, `cron` |
| `reply_handle` | no | When set AND the handle is in `imessage.allowed_contacts`, daemon sends a completion notification via osascript |

### Responses

- **202 Accepted** (queued):
  ```json
  { "job_id": "uuid", "status": "pending", "submitted_at": "ISO8601", "deduped": false }
  ```
  If the URL has a job still active, `deduped: true` and the existing `job_id` is returned (idempotent replay).

- **400 Bad Request** — `invalid_url`, `invalid_scheme`, `private_address_blocked`.

- **409 Conflict** — three subtypes:
  - `article_exists` — URL already saved in the store; resubmit with `force: true` to refresh.
    ```json
    { "error": "article_exists", "existing_article_id": "aaa", "title": "...", "url": "...", "action": "resubmit with \"force\": true to re-crawl and update" }
    ```
  - `duplicate_within_window` — same URL saved within `dedup_window_hours` (default 24h). Not bypassable by `force`.
  - (Phase 1) `duplicate_active_job` — an in-flight job covers this URL. Returned 202 with `deduped: true`, not 409. Listed here for completeness.

- **503 Service Unavailable** —
  - `ingest_disabled` when ingest is toggled off.
  - `persistence_degraded` when the queue has failed to persist N consecutive times (default 3). Admin must free disk + restart daemon.

### iMessage completion notification

When `source_hint == "imessage"` AND `reply_handle` is valid AND the handle
is listed in `config/davis/local.toml` under `[imessage].allowed_contacts`,
the daemon sends a Chinese-language iMessage reply after the job reaches a
terminal state:
- Saved: `已保存《<title>》`
- Rejected: `内容价值不高，已略过`
- Failed: `抓取失败：<reason>\n<url>`

Notifications are fire-and-forget; failure to deliver (permissions, offline,
unknown buddy) is logged at `warn` and does not change the job outcome.

### `force=true` semantics

`force=true` asks the daemon to re-crawl and update an existing record:
- Rule 0 (article-level dedup) is skipped.
- Rule 1 (active job dedup) still applies.
- Rule 2 (recent-saved window) still applies.
- Worker reuses the existing `article_id`, overwrites title / captured_at /
  content / summary / embedding files in place. Search results stay
  single-record-per-URL.

## Ingest Status

`GET http://127.0.0.1:3010/article-memory/ingest/<job_id>`

Returns the current `IngestJob` record: status, article_id (if assigned),
outcome summary, error (if failed), warnings, timestamps.

## Ingest List

`GET http://127.0.0.1:3010/article-memory/ingest?status=<status>&limit=<n>`

List jobs, optionally filtered by status. `status` values:
`pending|fetching|cleaning|judging|embedding|saved|rejected|failed`.
Default limit 20, max 200.
```

- [ ] **Step 3: Preserve the rest of the file**

Leave all other sections (`Status`, `List`, `Search`, any introductory
paragraphs) intact.

- [ ] **Step 4: Spot-check readability**

Run: `head -10 project-skills/article-memory/references/article_memory_api.md && echo --- && grep -c '^##' project-skills/article-memory/references/article_memory_api.md`
Expected: top of file still looks like Phase 1 preamble; `##` count increased by roughly 3 (URL Ingest, Ingest Status, Ingest List) and decreased by 1 (old Ingest Browser Page).

- [ ] **Step 5: Commit**

```bash
git add project-skills/article-memory/references/article_memory_api.md
git commit -m "docs(skills): rewrite article_memory_api.md for crawl4ai-backed ingest"
```

---

## Task 4a: Verify config render + Phase 1 spec revisions

**Files:**
- Verify only (no change expected): `src/model_routing.rs:60-94` — `render_runtime_config_str`
- Modify: `docs/superpowers/specs/2026-04-24-article-memory-crawl4ai-ingest-design.md` — 4 line-level edits + 1 addendum

**Context:** The new `[article_memory.ingest]` config in `local.toml` is consumed by the daemon directly (via `LocalConfig`); ZeroClaw likely does not need any of those fields. Verify that either (a) the render preserves the section for ZeroClaw to read, or (b) ZeroClaw does not reference those fields. If (b), add a comment in the rendered template explaining the section is daemon-only.

- [ ] **Step 1: Start daemon and inspect rendered config**

Run: `./target/release/daviszeroclaw stop 2>/dev/null; ./target/release/daviszeroclaw start`
Wait for startup. Then in another shell:

Run: `cat .runtime/davis/config.toml | grep -A 20 '\[article_memory'`
Observe: either the `[article_memory.ingest]` section is present or absent.

- [ ] **Step 2: Grep ZeroClaw source for references**

Run: `rg -n 'article_memory\.ingest|article_memory_ingest|ingest\.host_profiles' /Users/faillonexie/Projects/zeroclaw/src /Users/faillonexie/Projects/zeroclaw/crates 2>&1 | head -20`

If zero hits → ZeroClaw does not read these fields; the daemon is the sole consumer. Document this in the Phase 1 spec (step 5 below).

If hits exist → the render must propagate the section. Inspect `render_runtime_config_str` at `src/model_routing.rs:60-94` to see if the section is whitelisted or passed through.

- [ ] **Step 3 (conditional): Extend render only if ZeroClaw needs it**

Only if step 2 shows ZeroClaw references: read the render function and ensure `[article_memory.ingest]` is emitted. If not, add the section. Skip this step if ZeroClaw does not need the fields.

- [ ] **Step 4: Revise Phase 1 spec at 4 locations**

Edit `docs/superpowers/specs/2026-04-24-article-memory-crawl4ai-ingest-design.md`:

**Line ~12:** replace the sentence that claims iMessage is a peer of CLI/HTTP with:

```markdown
Direct callers (CLI, cron, Shortcut, webhooks) POST to `/article-memory/ingest`
over loopback HTTP. iMessage intake is not a direct caller — iMessage messages
reach this daemon through ZeroClaw's LLM tool-calling layer via the
`article-memory__ingest_*` skill tools defined in `project-skills/article-memory/`.
```

**Line ~43 (architecture diagram):** split the "Channels" box into two layers — "Direct HTTP callers" (CLI/cron/Shortcut/webhook) and "LLM tool-call path" (iMessage → ZeroClaw → skill → HTTP).

**Line ~527:** replace the phrase about `ingest?status=failed` being useful for iMessage bot with:

```markdown
`GET /article-memory/ingest?status=failed` is exposed to ZeroClaw's LLM as
the `article-memory__ingest_list` skill tool; iMessage users ask for "recent
failures" in natural language and the LLM calls this tool.
```

**Line ~616 (follow-ups):** replace the "iMessage bridge wiring" follow-up with:

```markdown
iMessage integration is complete in Phase 2 via the `article-memory` skill
tools and `reply_handle`-driven completion notifications; no separate
bridge wiring is needed.
```

- [ ] **Step 5: Append an addendum section**

At the end of the Phase 1 spec, add:

```markdown
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
```

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-04-24-article-memory-crawl4ai-ingest-design.md
git commit -m "docs(specs): revise Phase 1 iMessage framing and add Phase 2 addendum"
```

---

## Final integration smoke

After all tasks land, run one live pass:

- [ ] **Step 1: Build**

Run: `cargo build --release --bin daviszeroclaw --bin davis-local-proxy 2>&1 | tail -3`
Expected: `Finished release`.

- [ ] **Step 2: Clippy + fmt + tests**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --lib 2>&1 | tail -5`
Expected: all green; total test count ≈ 154 + new Phase 2 tests (~30).

- [ ] **Step 3: Verify iMessage preflight skip has been restored**

Run: `grep -A 2 'check_imessage_permissions' src/cli/service.rs`
Expected: real call (no `//` prefix). If smoke-time skip is still commented out, restore it:

```rust
    check_imessage_permissions()?;
    println!("Preflight OK.");
```

- [ ] **Step 4: Live smoke (optional, requires iMessage + Mac + Chromium)**

If the user wants to verify end-to-end:
1. Start daemon: `./target/release/daviszeroclaw start`.
2. POST a new URL with `reply_handle` set to an allowed contact. Observe WARN-free completion in the log.
3. Verify Messages.app shows the expected Chinese notification text.

---

# Self-Review

## Spec coverage check

| Spec section | Task |
|---|---|
| §4.1 Two ingress paths | T3a (skill tools), T3b (api docs), T4a (Phase 1 spec revisions) |
| §4.3 `force=true` update-in-place | T1b (field), T1c (submit), T1e (override), T1f (worker) |
| §4.4 iMessage notify path | T2a (sender), T2b (handle fields), T2c (text), T2d (worker hook), T2e (config) |
| §5.1 New file `imessage_send.rs` | T2a |
| §5.2 All modified files | T1a-h + T2a-e + T3a-b + T4a |
| §6.1 `IngestRequest` shape | T1b + T2b |
| §6.2 `IngestJob.reply_handle` persistence | T2b |
| §6.3 `IngestSubmitError::ArticleExists` | T1b + T1d |
| §7.1 Rule 0 order | T1c |
| §7.2 Startup migration | T1g |
| §7.3 Startup merge (Q11 D+A) | T1h |
| §7.4 Force worker path | T1f |
| §7.5 Reply text table | T2c |
| §7.6 osascript send + allowlist gate | T2a |
| §8.1 `[[tools]]` ingest_status / ingest_list | T3a |
| §8.2 Prompts rewrite | T3a |
| §8.3 api.md rewrite | T3b |
| §13 Backward compat (serde defaults) | Handled in T1b (`force`), T2b (`reply_handle`) |

All spec sections map to at least one task. No gaps.

## Placeholder scan

Manual inspection: no `TBD`, no `TODO`, no "add appropriate error handling", no "similar to Task N". All code blocks show complete snippets.

## Type consistency

- `IngestRequest.force: bool` — introduced Task 1b, used Task 1c (`req.force`), Task 1f (`job.force`).
- `IngestRequest.reply_handle: Option<String>` — introduced Task 2b, used Task 2d.
- `IngestJob.force: bool` + `IngestJob.reply_handle: Option<String>` — both persistent, same names across tasks.
- `IngestSubmitError::ArticleExists { existing_article_id: String, title: String, url: String }` — fields consistent across T1b, T1c, T1d.
- `find_article_by_normalized_url(&RuntimePaths, &str) -> Result<Option<ArticleMemoryRecord>>` — introduced T1a, used T1c, T1f.
- `add_article_memory_override(&RuntimePaths, ArticleMemoryAddRequest, &str) -> Result<ArticleMemoryRecord>` — introduced T1e, used T1f.
- `migrate_urls_to_normalized(&RuntimePaths) -> Result<usize>` — introduced T1g, called in T1g from `init_article_memory`.
- `merge_duplicate_urls(&RuntimePaths) -> Result<usize>` — introduced T1h, called in T1h from `init_article_memory`.
- `imessage_send::notify_user(&str, &str, &[String]) -> Result<()>` — introduced T2a, called in T2d.
- `reply_text::build_reply_text(&IngestJob, Option<&str>) -> String` — introduced T2c, called in T2d.
- `reply_text::humanize_issue_type(&str) -> &'static str` — introduced T2c, used inside `build_reply_text` same task.
- `IngestWorkerDeps.imessage_config: Arc<ImessageConfig>` — introduced T2d, wired T2e.

No drift.

---

**Execution handoff:**

Plan complete and saved to `docs/superpowers/plans/2026-04-24-article-memory-phase2-skill-and-dedup.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?


