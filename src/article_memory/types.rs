use crate::support::{isoformat, now_utc};
use serde::{Deserialize, Serialize};

use super::ARTICLE_MEMORY_INDEX_VERSION;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryIndex {
    pub version: u32,
    pub updated_at: String,
    #[serde(default)]
    pub articles: Vec<ArticleMemoryRecord>,
}

impl ArticleMemoryIndex {
    pub(super) fn new() -> Self {
        Self {
            version: ARTICLE_MEMORY_INDEX_VERSION,
            updated_at: isoformat(now_utc()),
            articles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryRecord {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub status: ArticleMemoryRecordStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_score: Option<f32>,
    pub captured_at: String,
    pub updated_at: String,
    pub content_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean_profile: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArticleMemoryRecordStatus {
    Candidate,
    #[default]
    Saved,
    Rejected,
    Archived,
}

impl std::fmt::Display for ArticleMemoryRecordStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Candidate => "candidate",
            Self::Saved => "saved",
            Self::Rejected => "rejected",
            Self::Archived => "archived",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleMemoryAddRequest {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
    #[serde(default)]
    pub status: ArticleMemoryRecordStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleMemoryStatusResponse {
    pub status: String,
    pub root: String,
    pub index_path: String,
    pub articles_dir: String,
    pub reports_dir: String,
    pub total_articles: usize,
    pub saved_articles: usize,
    pub candidate_articles: usize,
    pub rejected_articles: usize,
    pub archived_articles: usize,
    pub languages: Vec<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleMemoryListResponse {
    pub status: String,
    pub returned: usize,
    pub total_articles: usize,
    pub articles: Vec<ArticleMemoryRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleMemorySearchResponse {
    pub status: String,
    pub query: String,
    pub search_mode: String,
    pub returned: usize,
    pub total_hits: usize,
    pub hits: Vec<ArticleMemorySearchHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleMemorySearchHit {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub tags: Vec<String>,
    pub status: ArticleMemoryRecordStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_score: Option<f32>,
    pub captured_at: String,
    pub score: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub combined_score: Option<f32>,
    pub matched_fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub content_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translation_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryEmbeddingIndex {
    pub version: u32,
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub updated_at: String,
    #[serde(default)]
    pub vectors: Vec<ArticleMemoryEmbeddingRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryEmbeddingRecord {
    pub article_id: String,
    pub text_hash: String,
    pub indexed_at: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleMemoryEmbeddingRebuildResponse {
    pub status: String,
    pub provider: String,
    pub model: String,
    pub dimensions: usize,
    pub indexed: usize,
    pub skipped: usize,
    pub index_path: String,
}

#[derive(Debug, Clone)]
pub struct ResolvedArticleEmbeddingConfig {
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub dimensions: usize,
    pub max_input_chars: usize,
}

#[derive(Debug, Clone)]
pub struct ResolvedArticleNormalizeConfig {
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub llm_polish: bool,
    pub llm_summary: bool,
    pub min_polish_input_chars: usize,
    pub max_polish_input_chars: usize,
    pub summary_input_chars: usize,
    pub fallback_min_ratio: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleMemoryNormalizeResponse {
    pub status: String,
    pub article_id: String,
    pub clean_status: String,
    pub clean_profile: String,
    pub raw_chars: usize,
    pub normalized_chars: usize,
    pub final_chars: usize,
    pub polished: bool,
    pub summary_generated: bool,
    pub content_path: String,
    pub raw_path: String,
    pub normalized_path: String,
    pub clean_report_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_report_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ArticleCleaningConfig {
    #[serde(default)]
    pub defaults: ArticleCleaningDefaults,
    #[serde(default)]
    pub value: ArticleValueConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleCleaningDefaults {
    #[serde(default = "default_cleaning_min_kept_ratio")]
    pub min_kept_ratio: f32,
    #[serde(default = "default_cleaning_max_kept_ratio")]
    pub max_kept_ratio: f32,
    #[serde(default = "default_cleaning_min_normalized_chars")]
    pub min_normalized_chars: usize,
    #[serde(default)]
    pub exact_noise_lines: Vec<String>,
    #[serde(default)]
    pub contains_noise_lines: Vec<String>,
}

/// Algorithm knobs for the article value-judge stage. Lives in
/// `config/davis/article_memory.toml` (tracked in git) alongside other
/// cleaning thresholds. LLM provider credentials live separately in
/// `local.toml` as `ArticleMemoryValueConfig` so api_keys stay out of
/// tracked files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleValueConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub llm_judge: bool,
    #[serde(default = "default_value_max_input_chars")]
    pub max_input_chars: usize,
    #[serde(default = "default_value_min_normalized_chars")]
    pub min_normalized_chars: usize,
    #[serde(default = "default_value_save_threshold")]
    pub save_threshold: f32,
    #[serde(default = "default_value_candidate_threshold")]
    pub candidate_threshold: f32,
    #[serde(default)]
    pub target_topics: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedArticleValueConfig {
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub llm_judge: bool,
    pub max_input_chars: usize,
    pub min_normalized_chars: usize,
    pub save_threshold: f32,
    pub candidate_threshold: f32,
    pub target_topics: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedArticleCleaningStrategy {
    pub(super) name: String,
    pub(super) version: u32,
    pub(super) source: String,
    pub(super) min_kept_ratio: f32,
    pub(super) max_kept_ratio: f32,
    pub(super) min_normalized_chars: usize,
    pub(super) start_markers: Vec<String>,
    pub(super) end_markers: Vec<String>,
    pub(super) exact_noise_lines: Vec<String>,
    pub(super) contains_noise_lines: Vec<String>,
    pub(super) line_suffix_noise: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleCleanReport {
    pub article_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub strategy_name: String,
    pub strategy_version: u32,
    pub strategy_source: String,
    pub generated_at: String,
    pub clean_status: String,
    pub raw_chars: usize,
    pub prepared_chars: usize,
    pub normalized_chars: usize,
    pub final_chars: usize,
    pub kept_ratio: f32,
    pub removed_start_chars: usize,
    pub removed_end_chars: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_start_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_end_preview: Option<String>,
    pub noise_lines_removed: usize,
    pub removed_lines_sample: Vec<String>,
    pub leftover_noise_candidates: Vec<String>,
    pub risk_flags: Vec<String>,
    #[serde(default)]
    pub engine_chain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_engine: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleValueReport {
    pub article_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub judged_at: String,
    pub decision: String,
    pub value_score: f32,
    pub deterministic_reject: bool,
    pub reasons: Vec<String>,
    pub topic_tags: Vec<String>,
    pub risk_flags: Vec<String>,
    pub translation_needed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// LLM-reported extraction quality. Defaults to `"clean"` when parsing
    /// legacy responses that predate this field.
    #[serde(default = "default_extraction_quality")]
    pub extraction_quality: String,
    /// Specific issues flagged by the LLM when extraction_quality !=
    /// `"clean"`.
    #[serde(default)]
    pub extraction_issues: Vec<String>,
    /// Freeform hint for the rule-learning system when the LLM suggests a
    /// selector/filter refinement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_refinement_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleCleaningCheckResponse {
    pub status: String,
    pub config_path: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleCleanAuditResponse {
    pub status: String,
    pub returned: usize,
    pub reports: Vec<ArticleCleanReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleCleaningReplayResponse {
    pub status: String,
    pub returned: usize,
    pub reports: Vec<ArticleCleanReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleValueAuditResponse {
    pub status: String,
    pub returned: usize,
    pub reports: Vec<ArticleValueReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleStrategyReviewInputResponse {
    pub status: String,
    pub generated_at: String,
    pub report_path: String,
    pub config_path: String,
    pub implementation_requests_dir: String,
    pub recent: usize,
    pub clean_reports: usize,
    pub value_reports: usize,
    pub markdown: String,
}

impl Default for ArticleCleaningDefaults {
    fn default() -> Self {
        Self {
            min_kept_ratio: default_cleaning_min_kept_ratio(),
            max_kept_ratio: default_cleaning_max_kept_ratio(),
            min_normalized_chars: default_cleaning_min_normalized_chars(),
            exact_noise_lines: Vec::new(),
            contains_noise_lines: Vec::new(),
        }
    }
}

impl Default for ArticleValueConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            llm_judge: true,
            max_input_chars: default_value_max_input_chars(),
            min_normalized_chars: default_value_min_normalized_chars(),
            save_threshold: default_value_save_threshold(),
            candidate_threshold: default_value_candidate_threshold(),
            target_topics: Vec::new(),
        }
    }
}

pub(super) fn default_true() -> bool {
    true
}

pub(super) fn default_extraction_quality() -> String {
    "clean".to_string()
}

pub(super) fn default_value_max_input_chars() -> usize {
    12_000
}

pub(super) fn default_value_min_normalized_chars() -> usize {
    800
}

pub(super) fn default_value_save_threshold() -> f32 {
    0.75
}

pub(super) fn default_value_candidate_threshold() -> f32 {
    0.45
}

pub(super) fn default_cleaning_min_kept_ratio() -> f32 {
    0.20
}

pub(super) fn default_cleaning_max_kept_ratio() -> f32 {
    0.98
}

pub(super) fn default_cleaning_min_normalized_chars() -> usize {
    800
}
