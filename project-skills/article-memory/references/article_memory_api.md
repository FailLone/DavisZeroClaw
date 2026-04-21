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

## Ingest Browser Page

`POST /article-memory/ingest`

Extracts an article-like page through the Davis browser bridge and stores it as article memory. This is the preferred entry point when the user asks to save, capture, or evaluate the current browser page.

Body fields:

- `url`: optional URL to open before extraction.
- `profile`: browser profile, usually `user` or `managed`.
- `tab_id`: optional tab id; omit to use the active tab.
- `new_tab`: open `url` in a new tab when true.
- `source`: optional source label. If omitted, Davis uses the extracted site name.
- `language`: optional language override.
- `tags`: reusable topic tags.
- `status`: defaults to `candidate`; use `saved` only when the article has already passed review.
- `value_score`: optional score from `0.0` to `1.0`.
- `notes`: optional caveats or user context.

Response statuses:

- `ok`: article was extracted and stored.
- `duplicate`: a matching title or URL already exists; do not add another copy.
- `failed`: extraction or browser access failed.

Useful response fields:

- `article`: stored article record when `status` is `ok`.
- `extraction`: title, URL, language, author, site name, description, selector, and content length.
- `embedding_status`: `ok`, `disabled`, or an error string.

## Strategy Review CLI

`daviszeroclaw articles strategy review-input --recent 20`

Generates `.runtime/davis/article-memory/reports/strategy/latest.md` with recent clean/value report signals and the strict strategy-review boundary.

Use this before changing article-memory strategy. The reviewer may edit only `config/davis/article_memory.toml`. If the needed behavior cannot be expressed by that config, write an implementation request under `.runtime/davis/article-memory/reports/implementation-requests/`.
