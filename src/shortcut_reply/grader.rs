//! Pure decision: given agent reply content, pick a `ReplyMode` and
//! render the `speak_text` the Shortcut will speak.
//! Transitional: scaffolded in Task 3; callers land in Tasks 6-8.
//! Remove this attribute in Task 8 once every item has a live caller.
#![allow(dead_code)]

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
        // "hello你好": 7 chars, 11 utf-8 bytes. If grade() ever used .len()
        // instead of .chars().count(), a 7-char threshold would fail for
        // this string even though 7 chars fit.
        let s = "hello你好";
        assert_eq!(s.len(), 11); // 5 ASCII (1 byte each) + 2 CJK (3 bytes each)
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
