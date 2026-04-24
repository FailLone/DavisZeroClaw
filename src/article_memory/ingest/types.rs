use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IngestJobStatus {
    Pending,
    Fetching,
    Cleaning,
    Judging,
    Embedding,
    Saved,
    Rejected,
    Failed,
}

impl IngestJobStatus {
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Fetching | Self::Cleaning | Self::Judging | Self::Embedding
        )
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Saved | Self::Rejected | Self::Failed)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Fetching => "fetching",
            Self::Cleaning => "cleaning",
            Self::Judging => "judging",
            Self::Embedding => "embedding",
            Self::Saved => "saved",
            Self::Rejected => "rejected",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestJobError {
    pub issue_type: String,
    pub message: String,
    pub stage: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestOutcomeSummary {
    pub clean_status: String,
    pub clean_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_score: Option<f32>,
    pub normalized_chars: usize,
    pub polished: bool,
    pub summary_generated: bool,
    pub embedded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestJob {
    pub id: String,
    pub url: String,
    pub normalized_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_override: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<String>,
    pub profile_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_source: Option<String>,
    pub status: IngestJobStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub article_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<IngestOutcomeSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<IngestJobError>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub submitted_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default = "default_attempts")]
    pub attempts: u32,
}

fn default_attempts() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestRequest {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestResponse {
    pub job_id: String,
    pub status: IngestJobStatus,
    pub submitted_at: String,
    #[serde(default)]
    pub deduped: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IngestSubmitError {
    InvalidUrl(String),
    InvalidScheme,
    PrivateAddressBlocked(String),
    DuplicateSaved {
        existing_article_id: Option<String>,
        finished_at: String,
    },
    IngestDisabled,
    PersistenceError(String),
    PersistenceDegraded {
        consecutive_failures: usize,
        last_error: String,
    },
}

impl std::fmt::Display for IngestSubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl(d) => write!(f, "invalid url: {d}"),
            Self::InvalidScheme => write!(f, "only http and https schemes are allowed"),
            Self::PrivateAddressBlocked(d) => write!(f, "private address blocked: {d}"),
            Self::DuplicateSaved {
                existing_article_id,
                finished_at,
            } => write!(
                f,
                "article already saved within dedup window at {finished_at} (article_id={})",
                existing_article_id.as_deref().unwrap_or("-")
            ),
            Self::IngestDisabled => write!(f, "article memory ingest is disabled"),
            Self::PersistenceError(d) => write!(f, "failed to persist job: {d}"),
            Self::PersistenceDegraded {
                consecutive_failures,
                last_error,
            } => write!(
                f,
                "ingest queue persistence degraded after {consecutive_failures} consecutive failures: {last_error}"
            ),
        }
    }
}

impl std::error::Error for IngestSubmitError {}

#[derive(Debug, Clone)]
pub enum IngestOutcome {
    Saved {
        article_id: String,
        summary: IngestOutcomeSummary,
        warnings: Vec<String>,
    },
    Rejected {
        article_id: Option<String>,
        summary: IngestOutcomeSummary,
    },
    Failed(IngestJobError),
}

#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub status: Option<IngestJobStatus>,
    pub limit: Option<usize>,
    pub only_failed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_active_terminal_partition() {
        for s in [
            IngestJobStatus::Pending,
            IngestJobStatus::Fetching,
            IngestJobStatus::Cleaning,
            IngestJobStatus::Judging,
            IngestJobStatus::Embedding,
        ] {
            assert!(s.is_active());
            assert!(!s.is_terminal());
        }
        for s in [
            IngestJobStatus::Saved,
            IngestJobStatus::Rejected,
            IngestJobStatus::Failed,
        ] {
            assert!(!s.is_active());
            assert!(s.is_terminal());
        }
    }

    #[test]
    fn status_serializes_snake_case() {
        let json = serde_json::to_string(&IngestJobStatus::Pending).unwrap();
        assert_eq!(json, "\"pending\"");
        let back: IngestJobStatus = serde_json::from_str("\"fetching\"").unwrap();
        assert_eq!(back, IngestJobStatus::Fetching);
    }

    #[test]
    fn ingest_request_accepts_minimal_payload() {
        let req: IngestRequest =
            serde_json::from_str(r#"{"url": "https://example.com/"}"#).unwrap();
        assert_eq!(req.url, "https://example.com/");
        assert!(req.tags.is_empty());
        assert!(req.title.is_none());
        assert!(req.source_hint.is_none());
    }
}
