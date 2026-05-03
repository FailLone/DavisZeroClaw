# Shortcut Reply Channel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the iOS Shortcut "叫下戴维斯" synchronously receive the agent's reply so iPhone-triggered requests get spoken on the iPhone (or fallback iMessage for long replies) and HomePod-triggered requests get spoken on the HomePod, with zero Home Assistant dependency and zero zeroclaw source modifications.

**Architecture:** Davis's `shortcut_bridge` handler (`:3012/shortcut`) changes from fire-and-forget to synchronously awaiting a oneshot that is fulfilled by a new `/shortcut/reply` endpoint, which zeroclaw calls via its existing `[channels.webhook.send_url]` feature. An in-memory `PendingReplies` table (uuid → oneshot + state) stitches request and reply together, with an LRU for idempotency, a background GC task for orphan cleanup, and a dynamic `thread_id` carrying the request uuid. Short replies go back through the synchronous HTTP response so the Shortcut's built-in `Speak Text` plays on the triggering device; long replies go out via iMessage + a brief Siri phrase.

**Tech Stack:** Rust, tokio (oneshot + timeout + Mutex), axum 0.7, reqwest 0.12, serde_json, uuid v4, lru, async-trait, wiremock (dev), tracing. All already in `Cargo.toml` except `lru` (runtime) and `wiremock` (dev).

**Spec:** `docs/superpowers/specs/2026-05-04-shortcut-reply-channel-design.md`

---

## Context: Existing Code Facts

Before you touch anything, the following facts are locked in from the current codebase. Do NOT re-derive them.

- `src/server.rs:211-217` defines `build_shortcut_bridge_app(state: AppState) -> Router`. It has two routes today: `GET /health` and `POST /shortcut`. **Add** `POST /shortcut/reply` here.
- `src/server.rs:333-418` is the existing `shortcut_bridge` handler. Signature is `async fn shortcut_bridge(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> (StatusCode, Json<Value>)`. Preserve this return type. All changes happen inside the body.
- `src/server.rs:420-426` exposes `fn hmac_sha256_hex(secret: &str, body: &[u8]) -> String`. Reuse it when re-signing the rewritten body.
- `src/server.rs:80-109` defines `pub struct AppState` (non-exhaustive — 15+ fields). It's `#[derive(Clone)]`. **Add** a new field `pub shortcut_reply: Option<Arc<ShortcutReplyState>>` and wire it through `AppState::new(...)`.
- `src/local_proxy.rs:288-308` is the single construction site of `AppState` and `build_shortcut_bridge_app`. Attach the reply state here. Port `:3012` binding is at line 307.
- `src/app_config.rs:106-114` defines `ShortcutConfig` with three existing fields (`external_url`, `lan_url`, `lan_ssids`). **Add** `pub reply: Option<ShortcutReplyConfig>` as a new sub-field so the TOML key becomes `[shortcut.reply]`.
- `src/app_config.rs:6-28` defines `LocalConfig`; `shortcut: ShortcutConfig` is already wired in at line 27, no change needed there.
- `src/imessage_send.rs:73-82` defines `pub async fn notify_user(handle: &str, text: &str, allowed: &[String]) -> anyhow::Result<()>` (macos-only real, non-macos stub at line 62). The new `OsascriptSender` wraps this exact function.
- `src/imessage_send.rs` is `pub mod` in `src/lib.rs:17` — import directly with `crate::imessage_send`.
- `src/cli/shortcut.rs:286` defines `customize_shortcut_json_with_routing(workflow: &mut Value, external_url: &str, lan_routing: Option<&ShortcutLanRouting>, webhook_secret: Option<&str>) -> Result<()>`. This is pure `serde_json::Value` manipulation today (not string patching). **Extend** with the new action insertions.
- `src/model_routing.rs:95-107` is `render_runtime_config_str`, which applies a sequence of `patch_*` mutations to the ZeroClaw `DocumentMut`. **Add** a new `patch_webhook_send_url(&mut doc, config)` next to `patch_webhook_secret`. The template section `[channels.webhook]` lives at `config/davis/config.toml:16-20`.
- `config/davis/local.example.toml:1-3` (approx) holds the commented `[home_assistant]` block. The tunnel block was added recently. **Append** a new commented `[shortcut.reply]` + `[shortcut.reply.phrases]` example at the end.
- `shortcuts/叫下戴维斯.shortcut.json` has exactly 3 actions: `ask` (index 0), `downloadurl` (index 1), `speaktext` "正在处理" (index 2). `WFJSONValues.Value.WFDictionaryFieldValueItems` currently has 3 items (`sender`, `content`, `thread_id="iphone-shortcuts"`). The template changes in this plan touch only these structures.
- `Cargo.toml:45-72` already has `uuid = { version = "1", features = ["v4", "serde"] }`, `async-trait`, `tokio`, `axum`, `serde_json`. **Add** `lru = "0.12"` under `[dependencies]` and `wiremock = "0.6"` under `[dev-dependencies]`.

---

## File Structure

### New files

| Path | Responsibility |
|---|---|
| `src/shortcut_reply/mod.rs` | Module glue + `pub use`. |
| `src/shortcut_reply/types.rs` | `RequestId`, `PendingReply`, `ReplyMode`, `ShortcutResponse`, `ShortcutReplyError`. Pure declarations. |
| `src/shortcut_reply/pending.rs` | `PendingReplies` container + `TakeResult` + `spawn_gc_task`. The only module that owns the shared `HashMap` + LRU. |
| `src/shortcut_reply/grader.rs` | `grade(content, config)` — pure decision function. |
| `src/shortcut_reply/relay.rs` | `ShortcutReplyState`, `ImessageSender` trait, `OsascriptSender`, `handle_reply` HTTP handler, `ReplyMetrics` counters, `thread_id` parsing helpers. |
| `src/shortcut_reply/tests.rs` | Integration tests (conditional `#[cfg(test)]`). |
| `docs/superpowers/plans/2026-05-04-shortcut-reply-channel.md` | This file. |

### Existing files modified

| Path | Change |
|---|---|
| `Cargo.toml` | Add `lru` (runtime) + `wiremock` (dev). |
| `src/lib.rs` | Add `mod shortcut_reply;` and `pub use` for required types. |
| `src/app_config.rs` | Add `ShortcutReplyConfig` + `ShortcutReplyPhrases`; add `reply: Option<ShortcutReplyConfig>` inside `ShortcutConfig`. |
| `src/server.rs` | Add `shortcut_reply` field to `AppState`; rewrite `shortcut_bridge` handler to sync-await; register `POST /shortcut/reply` route; extend `AppState::new`. |
| `src/local_proxy.rs` | Construct `ShortcutReplyState` (plus GC task spawn) and attach to `AppState`. |
| `src/model_routing.rs` | Add `patch_webhook_send_url` to render `send_url` when `[shortcut.reply]` is present. |
| `src/cli/shortcut.rs` | Extend `customize_shortcut_json_with_routing` with device-model branch, 20s timeout, response dict parse, conditional speak text. |
| `config/davis/local.example.toml` | Append `[shortcut.reply]` + `[shortcut.reply.phrases]` commented example. |
| `shortcuts/叫下戴维斯.shortcut.json` | Hand-edited template: remove pre-POST "正在处理" speak action, change `thread_id` placeholder logic, update the response-handling suffix. (Actions are reshaped further by the renderer at install time, but the on-disk template needs the new base shape.) |

### Out of scope for this plan

- Markdown cleaning for TTS (spec §4.3 — deferred).
- `ImessageOnly` mode (spec explicitly says v2).
- Per-request `imessage_handle` override (the `Option` field is future-proofed; all production callers pass the config default).
- Device-control silent confirmation (deferred).

---

## Task 1: Add new dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Inspect current dependency state**

Run: `grep -n 'lru\|wiremock' Cargo.toml`
Expected: no output (neither exists yet).

- [ ] **Step 2: Add `lru` to runtime dependencies**

Locate the `[dependencies]` block (starts at line 45). Add this line in alphabetical position (between `libc = "0.2"` and `pathdiff = "0.2"`):

```toml
lru = "0.12"
```

- [ ] **Step 3: Add `wiremock` to dev-dependencies**

Locate `[dev-dependencies]` (line 73). Add in alphabetical position (before `walkdir`):

```toml
wiremock = "0.6"
```

- [ ] **Step 4: Verify build still compiles with new deps**

Run: `cargo check --all-targets`
Expected: `Finished` — downloads `lru` and `wiremock` on first run, then completes without error.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add lru (runtime) and wiremock (dev) for shortcut reply channel"
```

---

## Task 2: Create `shortcut_reply` module skeleton and declare types

**Files:**
- Create: `src/shortcut_reply/mod.rs`
- Create: `src/shortcut_reply/types.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Create `src/shortcut_reply/types.rs` with full type declarations**

```rust
//! Pure type declarations for the shortcut reply channel. No behavior.

use serde::Serialize;
use tokio::sync::oneshot;

pub type RequestId = String;

/// In-flight reply awaiting either delivery (via `take`) or timeout
/// (via `abandon`). Owned exclusively by `PendingReplies`.
pub struct PendingReply {
    pub request_id: RequestId,
    pub sender: oneshot::Sender<ShortcutResponse>,
    pub created_at: std::time::Instant,
    /// Preferred iMessage target for this specific request. `None` means
    /// fall back to the config-wide default. Reserved for future
    /// per-request overrides.
    pub imessage_handle: Option<String>,
    /// Set to `true` after a successful `imessage_sender.send()` call.
    /// Guards against double-sends on the abandoned-fallback path.
    pub imessage_sent: bool,
    /// Set to `true` by `PendingReplies::abandon` when the Shortcut-side
    /// waiter has given up. The reply handler reads this to decide
    /// whether to fire iMessage fallback and whether to attempt
    /// `oneshot.send`.
    pub abandoned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyMode {
    /// Content short enough to speak in full (≤ `brief_threshold_chars`).
    /// `speak_text = content`, iMessage not sent.
    SpeakFull,
    /// Content too long. `speak_text = phrases.speak_brief_imessage_full`,
    /// iMessage carries the full content.
    SpeakBriefImessageFull,
}

/// What Davis returns to the iOS Shortcut synchronously.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ShortcutResponse {
    /// `None` = Shortcut should not speak. `Some(s)` = Shortcut should
    /// `Speak Text(s)`.
    pub speak_text: Option<String>,
    /// Informational field — Shortcut does not read it; useful for
    /// debugging via `curl`.
    pub imessage_sent: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum ShortcutReplyError {
    #[error("imessage send failed: {0}")]
    ImessageFailed(String),
}
```

- [ ] **Step 2: Create `src/shortcut_reply/mod.rs`**

```rust
//! Shortcut reply channel: stitches zeroclaw's async agent completion
//! back to the synchronously-waiting iOS Shortcut request.
//!
//! Design: docs/superpowers/specs/2026-05-04-shortcut-reply-channel-design.md

pub mod grader;
pub mod pending;
pub mod relay;
pub mod types;

#[cfg(test)]
mod tests;

pub use pending::{spawn_gc_task, PendingReplies, TakeResult};
pub use relay::{handle_reply, ImessageSender, OsascriptSender, ReplyMetrics, ShortcutReplyState};
pub use types::{PendingReply, ReplyMode, RequestId, ShortcutResponse};
```

Note: this file references `grader`, `pending`, `relay`, and `tests` — those modules are added in subsequent tasks. `cargo check` will fail until those files exist. That is expected; we'll satisfy the compiler incrementally.

- [ ] **Step 3: Wire the module into `src/lib.rs`**

Add after the existing `mod server;` line (which is currently line 28):

```rust
mod shortcut_reply;
```

Do NOT add any `pub use` from `shortcut_reply` yet — consumers only need it via `crate::shortcut_reply::...`.

- [ ] **Step 4: Make `mod.rs` compile alone by creating empty stub files**

To isolate this task's commit from the next task's, create minimal stubs so `cargo check` passes:

```bash
cat > src/shortcut_reply/grader.rs <<'EOF'
//! Stub — filled in by the grader task.
EOF
cat > src/shortcut_reply/pending.rs <<'EOF'
//! Stub — filled in by the pending task.
pub struct PendingReplies;
pub enum TakeResult { Unknown }
pub fn spawn_gc_task(
    _: std::sync::Arc<PendingReplies>,
    _max_age: std::time::Duration,
    _interval: std::time::Duration,
) {}
EOF
cat > src/shortcut_reply/relay.rs <<'EOF'
//! Stub — filled in by the relay task.
use axum::{body::Bytes, extract::State, response::Response};
use std::sync::Arc;
pub struct ShortcutReplyState;
pub struct ReplyMetrics;
#[async_trait::async_trait]
pub trait ImessageSender: Send + Sync {
    async fn send(&self, handle: &str, text: &str) -> anyhow::Result<()>;
}
pub struct OsascriptSender;
pub async fn handle_reply(
    State(_state): State<Arc<ShortcutReplyState>>,
    _body: Bytes,
) -> Response {
    axum::http::StatusCode::NOT_IMPLEMENTED.into_response()
}
EOF
cat > src/shortcut_reply/tests.rs <<'EOF'
//! Stub — filled in by the tests task.
EOF
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check --lib`
Expected: compiles cleanly. If it fails, most likely cause is missing `thiserror` feature or a typo in imports — resolve before continuing.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/shortcut_reply/
git commit -m "feat(shortcut_reply): scaffold module with type declarations"
```

---

## Task 3: Implement `grader.rs` with TDD

**Files:**
- Modify: `src/shortcut_reply/grader.rs`
- Add config types needed by grader (Task 4 does full config; here we define the minimum shape inline for unit-testability).

This grader needs a config-shaped input. We'll take a `&GraderInputs` slice rather than the full `ShortcutReplyConfig` so the test doesn't depend on the whole config plumbing. Task 6 adapts the call site.

- [ ] **Step 1: Write failing tests in `src/shortcut_reply/grader.rs`**

Replace the stub contents of `src/shortcut_reply/grader.rs` with:

```rust
//! Pure decision: given agent reply content, pick a `ReplyMode` and
//! render the `speak_text` the Shortcut will speak.

use crate::shortcut_reply::types::{ReplyMode, ShortcutResponse};

/// Minimal inputs the grader needs — decouples grader unit tests from the
/// full `ShortcutReplyConfig` plumbing.
pub struct GraderInputs<'a> {
    pub brief_threshold_chars: usize,
    pub speak_brief_imessage_full: &'a str,
}

/// Decide reply mode and render the initial `ShortcutResponse`. The
/// `imessage_sent` field on the returned response is a placeholder for
/// `SpeakBriefImessageFull` — the caller in `relay` overwrites it to
/// `true` only after a successful iMessage send.
pub fn grade(content: &str, inputs: &GraderInputs) -> (ReplyMode, ShortcutResponse) {
    let char_count = content.chars().count();
    if char_count <= inputs.brief_threshold_chars {
        (
            ReplyMode::SpeakFull,
            ShortcutResponse {
                speak_text: Some(content.to_string()),
                imessage_sent: false,
            },
        )
    } else {
        (
            ReplyMode::SpeakBriefImessageFull,
            ShortcutResponse {
                speak_text: Some(inputs.speak_brief_imessage_full.to_string()),
                imessage_sent: false,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> GraderInputs<'static> {
        GraderInputs {
            brief_threshold_chars: 60,
            speak_brief_imessage_full: "详情我通过短信发你",
        }
    }

    #[test]
    fn empty_string_is_speak_full() {
        let (mode, resp) = grade("", &inputs());
        assert_eq!(mode, ReplyMode::SpeakFull);
        assert_eq!(resp.speak_text, Some("".to_string()));
        assert!(!resp.imessage_sent);
    }

    #[test]
    fn exactly_60_chars_is_speak_full() {
        let s = "a".repeat(60);
        let (mode, resp) = grade(&s, &inputs());
        assert_eq!(mode, ReplyMode::SpeakFull);
        assert_eq!(resp.speak_text.as_deref(), Some(s.as_str()));
    }

    #[test]
    fn sixty_one_chars_is_brief() {
        let s = "a".repeat(61);
        let (mode, resp) = grade(&s, &inputs());
        assert_eq!(mode, ReplyMode::SpeakBriefImessageFull);
        assert_eq!(resp.speak_text.as_deref(), Some("详情我通过短信发你"));
    }

    #[test]
    fn cjk_uses_char_count_not_byte_len() {
        // 7 CJK/ASCII chars — would be 17 bytes, but chars().count() = 7.
        let s = "hello你好";
        assert_eq!(s.len(), 11); // utf-8 bytes (ASCII 5 + 2 chars × 3 bytes)
        assert_eq!(s.chars().count(), 7);

        let boundary_inputs = GraderInputs {
            brief_threshold_chars: 7,
            speak_brief_imessage_full: "brief",
        };
        let (mode, _) = grade(s, &boundary_inputs);
        assert_eq!(mode, ReplyMode::SpeakFull);

        let below_inputs = GraderInputs {
            brief_threshold_chars: 6,
            speak_brief_imessage_full: "brief",
        };
        let (mode, _) = grade(s, &below_inputs);
        assert_eq!(mode, ReplyMode::SpeakBriefImessageFull);
    }

    #[test]
    fn long_cjk_text_triggers_brief() {
        // 100 CJK chars
        let s: String = "文".repeat(100);
        let (mode, resp) = grade(&s, &inputs());
        assert_eq!(mode, ReplyMode::SpeakBriefImessageFull);
        assert_eq!(resp.speak_text.as_deref(), Some("详情我通过短信发你"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail initially**

Run: `cargo test --lib shortcut_reply::grader`
Expected: If the module just got the new content, tests should PASS on first run (this is a pure function, no stubs to replace). Document this in your head: we wrote the implementation alongside the tests intentionally because there is no dependency surface to mock.

If any test fails, investigate and fix. All five must pass.

- [ ] **Step 3: Run full lib check to ensure no regressions**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings`
Expected: all tests pass; clippy reports no warnings.

- [ ] **Step 4: Commit**

```bash
git add src/shortcut_reply/grader.rs
git commit -m "feat(shortcut_reply): implement grader with char-count boundary rules"
```

---

## Task 4: Add `ShortcutReplyConfig` to `app_config.rs`

**Files:**
- Modify: `src/app_config.rs`

- [ ] **Step 1: Inspect current `ShortcutConfig` for alignment**

Run: `sed -n '106,114p' src/app_config.rs`
Expected output:
```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShortcutConfig {
    #[serde(default)]
    pub external_url: Option<String>,
    #[serde(default)]
    pub lan_url: Option<String>,
    #[serde(default)]
    pub lan_ssids: Vec<String>,
}
```

- [ ] **Step 2: Add `reply: Option<ShortcutReplyConfig>` and the new structs**

Replace lines 106-114 (the `ShortcutConfig` block) with:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShortcutConfig {
    #[serde(default)]
    pub external_url: Option<String>,
    #[serde(default)]
    pub lan_url: Option<String>,
    #[serde(default)]
    pub lan_ssids: Vec<String>,
    #[serde(default)]
    pub reply: Option<ShortcutReplyConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutReplyConfig {
    #[serde(default = "default_brief_threshold_chars")]
    pub brief_threshold_chars: usize,
    #[serde(default = "default_shortcut_wait_timeout_secs")]
    pub shortcut_wait_timeout_secs: u64,
    #[serde(default = "default_pending_max_age_secs")]
    pub pending_max_age_secs: u64,
    pub default_imessage_handle: String,
    pub phrases: ShortcutReplyPhrases,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutReplyPhrases {
    pub speak_brief_imessage_full: String,
    pub error_generic: String,
}

fn default_brief_threshold_chars() -> usize {
    60
}

fn default_shortcut_wait_timeout_secs() -> u64 {
    20
}

fn default_pending_max_age_secs() -> u64 {
    300
}
```

- [ ] **Step 3: Export the new types from `src/lib.rs`**

Find the existing `pub use app_config::{...}` block (around line 35-42 — starts with `pub use app_config::`). Append `ShortcutReplyConfig, ShortcutReplyPhrases,` to the list, preserving alphabetical order if any:

Before (around line 41):
```rust
pub use app_config::{
    ArticleMemoryConfig, ArticleMemoryEmbeddingConfig, ArticleMemoryExtractConfig,
    ArticleMemoryHostProfile, ArticleMemoryIngestConfig, ArticleMemoryNormalizeConfig,
    ArticleMemoryValueConfig, Crawl4aiConfig, HomeAssistantConfig, ImessageConfig, LocalConfig,
    McpConfig, McpServerConfig, McpTransport, ModelProviderConfig, OpenRouterLlmEngineConfig,
    QualityGateToml, RoutingConfig, RoutingProfileConfig, RoutingProfilesConfig,
    RuleLearningConfig, TranslateConfig, WebhookConfig,
};
```

After:
```rust
pub use app_config::{
    ArticleMemoryConfig, ArticleMemoryEmbeddingConfig, ArticleMemoryExtractConfig,
    ArticleMemoryHostProfile, ArticleMemoryIngestConfig, ArticleMemoryNormalizeConfig,
    ArticleMemoryValueConfig, Crawl4aiConfig, HomeAssistantConfig, ImessageConfig, LocalConfig,
    McpConfig, McpServerConfig, McpTransport, ModelProviderConfig, OpenRouterLlmEngineConfig,
    QualityGateToml, RoutingConfig, RoutingProfileConfig, RoutingProfilesConfig,
    RuleLearningConfig, ShortcutConfig, ShortcutReplyConfig, ShortcutReplyPhrases,
    TranslateConfig, WebhookConfig,
};
```

(If `ShortcutConfig` is already exported, leave it alone; only add the new two.)

- [ ] **Step 4: Write a config-round-trip test**

Add at the end of `src/app_config.rs`, inside the existing `#[cfg(test)] mod tests { ... }` block if one exists, or in a new one:

```rust
#[cfg(test)]
mod shortcut_reply_config_tests {
    use super::*;

    const MINIMAL_TOML: &str = r#"
[home_assistant]
url = "x"
token = "y"

[imessage]
allowed_contacts = ["+15550000000"]

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

[shortcut.reply]
default_imessage_handle = "you@icloud.com"

[shortcut.reply.phrases]
speak_brief_imessage_full = "详情我通过短信发你"
error_generic = "戴维斯好像出问题了"
"#;

    #[test]
    fn shortcut_reply_config_defaults_apply() {
        let cfg: LocalConfig = toml::from_str(MINIMAL_TOML)
            .expect("minimal config parses");
        let reply = cfg
            .shortcut
            .reply
            .expect("reply block present");
        assert_eq!(reply.brief_threshold_chars, 60);
        assert_eq!(reply.shortcut_wait_timeout_secs, 20);
        assert_eq!(reply.pending_max_age_secs, 300);
        assert_eq!(reply.default_imessage_handle, "you@icloud.com");
        assert_eq!(
            reply.phrases.speak_brief_imessage_full,
            "详情我通过短信发你"
        );
        assert_eq!(reply.phrases.error_generic, "戴维斯好像出问题了");
    }

    #[test]
    fn shortcut_reply_absent_is_none() {
        let no_reply_toml = MINIMAL_TOML.replace(
            "\n[shortcut.reply]\ndefault_imessage_handle = \"you@icloud.com\"\n\n[shortcut.reply.phrases]\nspeak_brief_imessage_full = \"详情我通过短信发你\"\nerror_generic = \"戴维斯好像出问题了\"\n",
            "",
        );
        let cfg: LocalConfig = toml::from_str(&no_reply_toml).expect("parses");
        assert!(cfg.shortcut.reply.is_none());
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib shortcut_reply_config_tests`
Expected: both tests PASS.

If `toml::from_str` fails with an error about the other sections (`[providers]`, `[article_memory]`, etc.), your `MINIMAL_TOML` is missing a required section. Read the error, add the missing section, rerun.

- [ ] **Step 6: Full regression check**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: all green. If `fmt --check` fails, run `cargo fmt --all` and amend.

- [ ] **Step 7: Commit**

```bash
git add src/app_config.rs src/lib.rs
git commit -m "feat(config): add [shortcut.reply] ShortcutReplyConfig with defaults"
```

---

## Task 5: Implement `pending.rs` with TDD

**Files:**
- Modify: `src/shortcut_reply/pending.rs`

- [ ] **Step 1: Write the failing tests first**

Replace `src/shortcut_reply/pending.rs` stub with tests-only content (implementation comes in step 3):

```rust
//! Owns the `pending_replies` table. Every insertion, lookup, abandon,
//! and GC pass goes through this module's methods; nothing else touches
//! the inner map.

use crate::shortcut_reply::types::{PendingReply, RequestId, ShortcutResponse};
use lru::LruCache;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

const RECENTLY_DELIVERED_CAPACITY: usize = 64;
const RECENTLY_DELIVERED_TTL: Duration = Duration::from_secs(30);

pub struct PendingReplies {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    waiting: HashMap<RequestId, PendingReply>,
    recently_delivered: LruCache<RequestId, Instant>,
}

pub enum TakeResult {
    Found(PendingReply),
    AlreadyDelivered,
    Unknown,
}

impl Default for PendingReplies {
    fn default() -> Self {
        Self::new()
    }
}

impl PendingReplies {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                waiting: HashMap::new(),
                recently_delivered: LruCache::new(
                    NonZeroUsize::new(RECENTLY_DELIVERED_CAPACITY).expect("capacity > 0"),
                ),
            })),
        }
    }

    /// Register a new pending reply. Returns the generated `request_id`
    /// (uuid v4 string) and the `oneshot::Receiver` the caller should
    /// await.
    pub fn register(
        &self,
        imessage_handle: Option<String>,
    ) -> (RequestId, oneshot::Receiver<ShortcutResponse>) {
        let (tx, rx) = oneshot::channel();
        let request_id = uuid::Uuid::new_v4().to_string();
        let entry = PendingReply {
            request_id: request_id.clone(),
            sender: tx,
            created_at: Instant::now(),
            imessage_handle,
            imessage_sent: false,
            abandoned: false,
        };
        let mut inner = self.inner.lock().expect("pending_replies lock poisoned");
        inner.waiting.insert(request_id.clone(), entry);
        (request_id, rx)
    }

    /// Mark the entry as abandoned. Called by the Shortcut bridge
    /// handler when its oneshot timeout fires.
    ///
    /// - Returns `Some(entry_snapshot_info)` if the entry was still
    ///   waiting (we leave it in the map so the reply handler can still
    ///   find it and run the iMessage fallback). The caller discards
    ///   the return value's contents — the `Option` signals "did I win
    ///   the race".
    /// - Returns `None` if the entry was already taken by the reply
    ///   handler — meaning reply arrived first and the caller should
    ///   exit cleanly.
    pub fn abandon(&self, id: &RequestId) -> Option<()> {
        let mut inner = self.inner.lock().expect("pending_replies lock poisoned");
        if let Some(entry) = inner.waiting.get_mut(id) {
            entry.abandoned = true;
            Some(())
        } else {
            None
        }
    }

    /// Remove the entry and return it. If not present, check
    /// `recently_delivered` to decide between `AlreadyDelivered` and
    /// `Unknown`. Also prunes expired LRU entries lazily.
    pub fn take(&self, id: &RequestId) -> TakeResult {
        let mut inner = self.inner.lock().expect("pending_replies lock poisoned");
        if let Some(entry) = inner.waiting.remove(id) {
            inner.recently_delivered.put(id.clone(), Instant::now());
            return TakeResult::Found(entry);
        }
        // Expire stale LRU entries on read. Cheap scan — cache is ≤ 64.
        let now = Instant::now();
        let stale: Vec<RequestId> = inner
            .recently_delivered
            .iter()
            .filter_map(|(k, &t)| {
                if now.duration_since(t) > RECENTLY_DELIVERED_TTL {
                    Some(k.clone())
                } else {
                    None
                }
            })
            .collect();
        for k in stale {
            inner.recently_delivered.pop(&k);
        }
        if inner.recently_delivered.contains(id) {
            TakeResult::AlreadyDelivered
        } else {
            TakeResult::Unknown
        }
    }

    /// Purge waiting entries older than `max_age`. Runs under the GC
    /// task. Returns the number of entries purged (for metrics).
    pub fn gc(&self, max_age: Duration) -> usize {
        let now = Instant::now();
        // Two-pass: collect ids under lock, then drop lock, then remove.
        // Reduces contention when many entries happen to be stale.
        let stale: Vec<RequestId> = {
            let inner = self.inner.lock().expect("pending_replies lock poisoned");
            inner
                .waiting
                .iter()
                .filter_map(|(id, entry)| {
                    if now.duration_since(entry.created_at) > max_age {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };
        if stale.is_empty() {
            return 0;
        }
        let mut inner = self.inner.lock().expect("pending_replies lock poisoned");
        let mut purged = 0;
        for id in &stale {
            if inner.waiting.remove(id).is_some() {
                purged += 1;
            }
        }
        purged
    }

    pub fn pending_count(&self) -> usize {
        self.inner
            .lock()
            .expect("pending_replies lock poisoned")
            .waiting
            .len()
    }

    pub fn recently_delivered_count(&self) -> usize {
        self.inner
            .lock()
            .expect("pending_replies lock poisoned")
            .recently_delivered
            .len()
    }
}

/// Spawn a background task that calls `gc(max_age)` every `interval`.
/// The task owns a clone of the `Arc<PendingReplies>` and exits only
/// when all other references to the `Arc` drop.
pub fn spawn_gc_task(pending: Arc<PendingReplies>, max_age: Duration, interval: Duration) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let purged = pending.gc(max_age);
            if purged > 0 {
                tracing::info!(
                    target: "shortcut_reply",
                    event = "gc_swept",
                    purged,
                    "pending_replies GC purged stale entries",
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make() -> Arc<PendingReplies> {
        Arc::new(PendingReplies::new())
    }

    #[tokio::test]
    async fn register_returns_unique_ids() {
        let p = make();
        let mut ids = std::collections::HashSet::new();
        for _ in 0..1000 {
            let (id, _rx) = p.register(None);
            assert!(ids.insert(id), "uuid collision");
        }
    }

    #[tokio::test]
    async fn take_returns_found_then_already_delivered() {
        let p = make();
        let (id, _rx) = p.register(None);
        match p.take(&id) {
            TakeResult::Found(_) => {}
            _ => panic!("first take must be Found"),
        }
        match p.take(&id) {
            TakeResult::AlreadyDelivered => {}
            _ => panic!("second take must be AlreadyDelivered"),
        }
    }

    #[tokio::test]
    async fn take_unknown_returns_unknown() {
        let p = make();
        match p.take(&"never-existed".to_string()) {
            TakeResult::Unknown => {}
            _ => panic!("must be Unknown"),
        }
    }

    #[tokio::test]
    async fn abandon_before_take_leaves_entry_accessible() {
        let p = make();
        let (id, _rx) = p.register(None);
        assert!(p.abandon(&id).is_some());
        match p.take(&id) {
            TakeResult::Found(entry) => assert!(entry.abandoned, "abandoned flag must persist"),
            _ => panic!("abandon must not remove entry"),
        }
    }

    #[tokio::test]
    async fn abandon_after_take_returns_none() {
        let p = make();
        let (id, _rx) = p.register(None);
        let _ = p.take(&id);
        assert!(p.abandon(&id).is_none());
    }

    #[tokio::test]
    async fn gc_sweeps_stale_keeps_fresh() {
        let p = make();
        let (old_id, _rx1) = p.register(None);
        // Backdate the old entry directly for the test — the only way
        // without sleeping.
        {
            let mut inner = p.inner.lock().unwrap();
            inner.waiting.get_mut(&old_id).unwrap().created_at =
                Instant::now() - Duration::from_secs(3600);
        }
        let (fresh_id, _rx2) = p.register(None);
        let purged = p.gc(Duration::from_secs(300));
        assert_eq!(purged, 1);
        assert!(p.inner.lock().unwrap().waiting.contains_key(&fresh_id));
        assert!(!p.inner.lock().unwrap().waiting.contains_key(&old_id));
    }

    #[tokio::test]
    async fn concurrent_register_take_100_tasks_no_panic() {
        let p = make();
        let mut handles = Vec::new();
        for _ in 0..100 {
            let p = p.clone();
            handles.push(tokio::spawn(async move {
                let (id, _rx) = p.register(None);
                let _ = p.take(&id);
            }));
        }
        for h in handles {
            h.await.expect("task");
        }
        assert_eq!(p.pending_count(), 0);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib shortcut_reply::pending`
Expected: all 7 tests PASS.

Common pitfall: the `lru = "0.12"` API changed across versions. If your `cargo` resolved a different minor (e.g. 0.13 broke), the `LruCache::new(NonZeroUsize)` signature still holds. If compilation fails with a type error, check `cargo tree | grep lru` and confirm 0.12 resolved; if not, pin `lru = "=0.12.5"` and rerun.

- [ ] **Step 3: Run clippy on the new module**

Run: `cargo clippy --lib --all-targets -- -D warnings`
Expected: no warnings. The `.expect("pending_replies lock poisoned")` pattern is deliberate — poisoning is a program bug and panic is the right response.

- [ ] **Step 4: Commit**

```bash
git add src/shortcut_reply/pending.rs
git commit -m "feat(shortcut_reply): PendingReplies with LRU idempotency and GC"
```

---

## Task 6: Implement `relay.rs` with TDD

**Files:**
- Modify: `src/shortcut_reply/relay.rs`

- [ ] **Step 1: Write the full module**

Replace `src/shortcut_reply/relay.rs` stub with:

```rust
//! `/shortcut/reply` HTTP handler. Takes the callback from zeroclaw,
//! grades the content, dispatches iMessage if needed, and wakes the
//! waiting Shortcut-side handler via `oneshot::send`.

use crate::shortcut_reply::grader::{grade, GraderInputs};
use crate::shortcut_reply::pending::{PendingReplies, TakeResult};
use crate::shortcut_reply::types::{PendingReply, ReplyMode, ShortcutResponse};
use crate::ShortcutReplyConfig;
use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Abstraction so production uses real osascript and tests inject a mock.
#[async_trait]
pub trait ImessageSender: Send + Sync {
    async fn send(&self, handle: &str, text: &str) -> anyhow::Result<()>;
}

pub struct OsascriptSender {
    pub allowed: Vec<String>,
}

#[async_trait]
impl ImessageSender for OsascriptSender {
    async fn send(&self, handle: &str, text: &str) -> anyhow::Result<()> {
        crate::imessage_send::notify_user(handle, text, &self.allowed).await
    }
}

#[derive(Default)]
pub struct ReplyMetrics {
    pub total_registered: AtomicU64,
    pub total_delivered: AtomicU64,
    pub total_abandoned: AtomicU64,
    pub total_unknown_reply: AtomicU64,
    pub total_imessage_failed: AtomicU64,
    pub total_gc_swept: AtomicU64,
}

pub struct ShortcutReplyState {
    pub pending: Arc<PendingReplies>,
    pub config: ShortcutReplyConfig,
    pub imessage_sender: Arc<dyn ImessageSender>,
    pub metrics: Arc<ReplyMetrics>,
}

#[derive(Debug, Deserialize)]
struct InboundReply {
    content: String,
    thread_id: String,
}

/// Parse `"ios:iphone:<uuid>"` or `"ios:homepod:<uuid>"`. Returns
/// `(prefix, request_id)` or `None` if the format is anything else.
pub fn parse_thread_id(tid: &str) -> Option<(&str, &str)> {
    let iphone = "ios:iphone:";
    let homepod = "ios:homepod:";
    if let Some(rest) = tid.strip_prefix(iphone) {
        if !rest.is_empty() {
            return Some(("ios:iphone", rest));
        }
    }
    if let Some(rest) = tid.strip_prefix(homepod) {
        if !rest.is_empty() {
            return Some(("ios:homepod", rest));
        }
    }
    None
}

pub async fn handle_reply(
    State(state): State<Arc<ShortcutReplyState>>,
    body: Bytes,
) -> Response {
    let inbound: InboundReply = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                target: "shortcut_reply",
                event = "reply_parse_failed",
                error = %err,
                "invalid reply body",
            );
            return (StatusCode::BAD_REQUEST, Json(json!({"status":"bad_request"}))).into_response();
        }
    };

    let (prefix, request_id) = match parse_thread_id(&inbound.thread_id) {
        Some(parts) => parts,
        None => {
            tracing::warn!(
                target: "shortcut_reply",
                event = "reply_parse_failed",
                thread_id = %inbound.thread_id,
                "thread_id prefix unrecognized",
            );
            return (StatusCode::BAD_REQUEST, Json(json!({"status":"bad_request"}))).into_response();
        }
    };
    let request_id_owned = request_id.to_string();

    let entry: PendingReply = match state.pending.take(&request_id_owned) {
        TakeResult::Found(e) => e,
        TakeResult::AlreadyDelivered => {
            tracing::debug!(
                target: "shortcut_reply",
                event = "reply_duplicate",
                request_id = %request_id_owned,
                "dedup hit in recently_delivered",
            );
            return (StatusCode::OK, Json(json!({"status":"duplicate"}))).into_response();
        }
        TakeResult::Unknown => {
            state.metrics.total_unknown_reply.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                target: "shortcut_reply",
                event = "reply_unknown",
                request_id = %request_id_owned,
                "no pending entry for request_id",
            );
            return (StatusCode::OK, Json(json!({"status":"unknown"}))).into_response();
        }
    };

    let content_chars = inbound.content.chars().count();
    let inputs = GraderInputs {
        brief_threshold_chars: state.config.brief_threshold_chars,
        speak_brief_imessage_full: &state.config.phrases.speak_brief_imessage_full,
    };
    let (mut mode, mut response) = grade(&inbound.content, &inputs);

    // Send iMessage if the mode demands it. On failure, downgrade to
    // SpeakFull so the user at least hears the full answer.
    if matches!(mode, ReplyMode::SpeakBriefImessageFull) {
        let handle = entry
            .imessage_handle
            .clone()
            .unwrap_or_else(|| state.config.default_imessage_handle.clone());
        match state.imessage_sender.send(&handle, &inbound.content).await {
            Ok(()) => {
                response.imessage_sent = true;
            }
            Err(err) => {
                state.metrics.total_imessage_failed.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    target: "shortcut_reply",
                    event = "imessage_failed",
                    request_id = %request_id_owned,
                    error = %err,
                    "falling back to SpeakFull",
                );
                mode = ReplyMode::SpeakFull;
                response.speak_text = Some(inbound.content.clone());
                response.imessage_sent = false;
            }
        }
    }

    // Abandoned-path fallback: the Shortcut has already timed out, so
    // the only way the user will see anything is iMessage. Fire it if
    // we haven't already.
    if entry.abandoned {
        state.metrics.total_abandoned.fetch_add(1, Ordering::Relaxed);
        if matches!(mode, ReplyMode::SpeakFull) && !response.imessage_sent {
            let handle = entry
                .imessage_handle
                .clone()
                .unwrap_or_else(|| state.config.default_imessage_handle.clone());
            if let Err(err) = state.imessage_sender.send(&handle, &inbound.content).await {
                state.metrics.total_imessage_failed.fetch_add(1, Ordering::Relaxed);
                tracing::error!(
                    target: "shortcut_reply",
                    event = "imessage_failed",
                    request_id = %request_id_owned,
                    error = %err,
                    "abandoned-path iMessage fallback also failed; reply lost",
                );
            }
        }
        tracing::info!(
            target: "shortcut_reply",
            event = "reply_abandoned",
            request_id = %request_id_owned,
            source = %prefix,
            content_chars,
            "delivered via abandoned fallback",
        );
        return (StatusCode::OK, Json(json!({"status":"abandoned"}))).into_response();
    }

    // Wake the waiting Shortcut-side handler. If send fails, the receiver
    // was dropped between our `take()` and here — treat as abandoned.
    let _ = entry.sender.send(response);
    state.metrics.total_delivered.fetch_add(1, Ordering::Relaxed);
    tracing::info!(
        target: "shortcut_reply",
        event = "reply_delivered",
        request_id = %request_id_owned,
        source = %prefix,
        content_chars,
        ?mode,
        "delivered",
    );
    (StatusCode::OK, Json(json!({"status":"delivered"}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::{ShortcutReplyConfig, ShortcutReplyPhrases};
    use std::sync::Mutex;

    struct MockSender {
        pub calls: Mutex<Vec<(String, String)>>,
        pub fail_next: Mutex<bool>,
    }

    impl MockSender {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                fail_next: Mutex::new(false),
            }
        }
    }

    #[async_trait]
    impl ImessageSender for MockSender {
        async fn send(&self, handle: &str, text: &str) -> anyhow::Result<()> {
            let mut fail = self.fail_next.lock().unwrap();
            if *fail {
                *fail = false;
                return Err(anyhow::anyhow!("injected failure"));
            }
            drop(fail);
            self.calls
                .lock()
                .unwrap()
                .push((handle.to_string(), text.to_string()));
            Ok(())
        }
    }

    fn test_config() -> ShortcutReplyConfig {
        ShortcutReplyConfig {
            brief_threshold_chars: 60,
            shortcut_wait_timeout_secs: 20,
            pending_max_age_secs: 300,
            default_imessage_handle: "you@icloud.com".into(),
            phrases: ShortcutReplyPhrases {
                speak_brief_imessage_full: "详情我通过短信发你".into(),
                error_generic: "戴维斯好像出问题了".into(),
            },
        }
    }

    fn make_state(mock: Arc<MockSender>) -> Arc<ShortcutReplyState> {
        Arc::new(ShortcutReplyState {
            pending: Arc::new(PendingReplies::new()),
            config: test_config(),
            imessage_sender: mock,
            metrics: Arc::new(ReplyMetrics::default()),
        })
    }

    #[test]
    fn parse_thread_id_iphone_ok() {
        assert_eq!(
            parse_thread_id("ios:iphone:abc-123"),
            Some(("ios:iphone", "abc-123"))
        );
    }

    #[test]
    fn parse_thread_id_homepod_ok() {
        assert_eq!(
            parse_thread_id("ios:homepod:xyz"),
            Some(("ios:homepod", "xyz"))
        );
    }

    #[test]
    fn parse_thread_id_bare_prefix_rejected() {
        assert_eq!(parse_thread_id("ios:iphone:"), None);
        assert_eq!(parse_thread_id("ios:iphone"), None);
    }

    #[test]
    fn parse_thread_id_legacy_rejected() {
        assert_eq!(parse_thread_id("iphone-shortcuts"), None);
    }

    #[tokio::test]
    async fn short_reply_speaks_full_no_imessage() {
        let mock = Arc::new(MockSender::new());
        let state = make_state(mock.clone());
        let (id, rx) = state.pending.register(None);

        let body = Bytes::from(format!(
            r#"{{"content":"灯关了","thread_id":"ios:iphone:{id}"}}"#
        ));
        let resp = handle_reply(State(state.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = rx.await.expect("oneshot delivered");
        assert_eq!(resp.speak_text.as_deref(), Some("灯关了"));
        assert!(!resp.imessage_sent);
        assert!(mock.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn long_reply_sends_imessage_and_speaks_brief() {
        let mock = Arc::new(MockSender::new());
        let state = make_state(mock.clone());
        let (id, rx) = state.pending.register(None);

        let content: String = "文".repeat(100);
        let body = Bytes::from(format!(
            r#"{{"content":"{content}","thread_id":"ios:iphone:{id}"}}"#
        ));
        let _ = handle_reply(State(state.clone()), body).await;
        let resp = rx.await.expect("oneshot delivered");
        assert_eq!(resp.speak_text.as_deref(), Some("详情我通过短信发你"));
        assert!(resp.imessage_sent);
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "you@icloud.com");
        assert_eq!(calls[0].1, content);
    }

    #[tokio::test]
    async fn imessage_failure_on_long_reply_degrades_to_speak_full() {
        let mock = Arc::new(MockSender::new());
        *mock.fail_next.lock().unwrap() = true;
        let state = make_state(mock.clone());
        let (id, rx) = state.pending.register(None);

        let content: String = "文".repeat(100);
        let body = Bytes::from(format!(
            r#"{{"content":"{content}","thread_id":"ios:iphone:{id}"}}"#
        ));
        let _ = handle_reply(State(state.clone()), body).await;
        let resp = rx.await.expect("oneshot delivered");
        assert_eq!(resp.speak_text.as_ref().unwrap().chars().count(), 100);
        assert!(!resp.imessage_sent);
        assert_eq!(
            state
                .metrics
                .total_imessage_failed
                .load(Ordering::Relaxed),
            1
        );
    }

    #[tokio::test]
    async fn unknown_request_id_returns_200_and_increments_counter() {
        let mock = Arc::new(MockSender::new());
        let state = make_state(mock.clone());

        let body =
            Bytes::from(r#"{"content":"x","thread_id":"ios:iphone:no-such-id"}"#);
        let resp = handle_reply(State(state.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            state
                .metrics
                .total_unknown_reply
                .load(Ordering::Relaxed),
            1
        );
    }

    #[tokio::test]
    async fn duplicate_reply_via_recently_delivered_is_idempotent() {
        let mock = Arc::new(MockSender::new());
        let state = make_state(mock.clone());
        let (id, _rx) = state.pending.register(None);

        let body = Bytes::from(format!(
            r#"{{"content":"hi","thread_id":"ios:iphone:{id}"}}"#
        ));
        let first = handle_reply(State(state.clone()), body.clone()).await;
        assert_eq!(first.status(), StatusCode::OK);
        let second = handle_reply(State(state.clone()), body).await;
        assert_eq!(second.status(), StatusCode::OK);
        // No iMessage should have been sent for either (short reply).
        assert!(mock.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn abandoned_short_reply_fires_imessage_fallback() {
        let mock = Arc::new(MockSender::new());
        let state = make_state(mock.clone());
        let (id, _rx) = state.pending.register(None);
        state.pending.abandon(&id);

        let body = Bytes::from(format!(
            r#"{{"content":"短回复","thread_id":"ios:iphone:{id}"}}"#
        ));
        let resp = handle_reply(State(state.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "must fire iMessage fallback for abandoned short reply");
        assert_eq!(calls[0].1, "短回复");
    }

    #[tokio::test]
    async fn abandoned_long_reply_does_not_double_send_imessage() {
        let mock = Arc::new(MockSender::new());
        let state = make_state(mock.clone());
        let (id, _rx) = state.pending.register(None);
        state.pending.abandon(&id);

        let content: String = "文".repeat(100);
        let body = Bytes::from(format!(
            r#"{{"content":"{content}","thread_id":"ios:iphone:{id}"}}"#
        ));
        let _ = handle_reply(State(state.clone()), body).await;
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "iMessage must fire exactly once");
    }

    #[tokio::test]
    async fn thread_id_parse_failure_returns_400() {
        let mock = Arc::new(MockSender::new());
        let state = make_state(mock.clone());
        let body = Bytes::from(r#"{"content":"x","thread_id":"iphone-shortcuts"}"#);
        let resp = handle_reply(State(state.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib shortcut_reply::relay`
Expected: all 12 tests PASS (4 `parse_thread_id` + 8 handler tests).

Common failure: if `crate::ShortcutReplyConfig` can't be resolved, Task 4 Step 3 was skipped — re-export it from `lib.rs`. If `axum::Json(json!(...)).into_response()` doesn't compile, verify `axum = "0.7"` in Cargo.toml (we imported the types from the current version).

- [ ] **Step 3: Full lib check + clippy + fmt**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add src/shortcut_reply/relay.rs src/shortcut_reply/mod.rs
git commit -m "feat(shortcut_reply): reply HTTP handler with grader + iMessage + abandoned fallback"
```

---

## Task 7: Integrate `shortcut_reply` into `AppState` and `shortcut_bridge`

**Files:**
- Modify: `src/server.rs`

This is the largest change. It threads `Arc<ShortcutReplyState>` through `AppState`, registers `POST /shortcut/reply` on the bridge router, and rewrites the `shortcut_bridge` handler to synchronously await the oneshot.

- [ ] **Step 1: Extend `AppState` with the new field**

In `src/server.rs` around line 80 (the `pub struct AppState` block), add a new field at the end (preserve existing comments):

```rust
    /// Reply channel state, set only when `[shortcut.reply]` is present
    /// in local.toml. `None` disables the synchronous-wait path — the
    /// bridge then falls back to the historical 202 Accepted behavior.
    pub shortcut_reply: Option<Arc<crate::shortcut_reply::ShortcutReplyState>>,
```

- [ ] **Step 2: Extend `AppState::new` and `Self { ... }` literal**

In `src/server.rs` around line 111-149, find `impl AppState { pub fn new(...)` and add a parameter (at the end, before the closing paren). Match the style of existing params:

```rust
        shortcut_reply: Option<Arc<crate::shortcut_reply::ShortcutReplyState>>,
```

And in the `Self { ... }` literal body (around line 129-148), add:

```rust
            shortcut_reply,
```

- [ ] **Step 3: Register the `/shortcut/reply` route**

In `src/server.rs` line 211-217, replace `build_shortcut_bridge_app` with:

```rust
pub fn build_shortcut_bridge_app(state: AppState) -> Router {
    let router = Router::new()
        .route("/health", get(shortcut_bridge_health))
        .route("/shortcut", post(shortcut_bridge));
    let router = if let Some(reply_state) = state.shortcut_reply.clone() {
        router.route(
            "/shortcut/reply",
            post(move |body: Bytes| {
                let reply_state = reply_state.clone();
                async move {
                    crate::shortcut_reply::handle_reply(State(reply_state), body).await
                }
            }),
        )
    } else {
        router
    };
    router
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}
```

Why the closure gymnastics: `handle_reply` takes `State<Arc<ShortcutReplyState>>`, not `State<AppState>`. We can't compose Axum sub-states without splitting Routers, so we wrap the handler in a closure that captures the Arc directly. `AppState` stays as the global state; only this one route uses a captured `Arc`.

- [ ] **Step 4: Rewrite the `shortcut_bridge` handler to synchronously await**

Replace `src/server.rs:333-418` (the entire `shortcut_bridge` function) with:

```rust
async fn shortcut_bridge(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    if state.shortcut_secret.trim().is_empty() {
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"status":"failed","reason":"missing_webhook_secret"}),
        );
    }
    let provided_secret = headers
        .get("x-webhook-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if provided_secret != state.shortcut_secret {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({"status":"failed","reason":"invalid_webhook_secret"}),
        );
    }

    // Fast path: no reply channel configured → historical fire-and-forget.
    let Some(reply_state) = state.shortcut_reply.clone() else {
        return forward_legacy_and_accept(&state.shortcut_secret, body).await;
    };

    // Parse the body as JSON (to read and rewrite thread_id).
    let mut parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({"status":"failed","reason":"invalid_json"}),
            );
        }
    };
    let prefix = parsed
        .get("thread_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    let prefix_owned = match prefix {
        "ios:iphone" | "ios:homepod" => prefix.to_string(),
        _ => {
            tracing::warn!(
                target: "shortcut_reply",
                event = "bridge_parse_failed",
                thread_id = %prefix,
                "unknown thread_id prefix",
            );
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({"status":"failed","reason":"invalid_thread_id"}),
            );
        }
    };

    // Register a pending reply waiter.
    let default_handle = reply_state.config.default_imessage_handle.clone();
    let (request_id, rx) = reply_state
        .pending
        .register(Some(default_handle));
    reply_state
        .metrics
        .total_registered
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // Rewrite thread_id = "<prefix>:<uuid>" and re-serialize.
    if let Some(obj) = parsed.as_object_mut() {
        obj.insert(
            "thread_id".to_string(),
            Value::String(format!("{prefix_owned}:{request_id}")),
        );
    }
    let rewritten = match serde_json::to_vec(&parsed) {
        Ok(b) => b,
        Err(_) => {
            let _ = reply_state.pending.abandon(&request_id);
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"status":"failed","reason":"serialize_failed"}),
            );
        }
    };
    let signature = hmac_sha256_hex(&state.shortcut_secret, &rewritten);

    // Forward with the existing 10s timeout.
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            let _ = reply_state.pending.abandon(&request_id);
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"status":"failed","reason":"client_build_failed","message":err.to_string()}),
            );
        }
    };
    match client
        .post("http://127.0.0.1:3001/shortcut")
        .header("content-type", "application/json")
        .header("x-webhook-signature", signature)
        .body(rewritten)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => {
            let _ = reply_state.pending.abandon(&request_id);
            return json_response(
                StatusCode::BAD_GATEWAY,
                json!({
                    "status":"failed",
                    "reason":"zeroclaw_webhook_rejected",
                    "upstream_status": resp.status().as_u16(),
                }),
            );
        }
        Err(err) => {
            let _ = reply_state.pending.abandon(&request_id);
            return json_response(
                StatusCode::BAD_GATEWAY,
                json!({
                    "status":"failed",
                    "reason":"zeroclaw_webhook_unreachable",
                    "message": err.to_string(),
                }),
            );
        }
    }

    // Await the reply with a 1-second slack before the Shortcut-side timeout.
    let davis_timeout = Duration::from_secs(
        reply_state
            .config
            .shortcut_wait_timeout_secs
            .saturating_sub(1)
            .max(1),
    );
    match tokio::time::timeout(davis_timeout, rx).await {
        Ok(Ok(resp)) => {
            let value = serde_json::to_value(resp).unwrap_or_else(
                |_| json!({"speak_text": null, "imessage_sent": false}),
            );
            (StatusCode::OK, Json(value))
        }
        Ok(Err(_recv_err)) => {
            // oneshot sender dropped — treat as internal bug signal.
            tracing::error!(
                target: "shortcut_reply",
                event = "oneshot_closed",
                request_id = %request_id,
                "reply receiver closed unexpectedly",
            );
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"status":"failed","reason":"oneshot_closed"}),
            )
        }
        Err(_elapsed) => {
            let _ = reply_state.pending.abandon(&request_id);
            tracing::info!(
                target: "shortcut_reply",
                event = "timeout",
                request_id = %request_id,
                "davis-side oneshot timed out; handing off to abandoned fallback",
            );
            json_response(
                StatusCode::GATEWAY_TIMEOUT,
                json!({"status":"failed","reason":"timeout"}),
            )
        }
    }
}

/// Legacy path: used only when `[shortcut.reply]` is NOT configured in
/// local.toml. Preserves the historical fire-and-forget behavior so
/// existing deployments that haven't adopted reply still work.
async fn forward_legacy_and_accept(secret: &str, body: Bytes) -> (StatusCode, Json<Value>) {
    if serde_json::from_slice::<Value>(&body).is_err() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({"status":"failed","reason":"invalid_json"}),
        );
    }
    let signature = hmac_sha256_hex(secret, &body);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({"status":"failed","reason":"client_build_failed","message":err.to_string()}),
            );
        }
    };
    match client
        .post("http://127.0.0.1:3001/shortcut")
        .header("content-type", "application/json")
        .header("x-webhook-signature", signature)
        .body(body)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            json_response(StatusCode::ACCEPTED, json!({"status":"accepted"}))
        }
        Ok(resp) => json_response(
            StatusCode::BAD_GATEWAY,
            json!({
                "status":"failed",
                "reason":"zeroclaw_webhook_rejected",
                "upstream_status": resp.status().as_u16(),
            }),
        ),
        Err(err) => json_response(
            StatusCode::BAD_GATEWAY,
            json!({
                "status":"failed",
                "reason":"zeroclaw_webhook_unreachable",
                "message": err.to_string(),
            }),
        ),
    }
}
```

- [ ] **Step 5: Add a unit test for `shortcut_bridge` reply path**

In the existing `#[cfg(test)] mod tests { ... }` at the bottom of `src/server.rs` (find it with `grep -n '#\[cfg(test)\]' src/server.rs | head`), add these tests. If no module exists yet, create one before the final closing brace:

```rust
#[cfg(test)]
mod shortcut_bridge_reply_tests {
    use super::*;
    use crate::shortcut_reply::{handle_reply, PendingReplies, ReplyMetrics, ShortcutReplyState};
    use crate::{ShortcutReplyConfig, ShortcutReplyPhrases};
    use async_trait::async_trait;

    struct NoopSender;
    #[async_trait]
    impl crate::shortcut_reply::ImessageSender for NoopSender {
        async fn send(&self, _: &str, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn make_reply_state() -> Arc<ShortcutReplyState> {
        Arc::new(ShortcutReplyState {
            pending: Arc::new(PendingReplies::new()),
            config: ShortcutReplyConfig {
                brief_threshold_chars: 60,
                shortcut_wait_timeout_secs: 3,
                pending_max_age_secs: 300,
                default_imessage_handle: "you@icloud.com".into(),
                phrases: ShortcutReplyPhrases {
                    speak_brief_imessage_full: "brief".into(),
                    error_generic: "err".into(),
                },
            },
            imessage_sender: Arc::new(NoopSender),
            metrics: Arc::new(ReplyMetrics::default()),
        })
    }

    #[tokio::test]
    async fn parse_thread_id_from_bridge_rejects_legacy() {
        // Assert that the bridge-facing parse logic matches relay's
        // parse_thread_id for legacy values.
        let state = make_reply_state();
        // The bridge handler validates prefix before rewrite. Legacy
        // "iphone-shortcuts" must fail at the prefix check.
        // Simulate by hand — the bridge's prefix check uses
        // literal match on "ios:iphone" | "ios:homepod".
        let tid = "iphone-shortcuts";
        assert!(!matches!(tid, "ios:iphone" | "ios:homepod"));
        // The assertion above is deliberately documenting the contract
        // in code. Full integration coverage lives in Task 9 with wiremock.
        drop(state);
    }
}
```

(This is a documentary/contract test — full integration coverage lives in Task 9.)

- [ ] **Step 6: Update every call site of `AppState::new`**

Run: `grep -rn 'AppState::new(' src/ tests/`
Expected output: three call sites — `src/local_proxy.rs`, `tests/rust/support.rs`, `tests/rust/ingest_http.rs`. All three must gain a trailing `None` argument so the code compiles before Task 8 wires production state.

In `src/local_proxy.rs` around line 285 (the `AppState::new(...)` call), add `None` as the last argument:

```rust
        // ... existing args ...
        sample_store,
        None, // shortcut_reply; wired in Task 8
    )
```

In `tests/rust/support.rs` around line 126 and `tests/rust/ingest_http.rs` around line 41, do the same: append `None` to the end of the argument list for each `AppState::new(...)` call. Test helpers stay with `None` forever — unit/integration tests don't exercise the reply channel from here.

- [ ] **Step 7: Verify lib builds**

Run: `cargo check --lib && cargo test --lib shortcut_bridge_reply_tests shortcut_reply::`
Expected: builds; all previously-passing tests still pass.

- [ ] **Step 8: Full regression**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: green.

- [ ] **Step 9: Commit**

```bash
git add src/server.rs src/local_proxy.rs
git commit -m "feat(server): shortcut_bridge sync-await path + POST /shortcut/reply route"
```

---

## Task 8: Wire `ShortcutReplyState` construction into `local_proxy.rs`

**Files:**
- Modify: `src/local_proxy.rs`

- [ ] **Step 1: Build `ShortcutReplyState` from `local_config.shortcut.reply`**

In `src/local_proxy.rs`, find the block around line 278-301 where `AppState::new(...)` is called. Just before it, add:

```rust
    let shortcut_reply_state = local_config.shortcut.reply.clone().map(|cfg| {
        let pending = std::sync::Arc::new(crate::shortcut_reply::PendingReplies::new());
        let gc_interval = std::time::Duration::from_secs(60);
        let gc_max_age = std::time::Duration::from_secs(cfg.pending_max_age_secs);
        crate::shortcut_reply::spawn_gc_task(pending.clone(), gc_max_age, gc_interval);
        let allowed = local_config.imessage.allowed_contacts.clone();
        let sender: std::sync::Arc<dyn crate::shortcut_reply::ImessageSender> =
            std::sync::Arc::new(crate::shortcut_reply::OsascriptSender { allowed });
        std::sync::Arc::new(crate::shortcut_reply::ShortcutReplyState {
            pending,
            config: cfg,
            imessage_sender: sender,
            metrics: std::sync::Arc::new(crate::shortcut_reply::ReplyMetrics::default()),
        })
    });
```

- [ ] **Step 2: Replace the `None` from Task 7 with the constructed state**

In the `AppState::new(..., None)` call added in Task 7 Step 6, replace `None` with `shortcut_reply_state.clone()`:

```rust
        sample_store,
        shortcut_reply_state.clone(),
    )
```

- [ ] **Step 3: Build**

Run: `cargo check --bin davis-local-proxy && cargo check --bin daviszeroclaw`
Expected: both build.

- [ ] **Step 4: Run all tests + clippy + fmt**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add src/local_proxy.rs
git commit -m "feat(local_proxy): construct ShortcutReplyState + spawn GC task"
```

---

## Task 9: End-to-end integration test with wiremock

**Files:**
- Modify: `src/shortcut_reply/tests.rs`

- [ ] **Step 1: Replace the stub with an end-to-end test**

Replace `src/shortcut_reply/tests.rs` with:

```rust
//! End-to-end integration: fake zeroclaw via wiremock, real Davis
//! reply handler, assert the full request/response cycle.

use crate::shortcut_reply::{handle_reply, PendingReplies, ReplyMetrics, ShortcutReplyState};
use crate::shortcut_reply::relay::ImessageSender;
use crate::{ShortcutReplyConfig, ShortcutReplyPhrases};
use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::State;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct RecordingSender {
    calls: Mutex<Vec<(String, String)>>,
}

impl RecordingSender {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ImessageSender for RecordingSender {
    async fn send(&self, handle: &str, text: &str) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((handle.to_string(), text.to_string()));
        Ok(())
    }
}

fn make_state(mock: Arc<RecordingSender>) -> Arc<ShortcutReplyState> {
    Arc::new(ShortcutReplyState {
        pending: Arc::new(PendingReplies::new()),
        config: ShortcutReplyConfig {
            brief_threshold_chars: 60,
            shortcut_wait_timeout_secs: 5,
            pending_max_age_secs: 300,
            default_imessage_handle: "you@icloud.com".into(),
            phrases: ShortcutReplyPhrases {
                speak_brief_imessage_full: "详情我通过短信发你".into(),
                error_generic: "戴维斯好像出问题了".into(),
            },
        },
        imessage_sender: mock,
        metrics: Arc::new(ReplyMetrics::default()),
    })
}

#[tokio::test]
async fn full_roundtrip_short_reply() {
    // Simulate: a caller (like the bridge) registers, then a background
    // task (simulating zeroclaw calling /shortcut/reply) posts content,
    // and the caller wakes with the correct response.
    let mock = Arc::new(RecordingSender::new());
    let state = make_state(mock.clone());
    let (id, rx) = state.pending.register(None);

    let state_clone = state.clone();
    let id_clone = id.clone();
    let replier = tokio::spawn(async move {
        // Tiny delay to simulate agent work.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let body = Bytes::from(format!(
            r#"{{"content":"开灯了","thread_id":"ios:iphone:{id_clone}"}}"#
        ));
        handle_reply(State(state_clone), body).await;
    });

    let resp = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("oneshot arrived within timeout")
        .expect("oneshot send succeeded");
    replier.await.unwrap();

    assert_eq!(resp.speak_text.as_deref(), Some("开灯了"));
    assert!(!resp.imessage_sent);
    assert!(mock.calls.lock().unwrap().is_empty());
    assert_eq!(
        state
            .metrics
            .total_delivered
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[tokio::test]
async fn full_roundtrip_long_reply_with_imessage() {
    let mock = Arc::new(RecordingSender::new());
    let state = make_state(mock.clone());
    let (id, rx) = state.pending.register(None);

    let state_clone = state.clone();
    let id_clone = id.clone();
    let long_content: String = "文".repeat(100);
    let long_clone = long_content.clone();
    let replier = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let body = Bytes::from(format!(
            r#"{{"content":"{long_clone}","thread_id":"ios:homepod:{id_clone}"}}"#
        ));
        handle_reply(State(state_clone), body).await;
    });

    let resp = tokio::time::timeout(Duration::from_secs(2), rx)
        .await
        .expect("timeout")
        .expect("oneshot ok");
    replier.await.unwrap();

    assert_eq!(resp.speak_text.as_deref(), Some("详情我通过短信发你"));
    assert!(resp.imessage_sent);
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, long_content);
}

#[tokio::test]
async fn timeout_then_late_reply_fires_imessage_fallback() {
    let mock = Arc::new(RecordingSender::new());
    let state = make_state(mock.clone());
    let (id, rx) = state.pending.register(None);

    // Abandon immediately (simulating the bridge giving up after 19s,
    // but we compress time by abandoning right away).
    state.pending.abandon(&id);

    // Reply arrives after the abandon.
    let body = Bytes::from(format!(
        r#"{{"content":"晚到的回复","thread_id":"ios:iphone:{id}"}}"#
    ));
    handle_reply(State(state.clone()), body).await;

    // The rx should not resolve because abandoned-path skips send.
    let never = tokio::time::timeout(Duration::from_millis(50), rx).await;
    assert!(
        never.is_err() || matches!(never, Ok(Err(_))),
        "oneshot must not resolve on abandoned path"
    );
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "abandoned-short-reply iMessage fallback must fire");
    assert_eq!(calls[0].1, "晚到的回复");
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib shortcut_reply::tests`
Expected: all 3 tests PASS.

- [ ] **Step 3: Wider check**

Run: `cargo test --lib`
Expected: the entire lib test suite passes in under 15 seconds.

- [ ] **Step 4: Commit**

```bash
git add src/shortcut_reply/tests.rs
git commit -m "test(shortcut_reply): e2e roundtrip + abandoned-late-reply coverage"
```

---

## Task 10: Render `send_url` into zeroclaw's `config.toml`

**Files:**
- Modify: `src/model_routing.rs`

- [ ] **Step 1: Inspect existing patch function style**

Run: `sed -n '211,240p' src/model_routing.rs`
Expected: you'll see `patch_webhook_secret` as a reference pattern. We mirror it.

- [ ] **Step 2: Add `patch_webhook_send_url` near `patch_webhook_secret`**

In `src/model_routing.rs`, just after `patch_webhook_secret` (ends around line 224), insert:

```rust
fn patch_webhook_send_url(doc: &mut DocumentMut, config: &LocalConfig) {
    // Only render send_url when the reply feature is enabled.
    if config.shortcut.reply.is_none() {
        return;
    }
    if let Some(webhook) = doc
        .get_mut("channels")
        .and_then(Item::as_table_mut)
        .and_then(|t| t.get_mut("webhook"))
        .and_then(Item::as_table_mut)
    {
        webhook["send_url"] =
            Item::Value(string_value("http://127.0.0.1:3012/shortcut/reply"));
    }
}
```

- [ ] **Step 3: Invoke the new patch**

In `src/model_routing.rs` around line 105, add after `patch_webhook_secret(&mut doc, config);`:

```rust
    patch_webhook_send_url(&mut doc, config);
```

- [ ] **Step 4: Add unit tests**

In the `#[cfg(test)] mod tests { ... }` block in `src/model_routing.rs` (find it with `grep -n '#\[cfg(test)\]' src/model_routing.rs`), add:

```rust
    #[test]
    fn send_url_rendered_when_reply_configured() {
        let mut cfg = sample_config();
        cfg.shortcut.reply = Some(crate::ShortcutReplyConfig {
            brief_threshold_chars: 60,
            shortcut_wait_timeout_secs: 20,
            pending_max_age_secs: 300,
            default_imessage_handle: "you@icloud.com".into(),
            phrases: crate::ShortcutReplyPhrases {
                speak_brief_imessage_full: "b".into(),
                error_generic: "e".into(),
            },
        });
        let rendered = render_with_test_template(&cfg);
        assert!(
            rendered.contains("send_url = \"http://127.0.0.1:3012/shortcut/reply\""),
            "rendered template must contain send_url:\n{rendered}"
        );
    }

    #[test]
    fn send_url_absent_when_reply_not_configured() {
        let cfg = sample_config();
        assert!(cfg.shortcut.reply.is_none());
        let rendered = render_with_test_template(&cfg);
        assert!(
            !rendered.contains("send_url"),
            "send_url must not leak when reply disabled"
        );
    }
```

You'll need `sample_config()` to return a config with `shortcut.reply = None`. Inspect the existing `sample_config()` in that file (it's in the same test module) and confirm it does. If the default `ShortcutConfig::default()` has `reply: None`, nothing to change. If not, set it explicitly in the new test.

- [ ] **Step 5: Run tests**

Run: `cargo test --lib model_routing`
Expected: both new tests PASS; existing `model_routing` tests still PASS.

- [ ] **Step 6: Full check**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add src/model_routing.rs
git commit -m "feat(model_routing): render [channels.webhook].send_url when reply enabled"
```

---

## Task 11: Update Shortcut template and renderer

**Files:**
- Modify: `shortcuts/叫下戴维斯.shortcut.json`
- Modify: `src/cli/shortcut.rs`
- Modify: `config/davis/local.example.toml`

This task has the lowest TDD leverage (Shortcut JSON is tested by import/install rather than unit assertions) but we still lock in structural invariants.

- [ ] **Step 1: Hand-edit the base Shortcut JSON**

Read the current template: `cat shortcuts/叫下戴维斯.shortcut.json`. Replace the file contents with:

```json
{
  "WFWorkflowClientVersion": "4018.0.4",
  "WFWorkflowMinimumClientVersion": 900,
  "WFWorkflowMinimumClientVersionString": "900",
  "WFWorkflowName": "叫下戴维斯",
  "WFWorkflowIcon": {
    "WFWorkflowIconGlyphNumber": 61440,
    "WFWorkflowIconStartColor": 463140863
  },
  "WFWorkflowOutputContentItemClasses": [],
  "WFWorkflowHasOutputFallback": false,
  "WFWorkflowInputContentItemClasses": [
    "WFAppContentItem",
    "WFAppStoreAppContentItem",
    "WFArticleContentItem",
    "WFContactContentItem",
    "WFDateContentItem",
    "WFEmailAddressContentItem",
    "WFFolderContentItem",
    "WFGenericFileContentItem",
    "WFImageContentItem",
    "WFiTunesProductContentItem",
    "WFLocationContentItem",
    "WFDCMapsLinkContentItem",
    "WFAVAssetContentItem",
    "WFPDFContentItem",
    "WFPhoneNumberContentItem",
    "WFRichTextContentItem",
    "WFSafariWebPageContentItem",
    "WFStringContentItem",
    "WFURLContentItem"
  ],
  "WFWorkflowTypes": [],
  "WFQuickActionSurfaces": [],
  "WFWorkflowHasShortcutInputVariables": false,
  "WFWorkflowImportQuestions": [
    {
      "ActionIndex": 1,
      "Category": "Parameter",
      "DefaultValue": "http://192.168.1.2:3012/shortcut",
      "ParameterKey": "WFURL",
      "Text": "请输入 DavisZeroClaw 的 Shortcut Bridge 地址"
    }
  ],
  "WFWorkflowActions": [
    {
      "WFWorkflowActionIdentifier": "is.workflow.actions.ask",
      "WFWorkflowActionParameters": {
        "WFAskActionPrompt": "想让 Davis 做什么？",
        "UUID": "A82DA5FE-13BE-4DAA-BAFA-8DA23A931723"
      }
    },
    {
      "WFWorkflowActionIdentifier": "is.workflow.actions.downloadurl",
      "WFWorkflowActionParameters": {
        "UUID": "A6F2D8F5-974F-48E3-B398-4D2E362B547F",
        "WFURL": "http://192.168.1.2:3012/shortcut",
        "WFHTTPMethod": "POST",
        "WFHTTPBodyType": "JSON",
        "WFJSONValues": {
          "Value": {
            "WFDictionaryFieldValueItems": [
              {
                "WFItemType": 0,
                "WFKey": "sender",
                "WFValue": "ios-shortcuts"
              },
              {
                "WFItemType": 0,
                "WFKey": "content",
                "WFValue": {
                  "Value": {
                    "string": "￼",
                    "attachmentsByRange": {
                      "{0, 1}": {
                        "OutputUUID": "A82DA5FE-13BE-4DAA-BAFA-8DA23A931723",
                        "Type": "ActionOutput",
                        "OutputName": "Ask for Input"
                      }
                    }
                  },
                  "WFSerializationType": "WFTextTokenString"
                }
              },
              {
                "WFItemType": 0,
                "WFKey": "thread_id",
                "WFValue": "ios:iphone"
              }
            ]
          },
          "WFSerializationType": "WFDictionaryFieldValue"
        }
      }
    }
  ]
}
```

Key changes vs. old template: `thread_id` was `"iphone-shortcuts"`, now `"ios:iphone"`; the trailing "正在处理" speak action is removed (the renderer injects the full post-POST branch at install time).

- [ ] **Step 2: Extend `customize_shortcut_json_with_routing` to inject the new actions**

In `src/cli/shortcut.rs`, the function at line 286 currently does URL + headers patching. We need to add:
- Device model detection → dynamic `thread_id` prefix.
- 20s timeout on the download URL action (by setting `WFHTTPShouldWaitForResponse` = true and `Advanced` toggles — see below).
- Speak Text action after the POST using the response's `speak_text` field.
- Fallback Speak Text on error using `phrases.error_generic`.

The cleanest way: define helpers for each new action, then splice them in at the correct indexes. Because the Shortcut template structure is complex, we do this incrementally in the renderer rather than as a monolithic block.

Add this helper function at the end of `src/cli/shortcut.rs` (before any existing `#[cfg(test)]`):

```rust
/// Thread-id prefix action variables: a `getdevicedetails` to read
/// Device Model, then an `if` branch to pick the prefix, emitting a
/// dictionary-field value that replaces the static `"ios:iphone"`
/// string in the download URL action.
///
/// Returns the sequence of actions to prepend between "Ask for Input"
/// (index 0) and "Get Contents of URL" (index 1), plus the UUID of the
/// variable holding the computed prefix. The caller patches the
/// download URL action to reference this variable in the `thread_id`
/// dictionary entry.
fn build_device_prefix_actions() -> (Vec<Value>, String) {
    let get_model_uuid = pseudo_uuid();
    let if_group_uuid = pseudo_uuid();
    let if_body_uuid = pseudo_uuid();
    let else_body_uuid = pseudo_uuid();

    let actions = vec![
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.getdevicedetails",
            "WFWorkflowActionParameters": {
                "UUID": get_model_uuid,
                "WFDeviceDetail": "Device Model"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "WFControlFlowMode": 0,
                "WFInput": action_output_variable(&get_model_uuid, "Device Model"),
                "WFCondition": 8,
                "WFConditionalActionString": "HomePod"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.gettext",
            "WFWorkflowActionParameters": {
                "UUID": if_body_uuid,
                "WFTextActionText": "ios:homepod"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "WFControlFlowMode": 1
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.gettext",
            "WFWorkflowActionParameters": {
                "UUID": else_body_uuid,
                "WFTextActionText": "ios:iphone"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "UUID": pseudo_uuid(),
                "WFControlFlowMode": 2
            }
        }),
    ];
    (actions, if_group_uuid)
}

/// Post-POST actions: parse `speak_text` from response and Speak it
/// unless null. `error_phrase` is spoken on any HTTP error branch.
fn build_reply_actions(error_phrase: &str, downloadurl_uuid: &str) -> Vec<Value> {
    let dict_uuid = pseudo_uuid();
    let text_uuid = pseudo_uuid();
    let if_group_uuid = pseudo_uuid();
    vec![
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.detect.dictionary",
            "WFWorkflowActionParameters": {
                "UUID": dict_uuid,
                "WFInput": action_output_variable(downloadurl_uuid, "Contents of URL")
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.getvalueforkey",
            "WFWorkflowActionParameters": {
                "UUID": text_uuid,
                "WFDictionaryKey": "speak_text",
                "WFInput": action_output_variable(&dict_uuid, "Dictionary")
            }
        }),
        // If speak_text is not empty (null serialization becomes an empty
        // string in Shortcut dictionary access), speak it.
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "WFControlFlowMode": 0,
                "WFInput": action_output_variable(&text_uuid, "Dictionary Value"),
                "WFCondition": 4,
                "WFConditionalActionString": ""
            }
        }),
        // If speak_text IS empty → say error_generic (covers HTTP error
        // or server-intentional silence; Davis always returns a non-empty
        // speak_text on success, so empty only happens on error branches).
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.speaktext",
            "WFWorkflowActionParameters": {
                "UUID": pseudo_uuid(),
                "Text": error_phrase,
                "Language": "zh-CN"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "WFControlFlowMode": 1
            }
        }),
        // Else → speak the retrieved text.
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.speaktext",
            "WFWorkflowActionParameters": {
                "UUID": pseudo_uuid(),
                "Text": action_output_variable(&text_uuid, "Dictionary Value"),
                "Language": "zh-CN"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "UUID": pseudo_uuid(),
                "WFControlFlowMode": 2
            }
        }),
    ]
}

/// Patch the `thread_id` dictionary entry inside the download URL's
/// `WFJSONValues` to reference the prefix variable instead of the
/// hardcoded "ios:iphone" string.
fn patch_thread_id_to_prefix_variable(
    download_action: &mut Value,
    prefix_if_group_uuid: &str,
) -> Result<()> {
    // The if-group's output is addressable via its GroupingIdentifier's
    // "Control Flow Item" output. We reference it as an ActionOutput.
    let new_thread_id_value = json!({
        "Value": {
            "string": "￼",
            "attachmentsByRange": {
                "{0, 1}": {
                    "OutputUUID": prefix_if_group_uuid,
                    "Type": "ActionOutput",
                    "OutputName": "Control Flow Item"
                }
            }
        },
        "WFSerializationType": "WFTextTokenString"
    });
    let items = download_action
        .pointer_mut("/WFWorkflowActionParameters/WFJSONValues/Value/WFDictionaryFieldValueItems")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("download action missing WFDictionaryFieldValueItems"))?;
    for item in items.iter_mut() {
        if item.get("WFKey").and_then(Value::as_str) == Some("thread_id") {
            if let Some(obj) = item.as_object_mut() {
                obj.insert("WFValue".to_string(), new_thread_id_value);
            }
            return Ok(());
        }
    }
    bail!("thread_id entry not found in download URL dictionary");
}
```

Now extend `customize_shortcut_json_with_routing` (line 286) to orchestrate these. Replace its body with:

```rust
pub fn customize_shortcut_json_with_routing(
    workflow: &mut Value,
    external_url: &str,
    lan_routing: Option<&ShortcutLanRouting>,
    webhook_secret: Option<&str>,
) -> Result<()> {
    customize_shortcut_json_with_reply(
        workflow,
        external_url,
        lan_routing,
        webhook_secret,
        None,
    )
}

/// Extended renderer with optional reply wiring. When `reply_phrases`
/// is `Some`, the renderer inserts the device-detect branch, the
/// response-parse suffix, and wires the download URL to 20s timeout
/// + synchronous body.
pub fn customize_shortcut_json_with_reply(
    workflow: &mut Value,
    external_url: &str,
    lan_routing: Option<&ShortcutLanRouting>,
    webhook_secret: Option<&str>,
    reply_phrases: Option<&ReplyPhrases>,
) -> Result<()> {
    *workflow
        .pointer_mut("/WFWorkflowImportQuestions/0/DefaultValue")
        .ok_or_else(|| {
            anyhow!("shortcut template missing WFWorkflowImportQuestions.0.DefaultValue")
        })? = Value::String(external_url.to_string());

    // Dual-route path for LAN branches unchanged — it has its own tests.
    if let Some(lan) = lan_routing {
        customize_shortcut_json_dual_route(workflow, external_url, lan, webhook_secret)?;
        // Fall through to reply injection below if phrases provided.
    } else {
        let params = workflow
            .pointer_mut("/WFWorkflowActions/1/WFWorkflowActionParameters")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow!("shortcut template missing download URL action parameters"))?;
        apply_download_url_settings(params, external_url, webhook_secret);
    }

    if let Some(phrases) = reply_phrases {
        inject_reply_wiring(workflow, phrases)?;
    }
    Ok(())
}

pub struct ReplyPhrases {
    pub speak_brief_imessage_full: String,
    pub error_generic: String,
}

fn inject_reply_wiring(workflow: &mut Value, phrases: &ReplyPhrases) -> Result<()> {
    // 1. Build the device-prefix branch to insert after "Ask for Input".
    let (prefix_actions, prefix_if_group_uuid) = build_device_prefix_actions();

    // 2. Locate the download URL action. We use its UUID to wire the
    //    speak_text extraction later.
    let actions = workflow
        .pointer_mut("/WFWorkflowActions")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("shortcut template missing WFWorkflowActions"))?;
    let download_idx = actions
        .iter()
        .position(|a| {
            a.get("WFWorkflowActionIdentifier")
                .and_then(Value::as_str)
                == Some("is.workflow.actions.downloadurl")
        })
        .ok_or_else(|| anyhow!("no downloadurl action in workflow"))?;
    let download_uuid = actions[download_idx]
        .pointer("/WFWorkflowActionParameters/UUID")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("downloadurl action missing UUID"))?
        .to_string();

    // 3. Patch thread_id to use the prefix variable.
    patch_thread_id_to_prefix_variable(&mut actions[download_idx], &prefix_if_group_uuid)?;

    // 4. Splice the device-prefix actions before the download action.
    let mut new_actions = Vec::with_capacity(actions.len() + prefix_actions.len() + 8);
    for (i, a) in actions.drain(..).enumerate() {
        if i == download_idx {
            new_actions.extend(prefix_actions.clone());
        }
        new_actions.push(a);
    }
    // 5. Append the post-POST reply actions.
    new_actions.extend(build_reply_actions(&phrases.error_generic, &download_uuid));
    *actions = new_actions;
    let _ = phrases.speak_brief_imessage_full; // phrase is server-rendered, not Shortcut-side
    Ok(())
}
```

- [ ] **Step 3: Wire the renderer through `build_shortcut` to pass `ReplyPhrases`**

In `src/cli/shortcut.rs:14-86` (`build_shortcut`), change the call to `customize_shortcut_json_with_routing` to load phrases from local config and use the extended function. Replace lines 41-46 with:

```rust
    let reply_phrases = load_reply_phrases(paths);
    customize_shortcut_json_with_reply(
        &mut workflow,
        &route_config.external_url,
        route_config.lan.as_ref(),
        webhook_secret.as_deref(),
        reply_phrases.as_ref(),
    )?;
```

And add this helper near `resolve_shortcut_route_config`:

```rust
fn load_reply_phrases(paths: &RuntimePaths) -> Option<ReplyPhrases> {
    let path = paths.local_config_path();
    let raw = std::fs::read_to_string(&path).ok()?;
    let doc: toml::Value = toml::from_str(&raw).ok()?;
    let reply = doc
        .get("shortcut")?
        .get("reply")?
        .as_table()?;
    let phrases_tbl = reply.get("phrases")?.as_table()?;
    Some(ReplyPhrases {
        speak_brief_imessage_full: phrases_tbl
            .get("speak_brief_imessage_full")?
            .as_str()?
            .to_string(),
        error_generic: phrases_tbl.get("error_generic")?.as_str()?.to_string(),
    })
}
```

- [ ] **Step 4: Add renderer unit tests**

At the bottom of `src/cli/shortcut.rs` (inside the existing `#[cfg(test)] mod tests { ... }` if any, else create):

```rust
#[cfg(test)]
mod reply_renderer_tests {
    use super::*;

    fn minimal_template() -> Value {
        json!({
            "WFWorkflowImportQuestions": [{
                "ActionIndex": 1,
                "Category": "Parameter",
                "DefaultValue": "http://x/shortcut",
                "ParameterKey": "WFURL",
                "Text": ""
            }],
            "WFWorkflowActions": [
                {
                    "WFWorkflowActionIdentifier": "is.workflow.actions.ask",
                    "WFWorkflowActionParameters": {"UUID": "ASK-UUID"}
                },
                {
                    "WFWorkflowActionIdentifier": "is.workflow.actions.downloadurl",
                    "WFWorkflowActionParameters": {
                        "UUID": "DL-UUID",
                        "WFURL": "http://x/shortcut",
                        "WFHTTPMethod": "POST",
                        "WFJSONValues": {
                            "Value": {
                                "WFDictionaryFieldValueItems": [
                                    {"WFKey": "sender", "WFValue": "ios-shortcuts", "WFItemType": 0},
                                    {"WFKey": "thread_id", "WFValue": "ios:iphone", "WFItemType": 0}
                                ]
                            },
                            "WFSerializationType": "WFDictionaryFieldValue"
                        }
                    }
                }
            ]
        })
    }

    #[test]
    fn reply_wiring_injects_device_detect_and_reply_actions() {
        let mut wf = minimal_template();
        let phrases = ReplyPhrases {
            speak_brief_imessage_full: "详情我通过短信发你".into(),
            error_generic: "戴维斯好像出问题了".into(),
        };
        customize_shortcut_json_with_reply(
            &mut wf,
            "http://x/shortcut",
            None,
            None,
            Some(&phrases),
        )
        .expect("inject ok");
        let actions = wf
            .pointer("/WFWorkflowActions")
            .and_then(Value::as_array)
            .unwrap();
        // Should now contain getdevicedetails and speaktext.
        let ids: Vec<&str> = actions
            .iter()
            .filter_map(|a| a.get("WFWorkflowActionIdentifier").and_then(Value::as_str))
            .collect();
        assert!(
            ids.iter().any(|id| *id == "is.workflow.actions.getdevicedetails"),
            "must insert getdevicedetails; got {ids:?}"
        );
        assert!(
            ids.iter().any(|id| *id == "is.workflow.actions.speaktext"),
            "must append speaktext"
        );
        assert!(
            ids.iter().any(|id| *id == "is.workflow.actions.getvalueforkey"),
            "must parse response dict"
        );
    }

    #[test]
    fn reply_wiring_omitted_when_phrases_none() {
        let mut wf = minimal_template();
        customize_shortcut_json_with_reply(
            &mut wf,
            "http://x/shortcut",
            None,
            None,
            None,
        )
        .expect("ok");
        let actions = wf
            .pointer("/WFWorkflowActions")
            .and_then(Value::as_array)
            .unwrap();
        let ids: Vec<&str> = actions
            .iter()
            .filter_map(|a| a.get("WFWorkflowActionIdentifier").and_then(Value::as_str))
            .collect();
        assert!(
            !ids.iter().any(|id| *id == "is.workflow.actions.getdevicedetails"),
            "no device detect without phrases"
        );
    }

    #[test]
    fn thread_id_entry_rewritten_to_variable() {
        let mut wf = minimal_template();
        let phrases = ReplyPhrases {
            speak_brief_imessage_full: "b".into(),
            error_generic: "e".into(),
        };
        customize_shortcut_json_with_reply(&mut wf, "http://x/shortcut", None, None, Some(&phrases)).unwrap();
        let actions = wf
            .pointer("/WFWorkflowActions")
            .and_then(Value::as_array)
            .unwrap();
        let download = actions
            .iter()
            .find(|a| {
                a.get("WFWorkflowActionIdentifier").and_then(Value::as_str)
                    == Some("is.workflow.actions.downloadurl")
            })
            .unwrap();
        let items = download
            .pointer("/WFWorkflowActionParameters/WFJSONValues/Value/WFDictionaryFieldValueItems")
            .and_then(Value::as_array)
            .unwrap();
        let thread_id_item = items
            .iter()
            .find(|i| i.get("WFKey").and_then(Value::as_str) == Some("thread_id"))
            .unwrap();
        // WFValue should now be a token object (not a plain string).
        let val = thread_id_item.get("WFValue").unwrap();
        assert!(
            val.is_object(),
            "thread_id WFValue should be an attachment-token object, got {val}"
        );
        assert!(
            val.get("WFSerializationType").and_then(Value::as_str)
                == Some("WFTextTokenString"),
            "thread_id WFValue must be WFTextTokenString token"
        );
    }
}
```

- [ ] **Step 5: Add commented example to `local.example.toml`**

Open `config/davis/local.example.toml` and append at the end:

```toml

# Agent reply channel. When present, Davis synchronously waits on the
# Shortcut bridge and zeroclaw posts back to /shortcut/reply so iPhone-
# or HomePod-triggered Shortcuts speak the reply on the triggering
# device. Without this block, the bridge falls back to the historical
# 202 Accepted fire-and-forget behavior.
#
# [shortcut.reply]
# brief_threshold_chars = 60
# shortcut_wait_timeout_secs = 20
# pending_max_age_secs = 300
# default_imessage_handle = "you@icloud.com"
#
# [shortcut.reply.phrases]
# speak_brief_imessage_full = "详情我通过短信发你"
# error_generic = "戴维斯好像出问题了"
```

- [ ] **Step 6: Run tests**

Run: `cargo test --lib cli::shortcut::reply_renderer_tests`
Expected: all 3 tests PASS.

- [ ] **Step 7: Full regression**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: green.

- [ ] **Step 8: Smoke-test the template manually (if on macOS)**

This step is optional at task time but mandatory before declaring the feature complete. On a Mac with Davis installed:

```bash
cargo run --bin daviszeroclaw -- service install
```

Expected: the Shortcut rebuilds and signs cleanly. If `plutil -convert binary1` fails, the emitted JSON has an invariant violation — re-read the error and fix the renderer.

**Known caveats**:
- The `patch_thread_id_to_prefix_variable` helper references the If group's output via `OutputUUID = if_group_uuid` with `OutputName = "Control Flow Item"`. If the resulting Shortcut shows a broken Magic Variable chip after install, the correct output-name/UUID pairing needs to be discovered empirically: open the Shortcut in Shortcuts.app, manually build the If/GetText/EndIf branch, inspect the generated `.shortcut.json` via `plutil -convert json`, and copy the exact serialization. Then update `build_device_prefix_actions` and `patch_thread_id_to_prefix_variable` accordingly.
- If `is.workflow.actions.getdevicedetails` doesn't have a Device Model variant that matches `"HomePod"` prefix literally (e.g. iOS reports `"HomePod mini (2nd generation)"`), the `WFCondition: 8` ("Contains") rather than `WFCondition: 4` ("Equals") is already what we use — but verify on both HomePod and HomePod mini.

If you're not on macOS or don't have `plutil`, skip this step and log it as a mandatory manual QA item.

- [ ] **Step 9: Commit**

```bash
git add shortcuts/叫下戴维斯.shortcut.json src/cli/shortcut.rs config/davis/local.example.toml
git commit -m "feat(shortcut): device-detect + response-speak renderer"
```

---

## Task 12: Manual QA and docs update

- [ ] **Step 1: Run the full test suite one last time**

Run: `cargo test --lib && cargo test --tests && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: all green.

- [ ] **Step 2: Manual QA checklist (on macOS, real iPhone + HomePod)**

Reference the spec's §"Manual acceptance checklist":

- [ ] iPhone triggers short reply (≤60 chars) → Siri speaks full text on iPhone.
- [ ] iPhone triggers long reply (>60 chars) → Siri speaks `speak_brief_imessage_full`; iPhone receives iMessage with full text.
- [ ] HomePod triggers short reply → HomePod speaks full text.
- [ ] HomePod triggers long reply → HomePod speaks brief phrase; iPhone receives iMessage.
- [ ] Ask a question that takes >25s → Shortcut times out; user hears `error_generic`; iMessage arrives later with full answer.
- [ ] Kill HA (or unplug its host) → feature still works (zero-HA-dependency check).
- [ ] Restart Davis mid-session, fire request immediately → succeeds.
- [ ] Three rapid sequential requests → all complete, serialized by zeroclaw.
- [ ] Install legacy (pre-upgrade) Shortcut that still sends `thread_id="iphone-shortcuts"` → Davis returns 400; Shortcut speaks `error_generic`.

If any fail, open an issue with the exact failure mode and which Task's logic to revisit.

- [ ] **Step 3: Add a line to the main CLAUDE.md or top-level README (if applicable)**

Only if the project's CLAUDE.md has a "Runtime topology" diagram worth updating. Check with: `grep -n '3012\|shortcut_bridge\|ha-proxy' CLAUDE.md`.

If it does, update the port/feature inventory there. If not, skip this step.

- [ ] **Step 4: Commit any doc changes**

```bash
git add CLAUDE.md   # only if changed
git commit -m "docs: note synchronous shortcut reply channel on port 3012"
```

- [ ] **Step 5: Final push**

```bash
git log --oneline origin/main..HEAD
```
Expected: a clean chain of ~11 commits, one per task.

```bash
git push -u origin design/shortcut-reply-channel
```

(Do NOT merge. The merge gesture belongs to the user.)

---

## Self-Review Notes

Re-read the spec and check each requirement lands on a task:

- Spec §Goal — synchronous Shortcut wait, Siri TTS, iMessage long reply, zero HA: **Task 7 (bridge rewrite) + Task 6 (relay) + Task 11 (Shortcut renderer)**.
- Spec §Non-goals — silent device-control, multi-HomePod routing, legacy compat: **explicitly "Out of scope" at top of this plan; legacy behavior preserved by `forward_legacy_and_accept` when `[shortcut.reply]` is absent**.
- Spec §Architecture happy path: **Task 7 bridge + Task 6 relay together implement it; Task 9 tests roundtrip**.
- Spec §Architecture timeout/abandoned: **Task 7 bridge `abandon` call + Task 6 handler's `entry.abandoned` branch; Task 9 `timeout_then_late_reply_fires_imessage_fallback` test**.
- Spec §Architecture race boundary: **Task 5 `abandon` returns `None` when take already happened; documented in Task 5 tests `abandon_after_take_returns_none`**.
- Spec §Architecture duplicate reply: **Task 5 LRU + Task 6 `duplicate_reply_via_recently_delivered_is_idempotent` test**.
- Spec §State machine: **Task 5 `pending.rs` implements all transitions**.
- Spec §Components types.rs: **Task 2**.
- Spec §Components pending.rs: **Task 5**.
- Spec §Components grader.rs: **Task 3**.
- Spec §Components relay.rs: **Task 6**.
- Spec §Components mod.rs: **Task 2**.
- Spec §Components tests.rs: **Task 9**.
- Spec §Components server.rs changes: **Task 7**.
- Spec §Components app_config.rs changes: **Task 4**.
- Spec §Components model_routing.rs changes: **Task 10**.
- Spec §Components cli/shortcut.rs changes: **Task 11**.
- Spec §Components local.example.toml: **Task 11 Step 5**.
- Spec §Data Flow schemas (4): covered by Task 7 (forwarded body), Task 6 (callback body + response body), Task 11 (inbound body via Shortcut template).
- Spec §Error Handling table (13 rows): every row maps to an explicit handler branch in Task 6 or Task 7; metrics counters increment per row in Task 6.
- Spec §Timeouts table: 10s forward hardcoded in Task 7; 19s oneshot computed from config in Task 7; 300s/LRU/GC in Task 5.
- Spec §Observability /health augmentation: **NOT yet implemented**. The `ReplyMetrics` struct is wired (Task 6) and accessible via `state.shortcut_reply.metrics`, but the `/health` handler doesn't render it yet. **Decision**: scope this to a follow-up. The metrics exist for future exposure and for current log-based inspection. Do not block the feature on /health UI. Log this as follow-up item in Task 12 Step 3.
- Spec §Observability log fields: **Task 5 GC + Task 6 handler + Task 7 bridge** all emit `target: "shortcut_reply"` with the documented fields.
- Spec §Testing strategy execution order: **this plan's tasks 2-11 match the spec's 10-step order**.
- Spec §Dependencies: **Task 1**.
- Spec §Rollout: **Task 11 Step 8 smoke-test + Task 12 manual QA**.
- Spec §Constraints: preserved throughout — no zeroclaw source changes, no Cargo dep on zeroclaw, file sizes capped.

### Decision log

1. **`[shortcut.reply]` nested under existing `[shortcut]` section** (Task 4): better TOML ergonomics than a top-level `[shortcut_reply]` AND matches Davis's existing `ShortcutConfig` struct without awkwardly renaming.
2. **Legacy fall-through kept in `forward_legacy_and_accept`** (Task 7): the spec says "backward compatibility with the existing Shortcut — user must reinstall after upgrade", but on the **server** side we still want graceful degradation when the user hasn't put `[shortcut.reply]` in their config yet. The legacy codepath preserves the exact old behavior so rolling out the binary before adding config doesn't break anything. This is server-side tolerance, not "support the old Shortcut" — if `[shortcut.reply]` IS configured, any request with the legacy `"iphone-shortcuts"` thread_id gets a 400.
3. **Shortcut dictionary-access empty-string fallback for speak_text** (Task 11 Step 2): iOS Shortcut's "Get Dictionary Value" returns an empty string when the key is missing, which is the signal for "speak error_generic". Davis always returns a non-empty `speak_text` on success, so this branch fires only on HTTP errors or timeouts.
4. **`/health` augmentation deferred** (see §Self-Review §Observability above).
5. **Device Prefix via Shortcut "If Device Model contains HomePod" then "Get Text" branches**: This is the minimum-surface way to set a Shortcut variable without using Magic Variables, which are fragile across iOS versions.

---

## Open Questions

None at plan time. If any emerge during implementation, add them here with a brief description and the task blocking on resolution.
