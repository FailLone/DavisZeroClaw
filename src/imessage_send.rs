//! iMessage notification for async ingest completion. macOS-only real
//! implementation shells out to `osascript`; non-macOS builds get a stub
//! so Linux CI and unit tests on dev machines still compile.

/// Accept `+E.164` phone (7..=15 digits after `+`) or a basic email
/// (`foo@bar.baz`). Matches the target shape ZeroClaw's IMessageChannel
/// uses, so behavior stays consistent across the stack.
pub fn is_valid_target(handle: &str) -> bool {
    if let Some(rest) = handle.strip_prefix('+') {
        return rest.len() >= 7 && rest.len() <= 15 && rest.chars().all(|c| c.is_ascii_digit());
    }
    let mut parts = handle.splitn(2, '@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    if local.is_empty() || domain.is_empty() || handle.contains(char::is_whitespace) {
        return false;
    }
    if handle.matches('@').count() != 1 {
        return false;
    }
    domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && local.chars().all(|c| !c.is_whitespace())
}

/// Escape a string for safe embedding between double quotes in an
/// AppleScript literal. Only backslash and double-quote need escaping;
/// CJK and emoji pass through since AppleScript source is Unicode-safe.
pub fn escape_applescript(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', r#"\""#)
}

#[cfg(target_os = "macos")]
pub async fn send_imessage(handle: &str, text: &str) -> anyhow::Result<()> {
    use anyhow::{anyhow, bail, Context};
    if !is_valid_target(handle) {
        bail!("invalid imessage target: {handle}");
    }
    let script = format!(
        "tell application \"Messages\"\n\
         \tset targetService to 1st account whose service type = iMessage\n\
         \tset targetBuddy to buddy \"{handle}\" of targetService\n\
         \tsend \"{text}\" to targetBuddy\n\
         end tell",
        handle = escape_applescript(handle),
        text = escape_applescript(text),
    );
    let output = tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .await
        .context("spawn osascript")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!("osascript failed: {stderr}"));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub async fn send_imessage(_handle: &str, _text: &str) -> anyhow::Result<()> {
    tracing::debug!("imessage_send stub (non-macos): no-op");
    Ok(())
}

/// Notify the user via iMessage, guarding against any handle not in
/// `allowed`. Defense-in-depth against upstream bypass: even if the
/// reply_handle came from a bad source, we re-check here. Not-in-allowlist
/// downgrades to a WARN log and returns Ok (the article is already saved,
/// so a missing notification should not fail the job).
pub async fn notify_user(handle: &str, text: &str, allowed: &[String]) -> anyhow::Result<()> {
    if !allowed.iter().any(|c| c == handle) {
        tracing::warn!(
            handle = %handle,
            "reply_handle not in allowed_contacts; skipping iMessage notification",
        );
        return Ok(());
    }
    send_imessage(handle, text).await
}
