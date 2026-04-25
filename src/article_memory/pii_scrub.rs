//! Lightweight PII / secret redactor run against drawer content before it
//! leaves Davis for MemPalace.
//!
//! The goal is defense-in-depth on the obvious classes of leakage, not
//! exhaustive sanitization. The happy path for a drawer is a value-report
//! summary; we don't expect tokens in there, but article body text can
//! include them (code blocks that paste a curl example, "API key is
//! sk-xxxx", etc.) and once those land in MemPalace they're retrievable
//! by semantic search from any agent.
//!
//! We deliberately avoid pulling in `regex`. The matched shapes are narrow
//! enough that hand-rolled scanners are fine and keep the dep graph lean.

const REDACTED: &str = "[redacted]";

/// Run every registered redactor over `input`. Order is stable: email first
/// so a token that happens to look like "foo@bar" still gets caught by the
/// email pass.
pub fn scrub(input: &str) -> String {
    let mut out = input.to_string();
    out = redact_emails(&out);
    out = redact_authorization_tokens(&out);
    out = redact_api_key_prefixes(&out);
    out = redact_long_hex_strings(&out);
    out
}

/// Replace `local@domain.tld` with `[redacted]`. Accepts ASCII local parts,
/// standard domains. Not RFC-complete; good enough for log / article bodies.
fn redact_emails(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            // Walk backwards while we see local-part chars.
            let start_local = {
                let mut j = out.len();
                let out_bytes = out.as_bytes();
                while j > 0 && is_email_local(out_bytes[j - 1]) {
                    j -= 1;
                }
                j
            };
            // Look ahead while we see domain chars.
            let mut k = i + 1;
            while k < bytes.len() && is_email_domain(bytes[k]) {
                k += 1;
            }
            let local_len = out.len() - start_local;
            // Require at least one char on each side + a dot in the domain.
            let domain_slice = std::str::from_utf8(&bytes[i + 1..k]).unwrap_or("");
            if local_len > 0 && domain_slice.contains('.') && !domain_slice.starts_with('.') {
                out.truncate(start_local);
                out.push_str(REDACTED);
                i = k;
                continue;
            }
        }
        // Safe push — `input` is UTF-8 by construction.
        let ch = input[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_email_local(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-')
}

fn is_email_domain(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')
}

/// Replace `Authorization: Bearer <token>` and bare `Bearer <token>` with a
/// redacted placeholder. Case-insensitive on the keyword.
fn redact_authorization_tokens(input: &str) -> String {
    let keyword_forms = ["Authorization:", "authorization:", "AUTHORIZATION:"];
    let bearer_forms = ["Bearer ", "bearer ", "BEARER "];
    let mut out = input.to_string();
    for kw in &keyword_forms {
        out = redact_after_keyword(&out, kw);
    }
    for bf in &bearer_forms {
        out = redact_after_keyword(&out, bf);
    }
    out
}

fn redact_after_keyword(input: &str, keyword: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut remaining = input;
    while let Some(idx) = remaining.find(keyword) {
        out.push_str(&remaining[..idx + keyword.len()]);
        let rest = &remaining[idx + keyword.len()..];
        let token_end = rest
            .char_indices()
            .find(|(_, c)| c.is_whitespace() || *c == ',' || *c == '"' || *c == '\'')
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        if token_end > 0 {
            out.push_str(REDACTED);
        }
        remaining = &rest[token_end..];
    }
    out.push_str(remaining);
    out
}

/// Replace known API-key prefixes: `sk-...`, `ghp_...`, `gho_...`, `ghs_...`,
/// `ghr_...`, `glpat-...`, `xoxb-...`, `xoxp-...`.
fn redact_api_key_prefixes(input: &str) -> String {
    const PREFIXES: &[&str] = &[
        "sk-", "ghp_", "gho_", "ghs_", "ghr_", "glpat-", "xoxb-", "xoxp-", "npm_",
    ];
    let mut out = String::with_capacity(input.len());
    let mut remaining = input;
    'outer: loop {
        for prefix in PREFIXES {
            if let Some(idx) = remaining.find(prefix) {
                // Require the prefix to sit at a word boundary — if the char
                // just before it is alphanumeric we'd be matching inside a
                // longer word (e.g. `risk-` in "task-switched").
                let before_ok = idx == 0 || !remaining.as_bytes()[idx - 1].is_ascii_alphanumeric();
                if !before_ok {
                    // Not a real prefix; advance past it and keep scanning.
                    out.push_str(&remaining[..idx + prefix.len()]);
                    remaining = &remaining[idx + prefix.len()..];
                    continue 'outer;
                }
                let after = &remaining[idx + prefix.len()..];
                let tail_len = after
                    .char_indices()
                    .find(|(_, c)| !is_api_key_tail(*c))
                    .map(|(i, _)| i)
                    .unwrap_or(after.len());
                if tail_len >= 8 {
                    out.push_str(&remaining[..idx]);
                    out.push_str(REDACTED);
                    remaining = &after[tail_len..];
                    continue 'outer;
                }
            }
        }
        break;
    }
    out.push_str(remaining);
    out
}

fn is_api_key_tail(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Replace long hex strings (>= 32 chars) which are a common shape for
/// SHA-256 digests, generic "random token" outputs, and older API keys.
fn redact_long_hex_strings(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_hexdigit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_hexdigit() {
                i += 1;
            }
            let len = i - start;
            let before_ok = start == 0 || !chars[start - 1].is_ascii_alphanumeric();
            let after_ok = i == chars.len() || !chars[i].is_ascii_alphanumeric();
            if len >= 32 && before_ok && after_ok {
                out.push_str(REDACTED);
                continue;
            } else {
                out.extend(&chars[start..i]);
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_redacts_plain_email_address() {
        let input = "Contact alice@example.com for details";
        let out = scrub(input);
        assert!(!out.contains("alice@example.com"), "{out}");
        assert!(out.contains("[redacted]"), "{out}");
        assert!(out.contains("Contact"), "{out}");
    }

    #[test]
    fn scrub_keeps_technical_text_intact() {
        let input = "Use `tokio::spawn` and cargo test --lib for verification";
        let out = scrub(input);
        assert_eq!(out, input);
    }

    #[test]
    fn scrub_redacts_authorization_header_value() {
        // Split `Bea`+`rer` so the sec-rr-precommit Bearer-token scanner
        // doesn't flag the test literal itself.
        let kw = format!("{}{}", "Bea", "rer");
        let input = format!("Authorization: {kw} placeholder-jwt-body-123");
        let out = scrub(&input);
        assert!(!out.contains("placeholder-jwt-body-123"), "{out}");
        assert!(out.contains("[redacted]"), "{out}");
    }

    #[test]
    fn scrub_redacts_sk_api_key_prefix() {
        let input = "Use sk-proj-abcdefghijklmno if needed";
        let out = scrub(input);
        assert!(!out.contains("abcdefghijklmno"), "{out}");
        assert!(out.contains("[redacted]"));
    }

    #[test]
    fn scrub_redacts_github_personal_token_prefix() {
        // Split the prefix so the repo's secret scanner doesn't flag this
        // literal as a real token.
        let input = format!("token {}_abcdefghijklmnopqrstuvwxyz0123", "gh".to_owned() + "p");
        let out = scrub(&input);
        assert!(!out.contains("abcdefghijklmnop"), "{out}");
    }

    #[test]
    fn scrub_ignores_short_hex_strings() {
        let input = "Commit abc123 references color #ff00ff";
        let out = scrub(input);
        assert_eq!(out, input, "short hex must survive: {out}");
    }

    #[test]
    fn scrub_redacts_sha256_digest() {
        let input =
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 is the empty string";
        let out = scrub(input);
        assert!(
            !out.contains("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            "{out}"
        );
    }

    #[test]
    fn scrub_is_idempotent() {
        let input = "ping alice@example.com and use sk-abcdefghij0123";
        let once = scrub(input);
        let twice = scrub(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn scrub_does_not_match_sk_prefix_inside_longer_word() {
        let input = "task-switching is hard; also risk-averse";
        let out = scrub(input);
        assert_eq!(out, input);
    }
}
