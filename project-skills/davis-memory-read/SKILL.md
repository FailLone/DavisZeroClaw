---
name: davis-memory-read
description: Use when the user asks about past Davis-generated state — "上周哪个灯坏了", "最近 lobste.rs 进了什么", "这条规则什么时候上线的", "ingest worker 今天卡过吗", "那个坏掉的灯应该换哪个", "客厅以前都有哪些设备", "哪些设备名字有问题", "这篇文章之后谁跟进了". Subject of the question is a Davis-tracked entity (HA device, article, rule, worker, component), not a user preference. Do NOT use for user personal facts (use mempalace-memory) or MemPalace maintenance (use the vendor mempalace skill).
---

# Davis Memory Read

Davis populates MemPalace with a fixed vocabulary of drawers, KG triples, and agent diaries every time HA refreshes, an article ingests, a rule lands, or a subsystem flips health state. This skill teaches you which tool to call and with which arguments to answer the user's recall-style question.

**Scope:** read-only queries against Davis's projections. Writes are fire-and-forget from Davis itself — never call MemPalace write tools from this skill.

## When to use

The user is asking about the **past** — something Davis already observed and projected. Look for time markers like "上次 / 上周 / 之前 / 最近 (+ 过去时) / 那次 / 曾经". Examples:

- "客厅主灯上次坏是什么时候" — past state transitions.
- "最近哪些设备不可用过" — historical `has_state` scan.
- "那次 XX 坏了之后我换成啥用了" — historical replacement.
- "之前存过讲 async rust 的文章吗" — article recall.
- "最近都从哪些网站存过东西" — article host aggregation.

## When NOT to use

**Critical boundary — most look-alike questions are actually current-state questions, not history.**

| User asks | Actual intent | Correct skill |
|---|---|---|
| "客厅有哪些灯开着" | Current live state | `ha-control` / live HA query |
| "客厅的灯怎么没反应" | Current troubleshooting | `ha-control` first; maybe this skill as step 2 |
| "客厅都有啥设备" | Ambiguous — usually means **now** | `ha-control`; use this skill only if user says "以前" / "登记过" |
| "我刚存的那篇怎么样了" | Current ingest job status | Davis HTTP `/article-memory/ingest/:job_id` |
| "现在有啥文章 / 有哪些书签" | Current library inventory | Davis HTTP `/article-memory/articles` |

Also do not use for:
- User's **own** preferences / facts — use `mempalace-memory`.
- MemPalace **maintenance** (setup / install / audit) — use the vendor `mempalace` skill.
- When Davis hasn't been running long enough to have projections — acknowledge the gap; do not fabricate.

**Rule of thumb**: if the answer depends on what things look like *right now*, it is not this skill. This skill is for **closed-window** facts Davis already wrote down.

## Davis projection surface (what is available to query)

**Drawer wings** (use `mempalace_search` with these exact `wing` values):

| Wing | What Davis writes there |
|---|---|
| `davis.articles` | Per-article compressed value-report drawer (title, URL, topics, decision, score). `room = <top-topic-slug>`. |
| `davis.ha` | Per-area HA findings narrative drawer (bad names, missing area, duplicates, replacement hints). `room = <area-slug>`. |

**Agent diaries** (use `mempalace_diary_read` with these exact `agent_name` values — NOT `mempalace_search`):

| Agent name | What Davis writes there |
|---|---|
| `davis.agent.ingest` | One line per saved/rejected article ingest job. |
| `davis.agent.ha-analyzer` | One line per HA live-context refresh (counts + top finding). |
| `davis.agent.rule-learner` | One line per saved/quarantined rule. |

Diary entries live under MemPalace-internal wings like `wing_davis.agent.ingest` (the tool auto-prefixes `wing_` and lowercases). You should NOT try to search those wings directly — `mempalace_diary_read(agent_name="davis.agent.ingest")` is the only supported read path.

**KG predicates** (MemPalace `kg_query` `direction="both"` / `kg_timeline`):

| Predicate | Subject → Object | User question it answers |
|---|---|---|
| `has_state` | entity → state-label | "上周哪个灯坏了" |
| `replacement_for` | entity → entity | "那个坏掉的灯应该换哪个" |
| `located_in` | entity → area | "客厅以前都有哪些设备" |
| `has_name_issue` | entity → issue-tag | "哪些设备名字有问题" |
| `discusses` | article → topic | "哪些文章讲过 async rust" |
| `cites` | article → article | "这篇之后谁跟进了" |
| `sourced_from` | article → host | "最近 lobste.rs 进了什么" |
| `rule_active_for` | host → rule-version | "这条规则改过几次 / 啥时候上的" |
| `rule_quarantined_by` | rule-version → reason-tag | "这条规则为什么不生效" |
| `worker_health` | worker → status | "ingest worker 今天卡过吗" |
| `component_reachability` | component → label | "zeroclaw 最近挂过多久" |

**Entity ID scheme** — Davis writes subjects/objects with these namespace prefixes (separator is `_`):

```
entity_<ha_entity_name>.<domain>     area_<slug>
host_<fqdn>                          article_<article_id>
topic_<slug>                         host_<fqdn>
ruleVersion_<host>.<iso-slug>        provider_<name>
routeProfile_<name>                  budgetScopeDaily_<YYYY-MM-DD>
budgetScopeMonthly_<YYYY-MM>         worker_<name>
component_<name>
```

CJK or otherwise non-ASCII source values (e.g. a Chinese HA device name "客厅主灯") are hashed into `x<sha256-prefix>` when no ASCII survives the slug. If you cannot construct the subject verbatim from the user's phrasing, use `mempalace_kg_timeline` instead of `mempalace_kg_query` and filter client-side.

## Tool selection

All three reads are MCP tools exposed by the `mempalace` server the daemon already has registered. Argument names below are the exact MCP parameter names.

### `mempalace_search` — semantic drawer search

Use for narrative questions where you need the content of a drawer Davis wrote.

- `query` (required): free-text search phrase.
- `wing` (optional but strongly preferred): constrain to `davis.articles`, `davis.ha`, etc.
- `room` (optional): constrain further by slug.
- `limit` (optional, default 5).

```
mempalace_search(query="async rust",
                 wing="davis.articles",
                 limit=5)
```

### `mempalace_kg_query` — exact-subject KG lookup

Use when you know the entity id exactly.

- `entity` (required): the full subject string, e.g. `host_lobste.rs`. **MemPalace lowercases `entity` internally** — pass it in whatever case; the match still hits.
- `as_of` (optional): ISO timestamp for point-in-time queries.
- `direction` (optional, default `"both"`): `"out"` (subject position), `"in"` (object position), `"both"`.

Returns `{entity, facts: [...], count}`. Each fact has `subject`, `predicate`, `object`, `valid_from`, `valid_to`, `current`.

```
mempalace_kg_query(entity="host_lobste.rs",
                   direction="both")
```

### `mempalace_kg_timeline` — chronological KG scan

Use when you do NOT know the exact entity id (e.g. versioned `ruleVersion_*` subjects), or when you want a chronological view.

- `entity` (optional): filter by one entity. Leave empty for a global scan.

Returns `{entity, timeline: [...], count}`. **Timeline items preserve the original case of subject/object** — do case-insensitive matching when you filter client-side.

```
mempalace_kg_timeline(entity="entity_light.livingroom.light")
```

### `mempalace_diary_read` — per-agent diary

Use when the user asks why Davis did something. Davis writes diaries under `davis.agent.ingest`, `davis.agent.ha-analyzer`, `davis.agent.rule-learner`.

## Decision recipes

Each recipe shows the user intent, the first tool call to make, and what to do with the result. Answer succinctly; do not pad the reply with tool-call metadata.

### "上周哪个灯坏了" / "最近什么设备不可用"

1. `mempalace_kg_timeline(entity=null)` — global scan.
2. Filter client-side for facts where `predicate == "has_state"` and `object == "entity_state.unavailable"` or `entity_state.unknown`.
3. Group by `subject`; report the ones whose `valid_from` falls in the user's time window.

### "客厅主灯为什么不可用 / 应该换什么"

1. Try to derive the exact entity id: `entity_<slug-of-name>.<domain>`. If the name is CJK-only, skip to step 3.
2. `mempalace_kg_query(entity=<exact-id>, direction="both")`.
3. If empty: `mempalace_kg_timeline` (no filter), case-insensitively grep facts whose subject contains the device name or tag the user used.
4. Look for `replacement_for` facts pointing AT the unavailable entity (as `object`).

### "客厅以前都有哪些设备" / "卧室现在有什么"

1. Slug the area: "客厅" → the user's area name may or may not have ASCII parts. If it does (e.g. "Living Room" → `living-room`), use `mempalace_kg_query(entity="area_<slug>", direction="in")` — that returns all `entity_* → located_in → area_<slug>` facts.
2. If the area is pure CJK, call `mempalace_kg_timeline(entity=null)` and filter client-side: `predicate == "located_in"` and `object` contains the area's hash prefix or ASCII fragment.
3. For "以前都有" / "历史上" the user wants historical entries too — include facts where `current == false`.
4. Group by `subject`; each is an `entity_<name>.<domain>`.

### "哪些设备名字有问题" / "哪些实体命名乱"

1. `mempalace_kg_timeline(entity=null)`.
2. Filter client-side: `predicate == "has_name_issue"` and `current == true`.
3. `object` is the issue tag (`entity_reason.mixed_cjk_ascii`, `entity_reason.duplicate-name`, etc.); `subject` is the misnamed entity.
4. Supplement with `mempalace_search(query="bad name", wing="davis.ha")` if the user wants the narrative around a finding.

### "哪些文章讲过 async rust"

1. `mempalace_search(query="async rust", wing="davis.articles", limit=5)`.
2. Fall through to `mempalace_kg_timeline` and filter `predicate == "discusses"` only if search is empty.

### "这篇文章之后还有谁跟进了"

1. Extract article id if the user gave a URL: `article_<article_id>`.
2. `mempalace_kg_query(entity=<id>, direction="in")` — pull facts where the article appears as OBJECT in a `cites` triple.

### "最近 lobste.rs 进了什么"

1. `mempalace_kg_query(entity="host_lobste.rs", direction="in")` — facts with `predicate == "sourced_from"` where lobste.rs is the object.
2. Each matched `subject` is an `article_<id>`; cross-reference with `mempalace_search(query="<user hint>", wing="davis.articles")` if the user wants titles.

### Deferred / not-written predicates — refuse politely

These predicates are NOT written by Davis (they live in zeroclaw daemon). If a user asks anything that maps to them, say Davis cannot answer from its own projections and stop; do NOT call MCP tools.

- "那天为什么切到 haiku" → `route_resolved_to`
- "最近超预算过吗" → `budget_event`
- "openrouter / anthropic 挂过吗" → `provider_health`

### Internal-observability recipes (developer-facing, rarely triggered by end-user language)

The following predicates are written by Davis for its own health tracking. End users almost never phrase questions that match them — a typical user does not know Davis has per-host scraping rules or internal workers. Include them for completeness only.

**`rule_active_for` / `rule_quarantined_by`** — "why isn't scraping working on host X"
1. `mempalace_kg_query(entity="host_<host>", direction="out")`, look for `rule_active_for` facts (`current == true` is the live version).
2. Take the rule-version id from the object, then `mempalace_kg_query(entity=<rule-version-id>, direction="out")` and look for `rule_quarantined_by`.
3. If the user phrasing is vague, `mempalace_kg_timeline(entity=null)` + client-side filter on those predicates.

**`worker_health` / `component_reachability`** — "did an internal subsystem flap"
1. `mempalace_kg_query(entity="worker_ingest", direction="out")` or `entity="component_ha-mcp"`.
2. Filter by predicate; `current == true` + unhealthy object = currently degraded.
3. Supplement with `mempalace_diary_read(agent_name="davis.agent.ingest")` for per-job detail.

Currently wired: `worker_ingest`, `component_ha-mcp`. Other workers/components are not yet observed.

## Traps learned the hard way

- **`mempalace_kg_query` parameter is `entity`, not `subject`**. Passing `subject` returns an MCP internal error.
- **`direction` defaults to `"both"`**, so `kg_query` returns facts where the entity is EITHER subject OR object. If you want one side only, pass `"out"` or `"in"`.
- **`kg_query` lowercases on write + read**; you can pass any case. But `kg_timeline` echoes original-case strings — match case-insensitively when filtering client-side.
- **Versioned subjects** like `ruleVersion_host.timestamp-slug` — you cannot reconstruct the exact timestamp-slug from user phrasing. Use `kg_timeline` and filter by predicate + substring of host.
- **CJK entity names** are hashed into `x<sha>` when they have no ASCII component, so a user asking "客厅主灯" may not map to a predictable entity id. Use `kg_timeline` + client-side contains on the ASCII parts (domain/area) when in doubt.
- **Article drawers don't include the raw `article_<id>`.** The drawer body has title, URL, topics, decision, score — but not the article_id string. Don't search `davis.articles` for an article_id; go through the KG (`kg_query(entity=article_<id>, direction="out")`) or search by title/URL/topic instead.
- **Diaries are NOT searchable by wing.** `mempalace_diary_read(agent_name="davis.agent.ingest")` is the only read path. `mempalace_search(wing="davis.agent.ingest")` returns empty because MemPalace stores diaries under `wing_davis.agent.ingest` (auto-prefixed) rather than the bare `agent_name` value.
- **Never invent a fact**. If the tool returns empty and you can't find a relevant drawer, say "no matching projection". Do not guess from general knowledge of HA or from article metadata you may already have in context.
