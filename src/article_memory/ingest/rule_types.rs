//! Data shapes for the rule-learning subsystem.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// LLM-generated per-host extraction rule (learned or hand-overridden).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LearnedRule {
    pub host: String,
    /// RFC3339 timestamp of when the rule was generated.
    pub version: String,
    pub content_selectors: Vec<String>,
    #[serde(default)]
    pub remove_selectors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_selector: Option<String>,
    #[serde(default)]
    pub start_markers: Vec<String>,
    #[serde(default)]
    pub end_markers: Vec<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub learned_from_sample_count: usize,
    #[serde(default)]
    pub stale: bool,
}

fn default_confidence() -> f32 {
    0.5
}

/// Hit/partial/poor counters + stale tracking per host.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleStats {
    #[serde(default)]
    pub rule_version: String,
    #[serde(default)]
    pub hits: u64,
    #[serde(default)]
    pub partial: u64,
    #[serde(default)]
    pub poor: u64,
    #[serde(default)]
    pub consecutive_issues: u32,
    #[serde(default)]
    pub last_relearn_trigger: Option<String>,
    #[serde(default)]
    pub last_updated: String,
}

/// A single captured HTML sample awaiting a learning round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSample {
    pub url: String,
    pub job_id: String,
    pub captured_at: String,
    /// Relative path from `runtime/article_memory/` to the HTML snapshot.
    pub html_snapshot_path: String,
    pub markdown_from_engine: String,
    pub failure_reason: String,
    #[serde(default)]
    pub failure_details: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learned_rule_default_confidence_deserializes_missing_field() {
        let json = r#"{"host":"example.com","version":"2026-04-25T00:00:00Z","content_selectors":["article"]}"#;
        let rule: LearnedRule = serde_json::from_str(json).unwrap();
        assert_eq!(rule.host, "example.com");
        assert!(!rule.stale);
        assert!((rule.confidence - 0.5).abs() < 1e-6);
    }

    #[test]
    fn rule_stats_default_is_all_zero() {
        let stats = RuleStats::default();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.consecutive_issues, 0);
    }

    #[test]
    fn rule_sample_roundtrips() {
        let sample = RuleSample {
            url: "https://x.example/a".into(),
            job_id: "j1".into(),
            captured_at: "2026-04-25T00:00:00Z".into(),
            html_snapshot_path: "rule_samples/x.example/j1.html".into(),
            markdown_from_engine: "# stub".into(),
            failure_reason: "hard_fail".into(),
            failure_details: vec!["markdown_too_short".into()],
        };
        let ser = serde_json::to_string(&sample).unwrap();
        let back: RuleSample = serde_json::from_str(&ser).unwrap();
        assert_eq!(back.url, sample.url);
    }
}
