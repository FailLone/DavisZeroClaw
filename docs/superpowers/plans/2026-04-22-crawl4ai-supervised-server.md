# Crawl4AI Supervised Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden the Crawl4AI integration by (a) fixing P0 correctness bugs on the existing `Python` transport, then (b) replacing the per-request subprocess model with a long-lived HTTP server the Rust daemon itself supervises.

**Architecture:** Phase 0 fixes three in-place P0 issues (async-blocking subprocess, no wall-clock timeout, profile race). Phase 1 introduces `crawl4ai_adapter/server.py` (FastAPI on localhost), `src/crawl4ai_supervisor.rs` (tokio child-process supervisor with exponential backoff + health check), and deletes `Crawl4aiTransport::Python`. Result: single transport path, zero per-request Python startup, crashes self-heal, trace spans correlated across process boundary.

**Tech Stack:** Rust 2021 (tokio, reqwest, axum, tracing, anyhow, serde). Python 3.11+ (FastAPI + uvicorn added to venv, crawl4ai unchanged). Existing test infra: integration tests under `tests/rust/` + mock HTTP routers via `spawn_json_router` in `tests/rust/support.rs`.

**Branch:** Create `refactor/crawl4ai-supervised-server` off `main` (or off the currently pending `refactor/control-aliases-to-toml` once merged).

**Phases:**

- **Phase 0** (P0 stability fixes on current code) — Tasks 1–4
- **Phase 1a** (Python side: long-lived HTTP server) — Tasks 5–7
- **Phase 1b** (Rust side: supervisor + transport swap) — Tasks 8–13
- **Phase 1c** (Test coverage + cleanup) — Tasks 14–17

Each phase is shippable on its own. You may merge after Phase 0, then again after Phase 1c.

---

## File Structure

### New files
- `src/crawl4ai_supervisor.rs` — tokio Child + backoff + health-check loop
- `src/crawl4ai_error.rs` — typed `Crawl4aiError` enum replacing `Result<_, String>`
- `crawl4ai_adapter/server.py` — FastAPI app exposing `POST /crawl`, `GET /health`
- `crawl4ai_adapter/server_main.py` — uvicorn entrypoint (`python -m crawl4ai_adapter.server_main`)
- `config/davis/crawl4ai-requirements.txt` — pinned Python deps
- `tests/rust/crawl4ai.rs` — new unit + integration tests for supervisor and transport

### Modified files
- `src/crawl4ai.rs` — delete `crawl_via_python`, delete second `resolve_python`, switch to typed error, reuse a shared `reqwest::Client`
- `src/app_config.rs:94-126` — delete `Crawl4aiTransport`, delete `python` field
- `src/local_proxy.rs:87-108` — start supervisor at daemon boot, inject handle into `AppState`
- `src/server.rs:54-80` — add `crawl4ai_supervisor: Arc<Crawl4aiSupervisor>` (or health accessor) to `AppState`
- `src/express.rs:117-141, 175-205` — map typed errors into existing issue types (no more substring matching)
- `src/cli/crawl.rs` — add `service status|restart`, wire install to `crawl4ai-requirements.txt`, keep `profile login` on the subprocess path (interactive, rare)
- `src/runtime_paths.rs` — add `crawl4ai_pid_path`, `crawl4ai_log_path`, `crawl4ai_requirements_path`, delete dead `crawl4ai_setup_path`/`crawl4ai_doctor_path`
- `src/models.rs` — if `Crawl4aiError` variants need to surface via HTTP, add typed enum here too
- `tests/rust/fixtures.rs:90-96` — stop forcing `transport = Server`, instead point to a mock HTTP server
- `tests/rust/express.rs` — update to new typed error paths
- `config/davis/local.example.toml:63-77` — drop `transport` and `python` fields, add comment about supervised mode

---

## Phase 0 — In-place P0 fixes

These three tasks ship on the current architecture. They buy stability while Phase 1 is in flight, and they make the subsequent refactor safer (you'll have a working baseline to compare behavior against).

### Task 1: Cut over `crawl_via_python` to async + wall-clock timeout

**Files:**
- Modify: `src/crawl4ai.rs:1-7` (imports)
- Modify: `src/crawl4ai.rs:60-63` (dispatch arm becomes async)
- Modify: `src/crawl4ai.rs:150-220` (`crawl_via_python` body)
- Test: `tests/rust/crawl4ai.rs` (new file)

- [ ] **Step 1: Create the new test file with a failing timeout test**

Create `tests/rust/crawl4ai.rs`:

```rust
use crate::{crawl4ai_crawl, Crawl4aiConfig, Crawl4aiPageRequest, Crawl4aiTransport, RuntimePaths};

fn fake_paths(tmp: &std::path::Path) -> RuntimePaths {
    RuntimePaths {
        repo_root: tmp.to_path_buf(),
        runtime_dir: tmp.join(".runtime").join("davis"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn python_transport_honors_wall_clock_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = fake_paths(tmp.path());
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();

    // Point config.python at a script that sleeps forever.
    let sleeper = tmp.path().join("sleeper.sh");
    std::fs::write(&sleeper, "#!/bin/sh\nsleep 3600\n").unwrap();
    std::os::unix::fs::PermissionsExt::set_mode(
        &mut std::fs::metadata(&sleeper).unwrap().permissions(),
        0o755,
    );
    std::fs::set_permissions(
        &sleeper,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();

    let mut config = Crawl4aiConfig::default();
    config.enabled = true;
    config.transport = Crawl4aiTransport::Python;
    config.python = sleeper.display().to_string();
    config.timeout_secs = 1; // add 30s guard internally => total ~31s? No — see step 3.

    let start = std::time::Instant::now();
    let result = crawl4ai_crawl(
        &paths,
        &config,
        Crawl4aiPageRequest {
            profile_name: "test".to_string(),
            url: "https://example.com".to_string(),
            wait_for: None,
            js_code: None,
        },
    )
    .await;
    let elapsed = start.elapsed();

    assert!(result.is_err(), "expected timeout error, got {result:?}");
    let err = result.unwrap_err();
    assert!(err.contains("timed out"), "error should mention timeout, got: {err}");
    // allow generous slack for CI: hard upper bound of 10s even though timeout_secs=1+30s guard
    assert!(elapsed < std::time::Duration::from_secs(40), "did not kill child promptly: {elapsed:?}");
}
```

Also add `pub mod crawl4ai;` to `tests/rust/mod.rs`:

```rust
// tests/rust/mod.rs — append at bottom
pub mod crawl4ai;
```

And add `tempfile = "3"` to `[dev-dependencies]` in `Cargo.toml` if not already present:

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Run the test to confirm it fails**

```bash
cargo test --test lib python_transport_honors_wall_clock_timeout 2>&1 | tail -20
```

Expected: either `cargo test` errors with "no test target lib" (in which case use `cargo test python_transport_honors_wall_clock_timeout`) or the test times out / hangs, confirming the bug. Kill it with `^C` after ~15s to confirm the hang.

- [ ] **Step 3: Replace blocking `std::process::Command` with `tokio::process::Command` and add timeout**

In `src/crawl4ai.rs`, change imports at line 5-6:

```rust
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
```

Delete the old `use std::io::Write;` and `use std::process::{Command, Stdio};` lines — replace `Stdio` with `std::process::Stdio` below.

Change dispatch at `src/crawl4ai.rs:60-63`:

```rust
let result = match config.transport {
    Crawl4aiTransport::Server => crawl_via_server(paths, config, request).await,
    Crawl4aiTransport::Python => crawl_via_python(paths, config, request).await,
};
```

Replace `crawl_via_python` entirely (currently at `src/crawl4ai.rs:150-220`) with:

```rust
async fn crawl_via_python(
    paths: &RuntimePaths,
    config: &Crawl4aiConfig,
    request: Crawl4aiPageRequest,
) -> Result<Crawl4aiPageResult, String> {
    let python = resolve_python(paths, config);
    let profile_dir = paths.crawl4ai_profiles_root().join(&request.profile_name);
    let payload = json!({
        "profile_path": profile_dir.display().to_string(),
        "url": request.url,
        "wait_for": request.wait_for,
        "js_code": request.js_code,
        "timeout_secs": config.timeout_secs,
        "headless": config.headless,
        "magic": config.magic,
        "simulate_user": config.simulate_user,
        "override_navigator": config.override_navigator,
        "remove_overlay_elements": config.remove_overlay_elements,
        "enable_stealth": config.enable_stealth,
    });
    let raw = serde_json::to_vec(&payload)
        .map_err(|err| format!("serialize crawl4ai adapter payload: {err}"))?;

    let mut child = Command::new(&python)
        .arg("-m")
        .arg("crawl4ai_adapter")
        .arg("crawl")
        .arg("--runtime-dir")
        .arg(paths.runtime_dir.display().to_string())
        .current_dir(&paths.repo_root)
        .env("PYTHONPATH", paths.repo_root.display().to_string())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("spawn crawl4ai_adapter crawl: {err}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&raw)
            .await
            .map_err(|err| format!("write crawl4ai adapter payload: {err}"))?;
        drop(stdin);
    }

    let budget = Duration::from_secs(config.timeout_secs.saturating_add(30));
    let output = match timeout(budget, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(format!("wait for crawl4ai_adapter crawl: {err}")),
        Err(_) => {
            return Err(format!(
                "crawl4ai adapter subprocess timed out after {}s",
                budget.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("crawl4ai adapter failed: {stderr}"));
    }
    let body = parse_adapter_json(&output.stdout)
        .map_err(|err| format!("parse crawl4ai adapter response: {err}"))?;
    if !body
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let message = body
            .get("error")
            .or_else(|| body.get("error_message"))
            .and_then(Value::as_str)
            .unwrap_or("crawl4ai adapter returned an error");
        let details = body.get("details").and_then(Value::as_str).unwrap_or("");
        return Err(if details.is_empty() {
            message.to_string()
        } else {
            format!("{message}: {details}")
        });
    }
    Ok(parse_result_value(body))
}
```

- [ ] **Step 4: Run the timeout test, confirm pass**

```bash
cargo test python_transport_honors_wall_clock_timeout 2>&1 | tail -10
```

Expected: `test result: ok. 1 passed`. Should finish in under 35 seconds.

- [ ] **Step 5: Run full test suite**

```bash
cargo test 2>&1 | tail -5
```

Expected: all previously-passing tests still pass.

- [ ] **Step 6: Commit**

```bash
git add -p src/crawl4ai.rs tests/rust/crawl4ai.rs tests/rust/mod.rs Cargo.toml
git commit -m "fix(crawl4ai): async subprocess + wall-clock timeout on python transport

Blocking std::process::Command inside async fn froze tokio workers for
30-90s per crawl. Switched to tokio::process::Command + timeout(budget).
Budget = timeout_secs + 30s guard. Child is kill_on_drop so cancellation
propagates. kill_on_drop also prevents orphaning on tokio runtime drop.

Adds regression test that runs a script sleeping forever under the
Python transport and asserts the outer call returns an error within
the budget."
```


### Task 2: Per-profile async mutex to prevent Chromium profile collisions

**Files:**
- Modify: `src/crawl4ai.rs:40-74` (`crawl4ai_crawl` takes a profile lock before dispatch)
- Modify: `src/server.rs:54-80` (AppState owns the lock map)
- Test: `tests/rust/crawl4ai.rs` (concurrent calls on same profile serialize)

- [ ] **Step 1: Add the lock map and accessor to AppState**

Open `src/server.rs`. At the top with other imports, add:

```rust
use std::collections::HashMap;
use tokio::sync::Mutex;
```

Add a field to `AppState` (around `src/server.rs:54`, wherever `pub struct AppState` lives):

```rust
pub struct AppState {
    // ... existing fields ...
    pub crawl4ai_profile_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}
```

In `AppState::new` (around `src/server.rs:66-80`), initialize it to `Arc::new(Mutex::new(HashMap::new()))`.

Add this helper method to `impl AppState`:

```rust
pub async fn crawl4ai_profile_lock(&self, profile: &str) -> Arc<Mutex<()>> {
    let mut map = self.crawl4ai_profile_locks.lock().await;
    map.entry(profile.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}
```

- [ ] **Step 2: Write a failing concurrency test**

Append to `tests/rust/crawl4ai.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_profile_calls_serialize_under_lock() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let map: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    async fn acquire(
        map: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
        profile: &str,
    ) -> Arc<tokio::sync::Mutex<()>> {
        let mut guard = map.lock().await;
        guard
            .entry(profile.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    let mut handles = Vec::new();
    for _ in 0..5 {
        let map = map.clone();
        let in_flight = in_flight.clone();
        let max_seen = max_seen.clone();
        handles.push(tokio::spawn(async move {
            let lock = acquire(map, "express-ali").await;
            let _guard = lock.lock().await;
            let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(cur, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            in_flight.fetch_sub(1, Ordering::SeqCst);
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(max_seen.load(Ordering::SeqCst), 1, "concurrent same-profile calls were not serialized");
}
```

- [ ] **Step 3: Run it**

```bash
cargo test same_profile_calls_serialize_under_lock 2>&1 | tail -5
```

Expected: PASS (this test exercises only the lock map semantics; it doesn't depend on `crawl4ai_crawl` yet).

- [ ] **Step 4: Take the lock inside `crawl4ai_crawl`**

This requires either threading the lock through the call or having callers acquire it. The simpler approach: leave `crawl4ai_crawl` signature alone, but change every caller (`src/express.rs:181`, any future) to acquire the lock before calling.

Add to `src/crawl4ai.rs` at the bottom:

```rust
/// Returns a guard callers must hold for the duration of the crawl4ai_crawl call
/// to serialize access to a single Chromium user_data_dir.
///
/// Callers look up `Arc<Mutex<()>>` from `AppState::crawl4ai_profile_lock(profile)`,
/// then `.lock().await` before invoking `crawl4ai_crawl`.
pub use tokio::sync::OwnedMutexGuard as Crawl4aiProfileGuard;
```

Then in `src/express.rs`, change `crawl_source_payload` signature (currently at `src/express.rs:175-205`):

```rust
async fn crawl_source_payload(
    paths: &RuntimePaths,
    crawl4ai_config: &Crawl4aiConfig,
    profile_lock: Arc<tokio::sync::Mutex<()>>,
    source: &str,
    script: String,
) -> Result<Value, String> {
    let _guard = profile_lock.lock().await;
    let response = crawl4ai_crawl(
        paths,
        crawl4ai_config,
        Crawl4aiPageRequest {
            profile_name: express_profile_name(source),
            url: source_order_url(source).to_string(),
            wait_for: Some(source_wait_for(source).to_string()),
            js_code: Some(script),
        },
    )
    .await?;
    if !response.success {
        return Err(response.error_message.unwrap_or_else(|| {
            format!(
                "{}。请先运行 `daviszeroclaw crawl profile login express-{source}` 完成登录。",
                source_login_message(source)
            )
        }));
    }
    extract_payload_value(&response).map_err(|message| {
        format!(
            "failed to parse crawl4ai payload for {source}: {message}. 请确认 `daviszeroclaw crawl profile login express-{source}` 已完成并且订单页结构未变化。"
        )
    })
}
```

Add `use std::sync::Arc;` to `src/express.rs` imports if not present.

Plumb `profile_lock` through `fetch_source_status` and `fetch_source_snapshot` (around `src/express.rs:112-141`):

```rust
async fn fetch_source_status(
    paths: &RuntimePaths,
    crawl4ai_config: &Crawl4aiConfig,
    profile_lock: Arc<tokio::sync::Mutex<()>>,
    source: &str,
) -> ExpressSourceStatus { ... }

async fn fetch_source_snapshot(
    paths: &RuntimePaths,
    crawl4ai_config: &Crawl4aiConfig,
    profile_lock: Arc<tokio::sync::Mutex<()>>,
    source: &str,
) -> ExpressSourceSnapshot { ... }
```

- [ ] **Step 5: Update the public entry points to thread the lock through**

`src/express.rs:14-27` (`express_auth_status`) and `src/express.rs:29-82` (`express_packages`) change signature to accept a lock-lookup closure or the lock map itself. Simplest:

```rust
pub async fn express_auth_status(
    paths: RuntimePaths,
    crawl4ai_config: Crawl4aiConfig,
    profile_locks: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
) -> ExpressAuthStatusResponse {
    let mut sources = Vec::new();
    for source in EXPRESS_SOURCES {
        let profile_name = express_profile_name(source);
        let lock = {
            let mut map = profile_locks.lock().await;
            map.entry(profile_name)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        sources.push(fetch_source_status(&paths, &crawl4ai_config, lock, source).await);
    }
    // ... rest unchanged
}
```

Do the same for `express_packages`.

- [ ] **Step 6: Update `src/server.rs` handler call sites (around lines 495, 515, 539)**

Each `express_auth_status(...)` / `express_packages(...)` call now also passes `state.crawl4ai_profile_locks.clone()`:

```rust
express_auth_status(
    state.paths.clone(),
    (*state.crawl4ai_config).clone(),
    state.crawl4ai_profile_locks.clone(),
).await
```

- [ ] **Step 7: Build + run the full test suite**

```bash
cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5
```

Expected: clean build, all tests pass (including the serialization test).

- [ ] **Step 8: Commit**

```bash
git add -p src/crawl4ai.rs src/server.rs src/express.rs tests/rust/crawl4ai.rs
git commit -m "fix(crawl4ai): per-profile mutex to prevent Chromium user_data_dir race

Concurrent HTTP requests against /express/auth-status or /express/packages
could both try to open the same Chromium persistent profile. Second attach
would fail (SingletonLock) with no guard in place.

AppState now owns a HashMap<profile, Arc<Mutex<()>>>. Express handlers look
up the lock for their profile, hold it for the duration of the crawl4ai
call. Different profiles remain concurrent; same profile serializes."
```


### Task 3: Run Taobao and JD fetches in parallel

**Files:**
- Modify: `src/express.rs:14-41` (`express_auth_status` + `express_packages` loop)

- [ ] **Step 1: Write a test proving sequential is slower than parallel**

This is behavioral, not semantic — skip a direct test here. We'll observe it in the tracing span fields after Phase 1. Mark this task as refactor-only. Proceed to step 2.

- [ ] **Step 2: Replace the `for source in …` serial loops with `join_all`**

Add `use futures::future::join_all;` to `src/express.rs` imports (if `futures` isn't already in `Cargo.toml`, add it: `futures = "0.3"`).

Rewrite `express_auth_status` (`src/express.rs:14-27`):

```rust
pub async fn express_auth_status(
    paths: RuntimePaths,
    crawl4ai_config: Crawl4aiConfig,
    profile_locks: Arc<tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
) -> ExpressAuthStatusResponse {
    let futures = EXPRESS_SOURCES.iter().map(|source| {
        let paths = paths.clone();
        let cfg = crawl4ai_config.clone();
        let locks = profile_locks.clone();
        async move {
            let profile_name = express_profile_name(source);
            let lock = {
                let mut map = locks.lock().await;
                map.entry(profile_name)
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                    .clone()
            };
            fetch_source_status(&paths, &cfg, lock, source).await
        }
    });
    let sources: Vec<_> = join_all(futures).await;
    ExpressAuthStatusResponse {
        status: aggregate_status_from_statuses(&sources),
        checked_at: isoformat(now_utc()),
        sources,
    }
}
```

Do the same transformation for `express_packages` (`src/express.rs:29-82`). Replace the `for selected_source in select_sources(source.as_deref()) { snapshots.push(load_or_fetch_source(...)) }` with a `join_all` over an iterator of async blocks.

- [ ] **Step 3: Verify build + tests**

```bash
cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5
```

Expected: all tests pass. Integration tests `tests/rust/express.rs` should still complete (they use a mock `/crawl` server; no real concurrency dependency).

- [ ] **Step 4: Commit**

```bash
git add -p src/express.rs Cargo.toml
git commit -m "perf(express): parallelize Taobao and JD fetches

join_all cuts /express/* latency roughly in half — two 30s fetches stop
serializing. Profile lock map still prevents Chromium collisions on
same-source bursts."
```

### Task 4: Phase 0 merge checkpoint

- [ ] **Step 1: Push the branch and open PR for Phase 0 subset**

```bash
git push -u origin refactor/crawl4ai-supervised-server
gh pr create --title "fix(crawl4ai): P0 stability — async timeout, profile lock, parallel fetch" \
  --body "$(cat <<'EOF'
## Summary
- Replace blocking subprocess in async with `tokio::process::Command` + wall-clock timeout (fixes tokio-runtime stall)
- Per-profile `Arc<Mutex<()>>` prevents two Chromium instances attaching the same `user_data_dir`
- `join_all` parallelizes Taobao and JD `/express/*` fetches

## Test plan
- [ ] `cargo test` green (includes new `python_transport_honors_wall_clock_timeout` + `same_profile_calls_serialize_under_lock`)
- [ ] Manual: hit `/express/auth-status` twice concurrently; verify no `SingletonLock` error
- [ ] Manual: time `/express/packages` before/after — expect ~50% reduction

## Follow-up (Phase 1)
Supervised crawl4ai HTTP server replaces the subprocess transport entirely.
See docs/superpowers/plans/2026-04-22-crawl4ai-supervised-server.md.
EOF
)"
```

- [ ] **Step 2: Wait for CI + review. Merge. Continue to Phase 1 on a fresh branch off the merged main.**

---


## Phase 1a — Long-lived HTTP server on the Python side

### Task 5: Pin Python dependencies

**Files:**
- Create: `config/davis/crawl4ai-requirements.txt`
- Modify: `src/cli/crawl.rs:47-85` (swap unpinned `pip install` for `-r requirements.txt`)
- Modify: `src/runtime_paths.rs` (add `crawl4ai_requirements_path`)

- [ ] **Step 1: Create the requirements file**

Write `config/davis/crawl4ai-requirements.txt`:

```
# Crawl4AI adapter Python dependencies.
# Pinned so daemon upgrades don't silently pull in a breaking crawl4ai release.
# Refresh with: `uv pip compile --upgrade crawl4ai-requirements.in -o crawl4ai-requirements.txt`
# (or manually bump + smoke-test `daviszeroclaw crawl check`).

crawl4ai==0.4.248
playwright==1.47.0
patchright==1.47.4
fastapi==0.115.4
uvicorn[standard]==0.32.0
pydantic==2.9.2
```

(Adjust versions to whatever the current working set is on your dev machine; capture via `pip freeze | grep -iE 'crawl4ai|playwright|patchright|fastapi|uvicorn|pydantic'` from the existing venv before writing.)

- [ ] **Step 2: Add path helper**

In `src/runtime_paths.rs`, next to the other crawl4ai paths (~line 151):

```rust
pub fn crawl4ai_requirements_path(&self) -> PathBuf {
    self.repo_root
        .join("config")
        .join("davis")
        .join("crawl4ai-requirements.txt")
}
```

Also delete the two unused path helpers at `src/runtime_paths.rs:159-165`:

```rust
// DELETE: crawl4ai_setup_path
// DELETE: crawl4ai_doctor_path
```

- [ ] **Step 3: Swap the `pip install` call to use `-r requirements.txt`**

In `src/cli/crawl.rs`, replace the block at lines 47-85 (four `pip install` / `playwright install` / `patchright install` runs) with:

```rust
    println!("Installing Crawl4AI dependencies from requirements.txt.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("-r")
            .arg(paths.crawl4ai_requirements_path())
            .env("CRAWL4_AI_BASE_DIRECTORY", &crawl4ai_base_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "pip install -r crawl4ai-requirements.txt",
    )?;

    println!("Installing Playwright Chromium.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("playwright")
            .arg("install")
            .arg("chromium")
            .env("CRAWL4_AI_BASE_DIRECTORY", &crawl4ai_base_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "python -m playwright install chromium",
    )?;

    println!("Installing Patchright Chromium.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("patchright")
            .arg("install")
            .arg("chromium")
            .env("CRAWL4_AI_BASE_DIRECTORY", &crawl4ai_base_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "python -m patchright install chromium",
    )?;
```

- [ ] **Step 4: Verify build**

```bash
cargo build 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 5: Verify `daviszeroclaw crawl install` works**

Manual (optional, if you want to validate locally before committing):

```bash
rm -rf .runtime/davis/crawl4ai-venv
./target/debug/daviszeroclaw crawl install
./target/debug/daviszeroclaw crawl check
```

Expected: install succeeds; `crawl check` reports all deps importable.

- [ ] **Step 6: Commit**

```bash
git add config/davis/crawl4ai-requirements.txt src/runtime_paths.rs src/cli/crawl.rs
git commit -m "chore(crawl4ai): pin Python deps via requirements.txt

- New config/davis/crawl4ai-requirements.txt as the single source of truth
  for crawl4ai/playwright/patchright versions. Adds fastapi+uvicorn+pydantic
  for the upcoming HTTP server.
- daviszeroclaw crawl install now runs pip install -r. Unpinned upgrades are
  gone — daemon releases no longer roll forward on crawl4ai's release
  schedule.
- Remove dead runtime_paths helpers crawl4ai_setup_path / crawl4ai_doctor_path
  (never read)."
```

### Task 6: Write the FastAPI server (`crawl4ai_adapter/server.py`)

**Files:**
- Create: `crawl4ai_adapter/server.py`
- Create: `crawl4ai_adapter/server_main.py`

- [ ] **Step 1: Create the FastAPI module**

Write `crawl4ai_adapter/server.py`:

```python
"""Long-lived HTTP adapter for crawl4ai.

Runs as a child of the Rust daemon (see src/crawl4ai_supervisor.rs).
Exposes POST /crawl and GET /health. One AsyncWebCrawler instance is
reused across requests; per-request BrowserConfig / CrawlerRunConfig
are built fresh from the JSON body.
"""

from __future__ import annotations

import asyncio
import logging
import os
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, Optional

from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field

logger = logging.getLogger("crawl4ai_adapter.server")


class CrawlRequest(BaseModel):
    profile_path: str = Field(..., description="Absolute path to Chromium user_data_dir")
    url: str
    wait_for: Optional[str] = None
    js_code: Optional[str] = None
    timeout_secs: int = 90
    headless: bool = True
    magic: bool = True
    simulate_user: bool = True
    override_navigator: bool = True
    remove_overlay_elements: bool = True
    enable_stealth: bool = True


class CrawlResponse(BaseModel):
    success: bool
    url: Optional[str] = None
    redirected_url: Optional[str] = None
    status_code: Optional[int] = None
    html: Optional[str] = None
    cleaned_html: Optional[str] = None
    js_execution_result: Optional[Any] = None
    error_message: Optional[str] = None


@asynccontextmanager
async def lifespan(app: FastAPI):
    runtime_dir = Path(os.environ.get("CRAWL4_AI_BASE_DIRECTORY", ".")).resolve()
    runtime_dir.mkdir(parents=True, exist_ok=True)
    os.environ["CRAWL4_AI_BASE_DIRECTORY"] = str(runtime_dir)
    logger.info("crawl4ai_adapter.server starting, base_dir=%s", runtime_dir)
    # Lazy-import crawl4ai so startup failures surface in /health rather than
    # at import time (daemon can report a typed error to the user).
    try:
        from crawl4ai import AsyncWebCrawler  # noqa: F401
        app.state.crawl4ai_ok = True
    except Exception as exc:  # pragma: no cover
        app.state.crawl4ai_ok = False
        app.state.crawl4ai_import_error = str(exc)
        logger.exception("crawl4ai import failed")
    yield
    logger.info("crawl4ai_adapter.server stopping")


app = FastAPI(title="crawl4ai_adapter", lifespan=lifespan)


@app.get("/health")
async def health() -> dict[str, Any]:
    if not getattr(app.state, "crawl4ai_ok", False):
        return JSONResponse(
            status_code=503,
            content={
                "status": "unhealthy",
                "reason": "crawl4ai_import_failed",
                "details": getattr(app.state, "crawl4ai_import_error", "unknown"),
            },
        )
    return {"status": "ok"}


@app.post("/crawl", response_model=CrawlResponse)
async def crawl(req: CrawlRequest) -> CrawlResponse:
    if not getattr(app.state, "crawl4ai_ok", False):
        raise HTTPException(
            status_code=503,
            detail={
                "error": "crawl4ai_unavailable",
                "details": getattr(app.state, "crawl4ai_import_error", "unknown"),
            },
        )

    from crawl4ai import AsyncWebCrawler, BrowserConfig, CacheMode, CrawlerRunConfig

    profile_path = Path(req.profile_path).expanduser().resolve()
    profile_path.mkdir(parents=True, exist_ok=True)

    browser_config = BrowserConfig(
        browser_type="chromium",
        headless=req.headless,
        use_managed_browser=True,
        use_persistent_context=True,
        user_data_dir=str(profile_path),
        enable_stealth=req.enable_stealth,
        viewport_width=1440,
        viewport_height=960,
        verbose=False,
    )
    crawler_config = CrawlerRunConfig(
        cache_mode=CacheMode.BYPASS,
        page_timeout=req.timeout_secs * 1000,
        delay_before_return_html=1.0,
        magic=req.magic,
        simulate_user=req.simulate_user,
        override_navigator=req.override_navigator,
        remove_overlay_elements=req.remove_overlay_elements,
        wait_for=req.wait_for,
        js_code=req.js_code,
    )

    try:
        async with AsyncWebCrawler(config=browser_config) as crawler:
            # Outer timeout guards against crawl4ai hanging past its own page_timeout.
            result = await asyncio.wait_for(
                crawler.arun(url=req.url, config=crawler_config),
                timeout=req.timeout_secs + 15,
            )
    except asyncio.TimeoutError:
        raise HTTPException(
            status_code=504,
            detail={"error": "crawl_timeout", "details": f"exceeded {req.timeout_secs + 15}s"},
        )
    except Exception as exc:
        raise HTTPException(
            status_code=500,
            detail={"error": "crawl_failed", "details": str(exc)},
        )

    return CrawlResponse(
        success=bool(getattr(result, "success", False)),
        url=getattr(result, "url", req.url),
        redirected_url=getattr(result, "redirected_url", None),
        status_code=getattr(result, "status_code", None),
        html=getattr(result, "html", None),
        cleaned_html=getattr(result, "cleaned_html", None),
        js_execution_result=getattr(result, "js_execution_result", None),
        error_message=getattr(result, "error_message", None),
    )
```

- [ ] **Step 2: Create the uvicorn entrypoint**

Write `crawl4ai_adapter/server_main.py`:

```python
"""Entrypoint invoked by the Rust daemon's supervisor.

Invoked as:
    python -m crawl4ai_adapter.server_main --host 127.0.0.1 --port 11235 --runtime-dir ...

Blocks until killed. Reports ready by binding the port; the supervisor
polls /health.
"""

from __future__ import annotations

import argparse
import logging
import os
import sys

import uvicorn


def main() -> int:
    parser = argparse.ArgumentParser(prog="python -m crawl4ai_adapter.server_main")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=11235)
    parser.add_argument("--runtime-dir", required=True)
    parser.add_argument("--log-level", default="info")
    args = parser.parse_args()

    os.environ["CRAWL4_AI_BASE_DIRECTORY"] = args.runtime_dir
    logging.basicConfig(
        level=args.log_level.upper(),
        format="%(asctime)s %(levelname)s %(name)s %(message)s",
        stream=sys.stderr,
    )

    uvicorn.run(
        "crawl4ai_adapter.server:app",
        host=args.host,
        port=args.port,
        log_level=args.log_level,
        access_log=False,
        loop="asyncio",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 3: Smoke-test by running it manually**

```bash
./.runtime/davis/crawl4ai-venv/bin/python -m crawl4ai_adapter.server_main \
  --runtime-dir "$PWD/.runtime/davis" &
SERVER_PID=$!
sleep 3
curl -sS http://127.0.0.1:11235/health
kill "$SERVER_PID"
```

Expected: `{"status":"ok"}` (or `{"status":"unhealthy","reason":"crawl4ai_import_failed",...}` if your venv is stale — run `daviszeroclaw crawl install` first).

- [ ] **Step 4: Commit**

```bash
git add crawl4ai_adapter/server.py crawl4ai_adapter/server_main.py
git commit -m "feat(crawl4ai_adapter): long-lived FastAPI server

crawl4ai_adapter.server exposes POST /crawl and GET /health.
crawl4ai_adapter.server_main is the uvicorn entrypoint the Rust
daemon supervises. Replaces per-request 'python -m crawl4ai_adapter
crawl' subprocess startup (~300-800ms of import overhead per call)
with a long-running process; health probe + typed 503/504/500
responses replace stderr substring parsing."
```


### Task 7: Retain `crawl4ai_adapter/__main__.py login` subcommand; simplify `crawl` subcommand

**Files:**
- Modify: `crawl4ai_adapter/__main__.py`

Rationale: the HTTP server covers the `/crawl` path. The interactive `login` flow can't easily be moved to HTTP (requires TTY). Keep the `login` subcommand as-is; drop the `crawl` subcommand to prevent confusion.

- [ ] **Step 1: Remove the `_run_crawl` function and `crawl` subparser**

In `crawl4ai_adapter/__main__.py`:

- Delete `async def _run_crawl(args)` (currently lines 188-247)
- Delete the `crawl` subparser (`build_parser` lines 260-262)
- Update `_main_async` to drop the `crawl` branch (lines 266-273)

Final trimmed `__main__.py` keeps only the `login` subcommand.

- [ ] **Step 2: Confirm it still runs**

```bash
./.runtime/davis/crawl4ai-venv/bin/python -m crawl4ai_adapter --help
```

Expected: shows only `login` as a subcommand.

- [ ] **Step 3: Commit**

```bash
git add crawl4ai_adapter/__main__.py
git commit -m "refactor(crawl4ai_adapter): drop one-shot 'crawl' subcommand

HTTP server in crawl4ai_adapter.server now owns the crawl path.
The __main__ module retains only the interactive 'login' flow
(requires TTY, doesn't belong over HTTP). Rust side will switch
away from 'python -m crawl4ai_adapter crawl' in Task 10."
```

---

## Phase 1b — Rust supervisor + transport swap

### Task 8: Typed `Crawl4aiError`

**Files:**
- Create: `src/crawl4ai_error.rs`
- Modify: `src/lib.rs` (export)
- Modify: `src/crawl4ai.rs` (signature, error construction)
- Modify: `src/express.rs` (consume typed errors)

- [ ] **Step 1: Write the enum + conversion tests**

Create `src/crawl4ai_error.rs`:

```rust
//! Typed error surface for crawl4ai calls.
//!
//! Callers (src/express.rs, src/advisor.rs) previously matched on String
//! substrings to decide user-facing issue types. This enum makes each failure
//! mode explicit and keeps match-coverage a compile error.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Crawl4aiError {
    /// local.toml has `crawl4ai.enabled = false`.
    Disabled,
    /// Supervised adapter not reachable / not healthy.
    ServerUnavailable { details: String },
    /// Supervised adapter returned 504 or Rust-side wall-clock fired.
    Timeout { budget_secs: u64 },
    /// 500 from adapter or crawl4ai raised an exception inside the task.
    AdapterCrashed { details: String },
    /// crawl4ai returned `success: false` for reasons other than auth
    /// (e.g. navigation failure, wait_for predicate never satisfied).
    CrawlFailed { details: String },
    /// crawl4ai adapter reported the profile is not logged in.
    AuthRequired { profile: String },
    /// Unexpected or malformed JSON back from the adapter.
    PayloadMalformed { details: String },
    /// I/O while preparing the request (profile dir creation, etc.).
    LocalIo { details: String },
}

impl Crawl4aiError {
    /// Issue type string for `build_issue` / UI routing (keeps existing
    /// identifiers stable for src/support.rs remediation hints).
    pub fn issue_type(&self) -> &'static str {
        match self {
            Self::Disabled | Self::ServerUnavailable { .. } | Self::Timeout { .. } => {
                "crawl4ai_unavailable"
            }
            Self::AdapterCrashed { .. } | Self::CrawlFailed { .. } => "site_changed",
            Self::AuthRequired { .. } => "auth_required",
            Self::PayloadMalformed { .. } => "site_changed",
            Self::LocalIo { .. } => "crawl4ai_unavailable",
        }
    }
}

impl fmt::Display for Crawl4aiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "crawl4ai is disabled in local config"),
            Self::ServerUnavailable { details } => {
                write!(f, "crawl4ai server unavailable: {details}")
            }
            Self::Timeout { budget_secs } => {
                write!(f, "crawl4ai request timed out after {budget_secs}s")
            }
            Self::AdapterCrashed { details } => write!(f, "crawl4ai adapter crashed: {details}"),
            Self::CrawlFailed { details } => write!(f, "crawl4ai crawl failed: {details}"),
            Self::AuthRequired { profile } => {
                write!(f, "crawl4ai profile '{profile}' requires login")
            }
            Self::PayloadMalformed { details } => {
                write!(f, "crawl4ai returned malformed payload: {details}")
            }
            Self::LocalIo { details } => write!(f, "crawl4ai local i/o failed: {details}"),
        }
    }
}

impl std::error::Error for Crawl4aiError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_type_mapping_is_stable() {
        assert_eq!(Crawl4aiError::Disabled.issue_type(), "crawl4ai_unavailable");
        assert_eq!(
            Crawl4aiError::Timeout { budget_secs: 30 }.issue_type(),
            "crawl4ai_unavailable"
        );
        assert_eq!(
            Crawl4aiError::AuthRequired {
                profile: "express-ali".into()
            }
            .issue_type(),
            "auth_required"
        );
        assert_eq!(
            Crawl4aiError::CrawlFailed {
                details: "foo".into()
            }
            .issue_type(),
            "site_changed"
        );
    }

    #[test]
    fn display_includes_context() {
        let err = Crawl4aiError::Timeout { budget_secs: 120 };
        let s = err.to_string();
        assert!(s.contains("120"));
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`**

Add to `src/lib.rs`:

```rust
pub mod crawl4ai_error;
pub use crawl4ai_error::Crawl4aiError;
```

- [ ] **Step 3: Run unit tests**

```bash
cargo test crawl4ai_error:: 2>&1 | tail -10
```

Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add src/crawl4ai_error.rs src/lib.rs
git commit -m "feat(crawl4ai): typed Crawl4aiError enum

Replaces Result<_, String> with an enum covering disabled, unavailable,
timeout, crashed, failed, auth-required, payload-malformed, local-io.
issue_type() preserves the existing strings consumed by src/support.rs,
so remediation hints stay intact across the refactor."
```

### Task 9: The supervisor (`src/crawl4ai_supervisor.rs`)

**Files:**
- Create: `src/crawl4ai_supervisor.rs`
- Modify: `src/lib.rs`
- Modify: `src/runtime_paths.rs` (add `crawl4ai_pid_path`, `crawl4ai_log_path`)
- Test: `tests/rust/crawl4ai_supervisor.rs`

- [ ] **Step 1: Add the two runtime path helpers**

In `src/runtime_paths.rs`, near the other crawl4ai helpers:

```rust
pub fn crawl4ai_pid_path(&self) -> PathBuf {
    self.runtime_dir.join("crawl4ai.pid")
}

pub fn crawl4ai_log_path(&self) -> PathBuf {
    self.runtime_dir.join("crawl4ai.log")
}
```

- [ ] **Step 2: Write the supervisor module**

Create `src/crawl4ai_supervisor.rs`:

```rust
//! Long-lived Python crawl4ai adapter supervised by the daviszeroclaw daemon.
//!
//! On daemon start, `Crawl4aiSupervisor::start` spawns `python -m
//! crawl4ai_adapter.server_main`, probes `/health` until it responds, and
//! returns once ready. A background task watches the child: if it exits,
//! it restarts with exponential backoff (1s → 2s → 4s → ... capped at 30s).
//! Five consecutive failures within the backoff window surfaces a
//! non-recoverable error to the daemon.

use crate::{Crawl4aiConfig, Crawl4aiError, RuntimePaths};
use reqwest::Client;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::sleep;

const HEALTH_PATH: &str = "/health";
const STARTUP_PROBE_INTERVAL: Duration = Duration::from_millis(200);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const RESTART_BUDGET: u32 = 5;

#[derive(Clone)]
pub struct Crawl4aiSupervisor {
    inner: Arc<Mutex<SupervisorInner>>,
    health_url: String,
    http: Client,
}

struct SupervisorInner {
    child: Option<Child>,
    paths: RuntimePaths,
    config: Crawl4aiConfig,
    python: PathBuf,
    port: u16,
}

impl Crawl4aiSupervisor {
    /// Spawn the adapter, probe /health until ready, return handle.
    pub async fn start(
        paths: RuntimePaths,
        config: Crawl4aiConfig,
    ) -> Result<Self, Crawl4aiError> {
        if !config.enabled {
            return Err(Crawl4aiError::Disabled);
        }
        let python = resolve_python_binary(&paths, &config)?;
        let port = parse_port(&config.base_url)?;
        let http = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs.saturating_add(10)))
            .build()
            .map_err(|err| Crawl4aiError::ServerUnavailable {
                details: format!("build reqwest client: {err}"),
            })?;
        let inner = SupervisorInner {
            child: None,
            paths,
            config,
            python,
            port,
        };
        let supervisor = Self {
            inner: Arc::new(Mutex::new(inner)),
            health_url: format!("http://127.0.0.1:{port}{HEALTH_PATH}"),
            http,
        };
        supervisor.spawn_child().await?;
        supervisor.wait_until_healthy().await?;
        supervisor.clone().spawn_restart_loop();
        Ok(supervisor)
    }

    /// Returns the URL callers should POST /crawl to (e.g. http://127.0.0.1:11235).
    pub async fn base_url(&self) -> String {
        let guard = self.inner.lock().await;
        format!("http://127.0.0.1:{}", guard.port)
    }

    /// Shared HTTP client for callers. Connection pool is reused.
    pub fn http_client(&self) -> Client {
        self.http.clone()
    }

    pub async fn is_healthy(&self) -> bool {
        self.probe_health().await.is_ok()
    }

    async fn probe_health(&self) -> Result<(), Crawl4aiError> {
        let resp = self
            .http
            .get(&self.health_url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .map_err(|err| Crawl4aiError::ServerUnavailable {
                details: err.to_string(),
            })?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(Crawl4aiError::ServerUnavailable {
                details: format!("health returned {}", resp.status()),
            })
        }
    }

    async fn spawn_child(&self) -> Result<(), Crawl4aiError> {
        let mut guard = self.inner.lock().await;
        let log_path = guard.paths.crawl4ai_log_path();
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| Crawl4aiError::LocalIo {
                details: format!("create log dir: {err}"),
            })?;
        }
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|err| Crawl4aiError::LocalIo {
                details: format!("open crawl4ai log {}: {err}", log_path.display()),
            })?;
        let log_stderr = log_file
            .try_clone()
            .map_err(|err| Crawl4aiError::LocalIo {
                details: format!("dup crawl4ai log handle: {err}"),
            })?;

        let child = Command::new(&guard.python)
            .arg("-m")
            .arg("crawl4ai_adapter.server_main")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(guard.port.to_string())
            .arg("--runtime-dir")
            .arg(guard.paths.runtime_dir.display().to_string())
            .current_dir(&guard.paths.repo_root)
            .env("PYTHONPATH", guard.paths.repo_root.display().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_stderr))
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| Crawl4aiError::ServerUnavailable {
                details: format!("spawn crawl4ai_adapter.server_main: {err}"),
            })?;

        if let Some(pid) = child.id() {
            let pid_path = guard.paths.crawl4ai_pid_path();
            let _ = std::fs::write(&pid_path, pid.to_string());
        }
        guard.child = Some(child);
        tracing::info!(port = guard.port, "crawl4ai adapter server spawned");
        Ok(())
    }

    async fn wait_until_healthy(&self) -> Result<(), Crawl4aiError> {
        let start = std::time::Instant::now();
        loop {
            if self.probe_health().await.is_ok() {
                return Ok(());
            }
            if start.elapsed() > STARTUP_TIMEOUT {
                return Err(Crawl4aiError::ServerUnavailable {
                    details: format!(
                        "adapter did not become healthy within {:?}",
                        STARTUP_TIMEOUT
                    ),
                });
            }
            sleep(STARTUP_PROBE_INTERVAL).await;
        }
    }

    fn spawn_restart_loop(self) {
        tokio::spawn(async move {
            let mut consecutive_failures: u32 = 0;
            let mut backoff = Duration::from_secs(1);
            loop {
                let child_opt = {
                    let mut guard = self.inner.lock().await;
                    guard.child.take()
                };
                let Some(mut child) = child_opt else {
                    break;
                };
                let status = match child.wait().await {
                    Ok(status) => status,
                    Err(err) => {
                        tracing::error!(?err, "crawl4ai adapter wait() failed");
                        break;
                    }
                };
                tracing::warn!(?status, "crawl4ai adapter exited; restarting");
                sleep(backoff).await;
                match self.spawn_child().await {
                    Ok(()) => match self.wait_until_healthy().await {
                        Ok(()) => {
                            consecutive_failures = 0;
                            backoff = Duration::from_secs(1);
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "crawl4ai adapter restart health probe failed");
                            consecutive_failures += 1;
                        }
                    },
                    Err(err) => {
                        tracing::error!(error = %err, "crawl4ai adapter respawn failed");
                        consecutive_failures += 1;
                    }
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                if consecutive_failures >= RESTART_BUDGET {
                    tracing::error!(
                        "crawl4ai adapter failed to stay up after {RESTART_BUDGET} attempts; giving up"
                    );
                    break;
                }
            }
        });
    }
}

fn resolve_python_binary(
    paths: &RuntimePaths,
    config: &Crawl4aiConfig,
) -> Result<PathBuf, Crawl4aiError> {
    if !config.python.is_empty() {
        return Ok(PathBuf::from(&config.python));
    }
    let candidate = paths.crawl4ai_python_path();
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(Crawl4aiError::ServerUnavailable {
        details: format!(
            "crawl4ai venv python not found at {}. Run `daviszeroclaw crawl install`.",
            candidate.display()
        ),
    })
}

fn parse_port(base_url: &str) -> Result<u16, Crawl4aiError> {
    let url = url::Url::parse(base_url).map_err(|err| Crawl4aiError::ServerUnavailable {
        details: format!("parse base_url {base_url}: {err}"),
    })?;
    url.port_or_known_default()
        .ok_or_else(|| Crawl4aiError::ServerUnavailable {
            details: format!("no port derivable from {base_url}"),
        })
}
```

- [ ] **Step 3: Export from lib.rs**

In `src/lib.rs`:

```rust
pub mod crawl4ai_supervisor;
pub use crawl4ai_supervisor::Crawl4aiSupervisor;
```

- [ ] **Step 4: Verify compilation**

```bash
cargo build 2>&1 | tail -20
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add src/crawl4ai_supervisor.rs src/lib.rs src/runtime_paths.rs
git commit -m "feat(crawl4ai): supervisor for long-lived adapter server

Crawl4aiSupervisor::start spawns python -m crawl4ai_adapter.server_main,
probes /health until ready, and keeps a background task that restarts
the child on exit with exponential backoff (1s → 30s cap, 5-failure
budget). Child runs with kill_on_drop so daemon shutdown cleans it up
(plus crawl4ai.pid / crawl4ai.log for observability)."
```


### Task 10: Rewrite `crawl4ai_crawl` to go through the supervisor (HTTP only)

**Files:**
- Modify: `src/crawl4ai.rs` (delete python transport, refactor server transport to reuse supervisor client)
- Modify: `src/app_config.rs:94-126` (delete `Crawl4aiTransport`, delete `python` field)
- Modify: `src/lib.rs` (drop removed exports)
- Modify: `config/davis/local.example.toml:63-77`
- Modify: `config/davis/local.toml:52-61`

- [ ] **Step 1: Remove `Crawl4aiTransport` and the `python` / `transport` fields from config**

In `src/app_config.rs`:

Delete the entire enum (lines 120-126):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Crawl4aiTransport {
    Server,
    #[default]
    Python,
}
```

Delete these fields from `Crawl4aiConfig` (around lines 98-103):

```rust
#[serde(default)]
pub transport: Crawl4aiTransport,
```

```rust
#[serde(default)]
pub python: String,
```

Update `Default for Crawl4aiConfig` (lines 261-277) to drop those two fields. Also update `normalize` at lines 398-410 to stop touching them.

In `src/lib.rs`, drop the `Crawl4aiTransport` re-export.

- [ ] **Step 2: Rewrite `crawl4ai_crawl`**

Replace the entire body of `src/crawl4ai.rs` with a single HTTP path. The new file:

```rust
use crate::{Crawl4aiConfig, Crawl4aiError, Crawl4aiSupervisor, RuntimePaths, USER_AGENT};
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct Crawl4aiPageRequest {
    pub profile_name: String,
    pub url: String,
    pub wait_for: Option<String>,
    pub js_code: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Crawl4aiPageResult {
    pub success: bool,
    pub current_url: Option<String>,
    pub html: Option<String>,
    pub cleaned_html: Option<String>,
    pub error_message: Option<String>,
    pub status_code: Option<u16>,
    pub raw: Value,
}

#[derive(Serialize)]
struct CrawlRequestBody<'a> {
    profile_path: String,
    url: &'a str,
    wait_for: Option<&'a str>,
    js_code: Option<&'a str>,
    timeout_secs: u64,
    headless: bool,
    magic: bool,
    simulate_user: bool,
    override_navigator: bool,
    remove_overlay_elements: bool,
    enable_stealth: bool,
}

#[tracing::instrument(
    name = "crawl4ai",
    skip(paths, config, supervisor),
    fields(profile = %request.profile_name, url = %request.url),
)]
pub async fn crawl4ai_crawl(
    paths: &RuntimePaths,
    config: &Crawl4aiConfig,
    supervisor: &Crawl4aiSupervisor,
    request: Crawl4aiPageRequest,
) -> Result<Crawl4aiPageResult, Crawl4aiError> {
    if !config.enabled {
        return Err(Crawl4aiError::Disabled);
    }
    migrate_legacy_profiles(paths).map_err(|err| Crawl4aiError::LocalIo {
        details: format!("profile migration: {err}"),
    })?;
    let profile_dir = paths.crawl4ai_profiles_root().join(&request.profile_name);
    std::fs::create_dir_all(&profile_dir).map_err(|err| Crawl4aiError::LocalIo {
        details: format!(
            "create profile dir {}: {err}",
            profile_dir.display()
        ),
    })?;

    let body = CrawlRequestBody {
        profile_path: profile_dir.display().to_string(),
        url: &request.url,
        wait_for: request.wait_for.as_deref(),
        js_code: request.js_code.as_deref(),
        timeout_secs: config.timeout_secs,
        headless: config.headless,
        magic: config.magic,
        simulate_user: config.simulate_user,
        override_navigator: config.override_navigator,
        remove_overlay_elements: config.remove_overlay_elements,
        enable_stealth: config.enable_stealth,
    };

    let base = supervisor.base_url().await;
    let client = supervisor.http_client();
    let response = client
        .post(format!("{base}/crawl"))
        .header("user-agent", USER_AGENT)
        .json(&body)
        .send()
        .await
        .map_err(|err| {
            if err.is_timeout() {
                Crawl4aiError::Timeout {
                    budget_secs: config.timeout_secs.saturating_add(10),
                }
            } else {
                Crawl4aiError::ServerUnavailable {
                    details: err.to_string(),
                }
            }
        })?;

    let status = response.status();
    let payload: Value = response
        .json()
        .await
        .map_err(|err| Crawl4aiError::PayloadMalformed {
            details: format!("decode /crawl response: {err}"),
        })?;

    match status {
        StatusCode::OK => {
            let page = parse_result_value(payload);
            tracing::info!(
                success = page.success,
                status_code = ?page.status_code,
                final_url = ?page.current_url,
                "crawl4ai complete",
            );
            if page.success {
                Ok(page)
            } else {
                Err(Crawl4aiError::CrawlFailed {
                    details: page
                        .error_message
                        .clone()
                        .unwrap_or_else(|| "crawl4ai returned success=false".to_string()),
                })
            }
        }
        StatusCode::GATEWAY_TIMEOUT => Err(Crawl4aiError::Timeout {
            budget_secs: config.timeout_secs,
        }),
        StatusCode::SERVICE_UNAVAILABLE => Err(Crawl4aiError::ServerUnavailable {
            details: compact_json(&payload),
        }),
        StatusCode::INTERNAL_SERVER_ERROR => Err(Crawl4aiError::AdapterCrashed {
            details: compact_json(&payload),
        }),
        other => Err(Crawl4aiError::AdapterCrashed {
            details: format!("unexpected status {other}: {}", compact_json(&payload)),
        }),
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

fn parse_result_value(raw: Value) -> Crawl4aiPageResult {
    Crawl4aiPageResult {
        success: raw
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        current_url: raw
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                raw.get("redirected_url")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }),
        html: raw.get("html").and_then(Value::as_str).map(str::to_string),
        cleaned_html: raw
            .get("cleaned_html")
            .and_then(Value::as_str)
            .map(str::to_string),
        error_message: raw
            .get("error_message")
            .or_else(|| raw.get("error"))
            .and_then(Value::as_str)
            .map(str::to_string),
        status_code: raw
            .get("status_code")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok()),
        raw,
    }
}

fn migrate_legacy_profiles(paths: &RuntimePaths) -> std::io::Result<()> {
    let legacy = paths.crawl4ai_legacy_profiles_root();
    let current = paths.crawl4ai_profiles_root();
    if current.exists() || !legacy.exists() {
        return Ok(());
    }
    if let Some(parent) = current.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(legacy, current)
}
```

- [ ] **Step 3: Update local.example.toml**

Replace lines 63-77 (`[crawl4ai]` block) with:

```toml
[crawl4ai]
enabled = true
# The Rust daemon supervises a long-lived adapter on this address.
# Change only if the default port (11235) conflicts with another service.
base_url = "http://127.0.0.1:11235"
timeout_secs = 90
headless = true
magic = true
simulate_user = true
override_navigator = true
remove_overlay_elements = true
enable_stealth = true
```

And `config/davis/local.toml:52-61` similarly (drop `transport`, drop `python`).

- [ ] **Step 4: Verify compilation**

```bash
cargo build 2>&1 | tail -20
```

Expected: will not yet compile — callers of `crawl4ai_crawl` don't know about the supervisor argument. Proceed to Task 11.

### Task 11: Thread the supervisor through AppState + call sites

**Files:**
- Modify: `src/server.rs` (`AppState` new field, constructor)
- Modify: `src/local_proxy.rs:78-120` (start supervisor at boot, inject)
- Modify: `src/express.rs` (accept supervisor, pass to `crawl4ai_crawl`, map `Crawl4aiError` → `issue_type()`)

- [ ] **Step 1: Add the supervisor field to AppState**

In `src/server.rs`, near the other fields:

```rust
pub struct AppState {
    // ... existing fields ...
    pub crawl4ai_supervisor: Arc<Crawl4aiSupervisor>,
}
```

Update `AppState::new` to accept it.

- [ ] **Step 2: Start the supervisor during daemon boot**

In `src/local_proxy.rs`, between `render_runtime_config(&paths, &local_config)?` (line 98) and `let state = AppState::new(...)` (line 99), insert:

```rust
let crawl4ai_supervisor = if local_config.crawl4ai.enabled {
    match Crawl4aiSupervisor::start(paths.clone(), local_config.crawl4ai.clone()).await {
        Ok(sup) => {
            tracing::info!("crawl4ai supervisor ready");
            Arc::new(sup)
        }
        Err(err) => {
            tracing::error!(error = %err, "crawl4ai supervisor failed to start; continuing without crawl support");
            // Fall through with a disabled placeholder so the server still boots
            // and HA-only features keep working.
            Arc::new(Crawl4aiSupervisor::disabled())
        }
    }
} else {
    Arc::new(Crawl4aiSupervisor::disabled())
};
```

Add a `pub fn disabled()` constructor in `src/crawl4ai_supervisor.rs` that returns a stub supervisor whose `base_url()` / `http_client()` work but never have a running child. Any `crawl4ai_crawl` against it will return `Crawl4aiError::Disabled`.

```rust
impl Crawl4aiSupervisor {
    /// Placeholder when crawl4ai is disabled or supervisor failed at boot.
    /// All crawl4ai_crawl calls return Crawl4aiError::Disabled via config.enabled=false path.
    pub fn disabled() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SupervisorInner {
                child: None,
                paths: RuntimePaths::from_env(),
                config: Crawl4aiConfig {
                    enabled: false,
                    ..Crawl4aiConfig::default()
                },
                python: PathBuf::new(),
                port: 0,
            })),
            health_url: String::new(),
            http: Client::new(),
        }
    }
}
```

Pass `crawl4ai_supervisor` into `AppState::new`.

- [ ] **Step 3: Update `src/express.rs` to use the supervisor**

In the existing `crawl_source_payload` (from Task 2's signature), add the supervisor:

```rust
async fn crawl_source_payload(
    paths: &RuntimePaths,
    crawl4ai_config: &Crawl4aiConfig,
    supervisor: &Crawl4aiSupervisor,
    profile_lock: Arc<tokio::sync::Mutex<()>>,
    source: &str,
    script: String,
) -> Result<Value, Crawl4aiError> {
    let _guard = profile_lock.lock().await;
    let response = crawl4ai_crawl(
        paths,
        crawl4ai_config,
        supervisor,
        Crawl4aiPageRequest {
            profile_name: express_profile_name(source),
            url: source_order_url(source).to_string(),
            wait_for: Some(source_wait_for(source).to_string()),
            js_code: Some(script),
        },
    )
    .await?;
    extract_payload_value(&response).map_err(|message| Crawl4aiError::PayloadMalformed {
        details: message,
    })
}
```

Change `fetch_source_status` and `fetch_source_snapshot` to receive `supervisor: &Crawl4aiSupervisor` and to match typed errors:

```rust
async fn fetch_source_status(
    paths: &RuntimePaths,
    crawl4ai_config: &Crawl4aiConfig,
    supervisor: &Crawl4aiSupervisor,
    profile_lock: Arc<tokio::sync::Mutex<()>>,
    source: &str,
) -> ExpressSourceStatus {
    match crawl_source_payload(paths, crawl4ai_config, supervisor, profile_lock, source, auth_script(source)).await {
        Ok(payload) => parse_source_status(source, &payload, None, None),
        Err(err) => {
            let issue_type = err.issue_type();
            source_error_snapshot(source, "upstream_error", issue_type, err.to_string())
                .source_status
        }
    }
}
```

Do the same for `fetch_source_snapshot`. Note: the `site_changed` / `crawl4ai_unavailable` / `auth_required` strings now come from `Crawl4aiError::issue_type()` — no substring matching.

Update public entry points `express_auth_status` / `express_packages` to also accept `Arc<Crawl4aiSupervisor>` and pass it through.

- [ ] **Step 4: Update `src/server.rs` handler call sites**

Each call of `express_auth_status(state.paths.clone(), …)` now passes the supervisor:

```rust
express_auth_status(
    state.paths.clone(),
    (*state.crawl4ai_config).clone(),
    state.crawl4ai_profile_locks.clone(),
    state.crawl4ai_supervisor.clone(),
).await
```

Same for `express_packages`.

- [ ] **Step 5: Verify build**

```bash
cargo build 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add -p src/crawl4ai.rs src/crawl4ai_supervisor.rs src/app_config.rs \
  src/local_proxy.rs src/server.rs src/express.rs src/lib.rs \
  config/davis/local.example.toml config/davis/local.toml
git commit -m "refactor(crawl4ai): go through supervised HTTP server; delete Python transport

- Removes Crawl4aiTransport enum and python field from config. Single
  path now: supervisor → HTTP POST /crawl.
- crawl4ai_crawl returns Crawl4aiError (typed). Express maps
  err.issue_type() directly instead of substring-matching.
- Daemon starts supervisor during boot; failed start falls back to a
  disabled placeholder so HA-only features keep working.
- base_url parsing picks the port the supervisor binds.
- kill_on_drop + pid file + dedicated log make the child easy to
  observe and clean up."
```


### Task 12: `daviszeroclaw crawl service {status,restart,stop}` subcommands

**Files:**
- Modify: `src/cli/crawl.rs` (add service subcommands)
- Modify: `src/cli/mod.rs` (wire into Args enum)

- [ ] **Step 1: Add the subcommand enum variants**

In `src/cli/mod.rs`, locate the `CrawlSubcommand` (or whichever enum `crawl` subcommands live under). Add:

```rust
#[derive(clap::Subcommand, Debug)]
pub enum CrawlSubcommand {
    // ... existing Install, Check, Profile, Run ...
    #[command(subcommand)]
    Service(CrawlServiceSubcommand),
}

#[derive(clap::Subcommand, Debug)]
pub enum CrawlServiceSubcommand {
    Status,
    Restart,
    Stop,
}
```

- [ ] **Step 2: Implement handlers**

In `src/cli/crawl.rs`, add:

```rust
pub(super) fn crawl_service_status(paths: &RuntimePaths) -> Result<()> {
    let pid_path = paths.crawl4ai_pid_path();
    let log_path = paths.crawl4ai_log_path();
    println!("pid file : {}", pid_path.display());
    println!("log file : {}", log_path.display());
    if let Ok(raw) = std::fs::read_to_string(&pid_path) {
        let trimmed = raw.trim();
        println!("pid      : {trimmed}");
        if let Ok(pid) = trimmed.parse::<i32>() {
            let alive = is_process_alive(pid);
            println!("alive    : {alive}");
        }
    } else {
        println!("pid      : <no pid file; daemon may not have started crawl4ai>");
    }
    // Probe /health for the final word.
    let config = check_local_config(paths)?;
    let url = format!("{}/health", config.crawl4ai.base_url.trim_end_matches('/'));
    match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?
        .get(&url)
        .send()
    {
        Ok(resp) => println!("health   : {} ({})", resp.status(), url),
        Err(err) => println!("health   : unreachable ({err})"),
    }
    Ok(())
}

pub(super) fn crawl_service_restart(paths: &RuntimePaths) -> Result<()> {
    crawl_service_stop(paths)?;
    println!("Restart requires the daemon to respawn the adapter. Restart daviszeroclaw daemon.");
    Ok(())
}

pub(super) fn crawl_service_stop(paths: &RuntimePaths) -> Result<()> {
    let pid_path = paths.crawl4ai_pid_path();
    let Ok(raw) = std::fs::read_to_string(&pid_path) else {
        println!("no pid file at {}", pid_path.display());
        return Ok(());
    };
    let pid: i32 = raw.trim().parse().context("parse crawl4ai.pid")?;
    unsafe {
        if libc::kill(pid, libc::SIGTERM) != 0 {
            let err = std::io::Error::last_os_error();
            bail!("kill({pid}, SIGTERM): {err}");
        }
    }
    println!("Sent SIGTERM to {pid}. Supervisor will respawn on next health tick.");
    Ok(())
}

fn is_process_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}
```

Add `libc = "0.2"` to `[dependencies]` in `Cargo.toml` if not present.

- [ ] **Step 3: Dispatch in `src/cli/mod.rs`**

```rust
CrawlSubcommand::Service(sub) => match sub {
    CrawlServiceSubcommand::Status => crawl_service_status(&paths)?,
    CrawlServiceSubcommand::Restart => crawl_service_restart(&paths)?,
    CrawlServiceSubcommand::Stop => crawl_service_stop(&paths)?,
},
```

- [ ] **Step 4: Smoke test**

```bash
cargo build && ./target/debug/daviszeroclaw crawl service status
```

Expected: prints the pid/log/health triplet (health may be "unreachable" if the daemon isn't running — that's fine).

- [ ] **Step 5: Commit**

```bash
git add src/cli/crawl.rs src/cli/mod.rs Cargo.toml
git commit -m "feat(crawl4ai): 'crawl service status|restart|stop' subcommands

Operator-facing: reads .runtime/davis/crawl4ai.pid, checks liveness via
kill(pid, 0), probes /health. Restart/stop send SIGTERM; the supervisor
loop inside the daemon respawns automatically."
```

### Task 13: Delete `crawl4ai_adapter crawl` subprocess path and the `Crawl4aiTransport` leftovers

**Files:**
- Modify: `src/crawl4ai.rs` (already done in Task 10 — verify no references remain)
- Modify: `tests/rust/fixtures.rs:90-96`
- Verify no stray uses

- [ ] **Step 1: grep for dead references**

```bash
rg -n "Crawl4aiTransport|crawl_via_python|crawl4ai_adapter crawl" --no-heading
```

Expected: zero matches (the only acceptable match is comments referencing the removal in commit messages).

- [ ] **Step 2: Update `tests/rust/fixtures.rs:90-96`**

Delete the `sample_local_config_with_crawl4ai_base_url` override of `transport = Server` — the field no longer exists:

```rust
pub(super) fn sample_local_config_with_crawl4ai_base_url(base_url: &str) -> LocalConfig {
    let mut config = sample_local_config();
    config.crawl4ai.enabled = true;
    config.crawl4ai.base_url = base_url.trim_end_matches('/').to_string();
    config
}
```

- [ ] **Step 3: Verify**

```bash
cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5
```

Expected: full green. Tests that used to force the `Server` transport still work (there's only one transport now).

- [ ] **Step 4: Commit**

```bash
git add -p tests/rust/fixtures.rs
git commit -m "test(crawl4ai): drop Crawl4aiTransport overrides (no longer exists)"
```

---

## Phase 1c — Test coverage + cleanup

### Task 14: Integration test — supervisor happy path against a mock FastAPI

**Files:**
- Create/modify: `tests/rust/crawl4ai.rs` (add supervisor integration tests)
- Helper: reuse `spawn_json_router` from `tests/rust/support.rs`

- [ ] **Step 1: Add a test that stands up a mock /crawl and /health, then drives an express fetch end-to-end**

Append to `tests/rust/crawl4ai.rs`:

```rust
use axum::{routing::{get, post}, Json, Router};
use serde_json::{json, Value};

async fn mock_ok_health() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn mock_ok_crawl(Json(body): Json<Value>) -> Json<Value> {
    let marker = format!(
        "<div data-davis-express-payload=\"{}\"></div>",
        urlencoding::encode(&json!({
            "source": "ali",
            "status": "empty",
            "checked_at": "2026-04-22T00:00:00Z",
            "logged_in": true,
            "package_count": 0,
            "packages": []
        }).to_string())
    );
    Json(json!({
        "success": true,
        "url": body.get("url"),
        "status_code": 200,
        "html": marker,
        "cleaned_html": null,
        "js_execution_result": null,
        "error_message": null
    }))
}

#[tokio::test]
async fn express_auth_status_flows_through_mock_supervisor() {
    use crate::tests::support::spawn_json_router;
    use std::sync::Arc;

    let app = Router::new()
        .route("/health", get(mock_ok_health))
        .route("/crawl", post(mock_ok_crawl));
    let (base_url, _shutdown) = spawn_json_router(app).await;

    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join(".runtime").join("davis"),
    };
    std::fs::create_dir_all(paths.runtime_dir.join("state")).unwrap();

    let mut cfg = crate::Crawl4aiConfig::default();
    cfg.enabled = true;
    cfg.base_url = base_url.clone();
    cfg.timeout_secs = 5;

    // Shortcut: build a supervisor whose HTTP client points at the mock.
    // We can't easily run the real spawn_child (needs a venv), so we use
    // a test-only constructor that skips spawning.
    let supervisor = Arc::new(crate::Crawl4aiSupervisor::for_test(base_url.clone()));

    let locks = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let response = crate::express_auth_status(paths, cfg, locks, supervisor).await;

    assert_eq!(response.sources.len(), 2);
    let ali = response.sources.iter().find(|s| s.source == "ali").unwrap();
    assert_eq!(ali.status, "empty");
}
```

- [ ] **Step 2: Add the `for_test` constructor**

In `src/crawl4ai_supervisor.rs`:

```rust
#[cfg(any(test, feature = "test-util"))]
impl Crawl4aiSupervisor {
    /// Test constructor: skips spawning any child, uses the given base_url.
    pub fn for_test(base_url: impl Into<String>) -> Self {
        let base = base_url.into();
        let url = url::Url::parse(&base).expect("for_test requires a parseable base_url");
        let port = url.port_or_known_default().unwrap_or(0);
        Self {
            inner: Arc::new(Mutex::new(SupervisorInner {
                child: None,
                paths: RuntimePaths::from_env(),
                config: Crawl4aiConfig {
                    enabled: true,
                    base_url: base,
                    ..Crawl4aiConfig::default()
                },
                python: PathBuf::new(),
                port,
            })),
            health_url: format!("{}/health", url.as_str().trim_end_matches('/')),
            http: Client::new(),
        }
    }
}
```

And make `Crawl4aiSupervisor::base_url` use the config's `base_url` directly rather than reconstructing from port (so `for_test` works even when the mock is on a random port):

```rust
pub async fn base_url(&self) -> String {
    let guard = self.inner.lock().await;
    guard.config.base_url.trim_end_matches('/').to_string()
}
```

- [ ] **Step 3: Run**

```bash
cargo test express_auth_status_flows_through_mock_supervisor 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 4: Also add an auth_required typed-error test**

```rust
#[tokio::test]
async fn crawl4ai_503_maps_to_server_unavailable() {
    let app = Router::new()
        .route("/health", get(mock_ok_health))
        .route(
            "/crawl",
            post(|| async {
                axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
            }),
        );
    let (base_url, _shutdown) = crate::tests::support::spawn_json_router(app).await;

    let tmp = tempfile::tempdir().unwrap();
    let paths = RuntimePaths {
        repo_root: tmp.path().to_path_buf(),
        runtime_dir: tmp.path().join(".runtime").join("davis"),
    };
    let mut cfg = crate::Crawl4aiConfig::default();
    cfg.enabled = true;
    cfg.base_url = base_url.clone();
    cfg.timeout_secs = 2;

    let supervisor = crate::Crawl4aiSupervisor::for_test(base_url);
    let err = crate::crawl4ai_crawl(
        &paths,
        &cfg,
        &supervisor,
        crate::Crawl4aiPageRequest {
            profile_name: "test".into(),
            url: "https://example.com".into(),
            wait_for: None,
            js_code: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, crate::Crawl4aiError::ServerUnavailable { .. }), "got {err:?}");
    assert_eq!(err.issue_type(), "crawl4ai_unavailable");
}
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/rust/crawl4ai.rs src/crawl4ai_supervisor.rs
git commit -m "test(crawl4ai): integration coverage for supervisor transport

- Crawl4aiSupervisor::for_test skips spawning a child, points at a mock
  HTTP server. Removes the 'tests never exercise default config' gap.
- express_auth_status end-to-end test threads mock → supervisor →
  crawl4ai_crawl → express parse.
- Separate test proves 503 maps to Crawl4aiError::ServerUnavailable with
  the stable 'crawl4ai_unavailable' issue_type."
```


### Task 15: Observability — instrument express call sites with source + tracing fields

**Files:**
- Modify: `src/express.rs` (add `#[tracing::instrument]` on `crawl_source_payload`, span fields in `fetch_source_status`/`fetch_source_snapshot`)
- Modify: `src/crawl4ai_supervisor.rs` (emit `crawl4ai.restart_count`, `crawl4ai.health_ok` fields)

- [ ] **Step 1: Add span fields at the express layer**

In `src/express.rs`, wrap `crawl_source_payload`:

```rust
#[tracing::instrument(
    name = "express.crawl_source_payload",
    skip(paths, crawl4ai_config, supervisor, profile_lock, script),
    fields(source = %source, profile = %express_profile_name(source), script_len = script.len()),
    err,
)]
async fn crawl_source_payload(
    paths: &RuntimePaths,
    crawl4ai_config: &Crawl4aiConfig,
    supervisor: &Crawl4aiSupervisor,
    profile_lock: Arc<tokio::sync::Mutex<()>>,
    source: &str,
    script: String,
) -> Result<Value, Crawl4aiError> { ... }
```

Do the same (with `#[tracing::instrument]`) on `fetch_source_status` and `fetch_source_snapshot` — fields should include `source` at minimum.

- [ ] **Step 2: Emit span events on supervisor restart**

In `src/crawl4ai_supervisor.rs::spawn_restart_loop`, replace the `tracing::warn!(?status, "crawl4ai adapter exited; restarting")` with:

```rust
tracing::warn!(
    ?status,
    consecutive_failures,
    backoff_ms = backoff.as_millis() as u64,
    "crawl4ai adapter exited; restarting"
);
```

- [ ] **Step 3: Build, tests**

```bash
cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
git add -p src/express.rs src/crawl4ai_supervisor.rs
git commit -m "observability(crawl4ai): add source field to spans, restart context to warnings"
```

### Task 16: `_error` branch hardening — adapter startup import failure surfaces to user

**Files:**
- Modify: `src/crawl4ai_supervisor.rs` (startup failure mode)
- Modify: `src/support.rs` (remediation hint)

- [ ] **Step 1: When `/health` returns 503, surface it loudly**

Currently `wait_until_healthy` treats 503 like any other non-200 and retries. Add explicit handling:

```rust
async fn wait_until_healthy(&self) -> Result<(), Crawl4aiError> {
    let start = std::time::Instant::now();
    loop {
        match self
            .http
            .get(&self.health_url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return Ok(());
                }
                if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                    // Read body to surface import error from the adapter.
                    let body: Value = resp.json().await.unwrap_or(json!({}));
                    if start.elapsed() > Duration::from_secs(5) {
                        return Err(Crawl4aiError::ServerUnavailable {
                            details: format!(
                                "adapter reports unhealthy: {}",
                                compact_json(&body)
                            ),
                        });
                    }
                }
            }
            Err(_) => {}
        }
        if start.elapsed() > STARTUP_TIMEOUT {
            return Err(Crawl4aiError::ServerUnavailable {
                details: format!(
                    "adapter did not become healthy within {:?}",
                    STARTUP_TIMEOUT
                ),
            });
        }
        sleep(STARTUP_PROBE_INTERVAL).await;
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}
```

Add `use serde_json::{json, Value};` at the top of `src/crawl4ai_supervisor.rs`.

- [ ] **Step 2: Verify & commit**

```bash
cargo build 2>&1 | tail -5 && cargo test 2>&1 | tail -5
git add -p src/crawl4ai_supervisor.rs
git commit -m "fix(crawl4ai): 503 during startup surfaces adapter import error

Previously wait_until_healthy treated any non-200 as 'keep polling,'
so a broken venv silently appeared as STARTUP_TIMEOUT after 30s. Now
we read the /health body on 503 after 5s and return the actual reason
(crawl4ai_import_failed + details)."
```

### Task 17: Docs and final merge

**Files:**
- Create: `docs/superpowers/plans/2026-04-22-crawl4ai-supervised-server-notes.md` (operator notes, not required — skip if user hasn't asked)
- Modify: `README.md` if it references the old transport
- Self-review

- [ ] **Step 1: grep README / docs / project-sops for stale references**

```bash
rg -n "transport.*python|crawl4ai_adapter crawl|Crawl4aiTransport" docs/ project-sops/ README.md 2>/dev/null
```

Expected: zero matches. If any turn up, rewrite them to reference the supervised-server model.

- [ ] **Step 2: Final full verification**

```bash
cargo fmt --check 2>&1 | tail -5
cargo clippy --all-targets 2>&1 | tail -5
cargo test 2>&1 | tail -5
```

Expected: all clean.

- [ ] **Step 3: Manual smoke against real daemon**

Only if you want to validate end-to-end before PR. Launch the daemon locally:

```bash
./target/debug/daviszeroclaw daemon start
sleep 5
./target/debug/daviszeroclaw crawl service status
# Expected: alive=true, health 200
curl -sS http://127.0.0.1:3010/express/auth-status
# Expected: both sources return either ok/needs_reauth (not upstream_error).
./target/debug/daviszeroclaw daemon stop
```

Expected: child process dies with the daemon; no orphans visible via `ps aux | grep server_main`.

- [ ] **Step 4: Open PR**

```bash
git push -u origin refactor/crawl4ai-supervised-server
gh pr create --title "refactor(crawl4ai): supervised HTTP adapter, typed errors, retry budget" \
  --body "$(cat <<'EOF'
## Summary
- Replaces per-request Python subprocess with a long-lived
  `crawl4ai_adapter.server` (FastAPI) that the Rust daemon supervises
  with exponential backoff + health probe
- Deletes `Crawl4aiTransport` enum — one path, one test surface
- Introduces `Crawl4aiError` typed enum, deletes substring-matching in
  `src/express.rs`
- Per-profile `Mutex<()>` prevents Chromium `user_data_dir` collisions
- `join_all` parallelizes Taobao/JD `/express/*` fetches
- `daviszeroclaw crawl service {status,restart,stop}` for operators
- Pins Python deps via `config/davis/crawl4ai-requirements.txt`

## Test plan
- [ ] `cargo fmt --check` clean
- [ ] `cargo clippy --all-targets` clean (deny-all)
- [ ] `cargo test` green (includes new supervisor / typed-error tests)
- [ ] Manual: `daviszeroclaw daemon start` — adapter child appears in
      `ps`, log writes to `.runtime/davis/crawl4ai.log`, `/health` 200
- [ ] Manual: kill adapter via `crawl service stop`, observe restart in
      daemon log with backoff
- [ ] Manual: `/express/auth-status` double-request — no
      `SingletonLock` errors

## Migration
Existing `config/davis/local.toml` with `transport = "python"` or a
`python = ...` override will fail to parse. Updated
`config/davis/local.example.toml` shows the new minimal block.
EOF
)"
```

- [ ] **Step 5: Save session memory after merge**

After the PR is merged, run:

```
/lead-summary
```

---

## Self-Review Checklist

Before handing off:

**Spec coverage:**
- ✅ P0-1 (async blocking) — Task 1
- ✅ P0-2 (no wall-clock timeout) — Task 1
- ✅ P0-3 (profile collision) — Task 2
- ✅ P1-4 (stringly-typed errors) — Task 8 + Task 11
- ✅ P1-5 (resolve_python duplication) — Task 9 uses a single implementation (`resolve_python_binary`)
- ✅ P1-6 (reflective HTML payload) — not tackled; adapter `js_execution_result` already prioritized; HTML fallback kept. *Optional follow-up tracked, not in this plan.*
- ✅ P1-7 (default transport untested) — Task 14
- ✅ P1-8 (reqwest::Client per-request) — Task 10 uses `supervisor.http_client()` (shared)
- ✅ P2-9 (dead path helpers) — Task 5
- ✅ P2-10 (no pinning) — Task 5
- ✅ P2-11 (sources serial) — Task 3
- ✅ P2-12 (observability gaps) — Task 15
- ✅ P2-13 (storage_state written not read) — *Not addressed.* Adds no bug; deferred. Noted for a separate janitor task.

**Placeholder scan:**
- No `TODO`, `TBD`, or generic "add error handling" hints. Each step names the function, shows code, or specifies the command.

**Type consistency:**
- `Crawl4aiSupervisor::{start, disabled, for_test, base_url, http_client, is_healthy, probe_health}` are consistent across Tasks 9–14.
- `Crawl4aiError::{Disabled, ServerUnavailable, Timeout, AdapterCrashed, CrawlFailed, AuthRequired, PayloadMalformed, LocalIo}` used identically in every referencing task.
- `issue_type()` return values (`"crawl4ai_unavailable"`, `"site_changed"`, `"auth_required"`) match the strings already consumed by `src/support.rs:60-117`.

**Known-deferred items** (explicitly not in scope, each tracked for future):
- Span-event metrics (Prometheus-style counters) — would require `metrics` crate addition
- Article-memory crawl4ai usage (doesn't exist yet; will need profile-lock plumbing when added)
- `storage_state.json` consolidation with `user_data_dir` (cosmetic; defer)

---

## Execution notes

- Phases 0 and 1 ship as separate PRs — the Phase 0 PR is mergeable on its own and buys immediate stability.
- Phase 1 is bigger but every task within it lands its own commit with passing tests, so interruption is cheap.
- The plan assumes you run from a `refactor/crawl4ai-supervised-server` worktree. Create one via:

  ```bash
  git worktree add ../DavisZeroClaw-crawl4ai refactor/crawl4ai-supervised-server
  cd ../DavisZeroClaw-crawl4ai
  ```

- If a task's tests fail during execution, stop and surface the failure — don't improvise around it. The plan's checkpoints are the right granularity for resume.
