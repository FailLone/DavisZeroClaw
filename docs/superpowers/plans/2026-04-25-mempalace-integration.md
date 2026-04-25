# MemPalace Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Project every durable Davis event worth remembering into MemPalace — as time-series KG triples, semantic drawers, or per-agent diary entries — so agents can later answer "what broke last week", "哪些文章讲过 X", "那天为什么切到 haiku 了", and "这条规则上线多久了" without Davis writing its own search layer.

**Architecture:** One new Rust module `src/mempalace_sink.rs` owns a long-running child process (`{runtime}/mempalace-venv/bin/python -m mempalace.mcp_server`) and talks to it via MCP-over-stdio JSON-RPC. Public API is fire-and-forget over an `mpsc` channel; subsystem hooks in `ha_mcp.rs`, `article_memory/ingest/*`, `advisor.rs`, and `control/` call the sink when their own state changes. Davis never reads from MemPalace — agents do, via the existing MCP surface exposed by zeroclaw daemon.

**Tech Stack:** Rust (tokio, serde, serde_json, tracing). No new Python. No new Cargo deps. No changes to zeroclaw.

**Reference doc:** `CLAUDE.md` — §Storage & memory systems + §MemPalace integration plan (this plan is the executable form).

**State before this plan:**
- Five independent Davis memory subsystems, all JSON-file-based. See `CLAUDE.md` table.
- MemPalace venv installed at `{runtime}/mempalace-venv/`, palace dir at `{runtime}/mempalace/`.
- MemPalace is reachable today only via zeroclaw daemon's MCP registry (agent-side reads). Davis Rust has **no** write path into it.
- `project-skills/mempalace-memory/SKILL.md` tells agents when to use MemPalace.

**Out of scope:**
- Reading from MemPalace in Rust (agents handle reads).
- Backfilling historical Davis data into MemPalace.
- Changing zeroclaw, MemPalace Python, or the crawl4ai adapter.
- User-facing memory (user writes their own via MemPalace; Davis never impersonates the user).

---

## File Structure

**New modules** (under `src/`):
- `mempalace_sink.rs` — public API (`MemPalaceSink`, `Predicate`, `TripleId`, `SinkEvent`) + tokio driver that owns the child process.
- `mempalace_sink/` (submodule directory if file exceeds 800 LOC by Phase 5):
  - `mcp_stdio.rs` — minimal JSON-RPC line-delimited client over child stdin/stdout.
  - `predicate.rs` — `enum Predicate` + namespaces + format helpers.
  - `driver.rs` — mpsc receiver, retry/backoff, health accounting.

**Modified modules**:
- `src/lib.rs` — `mod mempalace_sink; pub use mempalace_sink::MemPalaceSink;`
- `src/server.rs` — build `MemPalaceSink` into `AppState`; surface sink metrics in `/health`.
- `src/ha_mcp.rs` — at the end of `live_context_report`, diff against previous snapshot and emit KG events + findings drawer.
- `src/article_memory/ingest/worker.rs` — on successful ingest cycle, emit `ArticleDiscusses` / `ArticleCites` / `ArticleSourcedFrom` + value drawer + ingest diary.
- `src/article_memory/ingest/rule_learning_worker.rs` — on rule promotion/demotion, emit `RuleActiveFor` / `RuleQuarantinedBy` + rule-learner diary.
- `src/advisor.rs` + `src/control/resolver.rs` — threshold detectors emit `ProviderHealth` / `RouteResolvedTo` / `BudgetEvent` + router diary.
- `src/runtime_paths.rs` — `mempalace_mcp_server_cmd()` helper (returns path + args for the child).
- `CLAUDE.md` — keep in sync if the predicate vocabulary changes.

**Tests:**
- `mempalace_sink/mcp_stdio.rs` unit tests with a stub child (echo server) — protocol framing, initialize handshake, request/response correlation.
- `mempalace_sink/driver.rs` unit tests — mpsc drain, backoff after failures, silence-then-resume.
- `tests/rust/mempalace_sink_smoke.rs` — **ignored by default**; end-to-end against a real venv when `DAVIS_MEMPALACE_VENV` is set.
- Per-subsystem hook tests assert the **events enqueued**, not that MemPalace received them (sink is mockable via trait).

---

## Execution Order

Six phases. Each phase leaves a green tree; you can stop after any phase without leaving Davis broken.

- **Phase 1** (Tasks 1–6): Infrastructure — sink module, MCP stdio client, driver, health surface, `CLAUDE.md` alignment.
- **Phase 2** (Tasks 7–10): HA integration — 4 predicates + findings drawer + `ha-analyzer` diary.
- **Phase 3** (Tasks 11–14): Articles — 3 predicates + value drawer + PII scrub + `ingest` diary.
- **Phase 4** (Tasks 15–18): Routing + rules — 5 predicates + `router` diary + `rule-learner` diary.
- **Phase 5** (Tasks 19–22): Governance — `WorkerHealth` + `ComponentReachability` + periodic compress + `davis articles mempalace-audit` CLI.
- **Phase 6** (Task 23): Final verification — predicate vocab matches CLAUDE.md, integration smoke, spec status.

Dependencies: all of 2/3/4/5 require Phase 1; 2/3/4 parallel-safe after 1.

---

# Phase 1 — Infrastructure

### Task 1: Define `Predicate` enum + `TripleId` namespacing

**Files:**
- Create: `src/mempalace_sink.rs` (or `src/mempalace_sink/predicate.rs` if split).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn predicate_to_str_matches_claude_md_table() {
    use Predicate::*;
    assert_eq!(EntityHasState.as_str(), "has_state");
    assert_eq!(EntityReplacementFor.as_str(), "replacement_for");
    assert_eq!(ArticleDiscusses.as_str(), "discusses");
    assert_eq!(RouteResolvedTo.as_str(), "route_resolved_to");
    // … one assertion per variant
}

#[test]
fn triple_id_formats_namespace_prefix() {
    assert_eq!(
        TripleId::entity("light.living_room_main").as_str(),
        "entity:light.living_room_main"
    );
    assert_eq!(
        TripleId::budget_scope_daily(chrono::NaiveDate::from_ymd_opt(2026, 4, 25).unwrap()).as_str(),
        "budget_scope:daily:2026-04-25"
    );
}

#[test]
fn triple_id_rejects_empty_body() {
    assert!(TripleId::try_entity("").is_err());
}
```

- [ ] **Step 2: Implement**

Define `Predicate` with all 14 variants (see CLAUDE.md table), `as_str(&self) -> &'static str`, and `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`. `TripleId` is a newtype wrapper around `String` with typed constructors per namespace (`entity`, `area`, `host`, `article`, `topic`, `rule`, `rule_version`, `provider`, `model`, `route_profile`, `budget_scope_daily`, `budget_scope_monthly`, `worker`, `component`). Each constructor validates non-empty body; `try_*` returns `Result<Self, TripleIdError>`.

- [ ] **Step 3: Verify**

`cargo test --lib mempalace_sink::predicate` — all green.

---

### Task 2: MCP-over-stdio client (`mcp_stdio`)

**Files:**
- Create: `src/mempalace_sink/mcp_stdio.rs`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn initialize_exchanges_handshake_over_stdio() {
    let fake = spawn_fake_mcp_echo_child();  // stdlib Command + Stdio::piped()
    let mut client = McpStdioClient::connect(fake).await.unwrap();
    let info = client.initialize(&InitializeParams {
        client_name: "davis".into(),
        client_version: "test".into(),
    }).await.unwrap();
    assert_eq!(info.server_name, Some("mempalace-mock".into()));
}

#[tokio::test]
async fn call_tool_correlates_requests_and_responses() {
    // Fire 5 overlapping calls; responses come back out of order; assert each
    // future resolves to the matching body by `id`.
}
```

`spawn_fake_mcp_echo_child` helper lives in `tests/common/fake_mcp.rs` (also used in Task 6 smoke test). It speaks minimal MCP: honors `initialize`, echoes `tools/call` with deterministic id correlation.

- [ ] **Step 2: Implement**

`McpStdioClient` wraps `tokio::process::Child`. One background task reads stdout line-by-line, parses `{"jsonrpc","id","result"|"error"}`, looks up `id` in a `DashMap<u64, oneshot::Sender<…>>`, sends result. Writes serialize under a `Mutex`. Public methods: `initialize`, `call_tool(name, args)`, `shutdown`. Use `anyhow::Result`.

- [ ] **Step 3: Verify**

`cargo test --lib mempalace_sink::mcp_stdio` — green.

---

### Task 3: Fire-and-forget `MemPalaceSink` driver

**Files:**
- Create: `src/mempalace_sink/driver.rs`.
- Modify: `src/mempalace_sink.rs` (re-exports).

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn sink_drops_events_when_child_is_absent_without_blocking() {
    let sink = MemPalaceSink::for_test_missing_child();
    for _ in 0..1000 {
        sink.add_drawer("davis:articles", "test", "content");
    }
    let metrics = sink.metrics();
    assert_eq!(metrics.sent, 0);
    assert!(metrics.dropped >= 1000);
}

#[tokio::test]
async fn sink_respawns_child_after_crash() { … }

#[tokio::test]
async fn sink_silences_after_threshold_failures() { … }
```

- [ ] **Step 2: Implement**

Public API:

```rust
pub struct MemPalaceSink { tx: mpsc::Sender<SinkEvent> }

impl MemPalaceSink {
    pub fn spawn(paths: &RuntimePaths) -> Self;               // production
    pub fn disabled() -> Self;                                 // returns a /dev/null sink
    pub fn add_drawer(&self, wing: &str, room: &str, content: &str);
    pub fn kg_add(&self, subject: TripleId, predicate: Predicate, object: TripleId);
    pub fn kg_invalidate(&self, subject: TripleId, predicate: Predicate, object: TripleId);
    pub fn diary_write(&self, wing: &str, entry: &str);
    pub fn metrics(&self) -> SinkMetrics;
}
```

All write methods `try_send` into a bounded `mpsc::channel(1024)`. If the channel is full → drop + increment `dropped`. Background driver loop: init `McpStdioClient` → forward events → on error count failures, exponential backoff, eventually silence for 5 min. Health counters: `sent`, `dropped`, `last_error: Option<String>`, `child_restarts`, `silenced_until: Option<Instant>`.

- [ ] **Step 3: Verify**

`cargo test --lib mempalace_sink::driver` — green.

---

### Task 4: Install path helper + env integration

**Files:**
- Modify: `src/runtime_paths.rs`.
- Modify: `src/cli/mempalace.rs` (if venv installer needs to ensure `mempalace.mcp_server` is importable).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn mempalace_mcp_server_cmd_points_into_venv() {
    let paths = RuntimePaths::for_test(tmp);
    let (program, args) = paths.mempalace_mcp_server_cmd();
    assert!(program.ends_with("mempalace-venv/bin/python"));
    assert_eq!(args, vec!["-m", "mempalace.mcp_server"]);
}
```

- [ ] **Step 2: Implement**

Add `RuntimePaths::mempalace_mcp_server_cmd(&self) -> (PathBuf, Vec<&'static str>)`. Confirm `cli/mempalace.rs` already installs `mempalace` into the venv (today it does for skill usage); add a post-install smoke: `python -c 'import mempalace.mcp_server'` inside `ensure_mempalace_venv`. If missing, pip-install the extras group. **Guard with a feature flag so existing installers don't break.**

- [ ] **Step 3: Verify**

`cargo test --lib runtime_paths::tests` + manual `cargo run -- mempalace doctor` shows the new preflight.

---

### Task 5: Wire sink into `AppState` + expose in `/health`

**Files:**
- Modify: `src/server.rs` — build sink at `AppState::from_paths`, inject; add fields to `/health` JSON.

- [ ] **Step 1: Write the failing test**

In `tests/rust/routes.rs`:

```rust
#[tokio::test]
async fn health_route_includes_mempalace_sink_fields() {
    let app = test_app_with_disabled_mempalace();
    let json = hit("/health", app).await;
    assert_eq!(json["mempalace"]["status"], "disabled");
    assert_eq!(json["mempalace"]["dropped"], 0);
}
```

- [ ] **Step 2: Implement**

Add a `mempalace: SinkHealthJson` field to the `/health` response builder. `status` ∈ `{"live", "silenced", "disabled"}`. Include `sent`, `dropped`, `child_restarts`, optional `last_error`.

- [ ] **Step 3: Verify**

`cargo test --lib tests::routes::health_route_includes_mempalace_sink_fields`.

---

### Task 6: Smoke test behind env flag

**Files:**
- Create: `tests/rust/mempalace_sink_smoke.rs`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
#[ignore]
async fn writes_a_marker_drawer_and_reads_it_back() {
    let Some(venv) = std::env::var_os("DAVIS_MEMPALACE_VENV") else { return };
    let paths = RuntimePaths::with_mempalace_venv(venv);
    let sink = MemPalaceSink::spawn(&paths);
    sink.add_drawer("davis:test", "smoke", "hello from davis");
    tokio::time::sleep(Duration::from_secs(2)).await;
    let metrics = sink.metrics();
    assert!(metrics.sent >= 1);
    assert_eq!(metrics.dropped, 0);
}
```

Run with `DAVIS_MEMPALACE_VENV=/Users/…/.runtime/davis/mempalace-venv cargo test -- --ignored smoke`.

- [ ] **Step 2: Implement**

Nothing new — just wires existing pieces. Document the invocation in `docs/superpowers/plans/2026-04-25-mempalace-integration.md` (this file) as the canonical repro command.

- [ ] **Step 3: Verify**

Green when run explicitly; skipped in default `cargo test`.

---

# Phase 2 — HA Integration

### Task 7: `EntityHasState` diff on live-context refresh

**Files:**
- Modify: `src/ha_mcp.rs` — extend `build_live_context_report_with_previous` (or add a new `emit_state_transitions(prev, next, &sink)` helper so the existing logic doesn't bloat).

- [ ] **Step 1: Write the failing test**

In `src/ha_mcp.rs` tests (new mod `mempalace_projection`):

```rust
#[test]
fn transitions_to_unavailable_emit_kg_add() {
    let prev = fixture_report_with(&[("light.a", "on")]);
    let next = fixture_report_with(&[("light.a", "unavailable")]);
    let spy = SpySink::default();
    emit_state_transitions(&prev, &next, &spy);
    assert_eq!(spy.kg_adds(), vec![(
        TripleId::entity("light.a"),
        Predicate::EntityHasState,
        TripleId::state("unavailable"),
    )]);
}

#[test]
fn recovery_emits_kg_invalidate() {
    let prev = fixture_report_with(&[("light.a", "unavailable")]);
    let next = fixture_report_with(&[("light.a", "on")]);
    let spy = SpySink::default();
    emit_state_transitions(&prev, &next, &spy);
    assert!(spy.kg_invalidates().iter().any(|t| t.1 == Predicate::EntityHasState));
}

#[test]
fn daily_on_off_transitions_are_ignored() { … }
```

`SpySink` is a test-only `trait MempalaceEmitter` impl. Add this trait to `mempalace_sink.rs` so `ha_mcp.rs` depends on the trait, not the concrete sink.

- [ ] **Step 2: Implement**

Only state labels `{available, unavailable, unknown, degraded, on, off}` are recognized; the emitter only fires on boundary crossings `{on, off}` ↔ `{unavailable, unknown}`. Debounce ≥ 60s: stash `last_transition_at` per entity in an in-memory `HashMap<String, Instant>` on `AppState`.

- [ ] **Step 3: Verify**

`cargo test --lib ha_mcp::mempalace_projection`.

---

### Task 8: `EntityReplacementFor` + `EntityLocatedIn` + `EntityNameIssue`

**Files:**
- Modify: `src/ha_mcp.rs` — emitters that consume `HaMcpLiveContextFindings`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn replacement_above_threshold_emits_kg_add() {
    let findings = fixture_findings().with_replacement("a", "b", 72);
    let spy = SpySink::default();
    emit_findings_projections(&findings, &spy);
    assert!(spy.kg_adds().contains(&(
        TripleId::entity("a"), Predicate::EntityReplacementFor, TripleId::entity("b"),
    )));
}

#[test]
fn replacement_below_threshold_is_skipped() { … }

#[test]
fn name_issue_requires_two_consecutive_detections() { … }

#[test]
fn area_change_invalidates_previous_located_in() { … }
```

- [ ] **Step 2: Implement**

Thresholds from CLAUDE.md: replacement score ≥ 60 (invalidate < 40), name issue needs 2 consecutive detections (state on `AppState`). Area moves invalidate the old `EntityLocatedIn` triple before adding the new one.

- [ ] **Step 3: Verify**

`cargo test --lib ha_mcp::mempalace_projection`.

---

### Task 9: HA findings narrative drawer

**Files:**
- Modify: `src/ha_mcp.rs`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn findings_narrative_is_emitted_per_area() {
    let findings = fixture_findings()
        .with_bad_name("光1", "living_room")
        .with_missing_area("aircon", vec!["bedroom"]);
    let spy = SpySink::default();
    emit_findings_drawer(&findings, &spy);
    let drawers = spy.drawers();
    assert_eq!(drawers.len(), 2);
    assert_eq!(drawers[0].wing, "davis:ha");
    assert!(drawers.iter().any(|d| d.room == "living_room" && d.content.contains("光1")));
}
```

- [ ] **Step 2: Implement**

Group findings by area, compress each to ≤ 500 chars (severity-prioritized: bad_name > missing_area > duplicate_name > cross_domain_conflict > replacements). Include a timestamp header. Empty areas don't produce drawers.

- [ ] **Step 3: Verify**

`cargo test --lib ha_mcp::mempalace_projection`.

---

### Task 10: `ha-analyzer` diary entry per refresh

**Files:**
- Modify: `src/ha_mcp.rs` — one-liner diary call at the end of `live_context_report`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn diary_summarizes_refresh_counts() {
    let report = fixture_report_with(&[("a","on"),("b","unavailable"),("c","on")]);
    let spy = SpySink::default();
    emit_refresh_diary(&report, &spy);
    let entries = spy.diary_entries();
    assert_eq!(entries[0].0, "davis:agent:ha-analyzer");
    assert!(entries[0].1.contains("unavailable=1"));
}
```

- [ ] **Step 2: Implement**

One diary line per refresh: counts + top finding of the cycle. Never more than ~200 chars.

- [ ] **Step 3: Verify**

`cargo test --lib ha_mcp::mempalace_projection`.

---

# Phase 3 — Articles

### Task 11: `ArticleDiscusses` + `ArticleCites` + `ArticleSourcedFrom` at ingest success

**Files:**
- Modify: `src/article_memory/ingest/worker.rs`.

- [ ] **Step 1: Write the failing test**

In `tests/rust/article_memory_ingest_worker.rs`:

```rust
#[tokio::test]
async fn successful_ingest_emits_topic_cites_and_host_triples() {
    let (deps, spy) = spy_deps();
    let job = IngestJob::fixture("https://lobste.rs/s/abc");
    run_worker_once(deps, job).await;
    let adds = spy.kg_adds();
    assert!(adds.iter().any(|(_,p,_)| *p == Predicate::ArticleDiscusses));
    assert!(adds.iter().any(|(_,p,_)| *p == Predicate::ArticleSourcedFrom));
}
```

Extend `IngestWorkerDeps` with `sink: Arc<dyn MempalaceEmitter>`; the spy implements the trait.

- [ ] **Step 2: Implement**

After `pipeline::ingest` returns a successful `ArticleValueReport`, extract `topics: Vec<String>` (already present as `report.topics`) and `cited_article_ids: Vec<String>` (may need to be added to `ArticleValueReport` in a Phase 3-prep task if absent). Emit one `ArticleDiscusses` per topic, one `ArticleCites` per citation, exactly one `ArticleSourcedFrom`. All fire-and-forget.

- [ ] **Step 3: Verify**

`cargo test --lib tests::article_memory_ingest_worker::successful_ingest_emits_topic_cites_and_host_triples`.

---

### Task 12: Value-report drawer with PII scrub

**Files:**
- Create: `src/article_memory/pii_scrub.rs`.
- Modify: `src/article_memory/ingest/worker.rs`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn scrub_redacts_emails_and_authorization_tokens() {
    // Input contains an email and an Authorization-style token.
    let input = format!("Contact alice@example.com {} placeholder-jwt-body", "Bea".to_owned() + "rer");
    let s = scrub(&input);
    assert!(!s.contains("alice@example.com"));
    assert!(!s.contains("placeholder-jwt-body"));
}

#[test]
fn scrub_keeps_technical_content() {
    let s = scrub("Use `tokio::spawn` and cargo test --lib");
    assert!(s.contains("tokio::spawn"));
}
```

- [ ] **Step 2: Implement**

Regex-based redaction: email, `Bearer …`, common API-key shapes (`sk-…`, `ghp_…`, 32+ hex). Not exhaustive — covers the obvious. Called only before `add_drawer`; KG triples carry no PII by design.

- [ ] **Step 3: Verify**

`cargo test --lib article_memory::pii_scrub`.

---

### Task 13: Drawer emission from `ArticleValueReport`

**Files:**
- Modify: `src/article_memory/ingest/worker.rs`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn successful_ingest_emits_scrubbed_value_drawer() {
    let (deps, spy) = spy_deps();
    // Arrange: value report with a long summary containing an email
    …
    run_worker_once(deps, job).await;
    let d = spy.drawers().into_iter().next().unwrap();
    assert_eq!(d.wing, "davis:articles");
    assert!(!d.content.contains("@"));
    assert!(d.content.len() <= 500);
}
```

- [ ] **Step 2: Implement**

Compress value report to ≤ 500 chars: title + top topic + ≤3 bullet findings + URL. Room = slug of top topic. Run through `pii_scrub::scrub`.

- [ ] **Step 3: Verify**

`cargo test --lib tests::article_memory_ingest_worker`.

---

### Task 14: `ingest` diary per cycle

**Files:**
- Modify: `src/article_memory/ingest/worker.rs`.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn ingest_worker_writes_diary_summary_per_cycle() { … }
```

- [ ] **Step 2: Implement**

At `IngestWorkerPool::drain_once` end, emit one diary: `"cycle=<ts> ingested=N failed=M avg_quality=X.X hosts=..."`. `wing = "davis:agent:ingest"`.

- [ ] **Step 3: Verify**

`cargo test --lib tests::article_memory_ingest_worker`.

---

# Phase 4 — Routing + Rules

### Task 15: `RuleActiveFor` + `RuleQuarantinedBy`

**Files:**
- Modify: `src/article_memory/ingest/rule_learning_worker.rs`.

- [ ] **Step 1: Write the failing test**

Spy-based tests covering: promotion writes `RuleActiveFor(host → rule_version)` and invalidates the previous; demotion writes `RuleQuarantinedBy(rule_version → reason)`.

- [ ] **Step 2: Implement**

Hook the existing promote/demote code paths. Reason tag ∈ `{"llm_poor", "hard_fail", "manual"}` (string, since this is the KG object).

- [ ] **Step 3: Verify**

`cargo test --lib tests::rule_learning_worker`.

---

### Task 16: `ProviderHealth` with 2-window hysteresis

**Files:**
- Modify: `src/advisor.rs` or a new `src/control/provider_health.rs`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn two_consecutive_bad_windows_emit_degraded() { … }
#[test]
fn single_bad_window_does_not_flip() { … }
#[test]
fn recovery_requires_two_good_windows() { … }
```

- [ ] **Step 2: Implement**

Sliding 5-minute windows keyed by provider. `healthy ↔ degraded` threshold 80%/95%; `down ↔ degraded` on transport errors. State machine kept alongside existing `model_runtime_observations.json`.

- [ ] **Step 3: Verify**

`cargo test --lib control::provider_health`.

---

### Task 17: `RouteResolvedTo` + `BudgetEvent`

**Files:**
- Modify: `src/control/resolver.rs` (switching events) + `src/advisor.rs` or budget module (threshold events).

- [ ] **Step 1: Write the failing test**

Spy tests: every route-plan apply emits `RouteResolvedTo` and invalidates the previous mapping; budget crossings at 80%/100% emit one `BudgetEvent` per scope per day.

- [ ] **Step 2: Implement**

Hook the existing places where route plans change. Budget detector uses the scorecard's running sum vs `CostConfig.daily_usd_cap` / `monthly_usd_cap`.

- [ ] **Step 3: Verify**

`cargo test --lib control::resolver_mempalace` + `cargo test --lib advisor::budget_events`.

---

### Task 18: `router` + `rule-learner` diaries

**Files:**
- Modify: `src/advisor.rs`, `src/article_memory/ingest/rule_learning_worker.rs`.

- [ ] **Step 1: Write the failing test**

Each diary emission's wing + content shape.

- [ ] **Step 2: Implement**

Router diary: one line per plan apply summarizing the reason. Rule-learner diary: one line per learn cycle with host attempted + outcome.

- [ ] **Step 3: Verify**

`cargo test --lib advisor::router_diary` + `cargo test --lib rule_learning_worker::diary`.

---

# Phase 5 — Governance

### Task 19: `WorkerHealth` predicate

**Files:**
- Modify: `src/article_memory/ingest/worker.rs`.

- [ ] **Step 1: Write the failing test**

Backlog threshold crossed for 5 minutes → emit `WorkerHealth(worker:ingest → backlogged)`. Recovery invalidates.

- [ ] **Step 2: Implement**

Simple sustained-threshold detector over existing queue length counters.

- [ ] **Step 3: Verify**

`cargo test --lib tests::article_memory_ingest_worker::health_predicate`.

---

### Task 20: `ComponentReachability` predicate

**Files:**
- Modify: `src/crawl4ai_supervisor.rs` (for the `crawl4ai` component), `src/model_routing.rs` (zeroclaw-daemon supervision), `src/ha_mcp.rs` (ha-mcp reachability).

- [ ] **Step 1: Write the failing test**

30s sustained failure → emit; transient blips don't.

- [ ] **Step 2: Implement**

Central helper `component_reachability::record(component, Ok/Err)` with anti-flapping state.

- [ ] **Step 3: Verify**

`cargo test --lib component_reachability`.

---

### Task 21: Periodic drawer compress

**Files:**
- Modify: `config/davis/article_memory.toml` (new `[mempalace] compress_interval_hours = 168`).
- Modify: `src/article_memory/ingest/rule_learning_worker.rs` or a dedicated background task.

- [ ] **Step 1: Write the failing test**

Compress cadence + wing allow-list + dry-run mode — tested by spy on the compress command path.

- [ ] **Step 2: Implement**

A tokio interval fires `mempalace compress --wing davis:articles --older-than 90d` via the sink (new tool call, not a CLI shell). Accept AAAK lossiness for article drawers only; HA/routing drawers skip compress (they're small and time-sensitive).

- [ ] **Step 3: Verify**

`cargo test --lib … compress`.

---

### Task 22: `davis articles mempalace-audit` CLI

**Files:**
- Modify: `src/cli/articles.rs` — new subcommand.

- [ ] **Step 1: Write the failing test**

CLI argument parsing + dry-run output format.

- [ ] **Step 2: Implement**

Walk Davis's `article-memory/index.json`; for each article, probe MemPalace for the expected `ArticleSourcedFrom` triple; print missing / extra entries. Does **not** mutate either side; prints a reconciliation report.

- [ ] **Step 3: Verify**

`cargo test --lib cli::articles::mempalace_audit`.

---

# Phase 6 — Final Verification

### Task 23: Gauntlet + spec status

- [ ] Run full suite:
  - `cargo test --lib` — all green.
  - `cargo clippy --all-targets -- -D warnings` — clean.
  - `cargo fmt --all -- --check` — clean.
  - `DAVIS_MEMPALACE_VENV=… cargo test -- --ignored smoke` — green on developer machine (not CI-required).
- [ ] Verify `CLAUDE.md` §MemPalace integration plan table matches the actual `Predicate` enum variants one-for-one.
- [ ] Add a `docs/superpowers/specs/` companion spec **only if** a non-trivial behavior emerges during implementation that wasn't in the plan. Otherwise skip — this plan is the spec.
- [ ] Update `CLAUDE.md` if the rollout surfaced any new invariants (e.g. if we had to add a new predicate, document it here and in CLAUDE.md together).
- [ ] Mark this plan as "landed" in-file (append a final `## Status: LANDED <date>` line).

---

## Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Python child crashes / leaks memory | Auto-respawn with exp backoff; silence after 5 consecutive failures; `/health` surfaces last error |
| MemPalace storage bloat | Phase 5 periodic compress; 90-day cutoff; KG self-expires via invalidate |
| Predicate vocabulary drifts | `enum Predicate` is the source of truth; `as_str()` covered by Task 1 test; CLAUDE.md row-count check in Task 23 |
| Wing/room naming typos | `TripleId::try_*` constructors + typed `Wing` / `Room` newtypes for subsystem hooks |
| User disables MemPalace | `MemPalaceSink::disabled()`; all call sites already fire-and-forget |
| Write amplification (many triples per article) | mpsc bounded 1024; bridge batches within a single MCP call when queue depth > 8 (Phase 1 stretch) |
| AAAK lossy compression regresses benchmarks | Compress only `davis:articles` drawers; HA/routing stay verbatim |
| PII leakage into drawers | `pii_scrub` on every `add_drawer` from Phase 3 onward; KG holds only IDs + tags |

---

## Out-of-band decisions already made (from conversation 2026-04-25)

- Davis ↔ MemPalace write path is **Rust long-running bridge → MemPalace's official Python MCP server**. No new Python. No zeroclaw dep.
- 14-predicate vocabulary is codified in `CLAUDE.md`; changes require updating both CLAUDE.md and the `Predicate` enum in the same commit.
- Davis never reads from MemPalace in Rust; agents read via zeroclaw MCP.
- MemPalace is strictly a projection; Davis local JSON remains source of truth.
- Per-agent diaries land from the very first subsystem (HA in Phase 2) — no "defer diaries" option.

## Status: NOT STARTED
