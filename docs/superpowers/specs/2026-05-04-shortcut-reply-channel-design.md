# Shortcut Reply Channel — Design Spec

**Date**: 2026-05-04
**Status**: Draft (awaiting user review)
**Owner**: Davis

## Problem

Today, when the iOS Shortcut "叫下戴维斯" fires `POST :3012/shortcut`, Davis forwards the request to zeroclaw (`:3001/shortcut`), receives a `200 OK` meaning "message enqueued", and immediately returns `202 Accepted` to iOS. The agent's final answer **never reaches the user**. Neither iPhone nor HomePod gets any result; the user is left guessing whether the query succeeded.

## Goal

Deliver the agent's reply back to the user on the same Apple device that triggered the Shortcut (iPhone or HomePod), using built-in Siri TTS for speech and iMessage for long-form text, without introducing a dependency on Home Assistant or any third-party push service.

**Non-goals**:
- Distinguishing device-control commands from information queries (silent confirmations — deferred to a future iteration, requires zeroclaw-side cooperation).
- Supporting multiple HomePods in different rooms with per-room routing (deferred; device affinity is handled naturally by Shortcut execution locality).
- Backward compatibility with the existing Shortcut (user must reinstall after upgrade).

## Solution

Change the Shortcut to **synchronously wait** for Davis's response, and have Davis stitch zeroclaw's async webhook reply back to the waiting Shortcut request using an in-memory `pending_replies` table. The Shortcut then uses its built-in `Speak Text` action to announce the reply — this guarantees HomePod-triggered requests get spoken on the HomePod and iPhone-triggered requests get spoken on the iPhone, without any device routing logic in Davis.

For long replies, Davis sends an iMessage with the full text and instructs the Shortcut to speak a short phrase (`"详情我通过短信发你"`) instead. For timeouts, Davis falls back to iMessage asynchronously after the Shortcut has already given up.

**Key architectural properties**:
- Zero dependency on Home Assistant — no `notify.*`, `tts.*`, or `media_player.play_media` calls.
- Zero modification to zeroclaw source — uses the existing `[channels.webhook.send_url]` outbound mechanism already in zeroclaw.
- Zero persistent state — `pending_replies` is purely in-memory; restarts lose in-flight waiters but that's acceptable because the Shortcut would time out anyway.

## Architecture

### Request flow (happy path)

```
T=0.0s  iOS Shortcut POST :3012/shortcut
        body = { sender, content, thread_id: "ios:iphone" | "ios:homepod" }
        (Shortcut sends the prefix only; Davis appends the uuid.)

T=0.05s Davis shortcut_bridge:
        - Validate HMAC secret (existing).
        - Parse body, read thread_id. Valid prefixes: exactly "ios:iphone" or "ios:homepod".
          Anything else → 400 (see Error Handling).
        - pending.register() → (request_id uuid, oneshot Receiver<ShortcutResponse>).
        - Rewrite body.thread_id = "<prefix>:<request_id>".
        - Re-sign HMAC over the rewritten bytes.
        - Forward to http://127.0.0.1:3001/shortcut (10s timeout).
        - Await oneshot.recv() with tokio::time::timeout(19s, ...).

T=0.1s  zeroclaw returns 200 OK (message enqueued, agent not yet run).
        Davis keeps awaiting oneshot.

T=4.0s  Agent finishes. zeroclaw POSTs to configured send_url:
        http://127.0.0.1:3012/shortcut/reply
        body = { content: "<agent reply>", thread_id: "ios:iphone:<request_id>" }

T=4.05s Davis handle_reply:
        - Parse thread_id → request_id.
        - pending.take(request_id) → Found(entry).
        - grade(content) → (mode, response).
        - If mode in {SpeakBriefImessageFull, ImessageOnly}: send iMessage.
        - oneshot.send(response).
        - Return 200 OK to zeroclaw.

T=4.1s  Davis shortcut_bridge wakes up from oneshot:
        - Serialize ShortcutResponse → JSON.
        - Return 200 OK + JSON body to iOS Shortcut.

T=4.2s  Shortcut: if speak_text is not null, Speak Text.
```

### Request flow (timeout / abandoned path)

```
T=0.0s  Same as above.
T=19.0s Davis shortcut_bridge oneshot timeout fires.
        - pending.abandon(request_id) → Some(entry) (reply hasn't arrived).
        - Return 504 Gateway Timeout to iOS Shortcut.
T=19.1s Shortcut: Get Contents of URL fails → error branch → Speak Text(phrases.error_generic).

T=45.0s Agent finally finishes. zeroclaw POSTs /shortcut/reply.
T=45.1s Davis handle_reply:
        - pending.take(request_id) → Found(entry with abandoned=true).
        - Entry indicates user already gave up.
        - Send iMessage with full content as async fallback
          (so user still sees the answer when they check their phone).
        - oneshot.send is attempted but receiver is dropped — Err ignored.
        - Return 200 OK to zeroclaw.
```

### Request flow (race: reply arrives at exactly the timeout boundary)

Two orderings possible when reply and timeout fire within the same microsecond:

- **Timeout wins**: `abandon()` marks entry, then `take()` arrives later and goes down the abandoned path (iMessage fallback for SpeakFull, no-op for SpeakBriefImessageFull because iMessage was already sent).
- **Reply wins**: `take()` removes entry and enters `recently_delivered`. `abandon()` then finds nothing to abandon and returns `None`. Shortcut handler's timeout branch sees `None` return, concludes reply already succeeded, and exits cleanly.

Either way, no duplicate iMessage, no lost reply, no deadlock.

### Request flow (zeroclaw sends duplicate reply)

Protected by the `recently_delivered` LRU (capacity 64, 30s TTL). Second `POST /shortcut/reply` with the same request_id finds it in `recently_delivered` → returns 200 OK without any side effect.

### State machine for pending entry

```
              (register)
                  │
                  ▼
             ┌─────────┐
             │ Waiting │
             └────┬────┘
                  │
       ┌──────────┼────────────┐
       │          │            │
    (take)   (abandon        (gc after
             timer fires)   pending_max_age_secs)
       │          │            │
       ▼          ▼            ▼
 ┌───────────┐ ┌─────────┐  [dropped]
 │Delivered  │ │Abandoned│
 │ (removed, │ │(awaiting│
 │  in LRU)  │ │ late    │
 └───────────┘ │ reply)  │
               └────┬────┘
                    │
                 (take)
                    │
                    ▼
           Fallback iMessage
           (for SpeakFull only;
            long-reply iMessage
            was sent at T of take)
                    │
                    ▼
              [removed]
```

## Components

All new code lives under `src/shortcut_reply/`:

### `src/shortcut_reply/types.rs` (~80 LOC)

Pure type declarations, shared across the module:

```rust
pub type RequestId = String;

pub struct PendingReply {
    pub request_id: RequestId,
    pub sender: tokio::sync::oneshot::Sender<ShortcutResponse>,
    pub created_at: std::time::Instant,
    pub imessage_handle: Option<String>,
    pub imessage_sent: bool,
    pub abandoned: bool,
}

pub enum ReplyMode {
    SpeakFull,
    SpeakBriefImessageFull,
}

#[derive(serde::Serialize)]
pub struct ShortcutResponse {
    pub speak_text: Option<String>,
    pub imessage_sent: bool,
}

#[derive(thiserror::Error, Debug)]
pub enum ShortcutReplyError {
    #[error("imessage send failed: {0}")]
    ImessageFailed(String),
}
```

### `src/shortcut_reply/pending.rs` (~250 LOC)

Owns the entire `pending_replies` state. Nobody else touches the inner map.

```rust
pub struct PendingReplies { inner: std::sync::Arc<std::sync::Mutex<PendingRepliesInner>> }

struct PendingRepliesInner {
    waiting: std::collections::HashMap<RequestId, PendingReply>,
    recently_delivered: lru::LruCache<RequestId, std::time::Instant>,
}

impl PendingReplies {
    pub fn new() -> Self;

    pub fn register(
        &self,
        imessage_handle: Option<String>,
    ) -> (RequestId, tokio::sync::oneshot::Receiver<ShortcutResponse>);

    pub fn abandon(&self, id: &RequestId) -> Option<PendingReply>;

    pub fn take(&self, id: &RequestId) -> TakeResult;

    pub fn gc(&self, max_age: std::time::Duration) -> Vec<PendingReply>;
}

pub enum TakeResult {
    Found(PendingReply),
    AlreadyDelivered,
    Unknown,
}

pub fn spawn_gc_task(pending: std::sync::Arc<PendingReplies>, interval: std::time::Duration);
```

Invariants:
- Every public method holds the inner `Mutex` only for the duration of non-async work (no awaits while holding the lock).
- `register` uses `uuid::Uuid::new_v4()` for request_id.
- LRU capacity is 64, TTL 30s. Expired entries are pruned lazily on `take()`.
- `gc()` does a two-pass sweep (collect stale ids under lock → drop lock → call `waiting.remove()` per id under fresh short locks) to avoid long lock hold times.

### `src/shortcut_reply/grader.rs` (~120 LOC)

Pure function, no I/O, no global state:

```rust
pub fn grade(
    content: &str,
    config: &ShortcutReplyConfig,
) -> (ReplyMode, ShortcutResponse);
```

Rules:
- `content.chars().count() <= brief_threshold_chars` → `SpeakFull`, `speak_text = content`, `imessage_sent = false`.
- otherwise → `SpeakBriefImessageFull`, `speak_text = phrases.speak_brief_imessage_full`, `imessage_sent = false` (the caller in `relay` overwrites this to `true` only after `imessage_sender.send(...)` returns `Ok`).

The grader exposes only two modes. Future iterations may add an `ImessageOnly` mode (e.g. for device-control silent confirmations) — explicitly out of v1 scope.

**CJK character counting** must use `.chars().count()`, never `.len()` (which gives bytes).

### `src/shortcut_reply/relay.rs` (~250 LOC)

The `/shortcut/reply` HTTP handler and its helpers.

```rust
pub struct ShortcutReplyState {
    pub pending: std::sync::Arc<PendingReplies>,
    pub config: ShortcutReplyConfig,
    pub imessage_sender: std::sync::Arc<dyn ImessageSender>,
    pub imessage_allowed: Vec<String>,
    pub metrics: std::sync::Arc<ReplyMetrics>,
}

pub async fn handle_reply(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<ShortcutReplyState>>,
    body: axum::body::Bytes,
) -> axum::response::Response;

#[async_trait::async_trait]
pub trait ImessageSender: Send + Sync {
    async fn send(&self, handle: &str, text: &str) -> anyhow::Result<()>;
}

pub struct OsascriptSender { pub allowed: Vec<String> }  // wraps imessage_send::notify_user
```

Handler logic:
1. Deserialize body as `{ content: String, thread_id: String, ... }`. Non-JSON or missing fields → 400.
2. Parse `thread_id` as `"ios:iphone:<uuid>"` or `"ios:homepod:<uuid>"`. Any other prefix → 400 + warn log. **No backward-compat with the legacy `"iphone-shortcuts"` literal.**
3. `pending.take(request_id)`:
   - `Unknown` → 200 + warn log.
   - `AlreadyDelivered` → 200 + debug log (idempotent).
   - `Found(entry)` → proceed.
4. Call `grade(content, config)` → `(mode, response)`.
5. If `mode != SpeakFull`: resolve the iMessage handle — prefer `entry.imessage_handle`, fall back to `config.default_imessage_handle`. Then attempt `imessage_sender.send(handle, content)`.
   - Success: set `response.imessage_sent = true`, entry.imessage_sent = true.
   - Failure: log warn, **degrade to SpeakFull** (`response.speak_text = content`, `response.imessage_sent = false`). This ensures the user still hears the full answer.
6. If `entry.abandoned`:
   - For SpeakFull: fire iMessage fallback with full content (entry.imessage_sent guards against double-send).
   - For SpeakBriefImessageFull: iMessage already attempted in step 5; no-op.
   - Skip `oneshot.send` (receiver is already dropped).
7. Else: `oneshot.send(response)` (ignore Err — means receiver was dropped between the `take` and here, essentially same as abandoned).
8. Return 200 OK.

### `src/shortcut_reply/mod.rs` (~30 LOC)

Module-level documentation and `pub use`.

### `src/shortcut_reply/tests.rs` (~400 LOC)

Module-level integration tests. Covers:
- Full round-trip with mock zeroclaw (wiremock) and MockImessageSender.
- Abandoned paths.
- Race between timeout and reply.
- GC behavior with paused tokio time.

### Existing file changes

#### `src/server.rs`

Add `shortcut_reply` route and rewrite `shortcut_bridge` handler:

```rust
// build_shortcut_bridge_app():
app = app.route("/shortcut/reply", axum::routing::post(shortcut_reply::handle_reply))
         .with_state(shortcut_reply_state.clone());

// shortcut_bridge handler (new flow):
async fn shortcut_bridge(
    State(state): State<Arc<ShortcutBridgeState>>,
    body: Bytes,
) -> Response {
    // 1. validate HMAC (existing)
    // 2. parse body as InboundShortcut { sender, content, thread_id }
    // 3. determine prefix from thread_id ("ios:iphone" | "ios:homepod" | other→400)
    // 4. (request_id, rx) = state.pending.register(state.config.default_imessage_handle.clone())
    // 5. rewrite body.thread_id = format!("{}:{}", prefix, request_id), re-serialize
    // 6. re-compute HMAC on rewritten bytes
    // 7. forward to :3001/shortcut with 10s timeout
    //    - on forward failure: state.pending.abandon(&request_id); return 502
    // 8. match tokio::time::timeout(19s, rx).await {
    //        Ok(Ok(resp)) => (StatusCode::OK, Json(resp)).into_response(),
    //        Ok(Err(_recv_err)) => {
    //            // oneshot sender dropped before sending — bug signal
    //            StatusCode::INTERNAL_SERVER_ERROR.into_response()
    //        }
    //        Err(_elapsed) => {
    //            let _ = state.pending.abandon(&request_id);
    //            StatusCode::GATEWAY_TIMEOUT.into_response()
    //        }
    //    }
}
```

Net change: ~60 LOC added, ~20 LOC replaced.

#### `src/app_config.rs`

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct ShortcutReplyConfig {
    pub brief_threshold_chars: usize,
    pub shortcut_wait_timeout_secs: u64,
    pub pending_max_age_secs: u64,
    pub default_imessage_handle: String,
    pub phrases: ShortcutReplyPhrases,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ShortcutReplyPhrases {
    pub speak_brief_imessage_full: String,
    pub error_generic: String,
}

// AppConfig gets: pub shortcut_reply: Option<ShortcutReplyConfig>,
```

Defaults applied when field is absent at deserialization time:
- `brief_threshold_chars = 60`
- `shortcut_wait_timeout_secs = 20` (Shortcut side)
- `pending_max_age_secs = 300`
- Davis-side handler uses `shortcut_wait_timeout_secs - 1` (i.e. 19s) for its oneshot timeout to leave 1s slack.

#### `src/model_routing.rs`

When rendering zeroclaw's `config.toml`, if `AppConfig.shortcut_reply.is_some()`:

```toml
[channels.webhook]
# ...existing fields...
send_url = "http://127.0.0.1:3012/shortcut/reply"
```

Hardcoding `127.0.0.1:3012` matches Davis's bound port. This is the **only** coupling touch point per CLAUDE.md's "rendered zeroclaw config.toml" invariant.

#### `src/cli/shortcut.rs`

The existing Shortcut JSON template gets structural edits — no longer a pure string patch. The renderer must:

1. Insert a `Get Device Details` action after "Ask for Input" (before POST) to read `Device Model`.
2. Add an `If` / `Otherwise` / `End If` block: when Device Model starts with `"HomePod"`, set `thread_id = "ios:homepod"`, else `thread_id = "ios:iphone"`. The Shortcut emits only the prefix; Davis is responsible for appending a uuid before forwarding to zeroclaw.
3. Set the POST action's timeout to 20 seconds.
4. Remove the pre-POST "正在处理" `Speak Text` action.
5. After the POST, add:
   - `Get Dictionary Value "speak_text"` from the response.
   - `If result is not null → Speak Text` with the extracted value.
   - `Otherwise` (also handles Get Contents of URL failures like 504/5xx) → `Speak Text(phrases.error_generic)`.

The renderer injects `phrases.error_generic` into the final `Speak Text` action at install time; changing the config requires re-running `daviszeroclaw service install` to re-sign and reimport.

Template rendering in `cli/shortcut.rs` becomes structured JSON manipulation instead of `str::replace`. This is a net simplification: the existing code already assembles JSON before `plutil` conversion.

#### `config/davis/local.example.toml`

Add commented example block:

```toml
# [shortcut.reply]
# brief_threshold_chars = 60
# shortcut_wait_timeout_secs = 20
# pending_max_age_secs = 300
# default_imessage_handle = "you@icloud.com"

# [shortcut.reply.phrases]
# speak_brief_imessage_full = "详情我通过短信发你"
# error_generic = "戴维斯好像出问题了"
```

## Data Flow

### Inbound body (Shortcut → Davis)

```json
{
  "sender": "ios-shortcuts",
  "content": "<user's spoken or typed text>",
  "thread_id": "ios:iphone"
}
```

or `"thread_id": "ios:homepod"`. The Shortcut sends the prefix only; Davis appends the uuid before forwarding.

### Forwarded body (Davis → zeroclaw)

```json
{
  "sender": "ios-shortcuts",
  "content": "<unchanged>",
  "thread_id": "ios:iphone:01924f3d-7e4c-7b1a-b6a3-0e5d2d9c87af"
}
```

HMAC header `x-webhook-signature` is recomputed over the rewritten bytes.

### Callback body (zeroclaw → Davis `/shortcut/reply`)

```json
{
  "content": "<agent's final text reply>",
  "thread_id": "ios:iphone:01924f3d-7e4c-7b1a-b6a3-0e5d2d9c87af"
}
```

### Outbound body (Davis → Shortcut)

```json
{
  "speak_text": "<text to speak, or null>",
  "imessage_sent": true
}
```

## Error Handling

| Error source | Scenario | User experience | Davis behavior | Log level |
|---|---|---|---|---|
| Shortcut bridge entry | HMAC mismatch | Shortcut `error_generic` | 401 | warn |
| Shortcut bridge entry | Body not JSON / missing fields | Shortcut `error_generic` | 400 | warn |
| Shortcut bridge entry | Unknown thread_id prefix | Shortcut `error_generic` | 400 | warn |
| Forward to zeroclaw | Connection refused / 10s timeout | Shortcut `error_generic` | 502, rollback pending entry | error |
| Forward to zeroclaw | Non-2xx from zeroclaw | Shortcut `error_generic` | 502, rollback pending entry | error |
| Wait for reply | 19s timeout | Shortcut `error_generic`, iMessage fallback | 504, pending.abandon | info |
| Wait for reply | Oneshot receive error | Shortcut `error_generic` | 500 | error (bug) |
| Reply handler | thread_id parse failure | n/a | 400 | warn |
| Reply handler | Unknown request_id | n/a | 200 (silent) | warn |
| Reply handler | Already delivered (dedup) | n/a | 200 | debug |
| Reply handler | iMessage failure on long reply | Siri speaks full content instead | 200, mode downgraded | warn |
| Reply handler | iMessage failure + abandoned | Reply effectively lost | 200 | error |
| Background GC | Stale entry swept | n/a | drop sender | info (counter) |

Never panic, never propagate errors up past the handler boundary. Always return an HTTP status.

## Timeouts

| Timeout | Default | Configurable | Owner |
|---|---|---|---|
| Davis → zeroclaw forward | 10s | hardcoded (existing behavior) | `server.rs` |
| Oneshot wait inside bridge handler | 19s (= `shortcut_wait_timeout_secs - 1`) | via config `shortcut.reply.shortcut_wait_timeout_secs` | `server.rs` |
| Pending entry GC threshold | 300s | via config `shortcut.reply.pending_max_age_secs` | `pending.rs` |
| `recently_delivered` LRU TTL | 30s | hardcoded | `pending.rs` |
| `recently_delivered` LRU capacity | 64 | hardcoded | `pending.rs` |
| GC scan interval | 60s | hardcoded | `pending::spawn_gc_task` |
| Shortcut HTTP `Get Contents of URL` | 20s | embedded in Shortcut JSON | Shortcut template |

The 1-second gap between Shortcut (20s) and Davis (19s) guarantees Davis always finishes its handler before the Shortcut side gives up, avoiding "Shortcut timed out but Davis still trying" races.

## Observability

### `/health` endpoint augmentation

The existing `/health` response gets a new field:

```json
{
  "status": "ok",
  "shortcut_reply": {
    "pending_count": 0,
    "recently_delivered_count": 3,
    "total_registered": 142,
    "total_delivered": 140,
    "total_abandoned": 2,
    "total_unknown_reply": 0,
    "total_imessage_failed": 1,
    "last_error": null,
    "last_error_at": null
  }
}
```

All counters use `AtomicU64` (low contention). `last_error` is overwrite-only (no history).

### Log fields

All `tracing` calls from this subsystem use `target: "shortcut_reply"` and these structured fields:

| Field | Meaning |
|---|---|
| `event` | Enum string: `register` / `forward_ok` / `reply_received` / `reply_delivered` / `reply_abandoned` / `reply_unknown` / `imessage_failed` / `gc_swept` / `timeout` |
| `request_id` | uuid for cross-handler tracing |
| `source` | `"ios:iphone"` or `"ios:homepod"` |
| `content_chars` | `content.chars().count()` (no content leak) |
| `content_preview` | First 20 characters (used at debug only) |
| `mode` | `?ReplyMode` |
| `imessage_sent` | bool |
| `elapsed_ms` | From `register` to terminal state |

**PII discipline**: Never log full `content`, never log `default_imessage_handle`. Content previews are gated on debug-level logging.

## Testing Strategy

### TDD execution order

See the Implementation Plan for per-phase steps. Order summary:

1. Types (no tests — pure declarations).
2. `grader.rs` unit tests (5 tests covering boundaries and CJK counting).
3. `pending.rs` unit tests (~10 tests including 100-task concurrency).
4. `ImessageSender` trait + `OsascriptSender` + `MockImessageSender`.
5. `relay.rs` integration tests with mock pending + mock iMessage (8 path coverage tests).
6. `server.rs` bridge integration tests with wiremock (5 tests).
7. End-to-end smoke test (1 test, full chain).
8. `model_routing.rs` config rendering tests (2 tests).
9. `cli/shortcut.rs` template renderer tests (3 tests).
10. Manual acceptance checklist.

### Mock infrastructure

- **Mock zeroclaw**: `wiremock = "0.6"` (new dev-dep). Captures the forwarded request, responds with configurable 200/4xx/5xx, and optionally emits a `POST /shortcut/reply` callback after a configurable delay.
- **Mock iMessage**: `MockImessageSender(Arc<Mutex<Vec<(String, String)>>>)`. Production binds `OsascriptSender` wrapping the existing `imessage_send::notify_user`.
- **Time control**: `tokio::time::pause()` + `advance()` for all timeout-sensitive tests. Full suite must complete in under 10 seconds.

### Coverage target

≥80% per project rules. Expected actual coverage: `grader.rs` 100%, `pending.rs` >95%, `relay.rs` >90%, bridge changes >80%.

### Manual acceptance checklist

- [ ] iPhone triggers short reply (≤60 chars) → Siri speaks full text on iPhone.
- [ ] iPhone triggers long reply (>60 chars) → Siri speaks `speak_brief_imessage_full` + iPhone receives full iMessage.
- [ ] HomePod triggers short reply → HomePod speaks full text.
- [ ] HomePod triggers long reply → HomePod speaks brief phrase + iPhone receives full iMessage.
- [ ] Query designed to take >25s → Shortcut times out, user hears `error_generic`, iMessage arrives later with full answer.
- [ ] With HA unreachable (pull plug on HA host) → feature still works (validates zero-HA dependency).
- [ ] Davis restart mid-session, fire request immediately after → succeeds cleanly.
- [ ] Three rapid successive requests → all three complete serially (zeroclaw single agent loop constraint is understood and acceptable).
- [ ] Old Shortcut version pre-upgrade → returns 400, user hears `error_generic` prompting reinstall.

## Dependencies

### New Cargo dependencies

- `uuid = { version = "1", features = ["v4"] }` (runtime)
- `lru = "0.12"` (runtime)
- `async-trait = "0.1"` (runtime; existing — verify)
- `wiremock = "0.6"` (dev-dependency)

### No new dependencies on zeroclaw

CLAUDE.md invariant preserved:
- No `path`/`git` Cargo dep on zeroclaw.
- Only coupling touch points remain: `model_routing.rs` config render, and the subprocess invocation. The new `send_url` field is an additional line in the former.

## Rollout

This is a single atomic feature; no feature flag, no phased rollout. After merge:

1. User pulls and runs `cargo build --release`.
2. User runs `daviszeroclaw service install` (or restart). This re-renders the Shortcut JSON with the new logic and re-signs.
3. User imports the new Shortcut on their iPhone (this replaces the existing "叫下戴维斯" with the same name). Because the Shortcut syncs via iCloud, HomePod automatically picks up the new version within a short window. The old Shortcut behavior no longer works — any device still running a stale copy gets a 400 from Davis and speaks `error_generic`.
4. zeroclaw restarts automatically because `config.toml` changed (Davis already watches and re-launches it on config changes per existing behavior).

## Constraints Respected

- **CLAUDE.md**: no zeroclaw source changes, no new Cargo dep on zeroclaw, only existing coupling surfaces touched (`model_routing.rs` + subprocess — no new coupling introduced).
- **`coding-style.md`**: files in the typical 200-400 LOC band (largest is `pending.rs` and `relay.rs` at ~250 each; all well below the 800-line hard cap). Error handling explicit at every level. Mutation of `PendingReply` happens only through the owning `PendingReplies` container's methods.
- **`testing.md`**: TDD required; 80% coverage minimum; unit + integration + e2e tests all present.
- **`security.md`**: no PII in logs (content never logged in full), HMAC verification unchanged, iMessage allowlist enforced at delivery.

## Open Questions

None at spec time. Any that emerge during implementation go in `docs/superpowers/plans/2026-05-04-shortcut-reply-channel.md` under an "Open questions" section.
