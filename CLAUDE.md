# DavisZeroClaw — Architectural Invariants

End users install **Davis**. They don't know zeroclaw exists, and they don't care. Every architectural decision flows from this.

## Product positioning

- **Davis is a standalone product.** It has its own binary, its own HTTP server, its own storage, its own launchd service.
- **zeroclaw is a private engine** Davis launches as a subprocess. It is an implementation detail, not a dependency users can see.
- **Never modify zeroclaw source.** If zeroclaw lacks something Davis needs, work around it in Davis — don't patch upstream in-tree, don't vendor patches.
- **Never depend on zeroclaw as a Cargo crate** (no `path`, no `git` dep). Its 0.x churn must not leak to end users' upgrade path.
- Davis ↔ zeroclaw coupling lives in exactly two places: (1) the rendered zeroclaw `config.toml` (`model_routing.rs`), (2) the subprocess invocation (`Command::new("zeroclaw")`). Both are stable surfaces.

## What looks like duplication but is not

Apparent overlaps with zeroclaw are deliberate product-independence choices. Do not "fix" them:

| Davis module | Why it's not duplication |
|---|---|
| `llm_client.rs` (direct OpenRouter) | Internal batch jobs. Avoids coupling Davis reliability to zeroclaw's HTTP protocol stability. |
| `article_memory/` (own SQLite + embeddings + FTS) | Hot-path batch processing. Davis's data durability must not depend on a running zeroclaw daemon. |
| `ha_mcp.rs` | HA's native MCP output is weakly-structured markdown; agents underperform on it. The ~1400 LOC of parse/findings/replacement-inference/snapshot-diff is Davis's core product differentiation. Not a redundant MCP client. |
| `server.rs` | Davis is a standalone product; it must have its own HTTP frontend. |
| `crawl4ai.rs` + supervisor | Headless browser / login-state crawling. zeroclaw `web_fetch` is GET-only + Firecrawl SaaS. |
| `article_memory/ingest/queue + worker` | Davis business loop. Also zeroclaw cron has no `JobType::Custom`. |
| `model_routing.rs` | Not a runtime router — it's a **config renderer** that patches zeroclaw's `config.toml` from Davis's `local.toml`. Different abstraction layer. |
| `article_memory/translate` (inline zeroclaw HTTP client) | Davis keeps a private, module-scoped HTTP client to zeroclaw `/api/chat`. This is **not** a general-purpose `zeroclaw_client` shared with hot-path callers — hot-path stays direct-OpenRouter per CLAUDE.md. The inline client exists only because translation is non-hot-path and benefits from zeroclaw's failover/budget. Hot-path and enhancement callers have opposite failure semantics (see `docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md` §"Anchor decisions" A1/A3), so they intentionally do not share a dispatcher. |

If a future reviewer re-raises "Davis duplicates zeroclaw" — point them here first.

## When changing Davis

- Prefer editing existing files; new files only when a module's cohesion demands it.
- Target file size ≤ 800 lines; split if larger.
- No `#[allow(dead_code)]`. If it's not used, delete it. "Future use" is not a reason.
- TDD for new features and bug fixes. Run `cargo test --lib`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check` before claiming done.
- Python side exists for one reason: browser-layer automation (Chromium / Playwright / Puppeteer-style DOM operation, HTML extraction). Anything that does not need a browser belongs in Rust. Two adapters live there today:
  - `crawl4ai_adapter/` — article crawling: crawl4ai pruning + trafilatura + learned-rules CSS extraction.
  - `router_adapter/` — LAN device admin pages where the device only exposes a browser UI. Playwright-driven only; if a device offers a direct API, Davis talks to it from Rust.

  All LLM calls stay in Rust (`src/article_memory/llm_client.rs`). New Python adapters are admissible only if they require a browser; otherwise the work goes in Rust.

## Runtime topology

```
davis-local-proxy (3010)   ← agent-facing, the "Davis" surface
davis-ha-proxy / shortcut (3012)   ← /shortcut (inbound) + /shortcut/reply (zeroclaw→Davis callback, sync reply channel)
zeroclaw daemon (3000/3001) ← subprocess, launched and supervised by Davis
```

Davis writes zeroclaw's config (providers, routes, classifications, MCP servers), launches it, restarts it on route changes, and calls it over localhost. The user sees only Davis's ports.

## Storage & memory systems

Davis keeps five independent memory subsystems. All primary data stays on local disk; MemPalace is a **projection layer**, not a store of record.

| Subsystem | Path | Owner | Purpose |
|---|---|---|---|
| `article_memory` | `{runtime}/article-memory/` | Davis | Article ingest, embeddings, learned rules, samples |
| HA live-context snapshot | `{runtime}/state/ha_mcp_live_context.json` | Davis | Previous snapshot for diff-against-now |
| Model routing state | `{runtime}/state/model_{scorecard,route_plan,route_history,runtime_observations}.json` | Davis | Per-provider/model counters + plan |
| HA MCP capabilities cache | `{runtime}/state/ha_mcp_capabilities.json` | Davis | Tool/prompt list per HA server |
| MemPalace | `{runtime}/mempalace/` | External (Python MCP) | Semantic index + KG + per-agent diary |

**Data classification (decides where anything new goes):**

| Shape | Home |
|---|---|
| Raw payload / hot write path / primary durability | Davis local JSON |
| Current-state snapshot ("what does it look like now") | Davis local JSON |
| High-frequency counters / metrics | Davis local JSON |
| Time-series events ("X true from T1 to T2") | MemPalace KG |
| Semantically searchable fragments | MemPalace drawer |
| Agent decision narrative | MemPalace diary (per-agent wing) |
| Cross-entity relations | MemPalace KG |

**Hard rule:** MemPalace is always a projection. If MemPalace is unreachable, every Davis subsystem must still run; only read-back ("what broke last week") degrades.

## MemPalace integration plan

### Write path

Davis runs a long-running Rust bridge that speaks MCP over stdio to MemPalace's official Python MCP server (`python -m mempalace.mcp_server`, already installed in `{runtime}/mempalace-venv/`). No new Python code; no dependency on zeroclaw daemon.

- New Rust module: `src/mempalace_sink.rs` — spawns child process, owns a tokio task reading stdout, dispatches via `mpsc`.
- Public API is fire-and-forget: `add_drawer`, `kg_add`, `kg_invalidate`, `diary_write`. All return immediately.
- Child crash → auto-respawn with exponential backoff. Consecutive failures above threshold → silence sink for N minutes (avoid log spam).
- Expose sink health (writes sent / dropped / last error) under Davis `/health`.

### Wing & room taxonomy

```
wing=davis.articles            room=<topic-slug>
wing=davis.ha                  room=<area-slug>
wing=davis.routing             room=<profile-slug|budget>
wing=davis.agent.ingest        diary only
wing=davis.agent.router        diary only
wing=davis.agent.ha-analyzer   diary only
wing=davis.agent.rule-learner  diary only
```

### KG entity ID scheme

Fixed namespaces; Rust helper `TripleId::new(Ns, &str)` enforces formatting:

```
entity:<ha_entity_id>          area:<ha_area_slug>
host:<fqdn>                    article:<article_id>
topic:<slug>                   rule:<host>
rule_version:<host>:v<n>       provider:<name>
model:<id>                     route_profile:<profile>
budget_scope:daily:<YYYY-MM-DD>      budget_scope:monthly:<YYYY-MM>
worker:<name>                  component:<name>
```

### Predicate vocabulary

Fixed `enum Predicate` in `mempalace_sink.rs`. Adding a predicate is a code change, not a string. Adding one requires documenting: trigger, hysteresis/debounce, invalidation rule, the user question it answers.

| Predicate | Subject → Object | Written when | Debounce / hysteresis | Answers |
|---|---|---|---|---|
| `EntityHasState` | entity → state label | avail↔unavail boundary cross | state stable ≥ 60s | "上周客厅哪个灯坏了" |
| `EntityReplacementFor` | entity → entity | replacement_score ≥ 60 | invalidate when score < 40 | "坏掉的灯应该换哪个" |
| `EntityLocatedIn` | entity → area | first seen / area change | immediate | "客厅以前都有哪些设备" |
| `EntityNameIssue` | entity → issue tag | findings detects 2 cycles in a row | immediate invalidate when gone | "哪些设备名字有问题" |
| `ArticleDiscusses` | article → topic | ingest success | never invalidate (historical fact) | "哪些文章讲过 async rust" |
| `ArticleCites` | article → article | extracted from body / value report | never invalidate | "这篇之后谁跟进了" |
| `ArticleSourcedFrom` | article → host | ingest success | never invalidate | "最近 lobste.rs 进了什么" |
| `ArticleDiscoveredFrom` | article → source tag (`feed:<host>` / `sitemap:<host>` / `search:brave`) | discovery worker submits a new candidate | never invalidate | "这篇怎么发现的 / 这个 feed 这个月进了多少篇" |
| `ArticleTranslated` | article → language tag (`lang:zh-CN`) | translate worker writes translation.md | never invalidate | "这篇翻译过没 / 最近翻了几篇" |
| `RuleActiveFor` | host → rule_version | new version lands (old version invalidated) | immediate | "这条规则改过几次" |
| `RuleQuarantinedBy` | rule_version → reason tag | quality < threshold / repeated fails | immediate | "为什么这条规则不生效" |
| `ProviderHealth` | provider → health label | **deferred** — zeroclaw daemon owns per-call provider metrics; Davis would have to tap zeroclaw's `/api/cost` or logs to get the signal. Phase 5+ if demand warrants. | — | "openrouter 最近挂过吗" |
| `RouteResolvedTo` | route_profile → model | **deferred** — Davis's `model_routing.rs` is a config renderer, not a runtime router. Route-switching events happen inside zeroclaw. | — | "那天 fast 档为什么切 haiku" |
| `BudgetEvent` | budget_scope → event tag | **deferred** — budget tracking is zeroclaw's `CostTracker`; Davis does not see per-call cost. | — | "最近什么时候超预算" |
| `WorkerHealth` | worker → status | backlog/failures sustained | sustained trigger | "ingest worker 今天卡过吗" |
| `ComponentReachability` | component → label | failure persists ≥ 30s | anti-flapping | "zeroclaw 最近挂过多久" |

### What does NOT go in KG

- Entity names, article titles, URLs (→ drawer content or subject ID)
- Per-call LLM tokens/cost (→ Davis JSON scorecard)
- Embedding vectors (→ Davis `embeddings.json`)
- User preferences (users write those themselves via MemPalace; Davis does not impersonate the user)

### Subsystem hook points

- `article_memory/ingest/worker.rs` end-of-cycle — `add_drawer(articles/<topic>)` + `kg_add(ArticleDiscusses|Cites|SourcedFrom)` + ingest diary.
- `article_memory/ingest/rule_learning_worker.rs` — `RuleActiveFor` / `RuleQuarantinedBy` on promotions/demotions + rule-learner diary.
- `article_memory/discovery/worker.rs` end-of-cycle — `kg_add(ArticleDiscoveredFrom)` + discovery diary (wing `davis.agent.discovery`).
- `article_memory/translate/worker.rs` on successful translation — `kg_add(ArticleTranslated)` + translator diary (wing `davis.agent.translator`).
- `ha_mcp.rs` live-context refresh — diff previous snapshot → `EntityHasState` / `EntityReplacementFor` / `EntityNameIssue` / `EntityLocatedIn` + HA findings narrative drawer + ha-analyzer diary.
- Router / budget hooks: deliberately absent. Davis doesn't own those signals — see the three `deferred` rows in the predicate table above.
- `crawl4ai` / `imessage` / `express` / `ha_client` — no MemPalace hooks.

### Failure & governance

- Write failure: drop + tracing warn + metrics; never block Davis path.
- Davis local JSON is source of truth. MemPalace is eventually consistent. Reconciliation audit CLI comes in Phase 5.
- PII: scrub drawer content before `add_drawer` (addresses, full names, secrets). KG is safer because it only holds IDs + tags.
- Drawer retention: 90 days uncompressed; older drawers run through `mempalace compress --wing davis.*` (AAAK lossy — accept).

### Rollout phases (~2–3 weeks)

1. **Infra** (~1 wk): `mempalace_sink.rs` + stdio MCP client + install helper + smoke test + CLAUDE.md update (this section).
2. **HA** (~3 d): 4 HA predicates + findings drawer + ha-analyzer diary. Manual e2e: provoke an unavailable entity, ask agent later "what broke".
3. **Articles** (~3 d): 3 article predicates + value-report drawer + PII scrub + ingest diary.
4. **Routing + rules** (~2 d): 5 routing/rule predicates + router/rule-learner diaries.
5. **Governance** (~2 d): `WorkerHealth` / `ComponentReachability`, periodic compress, `davis articles mempalace-audit` CLI, `/health` surfacing of sink metrics.

### Read path

Davis Rust never reads from MemPalace. All retrieval goes through agents using the MCP tools via zeroclaw. Skill additions (`project-skills/mempalace-memory/SKILL.md` + new per-subsystem skills) teach agents which wing/room/predicate to query for which user question.

## History

Prior audits suggested deep library integration with zeroclaw (git-dep, `zeroclaw-memory` for rule stores, shared `CostTracker`/`Observer`, MCP registry for HA). **Those conclusions are withdrawn** — they presumed Davis was a subproject of zeroclaw, which contradicts the product positioning above. See MemPalace `DavisZeroClaw/decisions` authoritative correction drawer (2026-04-25) for the full reversal.
