//! Stub — filled in by the relay task.
//! Transitional: scaffolded in Task 2; real impl lands in Task 6. Remove
//! this attribute when the real file content replaces the stubs.
#![allow(dead_code)]
use axum::{
    body::Bytes,
    extract::State,
    response::{IntoResponse, Response},
};
use std::sync::Arc;
pub struct ShortcutReplyState;
pub struct ReplyMetrics;
#[async_trait::async_trait]
pub trait ImessageSender: Send + Sync {
    async fn send(&self, handle: &str, text: &str) -> anyhow::Result<()>;
}
pub struct OsascriptSender;
pub async fn handle_reply(State(_state): State<Arc<ShortcutReplyState>>, _body: Bytes) -> Response {
    axum::http::StatusCode::NOT_IMPLEMENTED.into_response()
}
