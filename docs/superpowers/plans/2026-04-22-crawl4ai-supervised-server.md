# Crawl4AI Supervised Server — Post-landing Notes

> Historical implementation plan (17 tasks, Phase 0 → 1c) landed on branch
> `refactor/control-aliases-to-toml` between 2026-04-22 and 2026-04-23, head
> `fc8c76d` (+ doc-only follow-ups). This file is the slim post-mortem — it
> keeps only what future work needs to know. Commits (`git log --grep crawl4ai`
> or `git log refactor/p0-declarative-model-routing..HEAD`) are the source of
> truth for the "how".

## Current shape

- **Python side:** `crawl4ai_adapter/server.py` (FastAPI) + `server_main.py`
  (uvicorn entrypoint). Runs as a long-lived child of the Rust daemon.
  `GET /health` echoes a `versions` map via `importlib.metadata`. `POST /crawl`
  is the single call site. `crawl4ai_adapter/__main__.py` only keeps the
  interactive `login` subcommand (TTY-bound; doesn't fit HTTP).
- **Rust side:** `src/crawl4ai_supervisor.rs` spawns the adapter with
  `tokio::process::Command`, probes `/health` until ready, restarts on exit
  with exponential backoff (1s → 30s cap, `RESTART_BUDGET = 5`). Structured
  503 past a 5 s grace window short-circuits the 30 s startup timeout — you
  see the real `crawl4ai_import_failed` instead of a silent wait.
- **Errors:** `Crawl4aiError` enum in `src/crawl4ai_error.rs`. `issue_type()`
  returns three stable strings consumed by `src/support.rs:60-117`:
  `crawl4ai_unavailable` / `site_changed` / `auth_required`. Locked by the
  `issue_type_mapping_is_stable` unit test — rename variants freely, don't
  rename strings.
- **Concurrency:** `AppState` owns `Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>`
  keyed by profile name. Same-profile calls serialize (no Chromium
  `SingletonLock` collisions); different profiles stay concurrent.
  `/express/*` uses `futures::future::join_all` to fan out ali + jd.
- **Config:** `[crawl4ai]` in `local.toml` — `enabled`, `base_url`,
  `timeout_secs`, and rendering flags. `transport` and `python` fields are
  gone. Changing the port here just works; supervisor and express both derive
  from the same string.
- **Operator CLI:** `daviszeroclaw crawl service {status,restart,stop}` reads
  `.runtime/davis/crawl4ai.pid`, probes `/health`, sends SIGTERM.
- **Runtime files:** `.runtime/davis/crawl4ai.pid`, `crawl4ai.log`,
  `crawl4ai-venv/`. Adapter logs go to `crawl4ai.log`; the daemon's own trace
  output still lands in `daemon.log`.

## Deliberate non-goals

- **No version pinning.** We stay current with crawl4ai. Breakages surface
  through `/health`'s versions map and the "crawl4ai adapter ready" info
  line; when upstream changes, we update our code rather than rot on old
  deps. If you ever reintroduce a `requirements.txt`, re-read Task 5's
  rationale first.
- **No per-request Python subprocess.** The cold-start overhead was ~300–800
  ms per call. Don't bring it back without a strong reason.

## Known-deferred items

These live here (not in an issue tracker) because each has a clear trigger
condition — circle back when it fires, not before.

1. **Graceful shutdown on daemon SIGTERM.** `daviszeroclaw stop` kills the
   daemon but leaves the adapter as an orphan on :11235.
   `tokio::process::Command::kill_on_drop(true)` only fires when `Child`
   is dropped, which doesn't happen reliably under external SIGTERM (runtime
   can exit before drop handlers run). Fix when it becomes user-visible:
   add `supervisor.shutdown().await` to the daemon's shutdown path —
   SIGTERM → `wait()` with a short budget → SIGKILL fallback. **Trigger:**
   launchd `KeepAlive` / zero-downtime upgrade / second operator. Today the
   workaround is `kill $(cat .runtime/davis/crawl4ai.pid)` after `stop`.

2. **HTML payload reflective parsing (was P1-6).** `src/express.rs` still
   extracts structured data from a `data-davis-express-payload` marker
   embedded in the HTML the adapter returns. Cleaner: put the structured
   result directly in the adapter's JSON response. **Trigger:** taobao or
   jd change their DOM and the marker breaks.

3. **Bounded `/express/*` fan-out.** `join_all` over `EXPRESS_SOURCES` has
   no concurrency cap. Fine today (2 sources). **Trigger:** adding a third
   source — switch to `futures::stream::buffer_unordered(N)` at that point.
   `TODO` comment already in `src/express.rs`.

4. **`storage_state.json` ↔ `user_data_dir` consolidation (was P2-13).**
   Cosmetic only; we write one but read the other. Defer until a janitor
   pass feels worth it.

5. **Span-event metrics.** No Prometheus-style counters for restart
   frequency / adapter latency today. **Trigger:** when observability
   asks for dashboards — will need the `metrics` crate.

6. **Article-memory + crawl4ai.** Module doesn't exist yet. When it does,
   it needs to go through `AppState::crawl4ai_profile_lock(profile)` or it
   will collide with express on the same profile.

## Operations quick reference

- **Status + versions:** `daviszeroclaw crawl service status` (pid, alive,
  `/health` body with `versions`).
- **Force restart the adapter:** `daviszeroclaw crawl service restart`. The
  supervisor sees SIGTERM, falls into the restart loop, comes back healthy.
- **Smoke test after upgrade:** hit `/express/auth-status`. Both sources
  should return `status ∈ {ok, empty, needs_reauth}`, never `upstream_error`.
  `needs_reauth` + `issue_type: "auth_required"` is a perfectly healthy
  result when nobody's logged in to taobao/jd.
- **Log locations:** `.runtime/davis/crawl4ai.log` (adapter stdout/stderr),
  `.runtime/davis/daemon.log` (Rust-side tracing, including
  `"crawl4ai adapter ready versions=…"`).
- **If `/health` keeps 503'ing:** venv broke. Run `daviszeroclaw crawl
  install` (re-upgrades crawl4ai + fastapi + uvicorn + pydantic +
  playwright + patchright). Body of the 503 tells you which import failed.

## Contract strings that must not drift

- `issue_type()` — `"crawl4ai_unavailable"` / `"site_changed"` / `"auth_required"`.
  Consumed by `src/support.rs:60-117`. Rename the enum variants freely,
  never rename the strings without updating `support.rs` in the same commit.
- `data-davis-express-payload` — HTML marker produced by the JS snippets in
  `src/express.rs` and consumed by `extract_payload_from_html`. Keep the
  attribute name until item 2 above lands.
- `crawl4ai.pid` / `crawl4ai.log` file names — read by
  `daviszeroclaw crawl service` subcommands. Don't rename without updating
  `runtime_paths.rs` + CLI dispatch together.
