# Article Memory API

Base URL: `http://127.0.0.1:3010`

Use only the Davis local proxy for article memory work.

## Status

`GET /article-memory/status`

Returns article memory root paths, counts, tags, languages, and index status.

Use this first when article memory availability is uncertain.

## List

`GET /article-memory/articles?limit=20`

Returns recent article records. Use this for reports or quick inventory.

## Search

`GET /article-memory/search?q=<query>&limit=10`

Searches title, URL, source, tags, notes, content, summary, and translation text. When article embedding is configured, this endpoint performs hybrid keyword plus semantic search by default.

Use this before answering from saved articles and before adding a likely duplicate.

Optional parameters:

- `semantic=false`: force keyword-only search.

Useful response fields:

- `search_mode`: `keyword` or `hybrid`.
- `semantic_status`: `ok`, `embedding_index_missing`, `embedding_index_empty`, or an error status.
- `hits[].semantic_score`: cosine similarity score when semantic search matched.
- `hits[].matched_fields`: includes `embedding` for semantic matches.

## Add

`POST /article-memory/articles`

Body fields:

- `title`: required article title.
- `content`: required article text or extracted markdown.
- `url`: source URL when available.
- `source`: source label such as `manual`, `browser`, `arxiv`, `blog`, `paper`, or a site name.
- `language`: article language such as `en` or `zh`.
- `tags`: reusable topic tags.
- `summary`: concise summary.
- `translation`: translated text when useful.
- `status`: `candidate`, `saved`, `rejected`, or `archived`.
- `value_score`: number from `0.0` to `1.0`.
- `notes`: caveats, uncertainty, or why the article was kept.

Save only material that passed the quality gate in the main skill.

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

When `reply_handle` is set AND the handle is listed in
`config/davis/local.toml` under `[imessage].allowed_contacts`, the daemon
sends a Chinese-language iMessage reply after the job reaches a terminal
state:
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

## Strategy Review CLI

`daviszeroclaw articles strategy review-input --recent 20`

Generates `.runtime/davis/article-memory/reports/strategy/latest.md` with recent clean/value report signals and the strict strategy-review boundary.

Use this before changing article-memory strategy. The reviewer may edit only `config/davis/article_memory.toml`. If the needed behavior cannot be expressed by that config, write an implementation request under `.runtime/davis/article-memory/reports/implementation-requests/`.
