//! End-to-end integration: fake zeroclaw via wiremock, real Davis
//! reply handler, assert the full request/response cycle.

use crate::shortcut_reply::relay::ImessageSender;
use crate::shortcut_reply::{handle_reply, PendingReplies, ReplyMetrics, ShortcutReplyState};
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
    assert_eq!(
        calls.len(),
        1,
        "abandoned-short-reply iMessage fallback must fire"
    );
    assert_eq!(calls[0].1, "晚到的回复");
}
