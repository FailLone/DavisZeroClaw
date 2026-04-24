//! Tests for the iMessage send module. Real osascript call is not
//! exercised (needs real iMessage account); we cover pure functions.

use crate::imessage_send::{escape_applescript, is_valid_target, notify_user};

#[test]
fn is_valid_target_accepts_e164_phone() {
    assert!(is_valid_target("+8618672954807"));
    assert!(is_valid_target("+1234567"));
    assert!(is_valid_target("+123456789012345"));
}

#[test]
fn is_valid_target_rejects_too_short_or_too_long_phone() {
    assert!(!is_valid_target("+123456"));
    assert!(!is_valid_target("+1234567890123456"));
    assert!(!is_valid_target("+"));
    assert!(!is_valid_target("+abcdefg"));
}

#[test]
fn is_valid_target_accepts_email() {
    assert!(is_valid_target("user@icloud.com"));
    assert!(is_valid_target("a@b.co"));
}

#[test]
fn is_valid_target_rejects_malformed_email() {
    assert!(!is_valid_target("user@icloud"));
    assert!(!is_valid_target("@icloud.com"));
    assert!(!is_valid_target("user@"));
    assert!(!is_valid_target("a@b@c.com"));
    assert!(!is_valid_target("user @icloud.com"));
}

#[test]
fn is_valid_target_rejects_group_thread_id() {
    assert!(!is_valid_target("chat000000123456"));
    assert!(!is_valid_target("iMessage;-;chat000"));
}

#[test]
fn escape_applescript_handles_quotes() {
    assert_eq!(escape_applescript(r#"He said "hi""#), r#"He said \"hi\""#);
}

#[test]
fn escape_applescript_handles_backslash() {
    assert_eq!(escape_applescript(r"a\b"), r"a\\b");
}

#[test]
fn escape_applescript_preserves_cjk_and_emoji() {
    assert_eq!(escape_applescript("已保存《标题》🎉"), "已保存《标题》🎉");
}

#[tokio::test]
async fn notify_user_skips_when_not_in_allowlist() {
    let allowed = vec!["+8618672954807".to_string()];
    notify_user("+8613800000000", "hi", &allowed).await.unwrap();
}

#[tokio::test]
async fn notify_user_stub_on_non_macos_returns_ok() {
    #[cfg(not(target_os = "macos"))]
    {
        let allowed = vec!["user@example.com".to_string()];
        notify_user("user@example.com", "hi", &allowed)
            .await
            .unwrap();
    }
}
