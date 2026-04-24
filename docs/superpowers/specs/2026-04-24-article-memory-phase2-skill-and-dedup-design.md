# Article Memory × crawl4ai — Phase 2 Skill + Dedup + iMessage Notify

**Status:** Approved for implementation planning (2026-04-24)
**Branch:** `docs/crawl4ai-plan-slim` (local-only, no push/PR per standing user directive)
**Predecessor:** `2026-04-24-article-memory-crawl4ai-ingest-design.md` (Phase 1, Landed)
**Audience:** Implementation-plan author next, implementers after

---

## 1. Goal

Close three gaps left after Phase 1 landed the async URL ingest pipeline:

1. **LLM-agent integration** — iMessage conversations reach this daemon through ZeroClaw's LLM tool-calling path, not through direct HTTP. Phase 1 shipped `/article-memory/ingest` without updating the skill-tool contract, leaving a silent protocol mismatch. Phase 2 fixes the `project-skills/article-memory/` surface so ZeroClaw's LLM can correctly submit, query, and refresh ingests.
2. **Article-level URL dedup with refresh intent** — Phase 1 dedup only covers a 24-hour window inside `IngestQueue`; the underlying `ArticleMemoryIndex` accepts duplicate records of the same URL across window boundaries because `article_id = sha256(title||url||captured_at)` rotates per submission. Phase 2 adds article-level dedup backed by a new `find_article_by_normalized_url`, exposes a `force: bool` bypass, and makes `force=true` update-in-place (same `article_id`).
3. **iMessage completion push** — For jobs submitted through the iMessage-LLM path, the daemon proactively notifies the user on success / rejection / failure. ZeroClaw has no outbound HTTP interface for iMessage, so the daemon shells out to `osascript` itself.

## 2. Non-goals

- Modifying ZeroClaw source (open-source dependency; strict no-fork policy).
- LAN/remote access to the daemon (deferred to future Cloudflare tunnel / Tailscale work).
- Modifying `shortcuts/叫下戴维斯.shortcut.json`. It is already a generic input funnel — every Shortcut input flows through ZeroClaw LLM, which then reaches skill tools. Treating Shortcut as URL-only would break its generality.
- URL-sniff or keyword whitelisting inside `shortcut_bridge`. Natural-language intent detection is LLM's job, not ours.
- Group-chat iMessage notifications. ZeroClaw's `IMessageChannel::send()` rejects any target that is not `+E.164` or email (`crates/zeroclaw-channels/src/imessage.rs:116-151`); we match that constraint.
- Automatic retry of failed jobs. Carried over from Phase 1 design.
- Article history/versioning. `force=true` is update-in-place; old content is overwritten.

## 3. Constraints

- Standing user directive: "永远只 commit，不push，不提pr" — all work stays on `docs/crawl4ai-plan-slim`.
- Zero ZeroClaw code changes. Every integration goes through configuration files (`[[mcp.servers]]`, `project-skills/*/SKILL.toml`) that ZeroClaw reads from the runtime workspace.
- Daemon stays single-host (no multi-daemon coordination).
- iMessage send from the daemon runs on macOS only; non-macOS builds get a stub so Linux CI compiles and tests pass.
- Must not break Phase 1 behavior for existing callers (CLI, direct HTTP, cron).

## 4. Architecture

### 4.1 Two ingress paths, one daemon

```
                  ┌──────────────────────────────────────────┐
                  │  External channels                        │
                  │                                           │
                  │  iMessage (natural language) ─┐           │
                  │                                │           │
                  │  Shortcut (generic text) ─────┤           │
                  │                                │           │
                  │  Webhook ─────────────────────┘           │
                  └───────────────────────┬──────────────────┘
                                          │
                                          ▼
                               ┌──────────────────┐
                               │   ZeroClaw       │
                               │   LLM agent      │
                               │   + skill tools  │
                               └──────┬───────────┘
                                      │ article-memory__ingest_status
                                      │ article-memory__ingest_list
                                      │ http_request (POST ingest)
                                      ▼ HTTP
      ┌─────────────────────────────────────────────────────┐
      │  daemon :3010 (127.0.0.1)                            │
      │                                                       │
      │  POST /article-memory/ingest                         │
      │     body: {url, force?, tags?, source_hint?,         │
      │            reply_handle?, title?}                     │
      │  GET  /article-memory/ingest/:job_id                 │
      │  GET  /article-memory/ingest                          │
      │                                                       │
      │     │                                                 │
      │     ▼                                                 │
      │  IngestQueue ──┬─ Rule 0 (NEW): article-level dedup │
      │                ├─ Rule 1: active job dedup           │
      │                └─ Rule 2: 24h saved dedup            │
      │                                                       │
      │     worker pool ◀── force? branch                    │
      │        │                                              │
      │        ├─ add_article_memory (new article)           │
      │        └─ add_article_memory_override (reuse id)     │
      │                                                       │
      │     on terminal state:                                │
      │        imessage_send::notify_user(handle, text, ...)  │
      └─────────────────────────────────────────────────────┘
                                      │
                                      ▼
      ┌─────────────────────────────────────────────────────┐
      │  Direct callers (bypass LLM, already Phase 1)         │
      │                                                       │
      │  CLI: daviszeroclaw articles ingest submit|history|show│
      │  curl POST /article-memory/ingest                     │
      │  cron / systemd timer                                 │
      └─────────────────────────────────────────────────────┘
```

### 4.2 Why skill, not MCP

Both are exposed to ZeroClaw. Skill tools (`project-skills/*/SKILL.toml` → synced to runtime workspace by `sync_runtime_skills`) register as LLM callable tools via `zeroclaw-runtime::tools::skill_http`. MCP servers register via `[[mcp.servers]]` and ZeroClaw's MCP client.

For our case both could work, but skill wins because:

- Every other daemon capability (HA control, HA audit, article search) already uses skills; precedent is strong.
- Skill `kind = "http"` is declarative — no new process.
- `http_request` skill prose already covers POST + body; we reuse that pattern for `ingest` (body schema too dynamic for `{{arg}}` template).
- GET endpoints fit `kind = "http"` + query-param template cleanly.

### 4.3 `force=true` update-in-place semantics

Default (`force=false`): `Rule 0` rejects with `ArticleExists` if URL already in store; LLM tells user "《title》已经收藏过了". No crawl, zero cost.

`force=true`: bypass `Rule 0`; worker re-crawls; if `find_article_by_normalized_url` hits on entry, use `add_article_memory_override(existing_id)` — overwrite title/captured_at/content/summary/embedding while keeping `article_id` stable. Search index stays single-record-per-URL. One URL → one canonical record, mutable via `force`.

### 4.4 iMessage notify path

```
worker finishes job
  │
  ├─ load job from queue
  ├─ if job.reply_handle.is_some():
  │    text = build_reply_text(job)
  │    imessage_send::notify_user(handle, text, allowed_contacts)
  │      ├─ handle ∈ allowed_contacts? (defense in depth)
  │      ├─ is_valid_target(handle)? (+E.164 or email)
  │      ├─ escape_applescript(text)
  │      └─ osascript -e 'tell application "Messages" ... send ... to buddy ...'
  │
  └─ log warn on send failure; do NOT change job state
```

Failure of the notify step never alters job outcome; the article is already saved.

## 5. Module layout

### 5.1 New files

| File | Purpose | Size |
|---|---|---|
| `src/imessage_send.rs` | osascript-based iMessage send with allowlist recheck + macOS/non-macOS cfg split | ~100 LOC |
| `tests/rust/imessage_notify.rs` | Unit tests for text builder + target validator + allowlist gate (stub path on non-macOS) | ~120 LOC |

### 5.2 Modified files

| File | Change | Est. LOC |
|---|---|---|
| `src/article_memory/mod.rs` | `find_article_by_normalized_url`, `add_article_memory_override`, `migrate_urls_to_normalized` | +80 |
| `src/article_memory/ingest/types.rs` | `IngestRequest.force`, `IngestRequest.reply_handle`, `IngestJob.reply_handle`, `IngestSubmitError::ArticleExists` | +25 |
| `src/article_memory/ingest/queue.rs` | `submit()` Rule 0 gate | +30 |
| `src/article_memory/ingest/worker.rs` | force branch + notify hook + `build_reply_text` + `humanize_issue_type` + `IngestWorkerDeps.imessage_config` | +80 |
| `src/server.rs` | map `ArticleExists` to 409 + action payload | +15 |
| `src/local_proxy.rs` | pass `imessage_config` to worker pool | +5 |
| `src/lib.rs` | `pub mod imessage_send;` | +1 |
| `src/model_routing.rs` | (conditional — only if T4 verify reveals `[article_memory.ingest]` is dropped by render) | ≤20 |
| `docs/superpowers/specs/2026-04-24-article-memory-crawl4ai-ingest-design.md` | line-level revisions at 12/43/527/616 + addendum | +30 lines prose |
| `project-skills/article-memory/SKILL.toml` | `[[tools]] ingest_status`, `[[tools]] ingest_list`, rewrite `[skill].prompts` | +60 |
| `project-skills/article-memory/references/article_memory_api.md` | Strip browser-bridge section (lines 60-89); rewrite "URL Ingest" section for crawl4ai contract | -30, +60 |

Total: ~300 Rust implementation, ~200 test, ~150 doc/config.

## 6. Data shapes

### 6.1 `IngestRequest`

```rust
pub struct IngestRequest {
    pub url: String,
    #[serde(default)]
    pub force: bool,
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub source_hint: Option<String>,
    pub reply_handle: Option<String>,   // "+86..." or "user@icloud.com"
}
```

`force` and `tags` have serde defaults so older callers (CLI, cron) are not forced to emit the new fields.

### 6.2 `IngestJob`

Adds `reply_handle: Option<String>` to persist the handle across daemon restart. Written to `ingest_jobs.json`; older on-disk jobs missing the field deserialize to `None` (serde default).

### 6.3 `IngestSubmitError` new variant

```rust
ArticleExists {
    existing_article_id: String,
    title: String,
    url: String,
}
```

### 6.4 HTTP error body for `ArticleExists`

Status `409 CONFLICT`, body:

```json
{
  "error": "article_exists",
  "existing_article_id": "bd17191b9eff47ba",
  "title": "LLM Powered Autonomous Agents | Lil'Log",
  "url": "https://lilianweng.github.io/posts/2023-06-23-agent/",
  "action": "resubmit with \"force\": true to re-crawl and update"
}
```

The `action` field is intentionally human-readable English — LLM copies it verbatim if in doubt.

## 7. Algorithms

### 7.1 `submit()` rule order (Phase 2)

1. Validate URL (scheme, length, SSRF guard) — unchanged from Phase 1.
2. `normalized = normalize_url(url)`.
3. **Rule 0 (NEW):** if `!req.force`, call `find_article_by_normalized_url(paths, &normalized)`:
   - Some(record) → return `ArticleExists` → 409 path.
   - None → continue.
4. Rule 1 (unchanged): if any active job with matching `normalized_url`, return existing `job_id` + `deduped=true` (202 idempotent replay).
5. Rule 2 (unchanged): if any Saved job with matching `normalized_url` within `dedup_window_hours`, return `DuplicateSaved` → 409.
6. Persist new job, notify worker.

`force=true` skips Rule 0 but **respects Rules 1 and 2**. This prevents two concurrent `force` requests from spawning duplicate Chromium tasks and blocks accidental "just saved, immediately re-force" spam.

### 7.2 Startup URL migration

```rust
pub fn migrate_urls_to_normalized(paths: &RuntimePaths) -> Result<usize> {
    let mut index = load_index(paths)?;
    let mut changed = 0;
    for article in &mut index.articles {
        if let Some(url) = &article.url {
            let normalized = normalize_url(url).unwrap_or_else(|_| url.clone());
            if normalized != *url {
                article.url = Some(normalized);
                changed += 1;
            }
        }
    }
    if changed > 0 {
        save_index_atomic(paths, &index)?;
    }
    Ok(changed)
}
```

Called once at `init_article_memory` end. Idempotent (normalize is a fixpoint). Logs count at `info` when `changed > 0`.

### 7.3 Startup duplicate merge

After `migrate_urls_to_normalized`, scan for records sharing a `normalized_url` and merge using:

**Winner selection (Q11.1 = D):**
1. Higher `value_score` wins.
2. Tie → later `captured_at` wins.
3. Tie → first-seen wins (stable).

**Loser cleanup (Q11.2 = A, no backup):**
- Remove loser from `index.articles`.
- Delete `{loser_id}.md`, `{loser_id}.summary.md`, `{loser_id}.bin` from disk.
- Persist index atomically (tempfile + atomic rename; inherited from Phase 1).
- Log `info` per merge: `{kept_id, dropped_id, normalized_url, reason}`.

Merge runs once at startup; no automatic re-run. If a merge fails mid-batch (e.g., delete error), the index write happens at end so partial state is rolled back; deletion of the orphaned files can be retried on next startup.

### 7.4 `force=true` worker path

```rust
let page = crawl4ai_crawl(...).await?;
let normalized = normalize_url(&job.url)?;

let target_id = if job.force {
    find_article_by_normalized_url(paths, &normalized)?.map(|r| r.id)
} else {
    None
};

match target_id {
    Some(id) => add_article_memory_override(paths, content, metadata, &id)?,
    None     => add_article_memory(paths, content, metadata)?,
}
```

`add_article_memory_override` reuses `id`, overwrites `title/captured_at/content_path/summary_path/value_score`, rewrites `{id}.md`, `{id}.summary.md`, `{id}.bin` in place, and re-runs `save_index_atomic`. Non-index files are written directly (their path is deterministic from `id`); index is the atomic source of truth.

### 7.5 Reply text synthesis

```
Saved     → "已保存《{title}》"            (title from article record, falls back to URL)
Rejected  → "内容价值不高，已略过"
Failed    → "抓取失败：{human_reason}\n{url}"

human_reason by issue_type (stable strings from Phase 1):
  crawl4ai_unavailable → "抓取服务暂时不可用，请稍后再试"
  auth_required        → "需要登录才能访问，请登录后再发"
  site_changed         → "页面结构无法识别，可能需要更新策略"
  empty_content        → "抓到的内容太短（可能是登录墙或 404）"
  pipeline_error       → "内部处理出错"
  default              → "未知错误：请查看 articles ingest show"
```

The `ArticleExists` dedup case is **not** produced by worker; it is surfaced by `submit()` via 409. LLM forms the "已经收藏过了" reply from the HTTP response, not from worker-driven osascript.

### 7.6 iMessage send

```rust
fn is_valid_target(handle: &str) -> bool {
    let phone = Regex::new(r"^\+\d{7,15}$").unwrap();
    let email = Regex::new(r"^[^@\s]+@[^@\s]+\.[^@\s]+$").unwrap();
    phone.is_match(handle) || email.is_match(handle)
}

fn escape_applescript(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', r#"\""#)
}

pub async fn send_imessage(handle: &str, text: &str) -> Result<()>;

pub async fn notify_user(handle: &str, text: &str, allowed: &[String]) -> Result<()> {
    if !allowed.iter().any(|c| c == handle) {
        tracing::warn!(handle, "reply_handle not in allowed_contacts; skipping");
        return Ok(());
    }
    send_imessage(handle, text).await
}
```

Non-macOS builds expose a stub `notify_user` that logs at debug and returns `Ok(())`, enabling Linux CI to compile and unit-test the text-building logic.

## 8. Skill surface

### 8.1 New `[[tools]]` entries

```toml
[[tools]]
name = "ingest_status"
description = """
查询 URL 抓取任务的当前状态。返回 status ∈ {pending, fetching, cleaning,
judging, embedding, saved, rejected, failed}，以及 article_id (若已存)、
error (若失败)、outcome 字段。用户问"那篇存好了吗"时调用。
"""
kind = "http"
command = "GET http://127.0.0.1:3010/article-memory/ingest/{{job_id}}"
[tools.args]
job_id = { type = "string", required = true, description = "submit 时返回的 UUID" }

[[tools]]
name = "ingest_list"
description = """
列出最近的抓取任务。可按状态过滤（status=failed 等）。用于用户问"最近
哪些抓取失败了"、"今天存了几篇"等复盘类问题。
"""
kind = "http"
command = "GET http://127.0.0.1:3010/article-memory/ingest?status={{status}}&limit={{limit}}"
[tools.args]
status = { type = "string", required = false, description = "pending|saved|failed 等" }
limit  = { type = "integer", required = false, description = "默认 20，最大 200" }
```

### 8.2 Rewritten `[skill].prompts`

```toml
prompts = [
  # save intent
  """
  用户发来一段纯 URL（或"存一下 <url>"、"收藏 <url>"等保存意图）：
  - 用 http_request POST 到 http://127.0.0.1:3010/article-memory/ingest
  - body: {"url": "<url>", "source_hint": "<channel>",
           "reply_handle": "<sender 或 null>", "tags": []}
  - channel: iMessage → "imessage"；webhook/Shortcut → "shortcut"
  - iMessage 场景 reply_handle 必填为 sender，daemon 完成后会主动推送
    通知；其他 channel 不填（null）
  - 202 + {job_id, status: "pending"} → 告诉用户"已收到，开始抓取"
  - 409 + {error: "article_exists", title, existing_article_id} →
    告诉用户"《title》已经收藏过了，需要刷新吗"
  - 409 + {error: "duplicate_recent", ...} →
    告诉用户"最近已经存过，请稍后再试"
  - 503 + {error: "persistence_degraded", ...} →
    告诉用户"daemon 存储出问题，请联系管理员处理磁盘"
  """,
  # refresh intent
  """
  用户表达"帮我刷新 <url>"、"这篇文档有没有更新"、"重抓一下"等意图：
  - 同上，body 里追加 "force": true
  - 409 ArticleExists 仅在 force=false 时出现；force=true 时跳过 article
    级去重，直接重抓并覆盖原记录（article_id 保持不变）
  """,
  # query intent
  """
  用户问"那篇存好了吗"、"最近失败了几篇"等复盘类问题：
  - article-memory__ingest_status 查具体 job
  - article-memory__ingest_list 配合 status=failed 列失败项
  """,
  # retrieval intent (preserved from Phase 1)
  """
  article-memory__search: 检索已存文章
  article-memory__list:   列最近几篇
  article-memory__status: 系统健康
  """,
]
```

### 8.3 `references/article_memory_api.md`

Delete existing "Ingest Browser Page" section (lines 60–89 of current file). Replace with a "URL Ingest (crawl4ai-backed, async)" section documenting the actual Phase 1 + Phase 2 contract:

- Request body schema `{url, tags?, title?, source_hint?, force?, reply_handle?}`
- Response envelopes: 202 (queued), 409 (three variants: `article_exists`, `duplicate_recent`, `duplicate_active_job`), 503 (`persistence_degraded`), 400 (invalid URL / SSRF).
- Asynchrony contract: 202 means queued; use `ingest_status` for real result.
- iMessage notify trigger: `source_hint == "imessage"` AND `reply_handle ∈ allowed_contacts`.
- `force=true` semantics: update-in-place, `article_id` stable.

Status/list/search sections stay.

## 9. HTTP surface

No new routes; all additions are request/response-body shape changes.

| Route | Change |
|---|---|
| `POST /article-memory/ingest` | body adds `force`, `reply_handle` |
| `GET /article-memory/ingest/:job_id` | response body includes `reply_handle` |
| `GET /article-memory/ingest` | no change |

## 10. Concurrency + persistence

No new concurrency invariants. `force=true` path inherits the Phase 1 per-profile Mutex (`Crawl4aiProfileLocks`); `add_article_memory_override` uses the same `save_index_atomic` pattern (tempfile + fsync + atomic rename) as `add_article_memory`.

Startup migration and merge run before the worker pool starts (in `init_article_memory`), so they cannot race with in-flight jobs.

## 11. Observability

`tracing::instrument` is added to:
- `find_article_by_normalized_url(paths, normalized_url)` — `found` boolean in span
- `add_article_memory_override(id)` — `id` in span
- `migrate_urls_to_normalized` — `count` after completion
- `notify_user(handle)` — `handle` (redacted to last 4) + `skipped_reason` (if any)

Existing worker `execute_job` instrument already covers force branch.

CLI `articles ingest show <id>` prints `reply_handle` when present.

## 12. Testing

### 12.1 T1 dedup + force (Rust integration, ~100 LOC test)

- `submit_without_force_rejects_when_article_exists` — Rule 0 returns 409 `ArticleExists`.
- `submit_with_force_bypasses_article_dedup` — force=true, job enters Pending.
- `submit_with_force_still_blocked_by_active_job` — force=true + active job → Rule 1 dedup.
- `submit_with_force_still_blocked_by_recent_saved` — force=true + saved within window → Rule 2 dedup.
- `worker_force_reuses_existing_article_id` — original id `aaa`, after force the id is still `aaa`, content updated.
- `worker_force_overwrites_all_files` — `{id}.md`, `{id}.summary.md`, `{id}.bin` mtimes change; contents reflect new crawl.
- `find_by_normalized_url_matches_regardless_of_trailing_slash` — `/post` and `/post/` resolve to same record.
- `article_id_stable_across_force_reingest` — 2 consecutive forces, `articles.len()` unchanged, id unchanged.
- `startup_migration_normalizes_legacy_urls` — seed index with raw URLs → migrate runs → all `url` fields normalized.
- `startup_merge_picks_higher_value_score` — seed 2 records same normalized_url, score 0.9 vs 0.7 → 0.9 kept, 0.7 id + files gone.
- `startup_merge_tiebreaks_by_captured_at` — same score, later captured_at wins.
- `startup_merge_is_idempotent` — run merge twice, second pass no-op.

### 12.2 T2 iMessage notify (Rust unit + stubbed integration, ~120 LOC test)

- `build_reply_text_saved_uses_title` / `falls_back_to_url` — title resolution.
- `build_reply_text_rejected` / `failed_includes_url` — template correctness.
- `humanize_issue_type_handles_all_stable_strings` — 5 stable types + default.
- `is_valid_target_accepts_phone_and_email` / `rejects_thread_id` — target validation.
- `escape_applescript_handles_quotes` / `preserves_cjk_and_emoji` — string safety.
- `notify_user_skips_when_not_in_allowlist` — allowlist recheck returns Ok + warn log.
- `notify_user_stub_always_ok_on_non_macos` — cfg branch protects CI.
- `worker_triggers_notify_on_saved` — mock `send_imessage`, assert call with expected text.
- `worker_triggers_notify_on_failed_with_url` — assert URL in text body.
- `worker_skips_notify_when_reply_handle_none` — CLI/cron submits without handle → no send.

### 12.3 T3 skill (manual verification, no Rust test)

Post-implementation: run `daviszeroclaw start`, grep `.runtime/davis/workspace/skills/article-memory/SKILL.toml` for the two new `[[tools]]` entries. Feed a sample URL through iMessage and observe ZeroClaw's LLM calls the right tool.

### 12.4 T4 render verification (manual check in plan)

`./target/release/daviszeroclaw start` → `rg '\[article_memory' .runtime/davis/config.toml`. Decide whether to extend `render_runtime_config_str` based on whether ZeroClaw actually needs any ingest config field. Most likely outcome: ZeroClaw needs nothing from that section; add a one-line comment in `config.toml` template noting the section is daemon-only.

## 13. Backward compatibility

- Existing CLI callers (`daviszeroclaw articles ingest submit <url>`) don't emit `force` or `reply_handle` → serde defaults handle it. No behavior change.
- Existing HTTP callers (curl, cron) unchanged.
- Phase 1 `ingest_jobs.json` on disk missing `reply_handle` → serde default → `None` after reload.
- Phase 1 article records with non-normalized URLs → migrated once at startup; subsequent submits correctly dedup.
- Phase 1 stale SKILL.toml body schema → rewritten; ZeroClaw LLM picks up new contract on next sync (happens every `daviszeroclaw start`).

## 14. Rollback

- T1 + T2 are additive at the request-shape level. `force` defaults to false, `reply_handle` defaults to None → removing callers that set them is safe.
- Database: startup merge deletes data. **No rollback path beyond git revert of the code + restoring `articles.json` from backup.** User explicitly accepted no pre-merge backup (Q11.2 = A). If user changes mind later, rollback strategy becomes "don't run migration on next startup" — code path gated by config flag; not added now per YAGNI.
- Code: git revert 23+ commits makes the branch identical to Phase 1 head (c8dafcd).

## 15. Follow-ups (not Phase 2 scope)

1. **Remote/LAN reach** via Cloudflare tunnel or Tailscale — deferred.
2. **ZeroClaw-side article-refresh intent as a classifier hint** — out-of-scope because we don't modify ZeroClaw.
3. **Article history versioning** — deliberately avoided; `force` overwrites.
4. **Group-chat iMessage notifications** — blocked by ZeroClaw's target validator; would require ZeroClaw changes.
5. **CLI `articles ingest force-all`** for bulk refresh — nice-to-have, not requested.
6. **Re-enable full daemon startup with the real `check_imessage_permissions` preflight** — user's smoke-test local skip at `src/cli/service.rs:35-39` was reverted after Phase 1 smoke; still in place for Phase 2 smoke if the user chooses.

## 16. Open questions

None at spec-close. All Q1–Q12 + Q11.1 + Q11.2 answered during brainstorm.

## 17. Glossary

- **skill tool** — A `[[tools]]` entry in `project-skills/*/SKILL.toml`, exposed to ZeroClaw's LLM as a callable named `<skill>__<tool>` with a JSON parameter schema.
- **reply_handle** — The iMessage sender's phone (`+E.164`) or email, forwarded by LLM from the `sender` field of the incoming message so the daemon knows where to send completion notification.
- **force=true** — Request flag that bypasses article-level dedup; worker re-crawls and overwrites the existing record with the same `article_id`.
- **ArticleExists** — New 409 response variant: the URL is already saved; resubmit with `force=true` to refresh.
- **article-level dedup** — `find_article_by_normalized_url` in `ArticleMemoryIndex`, distinct from Phase 1's `IngestQueue` 24h window dedup.
- **update-in-place** — `add_article_memory_override` semantics: reuse `article_id`, rewrite content/summary/embedding files, single canonical record per URL.
