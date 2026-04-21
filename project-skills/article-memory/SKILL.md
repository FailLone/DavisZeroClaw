---
name: article-memory
description: Store, search, and report on the user's curated article knowledge base through the Davis local article memory API. Use when the user asks Davis to remember useful articles, search saved articles, build a learning library, translate or summarize saved articles, evaluate article quality, or produce article collection reports. Do not use for personal preferences or cross-session personal facts; use mempalace-memory for that.
---

# Article Memory

Article memory is Davis's curated article knowledge base. It is separate from MemPalace personal memory.

Use MemPalace for durable facts about the user, Davis behavior, preferences, corrections, and conversation continuity. Use article memory for article records, source URLs, summaries, translations, value judgments, tags, and daily research reports.

## Use Article Memory When

- The user asks to save, find, summarize, translate, or report on articles.
- The user asks about previously collected research material.
- Davis has found an article that is worth preserving for future learning.
- A daily or scheduled research run needs to store accepted articles and later generate a report.

## Do Not Use Article Memory When

- The user is asking whether Davis remembers a personal fact or preference.
- The content is a secret, API key, password, token, private credential, or one-time code.
- The source is low-value advertising, duplicated content, shallow SEO filler, obvious spam, or materially unreliable.
- The answer can be handled from the current conversation without storing anything.

## Quality Gate

Before saving an article, decide whether it is worth keeping.

Keep articles that have clear authorship or provenance, durable technical insight, original analysis, useful empirical detail, strong references, or a practical workflow worth revisiting.

Reject or skip articles that are mostly promotional, copied, thin, misleading, unverifiable, or only momentarily interesting.

## Workflow

1. Read the API reference.
Use [references/article_memory_api.md](references/article_memory_api.md).

2. Search before adding.
Call `GET /article-memory/search` with the article title, URL, or topic to avoid duplicates.

3. Save deliberately.
Call `POST /article-memory/articles` only after the quality gate passes. Preserve the URL, title, source, language, tags, content, summary, translation when needed, value score, and caveats.

4. Ingest browser pages as candidates.
When the user asks to save or evaluate the current page, call `POST /article-memory/ingest`. This extracts the active browser page, stores it as `candidate` by default, checks duplicates, and indexes embeddings when configured.

5. Answer from stored material only after searching.
For questions about saved articles, call `GET /article-memory/search` first. If no relevant article is found, say that no saved article was found.

6. Prefer semantic search when available.
The search endpoint uses hybrid keyword plus embedding search by default when the semantic index is configured. If `semantic_status` is not `ok`, continue from keyword results and mention only if it affects confidence.

7. Keep boundaries clear.
If saving a durable user preference discovered during article work, use `mempalace-memory`. Do not put user preferences into article memory.

## Status Values

- `candidate`: found but not fully evaluated yet.
- `saved`: accepted into the article knowledge base.
- `rejected`: evaluated and intentionally not kept as useful knowledge.
- `archived`: old or superseded but retained for history.

## Reporting

For a daily article report, search or list recent article memory records, group them by topic, and lead with the most valuable saved articles. Mention skipped or rejected material only when it explains coverage or quality decisions.

## Strategy Review

When the user asks Davis to improve article cleaning, value judging, site-specific extraction quality, or recurring article-memory behavior, treat it as a strategy review.

First generate bounded review context:

`daviszeroclaw articles strategy review-input --recent 20`

The generated review input is the source of truth for what to inspect. It includes clean reports, value reports, validation commands, and the hard edit boundary.

During strategy review:

- Edit only `config/davis/article_memory.toml`.
- Do not edit Rust source, Cargo files, generated article files, or report JSON files.
- Prefer config-only changes: selectors, URL/source patterns, start/end markers, exact or contains noise lines, suffix noise, kept-ratio limits, minimum content length, value thresholds, and target topics.
- After editing, run `daviszeroclaw articles cleaning check`, `daviszeroclaw articles cleaning replay --all`, and the relevant audit commands.
- If the current config fields cannot express the needed behavior, write an implementation request under `.runtime/davis/article-memory/reports/implementation-requests/` instead of modifying Rust. Include affected sites/URLs, report evidence, the missing capability, and a minimal proposed code change.
