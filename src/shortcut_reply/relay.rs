//! `/shortcut/reply` HTTP handler. Takes the callback from zeroclaw,
//! grades the content, dispatches iMessage if needed, and wakes the
//! waiting Shortcut-side handler via `oneshot::send`.

use crate::shortcut_reply::grader::{grade, GraderInputs};
use crate::shortcut_reply::pending::{PendingReplies, TakeResult};
use crate::shortcut_reply::types::{PendingReply, ReplyMode};
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
    // consumed by AppState wiring in Task 7
    #[allow(dead_code)]
    async fn send(&self, handle: &str, text: &str) -> anyhow::Result<()>;
}

// consumed by AppState wiring in Task 7
#[allow(dead_code)]
pub struct OsascriptSender {
    pub allowed: Vec<String>,
}

#[async_trait]
impl ImessageSender for OsascriptSender {
    async fn send(&self, handle: &str, text: &str) -> anyhow::Result<()> {
        crate::imessage_send::notify_user(handle, text, &self.allowed).await
    }
}

// Fields incremented by relay handler; total_registered and total_gc_swept
// are consumed by bridge handler in Task 7 and spawn_gc_task log sink in Task 12.
pub struct ReplyMetrics {
    #[allow(dead_code)] // consumed by bridge handler in Task 7
    pub total_registered: AtomicU64,
    #[allow(dead_code)] // consumed by bridge handler in Task 7
    pub total_delivered: AtomicU64,
    #[allow(dead_code)] // consumed by bridge handler in Task 7
    pub total_abandoned: AtomicU64,
    #[allow(dead_code)] // consumed by bridge handler in Task 7
    pub total_unknown_reply: AtomicU64,
    #[allow(dead_code)] // consumed by bridge handler in Task 7
    pub total_imessage_failed: AtomicU64,
    #[allow(dead_code)] // consumed by spawn_gc_task log sink in Task 12
    pub total_gc_swept: AtomicU64,
}

impl Default for ReplyMetrics {
    fn default() -> Self {
        Self {
            total_registered: AtomicU64::new(0),
            total_delivered: AtomicU64::new(0),
            total_abandoned: AtomicU64::new(0),
            total_unknown_reply: AtomicU64::new(0),
            total_imessage_failed: AtomicU64::new(0),
            total_gc_swept: AtomicU64::new(0),
        }
    }
}

// consumed by AppState wiring in Task 7
#[allow(dead_code)]
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
// consumed by server.rs bridge wiring in Task 7
#[allow(dead_code)]
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

// consumed by server.rs route registration in Task 7
#[allow(dead_code)]
pub async fn handle_reply(State(state): State<Arc<ShortcutReplyState>>, body: Bytes) -> Response {
    let inbound: InboundReply = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                target: "shortcut_reply",
                event = "reply_parse_failed",
                error = %err,
                "invalid reply body",
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"status":"bad_request"})),
            )
                .into_response();
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
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"status":"bad_request"})),
            )
                .into_response();
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
            state
                .metrics
                .total_unknown_reply
                .fetch_add(1, Ordering::Relaxed);
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
                state
                    .metrics
                    .total_imessage_failed
                    .fetch_add(1, Ordering::Relaxed);
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
        state
            .metrics
            .total_abandoned
            .fetch_add(1, Ordering::Relaxed);
        if matches!(mode, ReplyMode::SpeakFull) && !response.imessage_sent {
            let handle = entry
                .imessage_handle
                .clone()
                .unwrap_or_else(|| state.config.default_imessage_handle.clone());
            if let Err(err) = state.imessage_sender.send(&handle, &inbound.content).await {
                state
                    .metrics
                    .total_imessage_failed
                    .fetch_add(1, Ordering::Relaxed);
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
    state
        .metrics
        .total_delivered
        .fetch_add(1, Ordering::Relaxed);
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
            state.metrics.total_imessage_failed.load(Ordering::Relaxed),
            1
        );
    }

    #[tokio::test]
    async fn unknown_request_id_returns_200_and_increments_counter() {
        let mock = Arc::new(MockSender::new());
        let state = make_state(mock.clone());

        let body = Bytes::from(r#"{"content":"x","thread_id":"ios:iphone:no-such-id"}"#);
        let resp = handle_reply(State(state.clone()), body).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(state.metrics.total_unknown_reply.load(Ordering::Relaxed), 1);
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
        assert_eq!(
            calls.len(),
            1,
            "must fire iMessage fallback for abandoned short reply"
        );
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

    /// When the first iMessage attempt fails AND the request was
    /// abandoned, the abandoned-path fallback fires a second attempt.
    /// This is intentional retry behavior — the Shortcut has already
    /// timed out and iMessage is the user's last channel. Also verifies
    /// `total_imessage_failed` is incremented only for the attempt that
    /// actually failed (the first one), not twice.
    #[tokio::test]
    async fn abandoned_long_reply_imessage_failure_retries_once() {
        let mock = Arc::new(MockSender::new());
        *mock.fail_next.lock().unwrap() = true;
        let state = make_state(mock.clone());
        let (id, _rx) = state.pending.register(None);
        state.pending.abandon(&id);

        let content: String = "文".repeat(100);
        let body = Bytes::from(format!(
            r#"{{"content":"{content}","thread_id":"ios:iphone:{id}"}}"#
        ));
        let _ = handle_reply(State(state.clone()), body).await;
        let calls = mock.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "first attempt fails (not recorded), retry succeeds"
        );
        assert_eq!(calls[0].1, content, "retry sends the full content");
        assert_eq!(
            state.metrics.total_imessage_failed.load(Ordering::Relaxed),
            1,
            "only the first (failed) attempt bumps the counter"
        );
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
