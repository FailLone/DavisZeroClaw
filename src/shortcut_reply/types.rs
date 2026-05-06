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

// No caller yet — reserved for error propagation from Task 8+ callers.
#[allow(dead_code)]
#[derive(thiserror::Error, Debug)]
pub enum ShortcutReplyError {
    #[error("imessage send failed: {0}")]
    ImessageFailed(String),
}
