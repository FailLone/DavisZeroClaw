//! Typed error surface for crawl4ai calls.
//!
//! Callers (src/express.rs, src/advisor.rs) previously matched on String
//! substrings to decide user-facing issue types. This enum makes each failure
//! mode explicit and keeps match-coverage a compile error.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Crawl4aiError {
    /// local.toml has `crawl4ai.enabled = false`.
    Disabled,
    /// Supervised adapter not reachable / not healthy.
    ServerUnavailable { details: String },
    /// Supervised adapter returned 504 or Rust-side wall-clock fired.
    Timeout { budget_secs: u64 },
    /// 500 from adapter or crawl4ai raised an exception inside the task.
    AdapterCrashed { details: String },
    /// crawl4ai returned `success: false` for reasons other than auth
    /// (e.g. navigation failure, wait_for predicate never satisfied).
    CrawlFailed { details: String },
    /// crawl4ai adapter reported the profile is not logged in.
    AuthRequired { profile: String },
    /// Unexpected or malformed JSON back from the adapter.
    PayloadMalformed { details: String },
    /// I/O while preparing the request (profile dir creation, etc.).
    LocalIo { details: String },
}

impl Crawl4aiError {
    /// Issue type string for `build_issue` / UI routing (keeps existing
    /// identifiers stable for src/support.rs remediation hints).
    pub fn issue_type(&self) -> &'static str {
        match self {
            Self::Disabled | Self::ServerUnavailable { .. } | Self::Timeout { .. } => {
                "crawl4ai_unavailable"
            }
            Self::AdapterCrashed { .. } | Self::CrawlFailed { .. } => "site_changed",
            Self::AuthRequired { .. } => "auth_required",
            Self::PayloadMalformed { .. } => "site_changed",
            Self::LocalIo { .. } => "crawl4ai_unavailable",
        }
    }
}

impl fmt::Display for Crawl4aiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "crawl4ai is disabled in local config"),
            Self::ServerUnavailable { details } => {
                write!(f, "crawl4ai server unavailable: {details}")
            }
            Self::Timeout { budget_secs } => {
                write!(f, "crawl4ai request timed out after {budget_secs}s")
            }
            Self::AdapterCrashed { details } => write!(f, "crawl4ai adapter crashed: {details}"),
            Self::CrawlFailed { details } => write!(f, "crawl4ai crawl failed: {details}"),
            Self::AuthRequired { profile } => {
                write!(f, "crawl4ai profile '{profile}' requires login")
            }
            Self::PayloadMalformed { details } => {
                write!(f, "crawl4ai returned malformed payload: {details}")
            }
            Self::LocalIo { details } => write!(f, "crawl4ai local i/o failed: {details}"),
        }
    }
}

impl std::error::Error for Crawl4aiError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_type_mapping_is_stable() {
        // Group 1: crawl4ai_unavailable
        assert_eq!(Crawl4aiError::Disabled.issue_type(), "crawl4ai_unavailable");
        assert_eq!(
            Crawl4aiError::ServerUnavailable {
                details: "conn refused".into()
            }
            .issue_type(),
            "crawl4ai_unavailable"
        );
        assert_eq!(
            Crawl4aiError::Timeout { budget_secs: 30 }.issue_type(),
            "crawl4ai_unavailable"
        );
        assert_eq!(
            Crawl4aiError::LocalIo {
                details: "mkdir failed".into()
            }
            .issue_type(),
            "crawl4ai_unavailable"
        );

        // Group 2: site_changed
        assert_eq!(
            Crawl4aiError::AdapterCrashed {
                details: "stacktrace".into()
            }
            .issue_type(),
            "site_changed"
        );
        assert_eq!(
            Crawl4aiError::CrawlFailed {
                details: "foo".into()
            }
            .issue_type(),
            "site_changed"
        );
        assert_eq!(
            Crawl4aiError::PayloadMalformed {
                details: "bad json".into()
            }
            .issue_type(),
            "site_changed"
        );

        // Group 3: auth_required
        assert_eq!(
            Crawl4aiError::AuthRequired {
                profile: "express-ali".into()
            }
            .issue_type(),
            "auth_required"
        );
    }

    #[test]
    fn display_includes_context() {
        let err = Crawl4aiError::Timeout { budget_secs: 120 };
        let s = err.to_string();
        assert!(s.contains("120"), "expected '120' in Display, got: {s}");
    }
}
