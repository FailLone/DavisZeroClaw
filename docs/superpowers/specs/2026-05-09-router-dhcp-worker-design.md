# Router DHCP Keeper Worker — Design Spec

**Date**: 2026-05-09
**Status**: Draft (awaiting user review)
**Owner**: Davis

## Problem

Currently, repeatedly disabling the GPON ONT router's DHCP server is automated by a standalone TypeScript/Bun + Puppeteer script (the `Faillone/Automation` repository, file `scripts/router-dhcp-check.ts`), driven by a separate `crontab` entry on the user's Mac. The script logs into the router admin page at `192.168.0.1`, navigates a SPA + iframe UI, checks `#dhcpSrvType`, and clicks "apply" if the toggle is enabled.

This out-of-tree side automation has three problems for Davis:

1. **It's another runtime to keep alive** — its own cron, its own logs, its own log-rotation script, its own `.env`, distinct from Davis's launchd-supervised daemon.
2. **No observability inside Davis.** When something stops working (e.g., router firmware update changed selectors), Davis has no way to surface "DHCP keeper is failing for the past 6 hours" alongside the rest of its self-reporting.
3. **Standalone Bun/Puppeteer install** doubles the headless-browser footprint already paid for by `crawl4ai_adapter/` (Playwright/Chromium).

## Goal

Bring the periodic router DHCP check inside the Davis daemon, with no functional behavior change visible to the router. Specifically:

- The check runs as a periodic worker inside the Davis daemon (no separate cron).
- The browser-automation layer remains in Python, reusing the same Chromium binary already installed for `crawl4ai_adapter/` (no second Chromium on disk).
- Davis owns scheduling, secret loading, failure summarization, and lightweight self-reporting (diary + `/health`).
- Configurable via `local.toml`; off by default.

**Non-goals**:

- Generalizing into a "router subsystem" that handles WiFi clients, port forwarding, dial-up health, etc. This is one specific check.
- Promoting router state into MemPalace KG predicates. We deliberately stay diary-only.
- Detecting why the router auto-re-enables DHCP (firmware behavior; out of Davis's reach).
- Rewriting the automation into pure Rust HTTP. We chose Playwright/Chromium because the user does not want to do packet-capture work to reverse-engineer the router's HTTP API.
- Multi-router or multi-host support. One router, one URL, one credential pair.

## Why this layout (alternatives considered)

| Option | Verdict |
|---|---|
| Pure Rust HTTP via `reqwest` (reverse-engineer the router protocol) | Rejected: requires user packet-capture; high risk of hitting client-side encryption walls |
| `chromiumoxide` Rust crate | Rejected: pulls a Rust dependency tree purely to drive a browser, when Davis already has Python + Chromium for crawl4ai |
| Merge into `crawl4ai_adapter/` | Rejected: violates `crawl4ai_adapter/`'s single responsibility (article crawling); would entangle health/restart semantics across two unrelated workloads |
| **Parallel Python adapter `router_adapter/` driven by a Rust supervisor** | **Adopted** |

The CLAUDE.md Python boundary is updated to make this admissible (see "CLAUDE.md change" below).

## Solution

A new periodic worker spawns a Python adapter as a one-shot subprocess every N seconds (default 600). The Python adapter performs the Playwright flow and emits a single JSON status line on stdout. Rust parses that line into a typed `RouterCheckOutcome`, applies dedupe rules, and writes a diary entry. No HTTP server, no long-lived child, no on-disk state.

**Key architectural properties**:

- **Spawn-and-die**: each tick spawns a fresh Python subprocess; Davis does not keep Chromium running between checks.
- **Single shared Chromium**: `PLAYWRIGHT_BROWSERS_PATH` is set by the supervisor at spawn time; both `crawl4ai_adapter/` and `router_adapter/` resolve to the same binary.
- **Dedupe before writing**: consecutive identical failures collapse to a single "started failing" + "still failing" + "recovered" diary triplet, not N+1 entries.
- **No retries**: a failed tick is just logged; the next tick attempts again. Ten-minute interval is the natural backoff.
- **Diary-only memory**: nothing goes into MemPalace KG predicates. Future expansion (if router-control grows into a subsystem) can introduce a `DeviceConfigState(host → key:value)` predicate; not now.
- **Off by default**: `[router_dhcp].enabled = false` in defaults; user opts in.

## Architecture

### Module layout

```
DavisZeroClaw/
├── router_adapter/                       (NEW — Python)
│   ├── pyproject.toml                    playwright + python-dotenv
│   ├── README.md                         standalone-run + selector-edit guide
│   └── router_dhcp_check.py              ports router-dhcp-check.ts
│
├── src/
│   ├── router_supervisor.rs              (NEW) types, parse_outcome, RouterChecker trait, PythonRouterChecker
│   ├── router_worker.rs                  (NEW) tick loop, dedupe state machine, /health field
│   ├── app_config.rs                     +RouterDhcpConfig, +LocalConfig.router_dhcp
│   ├── lib.rs                            +pub use ...
│   ├── local_proxy.rs (or main.rs)       +tokio::spawn(router_worker)
│   ├── cli/mod.rs                        +router-dhcp subcommand
│   └── cli/router_dhcp.rs                (NEW) `daviszeroclaw router-dhcp install | run-once`
│
├── tests/
│   ├── fixtures/router_stub.py           (NEW) one-line Python stub for integration test
│   └── rust/router_supervisor_spawn.rs   (NEW) spawn integration test
│
├── config/local.toml.example             +[router_dhcp] section, commented
└── CLAUDE.md                             Python boundary clarification
```

### Tick flow

```
T+0      tokio::time::interval fires (config.interval_secs, default 600)
T+0      RouterWorker::run_one_tick()
T+0      └─ check credential gate (env ROUTER_USERNAME, ROUTER_PASSWORD present?)
T+0      │     missing → write diary "ROUTER.disabled.no.creds" once, self-disable, return
T+0      └─ RouterChecker::check_once()  (PythonRouterChecker in prod)
T+0      │     └─ Command::new(python_bin)
T+0      │            .arg("-m").arg("router_adapter.router_dhcp_check")
T+0      │            .env("PLAYWRIGHT_BROWSERS_PATH", shared_path)
T+0      │            .env("ROUTER_URL", config.url)
T+0      │            .env("ROUTER_USERNAME", <from env>)
T+0      │            .env("ROUTER_PASSWORD", <from env>)
T+0      │            .stdout(Stdio::piped())
T+0      │            .stderr(Stdio::piped())
T+0      │            .kill_on_drop(true)
T+~30s   │     ├─ tokio::time::timeout(tick_timeout_secs, child.wait_with_output())
T+~30s   │     │     timeout → child.kill() → RouterCheckOutcome::Crashed
T+~30s   │     └─ parse_outcome(stdout, exit_status, stderr)
T+~30s   └─ apply dedupe, decide whether to write diary
T+~30s   └─ update /health snapshot (last_run, last_outcome, consecutive_failures)
T+~30s   sleep until next interval
```

### Stdout protocol (Rust ↔ Python contract)

Python's stdout MUST end with exactly one JSON line. Earlier lines are free-form human-readable logs (Rust passes them through to `tracing::info!`).

```jsonc
// DHCP was already off — no action taken
{"status":"ok", "action":"none",     "dhcp_was_enabled": false, "duration_ms": 28341}

// DHCP was on — script disabled it
{"status":"ok", "action":"disabled", "dhcp_was_enabled": true,  "duration_ms": 31204}

// Self-reported failure (login timeout, selector miss, iframe missing, ...)
{"status":"error", "stage":"login|navigate|iframe|toggle|apply|unhandled",
 "reason":"<short string>", "duration_ms": 12003}

// If Python crashes before printing this line, Rust observes a non-zero
// exit (or timeout) with no parseable trailing JSON → Crashed.
```

`stage` enum is closed: `login | navigate | iframe | toggle | apply | unhandled`. Python's outermost `try/except` MUST catch any uncaught exception and emit `unhandled`.

### Rust outcome type

```rust
pub enum RouterCheckOutcome {
    Ok        { action: RouterAction, dhcp_was_enabled: bool, duration_ms: u64 },
    Reported  { stage: String,        reason: String,         duration_ms: u64 },
    Crashed   { exit_code: Option<i32>, stderr_tail: String },
    SpawnFailed { reason: String },
}

pub enum RouterAction { None, Disabled }
```

`parse_outcome(stdout, exit_status, stderr) -> RouterCheckOutcome` is a pure function. It reads the last non-empty stdout line, attempts JSON deserialization, and falls back to `Crashed` on any of: non-zero exit + unparseable last line, missing required field, empty stdout.

### Worker dedupe state machine

```
struct WorkerState {
    last_outcome_kind: Option<OutcomeKind>,   // Ok | Reported(stage) | Crashed | SpawnFailed
    consecutive_failures: u32,
    creds_self_disabled: bool,
}
```

Diary write rules:

Rules are evaluated **top-down; first match wins.**

| # | Transition | Diary entry |
|---|---|---|
| 1 | First ever tick (any outcome) | one entry describing the outcome |
| 2 | Failure → success (any action) | diary entry tagged `RECOVERED\|prev.failed.N=<n>` |
| 3 | Success → success (`Disabled` action) | diary entry (DHCP was on; we want a record) |
| 4 | Same failure kind as last tick | **no diary entry**, only `tracing::warn!`; bump `consecutive_failures` |
| 5 | Different failure kind from last | new diary entry; reset `consecutive_failures = 1` |
| 6 | Success → success (no action) | **no diary entry**, only `tracing::debug!` |

Rule 2 takes priority over rule 6: a recovery is always recorded even if the recovered state happens to be "no action needed". Rule 6 only fires when the previous tick was *also* a no-action success — that is the steady state and would otherwise drown the wing.

### Diary format (AAAK)

`wing = "davis.agent.router-keeper"`, `topic = "tick"`.

Two prefix categories:

- `TICK:<iso>|...` — emitted by `RouterWorker::run_one_tick()` after a check completes. One per non-deduped state transition (per the table above).
- `INIT:<iso>|...` — emitted at most once per worker lifetime. Used today only for `disabled.no.creds`. Future startup-time anomalies should reuse this prefix.

Examples:

```
TICK:2026-05-09T16:23:11Z|router.dhcp|action=none|dur.28s|✓
TICK:2026-05-09T16:33:14Z|router.dhcp|action=disabled|was.on|dur.31s|★
TICK:2026-05-09T16:43:09Z|router.dhcp|stage.login|reason=selector.timeout|dur.12s|⚠️
TICK:2026-05-09T17:13:12Z|router.dhcp|RECOVERED|prev.failed.3|action=none|dur.27s|✓
INIT:2026-05-09T15:00:00Z|router.dhcp|disabled.no.creds|⚠️
```

### `/health` exposure

`local_proxy.rs`'s `/health` handler gains:

```jsonc
"router_dhcp": {
  "enabled": true,
  "last_run": "2026-05-09T16:23:11Z",   // ISO 8601, or null if no tick has run yet
  "last_outcome": "ok",                  // one of: "ok", "reported", "crashed", "spawn_failed", or null
  "consecutive_failures": 0
}
```

Field semantics:

- `enabled`: from config. If `false`, the other three fields are still emitted as `null`/`0`.
- `last_run`: `Option<DateTime>` — `null` until the first tick completes.
- `last_outcome`: `Option<&'static str>` derived from the `RouterCheckOutcome` discriminant — `null` until the first tick completes. The four non-null values map 1:1 to the Rust enum variants (`Ok | Reported | Crashed | SpawnFailed`).
- `consecutive_failures`: `u32`. Counts identical-kind consecutive failures (driven by the dedupe state machine). Reset to `0` on success.

Specific failure reasons stay in diary; `/health` answers "is this thing alive and broadly healthy."

### Configuration

```toml
# config/local.toml.example
[router_dhcp]
enabled = false                          # opt-in
interval_secs = 600                      # 10 min
tick_timeout_secs = 90                   # SIGKILL the Python child if a tick exceeds this
url = "http://192.168.0.1"
username_env = "ROUTER_USERNAME"
password_env = "ROUTER_PASSWORD"
```

Credentials are read from the env vars named here; never stored in `local.toml` directly. Davis spawns the Python child with these env values forwarded.

If `enabled = true` but the named env vars are unset at worker start, the worker writes one diary entry (`ROUTER.disabled.no.creds`) and self-disables; subsequent ticks are no-ops. A daemon restart re-checks.

### Failure semantics summary

| Layer | Trigger | Rust action | User-facing surface |
|---|---|---|---|
| L1 Spawn | python binary missing, PLAYWRIGHT_BROWSERS_PATH unreachable | `tracing::warn!`; diary `spawn.fail` (deduped); next tick retries | logs |
| L2 Reported | Python printed `{"status":"error",...}` | `tracing::warn!` with stage+reason; diary `<stage>.fail` (deduped) | logs |
| L3 Crashed | Python died without final JSON, or tick_timeout fired | `tracing::error!` with exit_code + stderr tail (256B max); diary `crash` (deduped) | logs |
| Creds missing | `enabled=true` + env vars unset | One-shot diary; worker self-disables | logs |

There is no alerting tier. The user explicitly chose tracing+diary only.

### Process safety

- Worker spawned via `tokio::spawn`; panics inside it are absorbed by the task boundary, never propagate to Davis main.
- Python children spawned with `kill_on_drop(true)`; daemon shutdown reliably reaps them.
- Tick timeout (`tick_timeout_secs`) prevents a hung Chromium from accumulating across ticks.
- Worker holds no open file handles between ticks (no on-disk state).

## CLAUDE.md change

Replace the Python-boundary line in CLAUDE.md (currently: "Python side (`crawl4ai_adapter/`) owns only: crawl4ai pruning + trafilatura + learned-rules CSS extraction. All LLM calls live in Rust (`src/article_memory/llm_client.rs`). Don't let LLM logic drift back into Python.") with:

```
Python side exists for one reason: browser-layer automation (Chromium /
Playwright / Puppeteer-style DOM operation, HTML extraction). Anything
that does not need a browser belongs in Rust. Two adapters live there
today:

- `crawl4ai_adapter/` — article crawling: crawl4ai pruning + trafilatura
  + learned-rules CSS extraction.
- `router_adapter/` — LAN device admin pages where the device only
  exposes a browser UI. Playwright-driven only; if a device offers a
  direct API, Davis talks to it from Rust.

All LLM calls stay in Rust (`src/article_memory/llm_client.rs`). New
Python adapters are admissible only if they require a browser; otherwise
the work goes in Rust.
```

## Testing strategy

### Unit tests (`cargo test --lib`)

| Test | Module | What it pins down |
|---|---|---|
| `parse_outcome` table-driven | `router_supervisor` | Each shape of stdout/exit → correct enum variant. Includes empty stdout, malformed JSON, missing fields, valid all four variants |
| `tick_state_machine` happy path | `router_worker` | First tick writes diary; second tick no diary if same outcome (no-action ok) |
| `tick_state_machine` failure dedupe | `router_worker` | Three consecutive identical failures → one diary entry. Different stage → new entry |
| `tick_state_machine` recovery | `router_worker` | Failure → success transition writes RECOVERED diary with prev.failed.N |
| `credential_gate` | `router_worker` | enabled+missing creds → one diary, then 0 calls to RouterChecker on subsequent ticks |
| `RouterDhcpConfig` toml parsing | `app_config` | Defaults applied correctly when section omitted; explicit values respected |

All `router_worker` tests inject `FakeRouterChecker` (returns canned outcomes from a `Vec`) and `SpySink` (existing test double from `mempalace_sink::emitter`). Worker logic is 100% IO-free under test.

### Integration test (`tests/rust/router_supervisor_spawn.rs`)

One test. Spawns `tests/fixtures/router_stub.py` (a one-liner that prints a fixed valid JSON line) using `PythonRouterChecker`, asserts the resulting `RouterCheckOutcome` matches. Validates: env injection, stdout capture, last-line parse, kill_on_drop hygiene.

CI prerequisite: a working `python3` on `$PATH`. (Davis CI already has it for `crawl4ai_adapter/` tests.)

### What we do NOT test

- `router_adapter/router_dhcp_check.py` itself: no unit tests. Mocking Playwright internals costs more than the file's ~200 lines. Manual e2e is documented in `router_adapter/README.md`.
- No CI test against a live router (impossible in CI; user-only manual verification).
- No load tests (10-minute interval, single subprocess).

### Manual verification entry points

Two CLI subcommands:

```
daviszeroclaw router-dhcp install     # Step 3: provision venv + Chromium
daviszeroclaw router-dhcp run-once    # Step 6: invoke checker once, print outcome
```

`run-once` calls `PythonRouterChecker::check_once()` directly and prints the outcome to stdout. Bypasses worker, dedupe, and diary. For human verification when changing selectors or debugging.

## Implementation plan

Six steps, five commits. Each step is independently testable, fully passes `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` + `cargo test --lib`, and can be reverted in isolation.

### Step 1 — Configuration scaffold (no runtime effect)

- `app_config.rs`: add `RouterDhcpConfig` struct, `LocalConfig.router_dhcp` field with `#[serde(default)]`.
- `lib.rs`: re-export `RouterDhcpConfig`.
- `config/local.toml.example`: add commented `[router_dhcp]` section.
- `CLAUDE.md`: replace Python-boundary paragraph.
- TDD: write the toml-parsing test first (red) → add struct (green).

### Step 2 — `RouterCheckOutcome` + `parse_outcome`

- New file `src/router_supervisor.rs` with **only** types and the pure `parse_outcome` function. No spawn logic yet.
- TDD: write all `parse_outcome` table cases first (red) → implement (green).

**Commit 1 = Steps 1 + 2** (both inert; no callers).

### Step 3 — Python adapter (independent of Rust)

- `router_adapter/pyproject.toml`: dependencies `playwright` + `python-dotenv`; tool config minimal. Mirrors `crawl4ai_adapter/`'s structure (no separate package, just a runnable module).
- `router_adapter/router_dhcp_check.py`: port `router-dhcp-check.ts`. File-top docstring documents the stdout-protocol contract. Outermost `try/except` enforces `{"status":"error","stage":"unhandled",...}` on any uncaught exception. Reads `ROUTER_URL`, `ROUTER_USERNAME`, `ROUTER_PASSWORD` from env, with no `'admin'/'admin'` defaults. Reads `PLAYWRIGHT_BROWSERS_PATH` if set; otherwise lets Playwright use its default.
- Provisioning: extend the existing `daviszeroclaw crawl install` CLI pattern (`src/cli/crawl.rs`) — add a sibling subcommand `daviszeroclaw router-dhcp install` that creates `.runtime/davis/router-adapter-venv/` via `python3 -m venv`, runs `pip install -e router_adapter/`, and runs `playwright install chromium` with `PLAYWRIGHT_BROWSERS_PATH=.runtime/davis/playwright-browsers/` set. The shared browsers path means crawl4ai and router_adapter resolve to the same Chromium on disk. (If the user already provisioned crawl4ai's Chromium under a different `PLAYWRIGHT_BROWSERS_PATH`, the install command surfaces a clear error rather than silently double-installing.)
- `router_adapter/README.md`: documents the `daviszeroclaw router-dhcp install` flow as the canonical setup; includes a manual fallback (raw `python3 -m venv` + `pip install`) for debugging; documents how to update selectors when router firmware changes.
- Manual verification: connect to the router LAN, run `.runtime/davis/router-adapter-venv/bin/python -m router_adapter.router_dhcp_check`, confirm stdout last line is valid JSON regardless of outcome.

**Commit 2.**

Note: Step 6 below (the CLI step) covers `router-dhcp run-once`. The provisioning subcommand `router-dhcp install` is added in this Step 3 because it's part of getting Python ready to run; the two CLI subcommands share the same `src/cli/router_dhcp.rs` module.

### Step 4 — `RouterChecker` trait + `PythonRouterChecker` + integration test

- `router_supervisor.rs`: add `RouterChecker` trait (`async fn check_once`) and `PythonRouterChecker` impl. Resolves Python via the same approach as `crawl4ai_supervisor::resolve_python_binary` (look under `{runtime}/<adapter>-venv/bin/python`), but for `router-adapter-venv`. `PLAYWRIGHT_BROWSERS_PATH` constant lives in `runtime_paths.rs`.
- Spawn implementation: `tokio::process::Command` with stdin null, stdout/stderr piped, `kill_on_drop(true)`. Wraps in `tokio::time::timeout(tick_timeout_secs)`; timeout → kill child → `Crashed { exit_code: None, stderr_tail: "<timeout>" }`.
- `tests/fixtures/router_stub.py`: one print statement with a fixed valid OK JSON.
- `tests/rust/router_supervisor_spawn.rs`: integration test using the stub. Asserts `Ok { action: None, .. }`.
- Add the test target to `Cargo.toml` (`[[test]]` block).

**Commit 3.**

### Step 5 — `RouterWorker` + dedupe + `/health`

- `src/router_worker.rs`: holds `Arc<dyn RouterChecker>`, `Arc<dyn MempalaceEmitter>`, config, in-memory state. `run_one_tick()`, `run_loop()`, `health_snapshot()`. Implements credential gate, dedupe, AAAK diary formatting.
- `local_proxy.rs` (or wherever the daemon's main loop lives — probably `src/bin/davis_local_proxy.rs`): on startup, if `config.router_dhcp.enabled`, `tokio::spawn(worker.run_loop())`. Stash a handle for `/health` access.
- `local_proxy.rs` `/health` handler: include `router_dhcp` snapshot.
- TDD: write the four worker state-machine tests first → implement.

**Commit 4.**

### Step 6 — `daviszeroclaw router-dhcp run-once` CLI action

- `src/cli/router_dhcp.rs` already exists from Step 3 (with the `install` action). Add the `run-once` action: build a `PythonRouterChecker`, call `check_once().await`, print outcome to stdout.
- No diary writes from this path (it's a debugging tool).

**Commit 5.**

### Step dependency graph

```
1 (config + CLAUDE.md) ─┐
                        ├─→ 5 (worker) ──→ 6 (cli)
2 (parse + types)    ──┼─→ 4 (trait + spawn)
3 (python adapter)   ──┘   (Step 4 uses tests/fixtures/router_stub.py,
                            independent of Step 3's real adapter)
```

Step 3 and Step 4 are decoupled via the stub. Step 3 must be in place before Step 5's worker runs in production with `enabled=true`.

### CI gates

```
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --lib
cargo test --test router_supervisor_spawn   # Step 4+
```

## Out of scope (explicit)

- No KG predicates. (Future: if router control grows into a subsystem with multiple checks across multiple devices, introduce a generic `DeviceConfigState(host → key:value)` predicate then.)
- No Davis-managed crontab migration. The user has confirmed the original Faillone/Automation cron is not active.
- No screenshot capture on failure. (User explicitly chose tracing+diary only.)
- No multi-router or multi-device support. The config is single-instance.
- No retry-with-backoff at the worker layer. Ten-minute interval is the natural backoff.

## Open risks

- **Selector drift after router firmware updates.** Mitigation: failures are deduped + visible in diary; user runs `daviszeroclaw router-dhcp run-once` to debug; updates `router_adapter/router_dhcp_check.py` selectors. This is the single largest ongoing maintenance vector and is unavoidable for browser-driven automation.
- **Chromium update dragging in two adapters at once.** Mitigation: shared `PLAYWRIGHT_BROWSERS_PATH` means one upgrade event affects both; manual coordination on the user's side. Acceptable because both adapters are owned by the same user.
- **`PLAYWRIGHT_BROWSERS_PATH` constant divergence.** Mitigation: declared once in `runtime_paths.rs`, referenced everywhere. Lint/review catches any string literal pointing elsewhere.
