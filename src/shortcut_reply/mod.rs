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

pub use pending::{spawn_gc_task, PendingReplies};
// consumed by Task 9 integration tests
#[allow(unused_imports)]
pub use pending::TakeResult;
pub use relay::{handle_reply, ImessageSender, OsascriptSender, ReplyMetrics, ShortcutReplyState};
// consumed by Task 9 integration tests
#[allow(unused_imports)]
pub use types::{PendingReply, ReplyMode, RequestId, ShortcutReplyError, ShortcutResponse};
