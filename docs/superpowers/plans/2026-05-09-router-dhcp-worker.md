# Router DHCP Keeper Worker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the standalone Faillone/Automation router-DHCP-check cron into Davis as a periodic worker, with Python (Playwright) doing the browser flow under a Rust supervisor.

**Architecture:** New `router_adapter/` Python module mirrors `crawl4ai_adapter/`'s shape; new `src/router_supervisor.rs` spawns it as a one-shot subprocess (~30s, dies, repeats every 10 min); new `src/router_worker.rs` owns the tick loop, dedupe state machine, and diary write. Davis daemon spawns the worker on startup if `[router_dhcp].enabled=true`. Shared Chromium across both Python adapters via a single `PLAYWRIGHT_BROWSERS_PATH`.

**Tech Stack:** Rust (tokio, serde, anyhow, reqwest, libc) + Python (playwright, python-dotenv) + existing Davis infra (RuntimePaths, MempalaceEmitter, AppState, axum, clap).

**Reference spec:** `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md` (read this first if you weren't part of the brainstorming).

---

## File Structure

### New Rust files

| Path | Responsibility |
|---|---|
| `src/router_supervisor.rs` | `RouterCheckOutcome` enum, `RouterAction` enum, `parse_outcome` (pure), `RouterChecker` trait, `PythonRouterChecker` impl |
| `src/router_worker.rs` | `RouterWorker` struct, dedupe state machine, AAAK diary formatting, `RouterHealthSnapshot` |
| `src/cli/router_dhcp.rs` | `daviszeroclaw router-dhcp install \| run-once` subcommand handlers |
| `tests/fixtures/router_stub.py` | One-line Python stub used by integration test |
| `tests/rust/router_supervisor_spawn.rs` | Integration test: spawn the stub, parse its output |

### New Python files

| Path | Responsibility |
|---|---|
| `router_adapter/__init__.py` | Empty package marker |
| `router_adapter/__main__.py` | `python -m router_adapter` entry → calls `router_dhcp_check.main()` |
| `router_adapter/router_dhcp_check.py` | Playwright flow; emits final-line JSON contract |
| `router_adapter/pyproject.toml` | Declares dependencies (playwright + python-dotenv) |
| `router_adapter/README.md` | Standalone-run + selector-edit guide |

### Modified files

| Path | What changes |
|---|---|
| `CLAUDE.md` | Replace Python-boundary paragraph (see spec §"CLAUDE.md change") |
| `src/app_config.rs` | Add `RouterDhcpConfig` struct + `LocalConfig.router_dhcp` field |
| `src/lib.rs` | Add `mod router_supervisor;`, `mod router_worker;`, re-exports |
| `src/runtime_paths.rs` | Add `router_adapter_venv_dir`, `router_adapter_python_path`, `router_adapter_dir`, `playwright_browsers_path` |
| `src/cli/mod.rs` | Add `RouterDhcp` subcommand variant + dispatch |
| `src/local_proxy.rs` | Spawn `RouterWorker` on daemon startup if enabled |
| `src/server.rs` | Extend `HealthResponse` with `router_dhcp` field |
| `Cargo.toml` | Add `[[test]] name = "router_supervisor_spawn"` |
| `config/davis/local.example.toml` | Add commented `[router_dhcp]` section |

---

## Task Sequencing

5 commits, each independently passing `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib`.

- **Tasks 1–3 → Commit 1** (pure scaffold, no callers)
- **Task 4 → Commit 2** (Python adapter, independent)
- **Tasks 5–7 → Commit 3** (`PythonRouterChecker` + integration test)
- **Tasks 8–11 → Commit 4** (`RouterWorker` + `/health` + daemon spawn)
- **Task 12 → Commit 5** (CLI `run-once` action)

---

## Task 1: CLAUDE.md Python boundary update

**Files:**
- Modify: `CLAUDE.md`

The current paragraph (line ~36): "Python side (`crawl4ai_adapter/`) owns only: crawl4ai pruning + trafilatura + learned-rules CSS extraction. All LLM calls live in Rust (`src/article_memory/llm_client.rs`). Don't let LLM logic drift back into Python."

- [ ] **Step 1: Locate the current Python-boundary paragraph**

Run: `grep -n "Python side" CLAUDE.md`
Expected: one match in the "When changing Davis" section.

- [ ] **Step 2: Replace the paragraph**

Replace the whole one-line paragraph with the multi-paragraph block below. Use `Edit` tool with `old_string` = the existing paragraph and `new_string` = the new content:

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

- [ ] **Step 3: Verify the change took**

Run: `grep -A 3 "browser-layer automation" CLAUDE.md`
Expected: shows the new paragraph.

- [ ] **Step 4: Stage the change but do not commit yet** (will be committed at end of Task 3)

Run: `git add CLAUDE.md`
Expected: no error.

---

## Task 2: Add `RouterDhcpConfig` to `app_config.rs`

**Files:**
- Modify: `src/app_config.rs` (add struct + LocalConfig field)
- Modify: `src/lib.rs` (re-export)
- Modify: `src/app_config.rs` (add unit test in same file's `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests { ... }` block at the bottom of `src/app_config.rs`. If no such block exists, create one. Add this test:

```rust
#[test]
fn router_dhcp_config_defaults_when_section_missing() {
    let toml_text = r#"
[home_assistant]
url = "http://example"
token = "x"
[imessage]
[providers]
[routing.profiles.home_control]
provider = "p"
model = "m"
[routing.profiles.general_qa]
provider = "p"
model = "m"
[routing.profiles.research]
provider = "p"
model = "m"
[routing.profiles.structured_lookup]
provider = "p"
model = "m"
"#;
    let cfg: LocalConfig = toml::from_str(toml_text).expect("parse");
    assert!(!cfg.router_dhcp.enabled);
    assert_eq!(cfg.router_dhcp.interval_secs, 600);
    assert_eq!(cfg.router_dhcp.tick_timeout_secs, 90);
    assert_eq!(cfg.router_dhcp.url, "http://192.168.0.1");
    assert_eq!(cfg.router_dhcp.username_env, "ROUTER_USERNAME");
    assert_eq!(cfg.router_dhcp.password_env, "ROUTER_PASSWORD");
}

#[test]
fn router_dhcp_config_explicit_values_respected() {
    let toml_text = r#"
[home_assistant]
url = "http://example"
token = "x"
[imessage]
[providers]
[routing.profiles.home_control]
provider = "p"
model = "m"
[routing.profiles.general_qa]
provider = "p"
model = "m"
[routing.profiles.research]
provider = "p"
model = "m"
[routing.profiles.structured_lookup]
provider = "p"
model = "m"

[router_dhcp]
enabled = true
interval_secs = 1200
tick_timeout_secs = 120
url = "http://192.168.1.1"
username_env = "MY_USER"
password_env = "MY_PASS"
"#;
    let cfg: LocalConfig = toml::from_str(toml_text).expect("parse");
    assert!(cfg.router_dhcp.enabled);
    assert_eq!(cfg.router_dhcp.interval_secs, 1200);
    assert_eq!(cfg.router_dhcp.tick_timeout_secs, 120);
    assert_eq!(cfg.router_dhcp.url, "http://192.168.1.1");
    assert_eq!(cfg.router_dhcp.username_env, "MY_USER");
    assert_eq!(cfg.router_dhcp.password_env, "MY_PASS");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib app_config::tests::router_dhcp_config -- --nocapture`
Expected: FAIL — compile error "no field `router_dhcp` on `LocalConfig`" or similar.

- [ ] **Step 3: Add `RouterDhcpConfig` struct and the `LocalConfig` field**

In `src/app_config.rs`, add to the `LocalConfig` struct (after `pub shortcut: ShortcutConfig,`):

```rust
    #[serde(default)]
    pub router_dhcp: RouterDhcpConfig,
```

Then add the new struct after the `ShortcutReplyPhrases` block (or near other config structs):

```rust
/// Periodic worker that drives the LAN router admin page (Playwright
/// flow lives in `router_adapter/`). Off by default. See
/// `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterDhcpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_router_dhcp_interval_secs")]
    pub interval_secs: u64,
    #[serde(default = "default_router_dhcp_tick_timeout_secs")]
    pub tick_timeout_secs: u64,
    #[serde(default = "default_router_dhcp_url")]
    pub url: String,
    #[serde(default = "default_router_dhcp_username_env")]
    pub username_env: String,
    #[serde(default = "default_router_dhcp_password_env")]
    pub password_env: String,
}

impl Default for RouterDhcpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: default_router_dhcp_interval_secs(),
            tick_timeout_secs: default_router_dhcp_tick_timeout_secs(),
            url: default_router_dhcp_url(),
            username_env: default_router_dhcp_username_env(),
            password_env: default_router_dhcp_password_env(),
        }
    }
}

fn default_router_dhcp_interval_secs() -> u64 {
    600
}

fn default_router_dhcp_tick_timeout_secs() -> u64 {
    90
}

fn default_router_dhcp_url() -> String {
    "http://192.168.0.1".to_string()
}

fn default_router_dhcp_username_env() -> String {
    "ROUTER_USERNAME".to_string()
}

fn default_router_dhcp_password_env() -> String {
    "ROUTER_PASSWORD".to_string()
}
```

- [ ] **Step 4: Re-export from `src/lib.rs`**

In `src/lib.rs`, find the existing `pub use app_config::{` block and add `RouterDhcpConfig,` to the import list (alphabetical order — between `RoutingProfilesConfig,` and `RuleLearningConfig,`):

```rust
pub use app_config::{
    ArticleMemoryConfig, ArticleMemoryEmbeddingConfig, ArticleMemoryExtractConfig,
    ArticleMemoryHostProfile, ArticleMemoryIngestConfig, ArticleMemoryNormalizeConfig,
    ArticleMemoryValueConfig, Crawl4aiConfig, HomeAssistantConfig, ImessageConfig, LocalConfig,
    McpConfig, McpServerConfig, McpTransport, ModelProviderConfig, OpenRouterLlmEngineConfig,
    QualityGateToml, RouterDhcpConfig, RoutingConfig, RoutingProfileConfig, RoutingProfilesConfig,
    RuleLearningConfig, ShortcutConfig, ShortcutReplyConfig, ShortcutReplyPhrases, TranslateConfig,
    WebhookConfig,
};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib app_config::tests::router_dhcp_config`
Expected: PASS (both tests).

- [ ] **Step 6: Run lint and format**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all`
Expected: no warnings, no diff after fmt.

- [ ] **Step 7: Add commented section to `local.example.toml`**

Find the path: `config/davis/local.example.toml` (use `find . -name local.example.toml | head -1` if unsure).

Append at end of file:

```toml

# Optional: periodic LAN router DHCP keeper (one specific check). Off by default.
# When enabled, every interval_secs the daemon spawns a Python child to log into
# the router admin UI at `url`, find the DHCP toggle, and disable it if on.
# Credentials are read from the env vars named below; never put plaintext here.
# See router_adapter/README.md for setup; CLAUDE.md for the Python boundary rule.
# [router_dhcp]
# enabled = false
# interval_secs = 600
# tick_timeout_secs = 90
# url = "http://192.168.0.1"
# username_env = "ROUTER_USERNAME"
# password_env = "ROUTER_PASSWORD"
```

- [ ] **Step 8: Stage but do not commit yet**

Run: `git add src/app_config.rs src/lib.rs config/davis/local.example.toml`
Expected: no error.

---

## Task 3: Add runtime path helpers + commit scaffolding

**Files:**
- Modify: `src/runtime_paths.rs`

- [ ] **Step 1: Write the failing test**

Append to `mod tests` at the bottom of `src/runtime_paths.rs`:

```rust
#[test]
fn router_adapter_paths_are_under_runtime_dir() {
    let paths = RuntimePaths {
        repo_root: std::path::PathBuf::from("/tmp/repo"),
        runtime_dir: std::path::PathBuf::from("/tmp/runtime"),
    };
    assert_eq!(
        paths.router_adapter_venv_dir(),
        std::path::PathBuf::from("/tmp/runtime/router-adapter-venv")
    );
    assert_eq!(
        paths.router_adapter_python_path(),
        std::path::PathBuf::from("/tmp/runtime/router-adapter-venv/bin/python")
    );
    assert_eq!(
        paths.router_adapter_dir(),
        std::path::PathBuf::from("/tmp/repo/router_adapter")
    );
    assert_eq!(
        paths.playwright_browsers_path(),
        std::path::PathBuf::from("/tmp/runtime/playwright-browsers")
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib runtime_paths::tests::router_adapter_paths`
Expected: FAIL — compile error "no method named `router_adapter_venv_dir`".

- [ ] **Step 3: Add the four path methods**

In `src/runtime_paths.rs`, add inside `impl RuntimePaths` (after `crawl4ai_python_path`):

```rust
    pub fn router_adapter_venv_dir(&self) -> PathBuf {
        self.runtime_dir.join("router-adapter-venv")
    }

    pub fn router_adapter_python_path(&self) -> PathBuf {
        self.router_adapter_venv_dir().join("bin").join("python")
    }

    pub fn router_adapter_dir(&self) -> PathBuf {
        self.repo_root.join("router_adapter")
    }

    /// Shared Playwright browser cache for ALL Python adapters that drive
    /// Chromium. Both `crawl4ai_adapter/` and `router_adapter/` MUST point
    /// here via `PLAYWRIGHT_BROWSERS_PATH=…` so we have exactly one
    /// Chromium binary on disk. See
    /// `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md`
    /// "Open risks" section.
    pub fn playwright_browsers_path(&self) -> PathBuf {
        self.runtime_dir.join("playwright-browsers")
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib runtime_paths::tests::router_adapter_paths`
Expected: PASS.

- [ ] **Step 5: Run full lint + tests + format**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check && cargo test --lib`
Expected: zero warnings, zero diff, all tests pass.

- [ ] **Step 6: Commit (Commit 1 = Tasks 1+2+3)**

```bash
git add src/runtime_paths.rs
git commit -m "$(cat <<'EOF'
feat(router-dhcp): scaffold config + runtime paths

- Add RouterDhcpConfig (off by default; 600s interval; 90s tick timeout).
- Add runtime path helpers for the new router-adapter venv and the
  shared playwright-browsers cache directory.
- Update CLAUDE.md to define the Python boundary as "browser-layer
  automation", admitting a second adapter (router_adapter) without
  loosening the rule.
- Document the new opt-in section in local.example.toml.

Inert until Task 11 wires the worker into the daemon. No runtime effect.
EOF
)"
```

Run: `git log --oneline -1`
Expected: shows the new commit.

---

## Task 4: Python adapter `router_adapter/`

**Files:**
- Create: `router_adapter/__init__.py`
- Create: `router_adapter/__main__.py`
- Create: `router_adapter/router_dhcp_check.py`
- Create: `router_adapter/pyproject.toml`
- Create: `router_adapter/README.md`

This task is independent of Rust. The integration test in Task 7 uses a stub, not this adapter, so a Rust engineer can validate Tasks 5-7 without finishing Task 4 first. But Task 11 (worker spawning in daemon) must NOT be enabled in production until this adapter is in place.

- [ ] **Step 1: Create the package marker**

```bash
mkdir -p router_adapter
```

Create `router_adapter/__init__.py` with content:

```python
"""Davis router adapter: Playwright-driven LAN router admin automation.

Owned by Davis (separate from crawl4ai_adapter). Spawned by
src/router_supervisor.rs as a one-shot subprocess; emits a single JSON
status line as its final stdout line. See
docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md.
"""
```

- [ ] **Step 2: Create `__main__.py`**

Create `router_adapter/__main__.py`:

```python
"""Entry point: `python -m router_adapter` runs the DHCP check."""
from router_adapter.router_dhcp_check import main

if __name__ == "__main__":
    main()
```

- [ ] **Step 3: Create `pyproject.toml`**

Create `router_adapter/pyproject.toml`:

```toml
[project]
name = "router_adapter"
version = "0.1.0"
description = "Davis Playwright adapter for LAN router admin automation"
requires-python = ">=3.10"
dependencies = [
    "playwright>=1.40",
    "python-dotenv>=1.0",
]

[build-system]
requires = ["setuptools>=68"]
build-backend = "setuptools.build_meta"

[tool.setuptools.packages.find]
where = ["."]
include = ["router_adapter*"]
```

- [ ] **Step 4: Create `router_dhcp_check.py`**

This is a 1:1 port of `Faillone/Automation`'s `scripts/router-dhcp-check.ts` to Python + sync Playwright API. CRITICAL: the LAST stdout line MUST be the JSON status. Earlier lines are free-form logs.

Create `router_adapter/router_dhcp_check.py`:

```python
"""DHCP-disable script for a specific GPON ONT router admin UI.

Stdout protocol contract (Rust supervisor depends on this):
    The LAST non-empty stdout line MUST be a single JSON object.
    Earlier lines are free-form human-readable logs.

Possible final lines:
    {"status":"ok",    "action":"none"|"disabled", "dhcp_was_enabled":bool, "duration_ms":int}
    {"status":"error", "stage":"<closed-enum>", "reason":"<short>", "duration_ms":int}

Closed `stage` enum: login | navigate | iframe | toggle | apply | unhandled.
The outermost try/except MUST emit `unhandled` for any uncaught exception.

Selectors are firmware-specific. If the router web UI changes after a
firmware update, update the constants below.
"""

from __future__ import annotations

import json
import os
import sys
import time
from typing import Any

from playwright.sync_api import (
    Frame,
    Page,
    Playwright,
    TimeoutError as PlaywrightTimeoutError,
    sync_playwright,
)

# --- Selectors (firmware-specific; update here when UI changes) ---
SEL_LOGIN_PHOTO = "#normalphoto"
SEL_USERNAME = "#txt_normalUsername"
SEL_PASSWORD = "#txt_normalPassword"
SEL_LOGIN_SUBMIT = "#PwdPain1 > div:nth-child(2)"
SEL_MAIN_MENU = "#mainMenu_1"
SEL_THIRD_MENU_DHCP = "#thirdMenu_2"
SEL_IFRAME = "#frameContent"
SEL_DHCP_CHECKBOX = "#dhcpSrvType"
SEL_APPLY = "#btnApply_ex"
SEL_LOGOUT = "#headerLogout"

# --- Timeouts (ms) ---
DEFAULT_TIMEOUT_MS = 5000
NAV_TIMEOUT_MS = 30000
LOGIN_NAV_TIMEOUT_MS = 10000


def emit(payload: dict[str, Any]) -> None:
    """Write the protocol JSON line to stdout. Always the LAST stdout call."""
    sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def log(message: str) -> None:
    """Free-form log line — visible to the Rust supervisor as tracing info."""
    print(message, file=sys.stdout, flush=True)


def main() -> None:
    started = time.monotonic()
    url = os.environ.get("ROUTER_URL", "http://192.168.0.1")
    username = os.environ.get("ROUTER_USERNAME")
    password = os.environ.get("ROUTER_PASSWORD")

    if not username or not password:
        emit({
            "status": "error",
            "stage": "unhandled",
            "reason": "missing ROUTER_USERNAME or ROUTER_PASSWORD env",
            "duration_ms": int((time.monotonic() - started) * 1000),
        })
        sys.exit(1)

    try:
        with sync_playwright() as p:
            outcome = run_check(p, url, username, password)
        outcome["duration_ms"] = int((time.monotonic() - started) * 1000)
        emit(outcome)
        sys.exit(0 if outcome["status"] == "ok" else 1)
    except Exception as exc:  # noqa: BLE001 — top-level safety net
        emit({
            "status": "error",
            "stage": "unhandled",
            "reason": f"{type(exc).__name__}: {exc}",
            "duration_ms": int((time.monotonic() - started) * 1000),
        })
        sys.exit(1)


def run_check(p: Playwright, url: str, username: str, password: str) -> dict[str, Any]:
    log(f"launching chromium for {url}")
    browser = p.chromium.launch(headless=True, args=["--no-sandbox"])
    try:
        context = browser.new_context(viewport={"width": 1280, "height": 800})
        page = context.new_page()

        # --- Navigate ---
        try:
            page.goto(url, wait_until="networkidle", timeout=NAV_TIMEOUT_MS)
            page.wait_for_timeout(2000)
        except PlaywrightTimeoutError as exc:
            return {"status": "error", "stage": "navigate", "reason": f"goto timeout: {exc}"}

        # --- Click photo to reveal login form (skip if not present) ---
        try:
            page.wait_for_selector(SEL_LOGIN_PHOTO, timeout=DEFAULT_TIMEOUT_MS)
            page.click(SEL_LOGIN_PHOTO)
            page.wait_for_timeout(1000)
            log("clicked login photo")
        except PlaywrightTimeoutError:
            log("no login photo (form may already be visible)")

        # --- Login ---
        try:
            page.wait_for_selector(SEL_USERNAME, timeout=DEFAULT_TIMEOUT_MS)
            page.fill(SEL_USERNAME, username)
            page.fill(SEL_PASSWORD, password)
            page.click(SEL_LOGIN_SUBMIT)
            try:
                page.wait_for_load_state("networkidle", timeout=LOGIN_NAV_TIMEOUT_MS)
            except PlaywrightTimeoutError:
                log("no navigation after login click; continuing")
            page.wait_for_timeout(2000)
            log("login submitted")
        except PlaywrightTimeoutError as exc:
            return {"status": "error", "stage": "login", "reason": f"login form selector miss: {exc}"}

        # --- Navigate basic config → DHCP ---
        try:
            page.wait_for_selector(SEL_MAIN_MENU, timeout=DEFAULT_TIMEOUT_MS)
            page.click(SEL_MAIN_MENU)
            page.wait_for_timeout(1500)
            page.wait_for_selector(SEL_THIRD_MENU_DHCP, timeout=DEFAULT_TIMEOUT_MS)
            page.click(SEL_THIRD_MENU_DHCP)
            page.wait_for_timeout(1500)
            log("navigated to DHCP page")
        except PlaywrightTimeoutError as exc:
            return {"status": "error", "stage": "navigate", "reason": f"menu selector miss: {exc}"}

        # --- Drop into iframe ---
        try:
            page.wait_for_selector(SEL_IFRAME, timeout=DEFAULT_TIMEOUT_MS)
            frame_handle = page.query_selector(SEL_IFRAME)
            frame = frame_handle.content_frame() if frame_handle else None
            if frame is None:
                return {"status": "error", "stage": "iframe", "reason": "iframe content_frame is None"}
        except PlaywrightTimeoutError as exc:
            return {"status": "error", "stage": "iframe", "reason": f"iframe wait timeout: {exc}"}

        # --- Read DHCP checkbox ---
        try:
            frame.wait_for_selector(SEL_DHCP_CHECKBOX, timeout=DEFAULT_TIMEOUT_MS)
            is_enabled = frame.evaluate(
                f"() => {{ const el = document.querySelector('{SEL_DHCP_CHECKBOX}'); return el ? el.checked : false; }}"
            )
        except PlaywrightTimeoutError as exc:
            return {"status": "error", "stage": "toggle", "reason": f"checkbox selector miss: {exc}"}

        if not is_enabled:
            log("DHCP already off; logging out")
            try_logout(page)
            return {"status": "ok", "action": "none", "dhcp_was_enabled": False}

        # --- Disable + apply ---
        log("DHCP on; disabling")
        try:
            frame.click(SEL_DHCP_CHECKBOX)
            page.wait_for_timeout(1000)
            frame.wait_for_selector(SEL_APPLY, timeout=DEFAULT_TIMEOUT_MS)
            frame.click(SEL_APPLY)
            page.wait_for_timeout(2000)
        except PlaywrightTimeoutError as exc:
            return {"status": "error", "stage": "apply", "reason": f"apply button selector miss: {exc}"}

        try_logout(page)
        return {"status": "ok", "action": "disabled", "dhcp_was_enabled": True}
    finally:
        browser.close()


def try_logout(page: Page) -> None:
    """Best-effort logout. Logged but never fatal."""
    try:
        page.wait_for_selector(SEL_LOGOUT, timeout=DEFAULT_TIMEOUT_MS)
        page.click(SEL_LOGOUT)
        page.wait_for_timeout(1000)
        log("logged out")
    except PlaywrightTimeoutError:
        log("no logout button found (acceptable)")


if __name__ == "__main__":
    main()
```

- [ ] **Step 5: Create `README.md`**

Create `router_adapter/README.md`:

````markdown
# router_adapter

Davis Playwright adapter that drives the LAN router admin page to keep DHCP disabled. Mirrors `crawl4ai_adapter/` in shape; not related to crawling. See `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md` for the design.

## Setup (canonical)

```bash
daviszeroclaw router-dhcp install
```

This creates `.runtime/davis/router-adapter-venv/`, installs `playwright` + `python-dotenv` into it, and runs `playwright install chromium` with `PLAYWRIGHT_BROWSERS_PATH=.runtime/davis/playwright-browsers/` set so Chromium is shared with the crawl4ai adapter.

## Manual setup (debugging)

```bash
python3 -m venv .runtime/davis/router-adapter-venv
.runtime/davis/router-adapter-venv/bin/pip install -e router_adapter
PLAYWRIGHT_BROWSERS_PATH=.runtime/davis/playwright-browsers \
  .runtime/davis/router-adapter-venv/bin/python -m playwright install chromium
```

## Standalone run

```bash
ROUTER_URL=http://192.168.0.1 \
ROUTER_USERNAME=admin \
ROUTER_PASSWORD='your_password' \
PLAYWRIGHT_BROWSERS_PATH=.runtime/davis/playwright-browsers \
.runtime/davis/router-adapter-venv/bin/python -m router_adapter
```

The LAST stdout line is JSON; everything before is free-form logging.

## Updating selectors

Selectors are firmware-specific. They live as constants at the top of `router_dhcp_check.py`:

| Constant | Purpose |
|---|---|
| `SEL_LOGIN_PHOTO` | First-page image to click before login form appears |
| `SEL_USERNAME`, `SEL_PASSWORD` | Login form text inputs |
| `SEL_LOGIN_SUBMIT` | Submit (it's a `<div>` styled as a button) |
| `SEL_MAIN_MENU`, `SEL_THIRD_MENU_DHCP` | Sidebar nav into DHCP settings |
| `SEL_IFRAME` | The settings iframe |
| `SEL_DHCP_CHECKBOX` | The toggle to read/click |
| `SEL_APPLY` | "Apply" button after toggling |
| `SEL_LOGOUT` | Header logout button |

To find the right selectors after a firmware change: open the router page in Chrome DevTools, use the inspector to read `id` / `class` / `name` attributes for each step, and update the constants. Run `daviszeroclaw router-dhcp run-once` after each change to validate.
````

- [ ] **Step 6: Commit (Commit 2)**

```bash
git add router_adapter/
git commit -m "$(cat <<'EOF'
feat(router-dhcp): port Playwright DHCP-keeper script

Python adapter under router_adapter/ — Playwright-driven; one-shot
subprocess; emits a final-line JSON status. Mirrors crawl4ai_adapter/
in shape but shares no code (different responsibility, different
lifecycle). README documents canonical install, manual fallback, and
the firmware-specific selector list.

Will be spawned by src/router_supervisor.rs in the next commit. No
Rust changes here.
EOF
)"
```

Run: `git log --oneline -1`
Expected: shows the new commit.

---

## Task 5: `RouterCheckOutcome` + `parse_outcome` (pure)

**Files:**
- Create: `src/router_supervisor.rs`
- Modify: `src/lib.rs` (add `pub mod router_supervisor;`)

- [ ] **Step 1: Add the module declaration in `src/lib.rs`**

Find the alphabetically-sorted module list in `src/lib.rs` (around line 1-32). Add `pub mod router_supervisor;` after `mod observability;` and before `mod runtime_paths;`:

```rust
pub mod router_supervisor;
```

(The module needs to be `pub` so the integration test in Task 7 can name it.)

- [ ] **Step 2: Create the file with type definitions only**

Create `src/router_supervisor.rs`:

```rust
//! Router DHCP keeper supervisor: spawns `router_adapter/` as a one-shot
//! subprocess every tick, parses its final-line JSON, returns a typed
//! outcome. Runs no scheduling logic itself — that lives in
//! `router_worker.rs`. See
//! `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md`.

use serde::Deserialize;

/// Closed enum of stages the Python adapter can report when it self-fails.
/// String-typed (not enum) on the Rust side because we only echo the
/// value back into diary lines and tracing — no behavior keys off it.
pub type ReportedStage = String;

/// What the action did during a successful tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterAction {
    /// DHCP was already off — no change made.
    None,
    /// DHCP was on — the adapter clicked it off.
    Disabled,
}

/// One tick's outcome. Four variants cover every observable case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterCheckOutcome {
    /// Adapter ran cleanly and reported `{"status":"ok",...}`.
    Ok {
        action: RouterAction,
        dhcp_was_enabled: bool,
        duration_ms: u64,
    },
    /// Adapter ran but reported `{"status":"error",...}` — a self-detected
    /// failure (selector miss, login timeout, etc.) within the closed
    /// `stage` enum.
    Reported {
        stage: ReportedStage,
        reason: String,
        duration_ms: u64,
    },
    /// Adapter died without printing the final JSON line, or the tick
    /// timeout expired and we killed the child.
    Crashed {
        exit_code: Option<i32>,
        stderr_tail: String,
    },
    /// We never even got the child started (e.g., python binary missing).
    SpawnFailed {
        reason: String,
    },
}

/// Discriminant kind used by the dedupe state machine in `router_worker.rs`.
/// `Reported` carries the stage so "login failure" and "iframe failure"
/// dedupe independently. Other variants don't carry detail because they
/// already represent a single failure shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeKind {
    OkNone,
    OkDisabled,
    Reported(ReportedStage),
    Crashed,
    SpawnFailed,
}

impl RouterCheckOutcome {
    pub fn kind(&self) -> OutcomeKind {
        match self {
            Self::Ok { action: RouterAction::None, .. } => OutcomeKind::OkNone,
            Self::Ok { action: RouterAction::Disabled, .. } => OutcomeKind::OkDisabled,
            Self::Reported { stage, .. } => OutcomeKind::Reported(stage.clone()),
            Self::Crashed { .. } => OutcomeKind::Crashed,
            Self::SpawnFailed { .. } => OutcomeKind::SpawnFailed,
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }
}

/// Internal shape of the trailing JSON line. Private to this module; not
/// exposed because callers only need `RouterCheckOutcome`.
#[derive(Debug, Deserialize)]
struct AdapterStatus {
    status: String,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    dhcp_was_enabled: Option<bool>,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

/// Pure: derive a `RouterCheckOutcome` from raw subprocess output.
///
/// Logic:
/// 1. If `stdout` last non-empty line parses as JSON with `status="ok"`:
///    return `Ok { ... }`.
/// 2. If it parses as JSON with `status="error"`: return `Reported { ... }`.
/// 3. Anything else (no JSON last line, malformed JSON, missing required
///    fields, unknown status string) → `Crashed { exit_code, stderr_tail }`.
///    The exit code might still be 0 in pathological cases (Python prints
///    nothing then exits cleanly) — we treat that as crashed too.
pub fn parse_outcome(
    stdout: &str,
    exit_code: Option<i32>,
    stderr: &str,
) -> RouterCheckOutcome {
    let stderr_tail = stderr_tail(stderr, 256);
    let last_line = stdout.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    let Ok(parsed) = serde_json::from_str::<AdapterStatus>(last_line.trim()) else {
        return RouterCheckOutcome::Crashed { exit_code, stderr_tail };
    };
    match parsed.status.as_str() {
        "ok" => {
            let action = match parsed.action.as_deref() {
                Some("none") => RouterAction::None,
                Some("disabled") => RouterAction::Disabled,
                _ => return RouterCheckOutcome::Crashed { exit_code, stderr_tail },
            };
            let Some(dhcp_was_enabled) = parsed.dhcp_was_enabled else {
                return RouterCheckOutcome::Crashed { exit_code, stderr_tail };
            };
            RouterCheckOutcome::Ok {
                action,
                dhcp_was_enabled,
                duration_ms: parsed.duration_ms.unwrap_or(0),
            }
        }
        "error" => {
            let stage = parsed.stage.unwrap_or_else(|| "unhandled".to_string());
            let reason = parsed.reason.unwrap_or_default();
            RouterCheckOutcome::Reported {
                stage,
                reason,
                duration_ms: parsed.duration_ms.unwrap_or(0),
            }
        }
        _ => RouterCheckOutcome::Crashed { exit_code, stderr_tail },
    }
}

/// Take the last `n` chars of `stderr` (not bytes — string slicing on a
/// byte boundary panics on multi-byte UTF-8). Used to surface adapter
/// crashes in a bounded way without dragging the whole stderr buffer
/// into diary entries.
fn stderr_tail(stderr: &str, n: usize) -> String {
    if stderr.chars().count() <= n {
        return stderr.to_string();
    }
    let skip = stderr.chars().count() - n;
    stderr.chars().skip(skip).collect()
}
```

- [ ] **Step 3: Add unit tests for `parse_outcome` to the same file**

Append to `src/router_supervisor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ok_action_none() {
        let stdout = r#"some log
{"status":"ok","action":"none","dhcp_was_enabled":false,"duration_ms":12000}
"#;
        let got = parse_outcome(stdout, Some(0), "");
        assert_eq!(
            got,
            RouterCheckOutcome::Ok {
                action: RouterAction::None,
                dhcp_was_enabled: false,
                duration_ms: 12000,
            }
        );
    }

    #[test]
    fn parses_ok_action_disabled() {
        let stdout = r#"{"status":"ok","action":"disabled","dhcp_was_enabled":true,"duration_ms":31000}"#;
        let got = parse_outcome(stdout, Some(0), "");
        assert_eq!(
            got,
            RouterCheckOutcome::Ok {
                action: RouterAction::Disabled,
                dhcp_was_enabled: true,
                duration_ms: 31000,
            }
        );
    }

    #[test]
    fn parses_reported_with_stage_and_reason() {
        let stdout = r#"{"status":"error","stage":"login","reason":"selector.timeout","duration_ms":5000}"#;
        let got = parse_outcome(stdout, Some(1), "");
        assert_eq!(
            got,
            RouterCheckOutcome::Reported {
                stage: "login".to_string(),
                reason: "selector.timeout".to_string(),
                duration_ms: 5000,
            }
        );
    }

    #[test]
    fn empty_stdout_yields_crashed() {
        let got = parse_outcome("", Some(139), "Segmentation fault");
        assert_eq!(
            got,
            RouterCheckOutcome::Crashed {
                exit_code: Some(139),
                stderr_tail: "Segmentation fault".to_string(),
            }
        );
    }

    #[test]
    fn malformed_json_last_line_yields_crashed() {
        let got = parse_outcome("not json\n", Some(0), "");
        assert!(matches!(got, RouterCheckOutcome::Crashed { .. }));
    }

    #[test]
    fn missing_required_field_yields_crashed() {
        let stdout = r#"{"status":"ok","action":"none"}"#;
        let got = parse_outcome(stdout, Some(0), "");
        assert!(matches!(got, RouterCheckOutcome::Crashed { .. }));
    }

    #[test]
    fn unknown_status_yields_crashed() {
        let stdout = r#"{"status":"weird","action":"none","dhcp_was_enabled":false}"#;
        let got = parse_outcome(stdout, Some(0), "");
        assert!(matches!(got, RouterCheckOutcome::Crashed { .. }));
    }

    #[test]
    fn unknown_action_yields_crashed() {
        let stdout = r#"{"status":"ok","action":"banana","dhcp_was_enabled":false}"#;
        let got = parse_outcome(stdout, Some(0), "");
        assert!(matches!(got, RouterCheckOutcome::Crashed { .. }));
    }

    #[test]
    fn stderr_tail_handles_multibyte() {
        let s = "前面无关的文字 末尾错误信息";
        let tail = stderr_tail(s, 6);
        assert_eq!(tail.chars().count(), 6);
        assert_eq!(tail, "末尾错误信息");
    }

    #[test]
    fn stderr_tail_returns_full_when_short() {
        assert_eq!(stderr_tail("hi", 256), "hi");
    }

    #[test]
    fn outcome_kind_distinguishes_reported_stages() {
        let login = RouterCheckOutcome::Reported {
            stage: "login".into(),
            reason: "x".into(),
            duration_ms: 0,
        };
        let iframe = RouterCheckOutcome::Reported {
            stage: "iframe".into(),
            reason: "x".into(),
            duration_ms: 0,
        };
        assert_ne!(login.kind(), iframe.kind());
    }

    #[test]
    fn outcome_kind_disabled_vs_none_distinct() {
        let none = RouterCheckOutcome::Ok {
            action: RouterAction::None,
            dhcp_was_enabled: false,
            duration_ms: 0,
        };
        let disabled = RouterCheckOutcome::Ok {
            action: RouterAction::Disabled,
            dhcp_was_enabled: true,
            duration_ms: 0,
        };
        assert_ne!(none.kind(), disabled.kind());
    }
}
```

- [ ] **Step 4: Run the tests; they should all pass**

Run: `cargo test --lib router_supervisor::`
Expected: all 11 tests PASS.

- [ ] **Step 5: Run full lint + format**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: zero warnings, zero diff.

- [ ] **Step 6: Stage but do not commit yet (commits at end of Task 7)**

```bash
git add src/router_supervisor.rs src/lib.rs
```

---

## Task 6: `RouterChecker` trait + `PythonRouterChecker`

**Files:**
- Modify: `src/router_supervisor.rs` (add trait + impl)

- [ ] **Step 1: Append the trait and prod implementation to `src/router_supervisor.rs`**

Add after the `parse_outcome` function and before the `#[cfg(test)] mod tests` block:

```rust
use crate::{RouterDhcpConfig, RuntimePaths};
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// One tick of work. Implementations spawn the Python adapter (prod) or
/// return canned outcomes (test).
#[async_trait]
pub trait RouterChecker: Send + Sync {
    async fn check_once(&self) -> RouterCheckOutcome;
}

/// Production implementation: spawns `python -m router_adapter` as a
/// one-shot subprocess.
pub struct PythonRouterChecker {
    paths: RuntimePaths,
    config: RouterDhcpConfig,
    /// Resolved `ROUTER_USERNAME` value at construction time. We snapshot
    /// at construction so a config reload doesn't half-update the worker.
    username: String,
    /// Resolved `ROUTER_PASSWORD` value at construction time.
    password: String,
}

impl PythonRouterChecker {
    /// Construct from config + env. Returns `None` when the required env
    /// vars are missing — caller (`RouterWorker`) interprets this as the
    /// credential-gate failure described in the spec.
    pub fn from_env(paths: RuntimePaths, config: RouterDhcpConfig) -> Option<Self> {
        let username = std::env::var(&config.username_env).ok()?;
        let password = std::env::var(&config.password_env).ok()?;
        if username.is_empty() || password.is_empty() {
            return None;
        }
        Some(Self {
            paths,
            config,
            username,
            password,
        })
    }

    fn python_path(&self) -> PathBuf {
        self.paths.router_adapter_python_path()
    }

    fn playwright_browsers_path(&self) -> PathBuf {
        self.paths.playwright_browsers_path()
    }
}

#[async_trait]
impl RouterChecker for PythonRouterChecker {
    async fn check_once(&self) -> RouterCheckOutcome {
        let python = self.python_path();
        if !python.is_file() {
            return RouterCheckOutcome::SpawnFailed {
                reason: format!(
                    "router-adapter python not found at {} — run `daviszeroclaw router-dhcp install`",
                    python.display()
                ),
            };
        }

        let mut cmd = Command::new(&python);
        cmd.arg("-m").arg("router_adapter")
            .env("ROUTER_URL", &self.config.url)
            .env("ROUTER_USERNAME", &self.username)
            .env("ROUTER_PASSWORD", &self.password)
            .env(
                "PLAYWRIGHT_BROWSERS_PATH",
                self.playwright_browsers_path(),
            )
            .env(
                "PYTHONPATH",
                self.paths.repo_root.display().to_string(),
            )
            .current_dir(&self.paths.repo_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                return RouterCheckOutcome::SpawnFailed {
                    reason: format!("spawn: {err}"),
                };
            }
        };

        let limit = Duration::from_secs(self.config.tick_timeout_secs);
        let output = match timeout(limit, child.wait_with_output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(err)) => {
                return RouterCheckOutcome::Crashed {
                    exit_code: None,
                    stderr_tail: format!("wait_with_output error: {err}"),
                };
            }
            Err(_elapsed) => {
                return RouterCheckOutcome::Crashed {
                    exit_code: None,
                    stderr_tail: format!(
                        "<tick exceeded {}s; child killed via kill_on_drop>",
                        self.config.tick_timeout_secs
                    ),
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        parse_outcome(&stdout, output.status.code(), &stderr)
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check --lib`
Expected: no errors. (Note: `async_trait` is already a dep — see `Cargo.toml`.)

- [ ] **Step 3: Run the full lib tests to make sure existing parse tests still pass**

Run: `cargo test --lib router_supervisor::`
Expected: all 11 tests still PASS.

- [ ] **Step 4: Run lint + format**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: zero warnings, zero diff.

- [ ] **Step 5: Stage but do not commit yet**

```bash
git add src/router_supervisor.rs
```

---

## Task 7: Integration test for `PythonRouterChecker`

**Files:**
- Create: `tests/fixtures/router_stub.py`
- Create: `tests/rust/router_supervisor_spawn.rs`
- Modify: `Cargo.toml` (add `[[test]]` block)

This test verifies that `PythonRouterChecker` can spawn ANY Python module and parse its stdout. We use a stub module to avoid depending on a real router or even a real Playwright install.

- [ ] **Step 1: Create the Python stub**

Create directory and stub:

```bash
mkdir -p tests/fixtures
```

The stub must be importable as `tests.fixtures.router_stub` from a `PYTHONPATH` rooted at the repo. We need a tiny package layout for it to import cleanly via `python -m`.

Create `tests/fixtures/__init__.py` (empty):

```python
```

Create `tests/fixtures/router_stub.py`:

```python
"""Test stub: prints a fixed final-line JSON status, then exits."""
import json
import sys

print("stub: pretending to talk to router")
sys.stdout.write(json.dumps({
    "status": "ok",
    "action": "none",
    "dhcp_was_enabled": False,
    "duration_ms": 42,
}) + "\n")
sys.stdout.flush()
sys.exit(0)
```

- [ ] **Step 2: Add the integration test target to `Cargo.toml`**

Find the existing `[[test]]` blocks (around line 23-37) and add:

```toml

[[test]]
name = "router_supervisor_spawn"
path = "tests/rust/router_supervisor_spawn.rs"
```

- [ ] **Step 3: Create the integration test**

Create `tests/rust/router_supervisor_spawn.rs`:

```rust
//! Integration test: spawn a Python stub via the same mechanism
//! `PythonRouterChecker` uses, parse its stdout, and assert the resulting
//! `RouterCheckOutcome`.
//!
//! This validates the "spawn → stdout capture → last-line parse → outcome"
//! path end-to-end without depending on Playwright, Chromium, or a real
//! router. It does require `python3` on PATH (the same prerequisite as
//! `tests/rust/topic_crawl_*.rs`).

use davis_zero_claw::router_supervisor::{
    parse_outcome, RouterAction, RouterCheckOutcome,
};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

#[tokio::test]
async fn python_stub_emits_parseable_ok_none() {
    let python = which_python3().expect("python3 must be on PATH");
    let repo_root: PathBuf = env!("CARGO_MANIFEST_DIR").into();

    let mut cmd = Command::new(&python);
    cmd.arg("-m")
        .arg("tests.fixtures.router_stub")
        .env("PYTHONPATH", repo_root.display().to_string())
        .current_dir(&repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = cmd.spawn().expect("spawn stub");
    let output = timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .expect("stub should not timeout")
        .expect("stub run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let outcome = parse_outcome(&stdout, output.status.code(), &stderr);

    match outcome {
        RouterCheckOutcome::Ok {
            action,
            dhcp_was_enabled,
            duration_ms,
        } => {
            assert_eq!(action, RouterAction::None);
            assert!(!dhcp_was_enabled);
            assert_eq!(duration_ms, 42);
        }
        other => panic!("expected Ok, got {other:?}\nstdout: {stdout}\nstderr: {stderr}"),
    }
}

fn which_python3() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("python3");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
```

- [ ] **Step 4: Make sure `router_supervisor` is reachable from external tests**

Since the integration test imports `davis_zero_claw::router_supervisor::...`, the module needs to be `pub` at the lib level. Verify:

Run: `grep -n "router_supervisor" src/lib.rs`
Expected: shows `pub mod router_supervisor;`.

If it shows `mod router_supervisor;` instead, change to `pub mod router_supervisor;`.

- [ ] **Step 5: Run the integration test**

Run: `cargo test --test router_supervisor_spawn`
Expected: PASS.

- [ ] **Step 6: Run the full suite to confirm nothing else broke**

Run: `cargo test --lib && cargo test --test router_supervisor_spawn && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: all PASS, zero warnings, zero fmt diff.

- [ ] **Step 7: Commit (Commit 3)**

```bash
git add src/router_supervisor.rs src/lib.rs Cargo.toml tests/fixtures/__init__.py tests/fixtures/router_stub.py tests/rust/router_supervisor_spawn.rs
git commit -m "$(cat <<'EOF'
feat(router-dhcp): RouterChecker trait + PythonRouterChecker

- parse_outcome (pure) covers all four RouterCheckOutcome shapes plus
  defensive-fallback to Crashed for malformed JSON / unknown status.
- PythonRouterChecker spawns `python -m router_adapter`, kill_on_drop,
  per-tick timeout, env injection (PLAYWRIGHT_BROWSERS_PATH shared with
  crawl4ai). Cred snapshot at construction so reloads don't half-update.
- Integration test: spawn a Python stub, parse its stdout, assert the
  Ok outcome — validates the spawn/parse path without Playwright.
EOF
)"
```

---

## Task 8: `RouterWorker` skeleton + dedupe state machine

**Files:**
- Create: `src/router_worker.rs`
- Modify: `src/lib.rs` (add `pub mod router_worker;`)

- [ ] **Step 1: Add module declaration to `src/lib.rs`**

Add after `pub mod router_supervisor;`:

```rust
pub mod router_worker;
```

- [ ] **Step 2: Write the failing test for the dedupe state machine**

Create `src/router_worker.rs`. Tests run after Step 3.

> **Note:** This task only adds pure logic + a test module. Imports for `MempalaceEmitter`, `RouterChecker`, `RouterDhcpConfig`, `Arc`, and `Mutex` are added in Task 9 when `RouterWorker` is introduced. To keep clippy `-D warnings` green at this checkpoint, only import what we use here.

```rust
//! Tick loop, dedupe state machine, and AAAK diary formatting for the
//! router DHCP keeper. See
//! `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md`
//! §"Worker dedupe state machine" for the table this implements.

use crate::router_supervisor::{
    OutcomeKind, RouterAction, RouterCheckOutcome,
};
use chrono::{DateTime, Utc};

const DIARY_WING: &str = "davis.agent.router-keeper";

/// Worker decision: did this tick warrant a diary entry?
#[derive(Debug, Clone, PartialEq, Eq)]
enum DiaryDecision {
    Write(String),
    Skip,
}

/// Mutable per-worker state. Lives behind a Mutex; tick logic is async.
#[derive(Debug, Default)]
pub struct WorkerState {
    last_kind: Option<OutcomeKind>,
    consecutive_failures: u32,
    creds_self_disabled: bool,
    last_run: Option<DateTime<Utc>>,
    last_outcome_label: Option<&'static str>,
}

/// Decision rules from the spec, evaluated top-down (first match wins):
/// 1. First ever tick → write.
/// 2. Failure → success → write RECOVERED.
/// 3. Success+Disabled action (regardless of prior state) → write.
/// 4. Same failure kind as last → skip.
/// 5. Different failure kind → write.
/// 6. Success+None when prior was Success+None → skip.
fn decide_and_advance(
    state: &mut WorkerState,
    outcome: &RouterCheckOutcome,
    now: DateTime<Utc>,
) -> DiaryDecision {
    let kind = outcome.kind();
    let prior = state.last_kind.clone();
    let is_first_ever = prior.is_none();
    let prior_was_failure = matches!(
        prior,
        Some(OutcomeKind::Reported(_)) | Some(OutcomeKind::Crashed) | Some(OutcomeKind::SpawnFailed)
    );
    let now_is_success = outcome.is_success();

    // Update fail counter BEFORE deciding so RECOVERED line can read it.
    let prior_failures = state.consecutive_failures;
    state.consecutive_failures = if now_is_success { 0 } else { prior_failures + 1 };
    state.last_run = Some(now);
    state.last_outcome_label = Some(outcome_label(outcome));

    let decision = if is_first_ever {
        DiaryDecision::Write(format_tick_line(outcome, now, None))
    } else if prior_was_failure && now_is_success {
        DiaryDecision::Write(format_tick_line(outcome, now, Some(prior_failures)))
    } else if matches!(kind, OutcomeKind::OkDisabled) {
        DiaryDecision::Write(format_tick_line(outcome, now, None))
    } else if Some(&kind) == prior.as_ref() {
        DiaryDecision::Skip
    } else if !now_is_success {
        DiaryDecision::Write(format_tick_line(outcome, now, None))
    } else {
        // Success+None → Success+None
        DiaryDecision::Skip
    };

    state.last_kind = Some(kind);
    decision
}

fn format_tick_line(
    outcome: &RouterCheckOutcome,
    now: DateTime<Utc>,
    recovered_after: Option<u32>,
) -> String {
    let ts = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let recovered_segment = recovered_after
        .map(|n| format!("|RECOVERED|prev.failed.{n}"))
        .unwrap_or_default();
    match outcome {
        RouterCheckOutcome::Ok {
            action: RouterAction::None,
            duration_ms,
            ..
        } => format!(
            "TICK:{ts}|router.dhcp{recovered_segment}|action=none|dur.{}s|✓",
            duration_ms / 1000
        ),
        RouterCheckOutcome::Ok {
            action: RouterAction::Disabled,
            duration_ms,
            ..
        } => format!(
            "TICK:{ts}|router.dhcp{recovered_segment}|action=disabled|was.on|dur.{}s|★",
            duration_ms / 1000
        ),
        RouterCheckOutcome::Reported {
            stage,
            reason,
            duration_ms,
        } => format!(
            "TICK:{ts}|router.dhcp|stage.{stage}|reason={reason}|dur.{}s|⚠️",
            duration_ms / 1000
        ),
        RouterCheckOutcome::Crashed {
            exit_code,
            stderr_tail,
        } => {
            let code = exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none".to_string());
            // Truncate stderr_tail in the diary line — full tail still
            // visible via tracing.
            let snippet: String = stderr_tail.chars().take(80).collect();
            format!("TICK:{ts}|router.dhcp|crash|exit.{code}|err={snippet}|⚠️")
        }
        RouterCheckOutcome::SpawnFailed { reason } => {
            format!("TICK:{ts}|router.dhcp|spawn.fail|reason={reason}|⚠️")
        }
    }
}

fn outcome_label(outcome: &RouterCheckOutcome) -> &'static str {
    match outcome {
        RouterCheckOutcome::Ok { .. } => "ok",
        RouterCheckOutcome::Reported { .. } => "reported",
        RouterCheckOutcome::Crashed { .. } => "crashed",
        RouterCheckOutcome::SpawnFailed { .. } => "spawn_failed",
    }
}

/// Read-only snapshot of worker health for the daemon's `/health` route.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RouterHealthSnapshot {
    pub enabled: bool,
    pub last_run: Option<DateTime<Utc>>,
    pub last_outcome: Option<&'static str>,
    pub consecutive_failures: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router_supervisor::{RouterAction, RouterCheckOutcome};
    use chrono::TimeZone;

    fn ok_none() -> RouterCheckOutcome {
        RouterCheckOutcome::Ok {
            action: RouterAction::None,
            dhcp_was_enabled: false,
            duration_ms: 28000,
        }
    }
    fn ok_disabled() -> RouterCheckOutcome {
        RouterCheckOutcome::Ok {
            action: RouterAction::Disabled,
            dhcp_was_enabled: true,
            duration_ms: 31000,
        }
    }
    fn reported(stage: &str) -> RouterCheckOutcome {
        RouterCheckOutcome::Reported {
            stage: stage.into(),
            reason: "selector.timeout".into(),
            duration_ms: 12000,
        }
    }

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[test]
    fn first_tick_always_writes() {
        let mut state = WorkerState::default();
        let d = decide_and_advance(&mut state, &ok_none(), ts(1700_000_000));
        assert!(matches!(d, DiaryDecision::Write(_)));
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn second_ok_none_after_first_ok_none_is_skipped() {
        let mut state = WorkerState::default();
        let _ = decide_and_advance(&mut state, &ok_none(), ts(1));
        let d = decide_and_advance(&mut state, &ok_none(), ts(2));
        assert_eq!(d, DiaryDecision::Skip);
    }

    #[test]
    fn ok_disabled_always_writes() {
        let mut state = WorkerState::default();
        let _ = decide_and_advance(&mut state, &ok_none(), ts(1));
        let d = decide_and_advance(&mut state, &ok_disabled(), ts(2));
        assert!(matches!(d, DiaryDecision::Write(s) if s.contains("action=disabled")));
    }

    #[test]
    fn three_identical_failures_dedupe_to_one_diary() {
        let mut state = WorkerState::default();
        let mut writes = 0;
        for _ in 0..3 {
            if matches!(
                decide_and_advance(&mut state, &reported("login"), ts(1)),
                DiaryDecision::Write(_)
            ) {
                writes += 1;
            }
        }
        assert_eq!(writes, 1);
        assert_eq!(state.consecutive_failures, 3);
    }

    #[test]
    fn different_failure_stage_writes_new_entry() {
        let mut state = WorkerState::default();
        let _ = decide_and_advance(&mut state, &reported("login"), ts(1));
        let d = decide_and_advance(&mut state, &reported("iframe"), ts(2));
        assert!(matches!(d, DiaryDecision::Write(s) if s.contains("stage.iframe")));
    }

    #[test]
    fn recovery_writes_with_prev_failed_count() {
        let mut state = WorkerState::default();
        for _ in 0..3 {
            let _ = decide_and_advance(&mut state, &reported("login"), ts(1));
        }
        let d = decide_and_advance(&mut state, &ok_none(), ts(2));
        match d {
            DiaryDecision::Write(s) => {
                assert!(s.contains("RECOVERED"), "got: {s}");
                assert!(s.contains("prev.failed.3"), "got: {s}");
            }
            DiaryDecision::Skip => panic!("recovery must write"),
        }
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn snapshot_reflects_state() {
        let mut state = WorkerState::default();
        let _ = decide_and_advance(&mut state, &reported("login"), ts(1700_000_000));
        let snap = RouterHealthSnapshot {
            enabled: true,
            last_run: state.last_run,
            last_outcome: state.last_outcome_label,
            consecutive_failures: state.consecutive_failures,
        };
        assert_eq!(snap.last_outcome, Some("reported"));
        assert_eq!(snap.consecutive_failures, 1);
        assert!(snap.last_run.is_some());
    }

}
```

The test module imports only what it actually uses. `MempalaceEmitter`, `Arc`, and `Mutex` come into scope in Task 9 when `RouterWorker` is added.

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib router_worker::tests`
Expected: compile error or test runtime failure (some tests should pass since the logic is in this same file). Actually the logic IS in the file; the test should pass. Check carefully — if all 7 tests pass, you can skip directly to Step 5.

- [ ] **Step 4: If any test fails, fix the bug it surfaces**

If `recovery_writes_with_prev_failed_count` fails: the order of update is critical — `prior_failures` must be captured BEFORE state update. Confirm the function reads `state.consecutive_failures` into `prior_failures`, then increments, then formats. The code above is correct; just double-check the indentation matches.

- [ ] **Step 5: Run lint + format**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check && cargo test --lib`
Expected: zero warnings, zero diff, all tests PASS.

- [ ] **Step 6: Stage but do not commit yet**

```bash
git add src/router_worker.rs src/lib.rs
```

---

## Task 9: `RouterWorker` runtime + tick driver

**Files:**
- Modify: `src/router_worker.rs` (add `RouterWorker` struct, `run_one_tick`, `run_loop`, `health_snapshot`)

- [ ] **Step 1: Write the failing tests for the credential gate and full tick**

Add at the bottom of the existing `#[cfg(test)] mod tests` block in `src/router_worker.rs`. (NOTE: the `Mutex`, `Arc`, and `MempalaceEmitter` imports come into scope via `use super::*;` once Task 9's Step 3 adds them at the module top.)

```rust
    use async_trait::async_trait;
    use crate::mempalace_sink::testing::NoopSink;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Test double: records call count, returns canned outcomes from a Vec.
    struct FakeChecker {
        outcomes: Mutex<Vec<RouterCheckOutcome>>,
        call_count: AtomicUsize,
    }

    impl FakeChecker {
        fn new(outcomes: Vec<RouterCheckOutcome>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes),
                call_count: AtomicUsize::new(0),
            }
        }
        fn calls(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl RouterChecker for FakeChecker {
        async fn check_once(&self) -> RouterCheckOutcome {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let mut q = self.outcomes.lock().await;
            q.remove(0)
        }
    }

    fn cfg_enabled() -> RouterDhcpConfig {
        RouterDhcpConfig {
            enabled: true,
            interval_secs: 600,
            tick_timeout_secs: 90,
            url: "http://example".into(),
            username_env: "ROUTER_USERNAME".into(),
            password_env: "ROUTER_PASSWORD".into(),
        }
    }

    #[tokio::test]
    async fn worker_skips_calls_when_creds_self_disabled() {
        let checker = Arc::new(FakeChecker::new(vec![ok_none(); 3]));
        let sink: Arc<dyn MempalaceEmitter> = Arc::new(NoopSink);
        let worker = RouterWorker::new(cfg_enabled(), checker.clone(), sink, /* creds_present = */ false);
        for _ in 0..3 {
            worker.run_one_tick().await;
        }
        assert_eq!(checker.calls(), 0);
        assert!(worker.health_snapshot().await.last_run.is_none());
    }

    #[tokio::test]
    async fn worker_calls_checker_when_creds_present() {
        let checker = Arc::new(FakeChecker::new(vec![ok_none()]));
        let sink: Arc<dyn MempalaceEmitter> = Arc::new(NoopSink);
        let worker = RouterWorker::new(cfg_enabled(), checker.clone(), sink, /* creds_present = */ true);
        worker.run_one_tick().await;
        assert_eq!(checker.calls(), 1);
        let snap = worker.health_snapshot().await;
        assert_eq!(snap.last_outcome, Some("ok"));
    }
```

(`AtomicUsize` is in std; the duplicate `use` line is benign — clippy's `unused_imports` does not flag re-imports inside nested modules. We'll fix any actual lint issues at the end.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib router_worker::tests::worker`
Expected: FAIL — `RouterWorker` does not exist.

- [ ] **Step 3: Replace the imports at the top of `src/router_worker.rs`**

The Task 8 file currently imports only what its pure logic uses. Now that `RouterWorker` lands, replace the top import block with:

```rust
use crate::mempalace_sink::MempalaceEmitter;
use crate::router_supervisor::{
    OutcomeKind, RouterAction, RouterCheckOutcome, RouterChecker,
};
use crate::RouterDhcpConfig;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
```

Then add the `RouterWorker` definition between the `RouterHealthSnapshot` struct and the test module:

```rust
/// Periodic worker. Holds a checker (prod or test), a MemPalace sink,
/// the config, and mutable state behind a Mutex.
pub struct RouterWorker {
    config: RouterDhcpConfig,
    checker: Arc<dyn RouterChecker>,
    sink: Arc<dyn MempalaceEmitter>,
    state: Mutex<WorkerState>,
    /// `false` ⇒ creds were absent at construction; we never call
    /// `checker.check_once()` and ticks are no-ops.
    creds_present: bool,
}

impl RouterWorker {
    pub fn new(
        config: RouterDhcpConfig,
        checker: Arc<dyn RouterChecker>,
        sink: Arc<dyn MempalaceEmitter>,
        creds_present: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            checker,
            sink,
            state: Mutex::new(WorkerState::default()),
            creds_present,
        })
    }

    /// Run exactly one tick. Returns immediately. No retries.
    pub async fn run_one_tick(&self) {
        if !self.creds_present {
            // Already self-disabled; do nothing. The INIT diary line for
            // missing creds is written by `run_loop` exactly once at
            // startup, not here.
            return;
        }
        let outcome = self.checker.check_once().await;
        self.record(&outcome).await;
    }

    /// Long-running loop. Spawned by the daemon. Bails out immediately
    /// when `creds_present == false` after writing one INIT diary line.
    pub async fn run_loop(self: Arc<Self>) {
        if !self.creds_present {
            let now = Utc::now();
            let line = format!(
                "INIT:{}|router.dhcp|disabled.no.creds|⚠️",
                now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            );
            tracing::warn!(
                "router-dhcp: required env vars unset; worker self-disabled"
            );
            self.sink.diary_write(DIARY_WING, &line);
            return;
        }
        tracing::info!(
            interval_secs = self.config.interval_secs,
            "router-dhcp worker starting tick loop"
        );
        let mut ticker =
            tokio::time::interval(Duration::from_secs(self.config.interval_secs));
        // Skip the immediate-fire tick to give Davis a quiet boot.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            self.run_one_tick().await;
        }
    }

    /// Snapshot for `/health` JSON.
    pub async fn health_snapshot(&self) -> RouterHealthSnapshot {
        let state = self.state.lock().await;
        RouterHealthSnapshot {
            enabled: self.config.enabled,
            last_run: state.last_run,
            last_outcome: state.last_outcome_label,
            consecutive_failures: state.consecutive_failures,
        }
    }

    async fn record(&self, outcome: &RouterCheckOutcome) {
        let now = Utc::now();
        let mut state = self.state.lock().await;
        let decision = decide_and_advance(&mut state, outcome, now);
        // Tracing is unconditional — diary is gated by dedupe.
        match outcome {
            RouterCheckOutcome::Ok { action, duration_ms, .. } => {
                tracing::debug!(?action, duration_ms, "router-dhcp tick ok");
            }
            RouterCheckOutcome::Reported { stage, reason, duration_ms } => {
                tracing::warn!(%stage, %reason, duration_ms, "router-dhcp tick failed (reported)");
            }
            RouterCheckOutcome::Crashed { exit_code, stderr_tail } => {
                tracing::error!(?exit_code, stderr_tail = %stderr_tail, "router-dhcp tick crashed");
            }
            RouterCheckOutcome::SpawnFailed { reason } => {
                tracing::error!(%reason, "router-dhcp tick spawn failed");
            }
        }
        if let DiaryDecision::Write(line) = decision {
            self.sink.diary_write(DIARY_WING, &line);
        }
    }
}
```

- [ ] **Step 4: Run tests; they should pass**

Run: `cargo test --lib router_worker::`
Expected: all tests PASS.

- [ ] **Step 5: Run lint + format**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: zero warnings, zero diff. If clippy flags the `use std::sync::atomic::AtomicUsize as _;` re-import in tests, remove it (the earlier `use std::sync::atomic::{AtomicUsize, Ordering};` line is sufficient).

- [ ] **Step 6: Stage but do not commit yet**

```bash
git add src/router_worker.rs
```

---

## Task 10: Re-export and verify worker is reachable

**Files:**
- Modify: `src/lib.rs`

- [ ] **Step 1: Add public re-exports**

Append to the existing block of `pub use ...` lines in `src/lib.rs` (alphabetically after `RoutingProfilesConfig`):

```rust
pub use router_supervisor::{
    parse_outcome, OutcomeKind, PythonRouterChecker, RouterAction, RouterCheckOutcome,
    RouterChecker,
};
pub use router_worker::{RouterHealthSnapshot, RouterWorker};
```

- [ ] **Step 2: Verify the lib still builds**

Run: `cargo build --lib`
Expected: zero errors.

- [ ] **Step 3: Run full lint + tests + format**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check && cargo test --lib`
Expected: zero warnings, zero diff, all PASS.

---

## Task 11: Spawn `RouterWorker` from the daemon + extend `/health`

**Files:**
- Modify: `src/local_proxy.rs` (spawn worker; pass handle to `AppState`)
- Modify: `src/server.rs` (extend `HealthResponse` and `health` handler)

- [ ] **Step 1: Add an `Option<Arc<RouterWorker>>` field to `AppState`**

In `src/server.rs`, find the `pub struct AppState { ... }` block (around line 81). Add this field after `pub shortcut_reply: Option<Arc<crate::shortcut_reply::ShortcutReplyState>>,`:

```rust
    /// Optional handle to the router DHCP keeper worker. `None` when
    /// `[router_dhcp].enabled = false` in local.toml.
    pub router_worker: Option<Arc<crate::RouterWorker>>,
```

Then update `AppState::new` to accept and assign it. Add the parameter at the END of the `new(...)` argument list:

```rust
        router_worker: Option<Arc<crate::RouterWorker>>,
```

And in the `Self { ... }` block initialize:

```rust
            router_worker,
```

- [ ] **Step 2: Update every `AppState::new(...)` call site**

Run: `grep -rn "AppState::new(" src/`

Each call site must pass `None` (test) or `router_worker_arc` (prod). For each match, append the new arg as the last positional. The known call sites (verify by grep):

- `src/local_proxy.rs` — production daemon. Pass the worker handle.
- Any tests that construct `AppState::new(...)` — pass `None`.

For tests, the simplest mechanical change is to pass `None`.

- [ ] **Step 3: Extend `HealthResponse`**

In `src/server.rs`, add to `struct HealthResponse`:

```rust
    /// Router DHCP keeper status. `None` when disabled in local.toml.
    router_dhcp: Option<crate::RouterHealthSnapshot>,
```

Then in `async fn health(...)` update the construction. After computing `mempalace`, add:

```rust
    let router_dhcp = match state.router_worker.as_ref() {
        Some(w) => Some(w.health_snapshot().await),
        None => None,
    };
```

And include `router_dhcp,` in the `HealthResponse { ... }` literal. Also add `"router_dhcp"` to the `features` vec.

- [ ] **Step 4: Spawn the worker in `local_proxy.rs`**

In `src/local_proxy.rs`, after the `shortcut_reply_state` block (around line 285-300) and BEFORE `AppState::new(...)` is called, add:

```rust
    let router_worker = if local_config.router_dhcp.enabled {
        let checker_opt = crate::PythonRouterChecker::from_env(
            paths.clone(),
            local_config.router_dhcp.clone(),
        );
        match checker_opt {
            Some(checker) => {
                let checker_arc: Arc<dyn crate::RouterChecker> = Arc::new(checker);
                let sink_arc: Arc<dyn crate::mempalace_sink::MempalaceEmitter> =
                    Arc::new(mempalace_sink.clone());
                let worker = crate::RouterWorker::new(
                    local_config.router_dhcp.clone(),
                    checker_arc,
                    sink_arc,
                    /* creds_present = */ true,
                );
                let loop_handle = worker.clone();
                tokio::spawn(async move { loop_handle.run_loop().await });
                tracing::info!("router-dhcp worker started");
                Some(worker)
            }
            None => {
                // Creds absent: build a no-op worker that emits the INIT
                // line via run_loop and self-disables. We still install
                // the handle so /health surfaces enabled=true with
                // last_run=null and consecutive_failures=0.
                struct ZeroChecker;
                #[async_trait::async_trait]
                impl crate::RouterChecker for ZeroChecker {
                    async fn check_once(&self) -> crate::RouterCheckOutcome {
                        crate::RouterCheckOutcome::SpawnFailed {
                            reason: "no creds; worker self-disabled".into(),
                        }
                    }
                }
                let sink_arc: Arc<dyn crate::mempalace_sink::MempalaceEmitter> =
                    Arc::new(mempalace_sink.clone());
                let worker = crate::RouterWorker::new(
                    local_config.router_dhcp.clone(),
                    Arc::new(ZeroChecker),
                    sink_arc,
                    /* creds_present = */ false,
                );
                let loop_handle = worker.clone();
                tokio::spawn(async move { loop_handle.run_loop().await });
                tracing::warn!("router-dhcp enabled but creds env vars unset; worker self-disabled");
                Some(worker)
            }
        }
    } else {
        None
    };
```

Then pass `router_worker` to `AppState::new(...)` as the last argument. (Find the call site within `local_proxy.rs` and add the trailing arg.)

- [ ] **Step 5: Build and lint**

Run: `cargo build && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: zero errors, zero warnings, zero diff.

- [ ] **Step 6: Run all lib tests**

Run: `cargo test --lib`
Expected: all PASS.

- [ ] **Step 7: Run integration tests**

Run: `cargo test --test router_supervisor_spawn`
Expected: PASS.

- [ ] **Step 8: Commit (Commit 4 = Tasks 8 + 9 + 10 + 11)**

```bash
git add src/router_worker.rs src/lib.rs src/local_proxy.rs src/server.rs
git commit -m "$(cat <<'EOF'
feat(router-dhcp): worker, dedupe, /health, daemon spawn

- RouterWorker drives the tick loop: credential gate, dedupe state
  machine (top-down rule precedence per spec), AAAK diary writes via
  the existing MempalaceEmitter trait.
- /health gains a router_dhcp section (enabled, last_run, last_outcome,
  consecutive_failures); detail stays in diary.
- local_proxy spawns the worker on startup when [router_dhcp].enabled.
  Missing env-var creds → worker self-disables with a single INIT diary.

7 unit tests cover all six dedupe transitions plus the credential gate
and snapshot. Worker is fully IO-free under test via FakeChecker +
NoopSink.
EOF
)"
```

---

## Task 12: CLI `daviszeroclaw router-dhcp install | run-once`

**Files:**
- Create: `src/cli/router_dhcp.rs`
- Modify: `src/cli/mod.rs` (add subcommand variant + dispatch + module include)

- [ ] **Step 1: Add the `RouterDhcp` subcommand to the clap tree**

In `src/cli/mod.rs`, in `enum Commands`, add:

```rust
    /// Manage the router DHCP keeper worker.
    RouterDhcp {
        #[command(subcommand)]
        command: RouterDhcpCommand,
    },
```

After the existing `enum CrawlCommand` block, add:

```rust
#[derive(Debug, Subcommand)]
enum RouterDhcpCommand {
    /// Provision the router_adapter venv and shared Playwright Chromium.
    Install,
    /// Run the Playwright check once and print the outcome.
    RunOnce,
}
```

In the `run_cli` match block, add a branch (next to `Commands::Crawl`):

```rust
        Commands::RouterDhcp { command } => match command {
            RouterDhcpCommand::Install => install_router_adapter(&paths),
            RouterDhcpCommand::RunOnce => run_once_router_check(&paths).await,
        },
```

At the bottom of the file (with the other `mod X; use X::*;` blocks), add:

```rust
mod router_dhcp;
use router_dhcp::*;
```

- [ ] **Step 2: Create `src/cli/router_dhcp.rs`**

This module mirrors the structure of `src/cli/crawl.rs::install_crawl4ai` but for the new venv. Reuse the existing `process` helpers (`run_status`, `tool_path_env`).

```rust
//! `daviszeroclaw router-dhcp …` subcommand handlers.

use super::*;
use crate::{check_local_config, PythonRouterChecker, RouterChecker, RuntimePaths};
use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::process::Command;

pub(super) fn install_router_adapter(paths: &RuntimePaths) -> Result<()> {
    let python3 = resolve_host_python3()
        .context("need python3.10+ on PATH (e.g. brew install python@3.13)")?;
    let venv_dir = paths.router_adapter_venv_dir();
    let python = paths.router_adapter_python_path();
    let adapter_dir = paths.router_adapter_dir();
    let browsers_path = paths.playwright_browsers_path();

    if !adapter_dir.join("__main__.py").is_file() {
        bail!(
            "router_adapter package not found at {} — repo checkout incomplete",
            adapter_dir.display()
        );
    }

    fs::create_dir_all(&paths.runtime_dir)?;
    fs::create_dir_all(&browsers_path)?;

    if !python.is_file() {
        println!("Creating router-adapter venv: {}", venv_dir.display());
        run_status(
            Command::new(&python3)
                .arg("-m")
                .arg("venv")
                .arg(&venv_dir)
                .env("PATH", tool_path_env())
                .current_dir(&paths.repo_root),
            "python3 -m venv .runtime/davis/router-adapter-venv",
        )?;
    } else {
        println!("router-adapter venv already exists: {}", venv_dir.display());
    }

    println!("Upgrading pip.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("pip")
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "router-adapter pip upgrade",
    )?;

    println!("Installing router_adapter package + playwright + python-dotenv.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("-e")
            .arg(&adapter_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "pip install -e router_adapter",
    )?;

    println!(
        "Installing Playwright Chromium into shared cache: {}",
        browsers_path.display()
    );
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("playwright")
            .arg("install")
            .arg("chromium")
            .env("PLAYWRIGHT_BROWSERS_PATH", &browsers_path)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "python -m playwright install chromium",
    )?;

    println!("router-adapter installed.");
    println!("Python: {}", python.display());
    println!(
        "Browsers: {}  (shared with crawl4ai_adapter; do not double-install)",
        browsers_path.display()
    );
    println!("Next: daviszeroclaw router-dhcp run-once");
    Ok(())
}

pub(super) async fn run_once_router_check(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let cfg = config.router_dhcp.clone();
    if !cfg.enabled {
        println!(
            "[router_dhcp].enabled is false in local.toml — running ad-hoc anyway."
        );
    }
    let checker = PythonRouterChecker::from_env(paths.clone(), cfg)
        .ok_or_else(|| anyhow!("ROUTER_USERNAME / ROUTER_PASSWORD env vars are not set"))?;
    let outcome = checker.check_once().await;
    println!("{outcome:#?}");
    Ok(())
}
```

- [ ] **Step 3: Verify the CLI compiles**

Run: `cargo build`
Expected: zero errors.

- [ ] **Step 4: Smoke-test the CLI parses correctly**

Run: `cargo run --bin daviszeroclaw -- router-dhcp --help`
Expected: prints subcommand help with `install` and `run-once`.

Run: `cargo run --bin daviszeroclaw -- router-dhcp run-once`
Expected: errors with "ROUTER_USERNAME / ROUTER_PASSWORD env vars are not set" (or similar) — does NOT panic.

- [ ] **Step 5: Run full lint + tests + format**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check && cargo test --lib && cargo test --test router_supervisor_spawn`
Expected: zero warnings, zero diff, all PASS.

- [ ] **Step 6: Commit (Commit 5 = Task 12)**

```bash
git add src/cli/mod.rs src/cli/router_dhcp.rs
git commit -m "$(cat <<'EOF'
feat(router-dhcp): CLI install + run-once

- daviszeroclaw router-dhcp install: provisions router-adapter venv,
  installs the package editable, runs `playwright install chromium`
  with PLAYWRIGHT_BROWSERS_PATH pointed at the shared cache so we
  reuse the same Chromium binary across both Python adapters.
- daviszeroclaw router-dhcp run-once: bypasses the worker / dedupe /
  diary path; calls PythonRouterChecker once and prints the typed
  outcome. Used for human verification when selectors drift.
EOF
)"
```

---

## Self-Review Checklist (run after Task 12)

- [ ] Run: `git log --oneline | head -10`
  Expected: 5 new commits since `main` head, in order: scaffold → python adapter → checker+integration test → worker+health+spawn → CLI.

- [ ] Run: `cargo test --lib && cargo test --test router_supervisor_spawn && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
  Expected: all PASS, zero warnings, zero diff.

- [ ] Run: `git diff main -- CLAUDE.md` and confirm Python boundary text matches the spec.

- [ ] Run: `daviszeroclaw router-dhcp --help` (after `cargo install --path . --bin daviszeroclaw` or in a debug build).
  Expected: shows install + run-once.

- [ ] Manually inspect `config/davis/local.example.toml` ends with the commented `[router_dhcp]` section.

- [ ] Read the worker source one more time. Confirm:
  - No `#[allow(dead_code)]` anywhere.
  - No `unwrap()` in the worker loop (all paths handled with explicit early-return + tracing).
  - Worker spawn in `local_proxy.rs` uses `tokio::spawn` so panics inside the loop do not crash the daemon.
  - `kill_on_drop(true)` is set on the spawn `Command`.

---

## Out-of-band human verification (after merge to main)

1. `daviszeroclaw router-dhcp install` — confirms the venv comes up clean.
2. Set `ROUTER_USERNAME` and `ROUTER_PASSWORD` env vars (or in your launchd plist for the daemon).
3. `daviszeroclaw router-dhcp run-once` while connected to the router LAN — should print `Ok { action: ... }`.
4. Add `[router_dhcp].enabled = true` to `config/davis/local.toml`.
5. Restart the daemon. Tail the daemon log (`tail -f .runtime/davis/local_proxy.log`) for `router-dhcp worker starting tick loop`.
6. After 10 minutes, check `curl -sS http://127.0.0.1:3010/health | jq .router_dhcp` — expect `last_outcome: "ok"`.
7. (Optional) `mempalace search` against `wing=davis.agent.router-keeper` to see the first `TICK:` diary entry.
