//! Shortcut reply channel: stitches zeroclaw's async agent completion
//! back to the synchronously-waiting iOS Shortcut request.
//!
//! Design: docs/superpowers/specs/2026-05-04-shortcut-reply-channel-design.md

// Re-exports not yet consumed at the lib-crate level; local_proxy.rs
// will import these in Task 8 (ShortcutReplyState, OsascriptSender, etc.)
#![allow(unused_imports)]

pub mod grader;
pub mod pending;
pub mod relay;
pub mod types;

#[cfg(test)]
mod tests;

pub use pending::{spawn_gc_task, PendingReplies, TakeResult};
pub use relay::{handle_reply, ImessageSender, OsascriptSender, ReplyMetrics, ShortcutReplyState};
pub use types::{PendingReply, ReplyMode, RequestId, ShortcutReplyError, ShortcutResponse};
