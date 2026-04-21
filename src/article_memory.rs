use crate::app_config::{
    ArticleMemoryEmbeddingConfig, ArticleMemoryNormalizeConfig, ModelProviderConfig,
};
use crate::support::{isoformat, now_utc};
use crate::RuntimePaths;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

const ARTICLE_MEMORY_INDEX_VERSION: u32 = 1;
const ARTICLE_MEMORY_EMBEDDINGS_VERSION: u32 = 1;
const BUILTIN_ARTICLE_MEMORY_POLICY_CONFIG: &str =
    include_str!("../config/davis/article_memory.toml");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryIndex {
    pub version: u32,
    pub updated_at: String,
    #[serde(default)]
    pub articles: Vec<ArticleMemoryRecord>,
}

impl ArticleMemoryIndex {
    fn new() -> Self {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArticleMemoryRecordStatus {
    Candidate,
    Saved,
    Rejected,
    Archived,
}

impl Default for ArticleMemoryRecordStatus {
    fn default() -> Self {
        Self::Saved
    }
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
    pub sites: Vec<ArticleCleaningSiteStrategy>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleCleaningSiteStrategy {
    pub name: String,
    #[serde(default = "default_cleaning_strategy_version")]
    pub version: u32,
    #[serde(default = "default_cleaning_strategy_status")]
    pub status: String,
    #[serde(default)]
    pub url_patterns: Vec<String>,
    #[serde(default)]
    pub source_patterns: Vec<String>,
    #[serde(default)]
    pub preferred_selectors: Vec<String>,
    #[serde(default)]
    pub start_markers: Vec<String>,
    #[serde(default)]
    pub end_markers: Vec<String>,
    #[serde(default)]
    pub exact_noise_lines: Vec<String>,
    #[serde(default)]
    pub contains_noise_lines: Vec<String>,
    #[serde(default)]
    pub line_suffix_noise: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleValueConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub llm_judge: bool,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
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
struct ResolvedArticleCleaningStrategy {
    name: String,
    version: u32,
    source: String,
    min_kept_ratio: f32,
    max_kept_ratio: f32,
    min_normalized_chars: usize,
    start_markers: Vec<String>,
    end_markers: Vec<String>,
    exact_noise_lines: Vec<String>,
    contains_noise_lines: Vec<String>,
    line_suffix_noise: Vec<String>,
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
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleCleaningCheckResponse {
    pub status: String,
    pub config_path: String,
    pub sites: Vec<String>,
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
            provider: String::new(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            max_input_chars: default_value_max_input_chars(),
            min_normalized_chars: default_value_min_normalized_chars(),
            save_threshold: default_value_save_threshold(),
            candidate_threshold: default_value_candidate_threshold(),
            target_topics: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_value_max_input_chars() -> usize {
    12_000
}

fn default_value_min_normalized_chars() -> usize {
    800
}

fn default_value_save_threshold() -> f32 {
    0.75
}

fn default_value_candidate_threshold() -> f32 {
    0.45
}

fn default_cleaning_min_kept_ratio() -> f32 {
    0.20
}

fn default_cleaning_max_kept_ratio() -> f32 {
    0.98
}

fn default_cleaning_min_normalized_chars() -> usize {
    800
}

fn default_cleaning_strategy_version() -> u32 {
    1
}

fn default_cleaning_strategy_status() -> String {
    "stable".to_string()
}

pub fn init_article_memory(paths: &RuntimePaths) -> Result<ArticleMemoryStatusResponse> {
    ensure_article_memory_dirs(paths)?;
    if !paths.article_memory_index_path().is_file() {
        write_index(paths, &ArticleMemoryIndex::new())?;
    }
    check_article_memory(paths)
}

pub fn article_memory_status(paths: &RuntimePaths) -> ArticleMemoryStatusResponse {
    match load_index(paths) {
        Ok(index) => build_status_response(paths, "ok", &index, None),
        Err(_) if !paths.article_memory_index_path().exists() => build_missing_status(paths),
        Err(error) => build_error_status(paths, error.to_string()),
    }
}

pub fn check_article_memory(paths: &RuntimePaths) -> Result<ArticleMemoryStatusResponse> {
    if !paths.article_memory_index_path().is_file() {
        bail!(
            "article memory index was not found: {}\nRun: daviszeroclaw articles init",
            paths.article_memory_index_path().display()
        );
    }
    let index = load_index(paths)?;
    Ok(build_status_response(paths, "ok", &index, None))
}

pub fn check_article_cleaning(paths: &RuntimePaths) -> Result<ArticleCleaningCheckResponse> {
    let config = load_article_cleaning_config(paths)?;
    let mut warnings = Vec::new();
    let mut seen = BTreeSet::new();
    for site in &config.sites {
        if !seen.insert(site.name.clone()) {
            warnings.push(format!("duplicate site strategy: {}", site.name));
        }
        if site.url_patterns.is_empty() && site.source_patterns.is_empty() {
            warnings.push(format!(
                "site strategy has no url_patterns/source_patterns: {}",
                site.name
            ));
        }
        if site.preferred_selectors.is_empty() {
            warnings.push(format!(
                "site strategy has no preferred_selectors: {}",
                site.name
            ));
        }
    }
    Ok(ArticleCleaningCheckResponse {
        status: if warnings.is_empty() { "ok" } else { "warn" }.to_string(),
        config_path: paths.article_cleaning_config_path().display().to_string(),
        sites: config.sites.into_iter().map(|site| site.name).collect(),
        warnings,
    })
}

pub fn article_cleaning_preferred_selectors(paths: &RuntimePaths) -> Result<Vec<String>> {
    let config = load_article_cleaning_config(paths)?;
    let mut seen = BTreeSet::new();
    let mut selectors = Vec::new();
    for site in config.sites {
        for selector in site.preferred_selectors {
            if seen.insert(selector.clone()) {
                selectors.push(selector);
            }
        }
    }
    selectors.extend(
        [
            "article",
            "main",
            "[role=main]",
            ".article",
            ".post",
            ".post-content",
            ".entry-content",
            ".content",
            "#content",
            "body",
        ]
        .into_iter()
        .filter(|selector| seen.insert((*selector).to_string()))
        .map(str::to_string),
    );
    Ok(selectors)
}

pub fn add_article_memory(
    paths: &RuntimePaths,
    request: ArticleMemoryAddRequest,
) -> Result<ArticleMemoryRecord> {
    ensure_article_memory_dirs(paths)?;
    if !paths.article_memory_index_path().is_file() {
        write_index(paths, &ArticleMemoryIndex::new())?;
    }

    let title = clean_required("title", &request.title)?;
    let content = clean_required("content", &request.content)?;
    let source = clean_optional(&request.source).unwrap_or_else(|| "manual".to_string());
    let now = isoformat(now_utc());
    let id = article_id(&title, request.url.as_deref(), &now);

    let content_path = format!("articles/{id}.md");
    let raw_path = format!("articles/{id}.raw.txt");
    let normalized_path = format!("articles/{id}.normalized.md");
    let summary_path = request
        .summary
        .as_deref()
        .and_then(clean_optional)
        .map(|_| format!("articles/{id}.summary.md"));
    let translation_path = request
        .translation
        .as_deref()
        .and_then(clean_optional)
        .map(|_| format!("articles/{id}.translation.md"));

    fs::write(resolve_article_path(paths, &raw_path), &content)
        .with_context(|| format!("failed to write article raw content for {id}"))?;
    fs::write(resolve_article_path(paths, &normalized_path), &content)
        .with_context(|| format!("failed to write article normalized content for {id}"))?;
    fs::write(resolve_article_path(paths, &content_path), &content)
        .with_context(|| format!("failed to write article content for {id}"))?;
    if let (Some(summary), Some(path)) = (request.summary.as_deref(), summary_path.as_deref()) {
        fs::write(resolve_article_path(paths, path), summary.trim())
            .with_context(|| format!("failed to write article summary for {id}"))?;
    }
    if let (Some(translation), Some(path)) =
        (request.translation.as_deref(), translation_path.as_deref())
    {
        fs::write(resolve_article_path(paths, path), translation.trim())
            .with_context(|| format!("failed to write article translation for {id}"))?;
    }

    let mut index = load_index(paths)?;
    let record = ArticleMemoryRecord {
        id,
        title,
        url: request.url.and_then(|value| clean_optional(&value)),
        source,
        language: request.language.and_then(|value| clean_optional(&value)),
        tags: normalize_tags(request.tags),
        status: request.status,
        value_score: normalize_score(request.value_score)?,
        captured_at: now.clone(),
        updated_at: now,
        content_path,
        raw_path: Some(raw_path),
        normalized_path: Some(normalized_path),
        summary_path,
        translation_path,
        notes: request.notes.and_then(|value| clean_optional(&value)),
        clean_status: Some("raw".to_string()),
        clean_profile: None,
    };
    index.articles.push(record.clone());
    index.updated_at = isoformat(now_utc());
    write_index(paths, &index)?;
    Ok(record)
}

pub fn list_article_memory(paths: &RuntimePaths, limit: usize) -> ArticleMemoryListResponse {
    match load_index(paths) {
        Ok(mut index) => {
            index.articles.sort_by(|a, b| {
                b.captured_at
                    .cmp(&a.captured_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
            let limit = normalize_limit(limit);
            let total_articles = index.articles.len();
            let articles = index.articles.into_iter().take(limit).collect::<Vec<_>>();
            ArticleMemoryListResponse {
                status: "ok".to_string(),
                returned: articles.len(),
                total_articles,
                articles,
                message: None,
            }
        }
        Err(_) if !paths.article_memory_index_path().exists() => ArticleMemoryListResponse {
            status: "missing".to_string(),
            returned: 0,
            total_articles: 0,
            articles: Vec::new(),
            message: Some("article memory is not initialized".to_string()),
        },
        Err(error) => ArticleMemoryListResponse {
            status: "error".to_string(),
            returned: 0,
            total_articles: 0,
            articles: Vec::new(),
            message: Some(error.to_string()),
        },
    }
}

pub fn search_article_memory(
    paths: &RuntimePaths,
    query: &str,
    limit: usize,
) -> ArticleMemorySearchResponse {
    let query = query.trim().to_string();
    if query.is_empty() {
        return ArticleMemorySearchResponse {
            status: "bad_request".to_string(),
            query,
            search_mode: "keyword".to_string(),
            returned: 0,
            total_hits: 0,
            hits: Vec::new(),
            semantic_status: None,
            message: Some("query is required".to_string()),
        };
    }

    let index = match load_index(paths) {
        Ok(index) => index,
        Err(_) if !paths.article_memory_index_path().exists() => {
            return ArticleMemorySearchResponse {
                status: "missing".to_string(),
                query,
                search_mode: "keyword".to_string(),
                returned: 0,
                total_hits: 0,
                hits: Vec::new(),
                semantic_status: None,
                message: Some("article memory is not initialized".to_string()),
            }
        }
        Err(error) => {
            return ArticleMemorySearchResponse {
                status: "error".to_string(),
                query,
                search_mode: "keyword".to_string(),
                returned: 0,
                total_hits: 0,
                hits: Vec::new(),
                semantic_status: None,
                message: Some(error.to_string()),
            }
        }
    };

    let mut hits = index
        .articles
        .iter()
        .filter_map(|record| score_record(paths, record, &query))
        .collect::<Vec<_>>();
    hits.sort_by(compare_hits);
    let total_hits = hits.len();
    let limit = normalize_limit(limit);
    hits.truncate(limit);

    ArticleMemorySearchResponse {
        status: if total_hits == 0 { "empty" } else { "ok" }.to_string(),
        query,
        search_mode: "keyword".to_string(),
        returned: hits.len(),
        total_hits,
        hits,
        semantic_status: None,
        message: None,
    }
}

pub fn list_article_clean_reports(
    paths: &RuntimePaths,
    limit: usize,
) -> Result<ArticleCleanAuditResponse> {
    ensure_article_memory_dirs(paths)?;
    let reports_dir = paths.article_memory_clean_reports_dir();
    if !reports_dir.is_dir() {
        return Ok(ArticleCleanAuditResponse {
            status: "empty".to_string(),
            returned: 0,
            reports: Vec::new(),
        });
    }
    let mut entries = fs::read_dir(&reports_dir)
        .with_context(|| {
            format!(
                "failed to read clean reports dir: {}",
                reports_dir.display()
            )
        })?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        let a_modified = a.metadata().and_then(|metadata| metadata.modified()).ok();
        let b_modified = b.metadata().and_then(|metadata| metadata.modified()).ok();
        b_modified.cmp(&a_modified)
    });
    let mut reports = Vec::new();
    for entry in entries.into_iter().take(normalize_limit(limit)) {
        let path = entry.path();
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read clean report: {}", path.display()))?;
        let report: ArticleCleanReport = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse clean report: {}", path.display()))?;
        reports.push(report);
    }
    Ok(ArticleCleanAuditResponse {
        status: if reports.is_empty() { "empty" } else { "ok" }.to_string(),
        returned: reports.len(),
        reports,
    })
}

pub fn list_article_value_reports(
    paths: &RuntimePaths,
    limit: usize,
) -> Result<ArticleValueAuditResponse> {
    ensure_article_memory_dirs(paths)?;
    let reports_dir = paths.article_memory_value_reports_dir();
    if !reports_dir.is_dir() {
        return Ok(ArticleValueAuditResponse {
            status: "empty".to_string(),
            returned: 0,
            reports: Vec::new(),
        });
    }
    let mut entries = fs::read_dir(&reports_dir)
        .with_context(|| {
            format!(
                "failed to read value reports dir: {}",
                reports_dir.display()
            )
        })?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        let a_modified = a.metadata().and_then(|metadata| metadata.modified()).ok();
        let b_modified = b.metadata().and_then(|metadata| metadata.modified()).ok();
        b_modified.cmp(&a_modified)
    });
    let mut reports = Vec::new();
    for entry in entries.into_iter().take(normalize_limit(limit)) {
        let path = entry.path();
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read value report: {}", path.display()))?;
        let report: ArticleValueReport = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse value report: {}", path.display()))?;
        reports.push(report);
    }
    Ok(ArticleValueAuditResponse {
        status: if reports.is_empty() { "empty" } else { "ok" }.to_string(),
        returned: reports.len(),
        reports,
    })
}

pub fn build_article_strategy_review_input(
    paths: &RuntimePaths,
    recent: usize,
) -> Result<ArticleStrategyReviewInputResponse> {
    ensure_article_memory_dirs(paths)?;
    let recent = normalize_limit(recent);
    let generated_at = isoformat(now_utc());
    let cleaning_check = check_article_cleaning(paths)?;
    let clean_audit = list_article_clean_reports(paths, recent)?;
    let value_audit = list_article_value_reports(paths, recent)?;
    let markdown = render_article_strategy_review_input(
        paths,
        &generated_at,
        recent,
        &cleaning_check,
        &clean_audit,
        &value_audit,
    );
    let report_path = paths
        .article_memory_strategy_reports_dir()
        .join("latest.md");
    fs::write(&report_path, &markdown).with_context(|| {
        format!(
            "failed to write strategy review input: {}",
            report_path.display()
        )
    })?;

    let has_report_risks = clean_audit
        .reports
        .iter()
        .any(|report| !report.risk_flags.is_empty())
        || value_audit
            .reports
            .iter()
            .any(|report| !report.risk_flags.is_empty());
    let status = if cleaning_check.status == "ok" && !has_report_risks {
        "ok"
    } else {
        "review"
    };
    Ok(ArticleStrategyReviewInputResponse {
        status: status.to_string(),
        generated_at,
        report_path: report_path.display().to_string(),
        config_path: paths.article_cleaning_config_path().display().to_string(),
        implementation_requests_dir: paths
            .article_memory_implementation_requests_dir()
            .display()
            .to_string(),
        recent,
        clean_reports: clean_audit.returned,
        value_reports: value_audit.returned,
        markdown,
    })
}

fn render_article_strategy_review_input(
    paths: &RuntimePaths,
    generated_at: &str,
    recent: usize,
    cleaning_check: &ArticleCleaningCheckResponse,
    clean_audit: &ArticleCleanAuditResponse,
    value_audit: &ArticleValueAuditResponse,
) -> String {
    let config_path = paths.article_cleaning_config_path().display().to_string();
    let implementation_requests_dir = paths
        .article_memory_implementation_requests_dir()
        .display()
        .to_string();
    let mut lines = vec![
        "# Article Memory Strategy Review Input".to_string(),
        String::new(),
        format!("Generated at: {generated_at}"),
        format!("Recent report limit: {recent}"),
        String::new(),
        "## Hard Boundary".to_string(),
        String::new(),
        format!("- You may edit only: `{config_path}`"),
        "- Do not edit Rust source, Cargo files, generated article files, or report JSON files."
            .to_string(),
        format!(
            "- If the current strategy fields are insufficient, write an implementation request under: `{implementation_requests_dir}`"
        ),
        "- The implementation request should explain the missing capability, affected URLs/sites, evidence from reports, and a minimal proposed Rust change.".to_string(),
        String::new(),
        "## Review Commands".to_string(),
        String::new(),
        "- `daviszeroclaw articles cleaning check`".to_string(),
        "- `daviszeroclaw articles cleaning replay --all`".to_string(),
        format!("- `daviszeroclaw articles cleaning audit --recent {recent}`"),
        format!("- `daviszeroclaw articles judging audit --recent {recent}`"),
        format!("- `daviszeroclaw articles strategy review-input --recent {recent}`"),
        String::new(),
        "## Strategy Config Status".to_string(),
        String::new(),
        format!("- status: {}", cleaning_check.status),
        format!("- config: `{}`", cleaning_check.config_path),
        format!(
            "- sites: {}",
            if cleaning_check.sites.is_empty() {
                "none".to_string()
            } else {
                cleaning_check.sites.join(", ")
            }
        ),
    ];
    if cleaning_check.warnings.is_empty() {
        lines.push("- warnings: none".to_string());
    } else {
        lines.push("- warnings:".to_string());
        for warning in &cleaning_check.warnings {
            lines.push(format!("  - {}", one_line(warning, 220)));
        }
    }

    lines.extend([
        String::new(),
        "## Clean Report Signals".to_string(),
        String::new(),
    ]);
    if clean_audit.reports.is_empty() {
        lines.push("- No clean reports found.".to_string());
    } else {
        for report in &clean_audit.reports {
            lines.push(format!(
                "- `{}` | {} | strategy={}@{} | clean={} | raw={} normalized={} final={} kept={:.2} | risks={}",
                report.article_id,
                one_line(&report.title, 120),
                report.strategy_name,
                report.strategy_version,
                report.clean_status,
                report.raw_chars,
                report.normalized_chars,
                report.final_chars,
                report.kept_ratio,
                join_or_none(&report.risk_flags)
            ));
            if let Some(url) = &report.url {
                lines.push(format!("  - url: {}", one_line(url, 220)));
            }
            if !report.removed_lines_sample.is_empty() {
                lines.push(format!(
                    "  - removed sample: {}",
                    report
                        .removed_lines_sample
                        .iter()
                        .take(5)
                        .map(|line| one_line(line, 80))
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
            if !report.leftover_noise_candidates.is_empty() {
                lines.push(format!(
                    "  - leftover candidates: {}",
                    report
                        .leftover_noise_candidates
                        .iter()
                        .take(8)
                        .map(|line| one_line(line, 80))
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
        }
    }

    lines.extend([
        String::new(),
        "## Value Report Signals".to_string(),
        String::new(),
    ]);
    if value_audit.reports.is_empty() {
        lines.push("- No value reports found.".to_string());
    } else {
        for report in &value_audit.reports {
            lines.push(format!(
                "- `{}` | {} | decision={} | score={:.2} | topics={} | risks={}",
                report.article_id,
                one_line(&report.title, 120),
                report.decision,
                report.value_score,
                join_or_none(&report.topic_tags),
                join_or_none(&report.risk_flags)
            ));
            if let Some(url) = &report.url {
                lines.push(format!("  - url: {}", one_line(url, 220)));
            }
            if !report.reasons.is_empty() {
                lines.push(format!(
                    "  - reasons: {}",
                    report
                        .reasons
                        .iter()
                        .take(4)
                        .map(|reason| one_line(reason, 120))
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
        }
    }

    lines.extend([
        String::new(),
        "## Expected Reviewer Output".to_string(),
        String::new(),
        "- State whether the strategy changed.".to_string(),
        "- If changed, list the exact site/default/value fields edited and why.".to_string(),
        "- Run the check/replay/audit commands above and summarize the evidence.".to_string(),
        "- If no config-only change can solve the issue, name the implementation request file created.".to_string(),
        String::new(),
    ]);
    lines.join("\n")
}

fn one_line(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&compact, max_chars)
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(",")
    }
}

pub fn replay_article_cleaning(
    paths: &RuntimePaths,
    article_id: Option<&str>,
) -> Result<ArticleCleaningReplayResponse> {
    ensure_article_memory_dirs(paths)?;
    let index = load_index(paths)?;
    let cleaning_config = load_article_cleaning_config(paths)?;
    let mut reports = Vec::new();
    for article in index.articles {
        if let Some(article_id) = article_id {
            if article.id != article_id {
                continue;
            }
        }
        let raw_text = read_article_raw_text(paths, &article)?;
        let strategy = resolve_article_cleaning_strategy(&cleaning_config, &article);
        let normalized = normalize_article_text(&article, &raw_text, &strategy);
        let raw_chars = raw_text.chars().count();
        let normalized_chars = normalized.markdown.chars().count();
        let clean_status = deterministic_clean_status(&strategy, raw_chars, normalized_chars);
        reports.push(build_clean_report(
            &article,
            &strategy,
            &normalized,
            &clean_status,
            raw_chars,
            normalized_chars,
            normalized_chars,
        ));
    }
    if article_id.is_some() && reports.is_empty() {
        bail!("article not found: {}", article_id.unwrap());
    }
    Ok(ArticleCleaningReplayResponse {
        status: if reports.is_empty() { "empty" } else { "ok" }.to_string(),
        returned: reports.len(),
        reports,
    })
}

pub async fn judge_article_value_memory(
    paths: &RuntimePaths,
    value_config: &ResolvedArticleValueConfig,
    article_id: &str,
) -> Result<ArticleValueReport> {
    ensure_article_memory_dirs(paths)?;
    let mut index = load_index(paths)?;
    let article_position = index
        .articles
        .iter()
        .position(|article| article.id == article_id)
        .ok_or_else(|| anyhow!("article not found: {article_id}"))?;
    let mut article = index.articles[article_position].clone();
    let raw_text = read_article_raw_text(paths, &article)?;
    let cleaning_config = load_article_cleaning_config(paths)?;
    let strategy = resolve_article_cleaning_strategy(&cleaning_config, &article);
    let normalized = normalize_article_text(&article, &raw_text, &strategy);
    let raw_chars = raw_text.chars().count();
    let normalized_chars = normalized.markdown.chars().count();
    let clean_status = deterministic_clean_status(&strategy, raw_chars, normalized_chars);
    let clean_report = build_clean_report(
        &article,
        &strategy,
        &normalized,
        &clean_status,
        raw_chars,
        normalized_chars,
        normalized_chars,
    );
    let report =
        judge_article_value(value_config, &article, &clean_report, &normalized.markdown).await?;
    write_value_report(paths, &report)?;
    article.value_score = Some(report.value_score);
    match report.decision.as_str() {
        "reject" => article.status = ArticleMemoryRecordStatus::Rejected,
        "save" => article.status = ArticleMemoryRecordStatus::Saved,
        _ => {}
    }
    article.updated_at = isoformat(now_utc());
    index.articles[article_position] = article;
    index.updated_at = isoformat(now_utc());
    write_index(paths, &index)?;
    Ok(report)
}

pub async fn judge_all_article_value_memory(
    paths: &RuntimePaths,
    value_config: &ResolvedArticleValueConfig,
) -> Result<Vec<ArticleValueReport>> {
    let index = load_index(paths)?;
    let mut reports = Vec::new();
    for article in index.articles {
        reports.push(judge_article_value_memory(paths, value_config, &article.id).await?);
    }
    Ok(reports)
}

pub fn resolve_article_embedding_config(
    embedding: &ArticleMemoryEmbeddingConfig,
    providers: &[ModelProviderConfig],
) -> Result<Option<ResolvedArticleEmbeddingConfig>> {
    if !embedding.enabled {
        return Ok(None);
    }

    let provider = embedding.provider.trim();
    let provider_config = if provider.is_empty() {
        None
    } else {
        providers.iter().find(|item| item.name == provider)
    };
    let api_key = first_non_empty(
        embedding.api_key.trim(),
        provider_config
            .map(|item| item.api_key.as_str())
            .unwrap_or_default(),
    );
    let base_url = first_non_empty(
        embedding.base_url.trim(),
        provider_config
            .map(|item| item.base_url.as_str())
            .unwrap_or_default(),
    )
    .trim_end_matches('/')
    .to_string();
    if api_key.is_empty() || base_url.is_empty() {
        bail!("article memory embedding requires api_key and base_url");
    }

    let provider_name = if !provider.is_empty() {
        provider.to_string()
    } else {
        "direct".to_string()
    };

    Ok(Some(ResolvedArticleEmbeddingConfig {
        provider: provider_name,
        api_key,
        base_url,
        model: embedding.model.trim().to_string(),
        dimensions: embedding.dimensions,
        max_input_chars: embedding.max_input_chars,
    }))
}

pub fn resolve_article_normalize_config(
    normalize: &ArticleMemoryNormalizeConfig,
    providers: &[ModelProviderConfig],
) -> Result<Option<ResolvedArticleNormalizeConfig>> {
    if !normalize.llm_polish && !normalize.llm_summary {
        return Ok(None);
    }
    let provider = normalize.provider.trim();
    let provider_config = if provider.is_empty() {
        None
    } else {
        providers.iter().find(|item| item.name == provider)
    };
    let api_key = first_non_empty(
        normalize.api_key.trim(),
        provider_config
            .map(|item| item.api_key.as_str())
            .unwrap_or_default(),
    );
    let base_url = first_non_empty(
        normalize.base_url.trim(),
        provider_config
            .map(|item| item.base_url.as_str())
            .unwrap_or_default(),
    )
    .trim_end_matches('/')
    .to_string();
    let model = if normalize.model.trim().is_empty() {
        provider_config
            .and_then(|item| item.allowed_models.first())
            .cloned()
            .unwrap_or_default()
    } else {
        normalize.model.trim().to_string()
    };
    if api_key.is_empty() || base_url.is_empty() || model.is_empty() {
        bail!("article memory normalize requires api_key, base_url, and model");
    }
    Ok(Some(ResolvedArticleNormalizeConfig {
        provider: if provider.is_empty() {
            "direct".to_string()
        } else {
            provider.to_string()
        },
        api_key,
        base_url,
        model,
        llm_polish: normalize.llm_polish,
        llm_summary: normalize.llm_summary,
        min_polish_input_chars: normalize.min_polish_input_chars,
        max_polish_input_chars: normalize.max_polish_input_chars,
        summary_input_chars: normalize.summary_input_chars,
        fallback_min_ratio: normalize.fallback_min_ratio,
    }))
}

pub fn resolve_article_value_config(
    paths: &RuntimePaths,
    providers: &[ModelProviderConfig],
) -> Result<Option<ResolvedArticleValueConfig>> {
    let policy = load_article_cleaning_config(paths)?;
    let value = policy.value;
    if !value.enabled {
        return Ok(None);
    }
    let provider = value.provider.trim();
    let provider_config = if provider.is_empty() {
        None
    } else {
        providers.iter().find(|item| item.name == provider)
    };
    let api_key = first_non_empty(
        value.api_key.trim(),
        provider_config
            .map(|item| item.api_key.as_str())
            .unwrap_or_default(),
    );
    let base_url = first_non_empty(
        value.base_url.trim(),
        provider_config
            .map(|item| item.base_url.as_str())
            .unwrap_or_default(),
    )
    .trim_end_matches('/')
    .to_string();
    let model = if value.model.trim().is_empty() {
        provider_config
            .and_then(|item| item.allowed_models.first())
            .cloned()
            .unwrap_or_default()
    } else {
        value.model.trim().to_string()
    };
    let llm_judge =
        value.llm_judge && !api_key.is_empty() && !base_url.is_empty() && !model.is_empty();
    Ok(Some(ResolvedArticleValueConfig {
        provider: if provider.is_empty() {
            "direct".to_string()
        } else {
            provider.to_string()
        },
        api_key,
        base_url,
        model,
        llm_judge,
        max_input_chars: value.max_input_chars,
        min_normalized_chars: value.min_normalized_chars,
        save_threshold: value.save_threshold,
        candidate_threshold: value.candidate_threshold,
        target_topics: value.target_topics,
    }))
}

pub async fn normalize_article_memory(
    paths: &RuntimePaths,
    config: Option<&ResolvedArticleNormalizeConfig>,
    value_config: Option<&ResolvedArticleValueConfig>,
    article_id: &str,
) -> Result<ArticleMemoryNormalizeResponse> {
    ensure_article_memory_dirs(paths)?;
    let mut index = load_index(paths)?;
    let article_position = index
        .articles
        .iter()
        .position(|article| article.id == article_id)
        .ok_or_else(|| anyhow!("article not found: {article_id}"))?;
    let mut article = index.articles[article_position].clone();
    let raw_text = read_article_raw_text(paths, &article)?;
    let raw_path = article
        .raw_path
        .clone()
        .unwrap_or_else(|| format!("articles/{}.raw.txt", article.id));
    let normalized_path = article
        .normalized_path
        .clone()
        .unwrap_or_else(|| format!("articles/{}.normalized.md", article.id));
    let content_path = article.content_path.clone();
    let cleaning_config = load_article_cleaning_config(paths)?;
    let strategy = resolve_article_cleaning_strategy(&cleaning_config, &article);
    let profile = strategy.name.clone();
    let normalized_output = normalize_article_text(&article, &raw_text, &strategy);
    let normalized = normalized_output.markdown.clone();
    let raw_chars = raw_text.chars().count();
    let normalized_chars = normalized.chars().count();
    let mut final_text = normalized.clone();
    let mut clean_status = deterministic_clean_status(&strategy, raw_chars, normalized_chars);
    let mut polished = false;
    let mut message = None;
    let mut value_decision = None;
    let mut value_score = None;
    let mut value_report_path = None;
    let mut allow_polish = clean_status != "fallback_raw";
    let mut allow_summary = true;

    if let Some(value_config) = value_config {
        let clean_report_for_value = build_clean_report(
            &article,
            &strategy,
            &normalized_output,
            &clean_status,
            raw_chars,
            normalized_chars,
            normalized_chars,
        );
        let value_report =
            judge_article_value(value_config, &article, &clean_report_for_value, &normalized)
                .await?;
        value_decision = Some(value_report.decision.clone());
        value_score = Some(value_report.value_score);
        let path = write_value_report(paths, &value_report)?;
        value_report_path = Some(path.display().to_string());
        match value_report.decision.as_str() {
            "reject" => {
                final_text = normalized.clone();
                clean_status = "rejected".to_string();
                article.status = ArticleMemoryRecordStatus::Rejected;
                allow_polish = false;
                allow_summary = false;
            }
            "candidate" => {
                allow_polish = false;
            }
            "save" => {
                article.status = ArticleMemoryRecordStatus::Saved;
            }
            _ => {}
        }
    }

    if clean_status == "fallback_raw" {
        final_text = raw_text.clone();
    } else if allow_polish {
        if let Some(config) = config.filter(|config| config.llm_polish) {
            let input_chars = normalized.chars().count();
            if input_chars >= config.min_polish_input_chars {
                let polish_input = truncate_chars(&normalized, config.max_polish_input_chars);
                match polish_markdown(config, &polish_input).await {
                    Ok(candidate)
                        if polished_is_valid(
                            &normalized,
                            &candidate,
                            config.fallback_min_ratio,
                        ) =>
                    {
                        final_text = candidate;
                        polished = true;
                        clean_status = "polished".to_string();
                    }
                    Ok(_) => {
                        message = Some(
                            "LLM polish rejected by validation; kept normalized markdown"
                                .to_string(),
                        );
                    }
                    Err(error) => {
                        message = Some(format!(
                            "LLM polish failed; kept normalized markdown: {error}"
                        ));
                    }
                }
            }
        }
    }

    let summary_path = format!("articles/{}.summary.md", article.id);
    let mut summary_generated = false;
    if allow_summary {
        if let Some(config) = config.filter(|config| config.llm_summary) {
            let summary_input = truncate_chars(&final_text, config.summary_input_chars);
            match summarize_markdown(config, &summary_input).await {
                Ok(summary) if !summary.trim().is_empty() => {
                    fs::write(resolve_article_path(paths, &summary_path), summary.trim())
                        .with_context(|| {
                            format!("failed to write article summary for {}", article.id)
                        })?;
                    article.summary_path = Some(summary_path.clone());
                    summary_generated = true;
                }
                Ok(_) => {}
                Err(error) => {
                    message = Some(format!("LLM summary failed: {error}"));
                }
            }
        }
    }

    fs::write(resolve_article_path(paths, &raw_path), &raw_text)
        .with_context(|| format!("failed to write article raw content for {}", article.id))?;
    fs::write(resolve_article_path(paths, &normalized_path), &normalized)
        .with_context(|| format!("failed to write normalized article for {}", article.id))?;
    fs::write(
        resolve_article_path(paths, &content_path),
        final_text.trim(),
    )
    .with_context(|| format!("failed to write final article for {}", article.id))?;
    let clean_report = build_clean_report(
        &article,
        &strategy,
        &normalized_output,
        &clean_status,
        raw_chars,
        normalized_chars,
        final_text.chars().count(),
    );
    let clean_report_path = write_clean_report(paths, &clean_report)?;

    article.raw_path = Some(raw_path.clone());
    article.normalized_path = Some(normalized_path.clone());
    article.clean_status = Some(clean_status.clone());
    article.clean_profile = Some(profile.clone());
    if let Some(score) = value_score {
        article.value_score = Some(score);
    }
    article.updated_at = isoformat(now_utc());
    index.articles[article_position] = article;
    index.updated_at = isoformat(now_utc());
    write_index(paths, &index)?;

    Ok(ArticleMemoryNormalizeResponse {
        status: "ok".to_string(),
        article_id: article_id.to_string(),
        clean_status,
        clean_profile: profile,
        raw_chars,
        normalized_chars,
        final_chars: final_text.chars().count(),
        polished,
        summary_generated,
        content_path: resolve_article_path(paths, &content_path)
            .display()
            .to_string(),
        raw_path: resolve_article_path(paths, &raw_path).display().to_string(),
        normalized_path: resolve_article_path(paths, &normalized_path)
            .display()
            .to_string(),
        clean_report_path: clean_report_path.display().to_string(),
        value_decision,
        value_score,
        value_report_path,
        summary_path: summary_generated.then(|| {
            resolve_article_path(paths, &summary_path)
                .display()
                .to_string()
        }),
        message,
    })
}

pub async fn normalize_all_article_memory(
    paths: &RuntimePaths,
    config: Option<&ResolvedArticleNormalizeConfig>,
    value_config: Option<&ResolvedArticleValueConfig>,
) -> Result<Vec<ArticleMemoryNormalizeResponse>> {
    let index = load_index(paths)?;
    let mut responses = Vec::new();
    for article in index.articles {
        responses.push(normalize_article_memory(paths, config, value_config, &article.id).await?);
    }
    Ok(responses)
}

fn build_clean_report(
    article: &ArticleMemoryRecord,
    strategy: &ResolvedArticleCleaningStrategy,
    normalized: &NormalizedArticleText,
    clean_status: &str,
    raw_chars: usize,
    normalized_chars: usize,
    final_chars: usize,
) -> ArticleCleanReport {
    let kept_ratio = if raw_chars == 0 {
        0.0
    } else {
        normalized_chars as f32 / raw_chars as f32
    };
    let mut risk_flags = Vec::new();
    if clean_status == "fallback_raw" {
        risk_flags.push("fallback_raw".to_string());
    }
    if kept_ratio < strategy.min_kept_ratio {
        risk_flags.push("low_kept_ratio".to_string());
    }
    if kept_ratio > strategy.max_kept_ratio {
        risk_flags.push("high_kept_ratio".to_string());
    }
    if normalized_chars < strategy.min_normalized_chars {
        risk_flags.push("normalized_too_short".to_string());
    }
    if !normalized.leftover_noise_candidates.is_empty() {
        risk_flags.push("leftover_noise_candidates".to_string());
    }
    if normalized.removed_start_chars > raw_chars / 2
        || normalized.removed_end_chars > raw_chars / 2
    {
        risk_flags.push("large_boundary_cut".to_string());
    }
    ArticleCleanReport {
        article_id: article.id.clone(),
        title: article.title.clone(),
        url: article.url.clone(),
        strategy_name: strategy.name.clone(),
        strategy_version: strategy.version,
        strategy_source: strategy.source.clone(),
        generated_at: isoformat(now_utc()),
        clean_status: clean_status.to_string(),
        raw_chars,
        prepared_chars: normalized.prepared_chars,
        normalized_chars,
        final_chars,
        kept_ratio,
        removed_start_chars: normalized.removed_start_chars,
        removed_end_chars: normalized.removed_end_chars,
        removed_start_preview: normalized.removed_start_preview.clone(),
        removed_end_preview: normalized.removed_end_preview.clone(),
        noise_lines_removed: normalized.noise_lines_removed,
        removed_lines_sample: normalized.removed_lines_sample.clone(),
        leftover_noise_candidates: normalized.leftover_noise_candidates.clone(),
        risk_flags,
    }
}

fn deterministic_clean_status(
    strategy: &ResolvedArticleCleaningStrategy,
    raw_chars: usize,
    normalized_chars: usize,
) -> String {
    let kept_ratio = if raw_chars == 0 {
        0.0
    } else {
        normalized_chars as f32 / raw_chars as f32
    };
    if (raw_chars >= strategy.min_normalized_chars
        && normalized_chars < strategy.min_normalized_chars)
        || kept_ratio < strategy.min_kept_ratio
    {
        "fallback_raw".to_string()
    } else {
        "ok".to_string()
    }
}

fn write_clean_report(paths: &RuntimePaths, report: &ArticleCleanReport) -> Result<PathBuf> {
    ensure_article_memory_dirs(paths)?;
    let path = paths
        .article_memory_clean_reports_dir()
        .join(format!("{}.json", report.article_id));
    let body = serde_json::to_string_pretty(report)?;
    fs::write(&path, body)
        .with_context(|| format!("failed to write clean report: {}", path.display()))?;
    Ok(path)
}

async fn judge_article_value(
    config: &ResolvedArticleValueConfig,
    article: &ArticleMemoryRecord,
    clean_report: &ArticleCleanReport,
    normalized: &str,
) -> Result<ArticleValueReport> {
    let mut report = deterministic_value_report(config, article, clean_report, normalized);
    if report.deterministic_reject || !config.llm_judge {
        return Ok(report);
    }

    let prompt_input = truncate_chars(normalized, config.max_input_chars);
    let system = "You judge whether an article is worth saving in an AI-agent learning memory. Return strict JSON only. Do not use markdown.";
    let user = format!(
        concat!(
            "Target topics: {topics}\n",
            "Article title: {title}\n",
            "URL: {url}\n",
            "Clean report: raw_chars={raw_chars}, normalized_chars={normalized_chars}, kept_ratio={kept_ratio:.2}, risk_flags={risk_flags}\n\n",
            "Judge the article. Reject ads, shallow SEO, misinformation, duplicates, and off-topic content. ",
            "Prefer practical experience, architecture analysis, official docs, source/code analysis, benchmarks, and durable workflows.\n",
            "Return JSON with keys: decision (save|candidate|reject), value_score (0..1), reasons (array), topic_tags (array), risk_flags (array), translation_needed (boolean).\n\n",
            "Article:\n{article}"
        ),
        topics = config.target_topics.join(", "),
        title = article.title,
        url = article.url.as_deref().unwrap_or_default(),
        raw_chars = clean_report.raw_chars,
        normalized_chars = clean_report.normalized_chars,
        kept_ratio = clean_report.kept_ratio,
        risk_flags = clean_report.risk_flags.join(", "),
        article = prompt_input,
    );
    match create_chat_completion_for_value(config, system, &user, 1200).await {
        Ok(content) => match parse_value_judge_response(&content, config, article) {
            Ok(mut judged) => {
                judged.model = Some(format!("{}/{}", config.provider, config.model));
                Ok(judged)
            }
            Err(error) => {
                report.risk_flags.push("llm_judge_parse_failed".to_string());
                report
                    .reasons
                    .push(format!("LLM judge parse failed: {error}"));
                Ok(report)
            }
        },
        Err(error) => {
            report.risk_flags.push("llm_judge_failed".to_string());
            report.reasons.push(format!("LLM judge failed: {error}"));
            Ok(report)
        }
    }
}

fn deterministic_value_report(
    config: &ResolvedArticleValueConfig,
    article: &ArticleMemoryRecord,
    clean_report: &ArticleCleanReport,
    normalized: &str,
) -> ArticleValueReport {
    let mut reasons = Vec::new();
    let mut risk_flags = Vec::new();
    let matched_topics = matched_value_topics(config, article, normalized);
    let mut deterministic_reject = false;
    let mut score: f32 = 0.55;

    if clean_report.clean_status == "fallback_raw" {
        deterministic_reject = true;
        score = 0.10;
        risk_flags.push("fallback_raw".to_string());
        reasons.push("cleaning fell back to raw content".to_string());
    }
    if clean_report.normalized_chars < config.min_normalized_chars {
        deterministic_reject = true;
        score = score.min(0.20);
        risk_flags.push("normalized_too_short".to_string());
        reasons.push("normalized article is too short".to_string());
    }
    if matched_topics.is_empty() && !config.target_topics.is_empty() {
        deterministic_reject = true;
        score = score.min(0.25);
        risk_flags.push("off_topic".to_string());
        reasons.push("no target topic matched the article".to_string());
    }
    if !clean_report.risk_flags.is_empty() {
        risk_flags.extend(clean_report.risk_flags.clone());
    }
    if reasons.is_empty() {
        reasons.push("passed deterministic value prefilter".to_string());
        score = if matched_topics.len() >= 2 {
            0.65
        } else {
            0.55
        };
    }
    let decision = if deterministic_reject {
        "reject".to_string()
    } else if score >= config.save_threshold {
        "save".to_string()
    } else if score >= config.candidate_threshold {
        "candidate".to_string()
    } else {
        "reject".to_string()
    };
    ArticleValueReport {
        article_id: article.id.clone(),
        title: article.title.clone(),
        url: article.url.clone(),
        judged_at: isoformat(now_utc()),
        decision,
        value_score: score,
        deterministic_reject,
        reasons,
        topic_tags: matched_topics,
        risk_flags,
        translation_needed: article
            .language
            .as_deref()
            .map(|language| !language.to_lowercase().starts_with("zh"))
            .unwrap_or(false),
        model: None,
    }
}

fn matched_value_topics(
    config: &ResolvedArticleValueConfig,
    article: &ArticleMemoryRecord,
    normalized: &str,
) -> Vec<String> {
    let haystack = format!(
        "{} {} {}",
        article.title,
        article.tags.join(" "),
        truncate_chars(normalized, config.max_input_chars)
    )
    .to_lowercase();
    config
        .target_topics
        .iter()
        .filter(|topic| haystack.contains(&topic.to_lowercase()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn parse_value_judge_response(
    content: &str,
    config: &ResolvedArticleValueConfig,
    article: &ArticleMemoryRecord,
) -> Result<ArticleValueReport> {
    let json_text = extract_json_object(content)?;
    let value: serde_json::Value =
        serde_json::from_str(&json_text).context("value judge response was not valid JSON")?;
    let raw_decision = value
        .get("decision")
        .and_then(|value| value.as_str())
        .unwrap_or("candidate")
        .trim()
        .to_lowercase();
    let score = value
        .get("value_score")
        .and_then(|value| value.as_f64())
        .unwrap_or(config.candidate_threshold as f64)
        .clamp(0.0, 1.0) as f32;
    let decision = normalize_value_decision(&raw_decision, score, config);
    Ok(ArticleValueReport {
        article_id: article.id.clone(),
        title: article.title.clone(),
        url: article.url.clone(),
        judged_at: isoformat(now_utc()),
        decision,
        value_score: score,
        deterministic_reject: false,
        reasons: json_string_array(&value, "reasons"),
        topic_tags: json_string_array(&value, "topic_tags"),
        risk_flags: json_string_array(&value, "risk_flags"),
        translation_needed: value
            .get("translation_needed")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        model: None,
    })
}

fn normalize_value_decision(
    raw_decision: &str,
    score: f32,
    config: &ResolvedArticleValueConfig,
) -> String {
    if score >= config.save_threshold {
        "save".to_string()
    } else if score < config.candidate_threshold {
        "reject".to_string()
    } else {
        match raw_decision {
            "reject" => "reject".to_string(),
            _ => "candidate".to_string(),
        }
    }
}

fn json_string_array(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .filter_map(clean_optional)
                .collect()
        })
        .unwrap_or_default()
}

fn extract_json_object(content: &str) -> Result<String> {
    let trimmed = content.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Ok(trimmed.to_string());
    }
    let start = trimmed
        .find('{')
        .ok_or_else(|| anyhow!("response did not contain a JSON object"))?;
    let end = trimmed
        .rfind('}')
        .ok_or_else(|| anyhow!("response did not contain a JSON object"))?;
    Ok(trimmed[start..=end].to_string())
}

fn write_value_report(paths: &RuntimePaths, report: &ArticleValueReport) -> Result<PathBuf> {
    ensure_article_memory_dirs(paths)?;
    let path = paths
        .article_memory_value_reports_dir()
        .join(format!("{}.json", report.article_id));
    let body = serde_json::to_string_pretty(report)?;
    fs::write(&path, body)
        .with_context(|| format!("failed to write value report: {}", path.display()))?;
    Ok(path)
}

pub async fn rebuild_article_memory_embeddings(
    paths: &RuntimePaths,
    config: &ResolvedArticleEmbeddingConfig,
) -> Result<ArticleMemoryEmbeddingRebuildResponse> {
    ensure_article_memory_dirs(paths)?;
    let index = load_index(paths)?;
    let mut vectors = Vec::new();
    let mut skipped = 0;
    for article in &index.articles {
        if article.status == ArticleMemoryRecordStatus::Rejected {
            skipped += 1;
            continue;
        }
        let text = article_embedding_text(paths, article, config.max_input_chars)?;
        if text.trim().is_empty() {
            skipped += 1;
            continue;
        }
        let vector = create_embedding(config, &text).await?;
        vectors.push(ArticleMemoryEmbeddingRecord {
            article_id: article.id.clone(),
            text_hash: text_hash(&text),
            indexed_at: isoformat(now_utc()),
            vector,
        });
    }
    let embedding_index = ArticleMemoryEmbeddingIndex {
        version: ARTICLE_MEMORY_EMBEDDINGS_VERSION,
        provider: config.provider.clone(),
        model: config.model.clone(),
        dimensions: config.dimensions,
        updated_at: isoformat(now_utc()),
        vectors,
    };
    write_embedding_index(paths, &embedding_index)?;
    Ok(ArticleMemoryEmbeddingRebuildResponse {
        status: "ok".to_string(),
        provider: config.provider.clone(),
        model: config.model.clone(),
        dimensions: config.dimensions,
        indexed: embedding_index.vectors.len(),
        skipped,
        index_path: paths.article_memory_embeddings_path().display().to_string(),
    })
}

pub async fn upsert_article_memory_embedding(
    paths: &RuntimePaths,
    config: &ResolvedArticleEmbeddingConfig,
    article: &ArticleMemoryRecord,
) -> Result<()> {
    if article.status == ArticleMemoryRecordStatus::Rejected {
        return Ok(());
    }
    let text = article_embedding_text(paths, article, config.max_input_chars)?;
    if text.trim().is_empty() {
        return Ok(());
    }
    let vector = create_embedding(config, &text).await?;
    let mut index = load_embedding_index(paths).unwrap_or_else(|_| ArticleMemoryEmbeddingIndex {
        version: ARTICLE_MEMORY_EMBEDDINGS_VERSION,
        provider: config.provider.clone(),
        model: config.model.clone(),
        dimensions: config.dimensions,
        updated_at: isoformat(now_utc()),
        vectors: Vec::new(),
    });
    index.provider = config.provider.clone();
    index.model = config.model.clone();
    index.dimensions = config.dimensions;
    index.updated_at = isoformat(now_utc());
    index
        .vectors
        .retain(|record| record.article_id != article.id);
    index.vectors.push(ArticleMemoryEmbeddingRecord {
        article_id: article.id.clone(),
        text_hash: text_hash(&text),
        indexed_at: isoformat(now_utc()),
        vector,
    });
    write_embedding_index(paths, &index)
}

pub async fn hybrid_search_article_memory(
    paths: &RuntimePaths,
    config: Option<&ResolvedArticleEmbeddingConfig>,
    query: &str,
    limit: usize,
) -> ArticleMemorySearchResponse {
    let keyword_limit = normalize_limit(limit).max(20);
    let keyword_response = search_article_memory(paths, query, keyword_limit);
    let Some(config) = config else {
        return keyword_response;
    };
    let embedding_index = match load_embedding_index(paths) {
        Ok(index) if !index.vectors.is_empty() => index,
        Ok(_) => {
            return with_semantic_status(keyword_response, "embedding_index_empty");
        }
        Err(_) if !paths.article_memory_embeddings_path().exists() => {
            return with_semantic_status(keyword_response, "embedding_index_missing");
        }
        Err(error) => {
            return with_semantic_status(
                keyword_response,
                &format!("embedding_index_error: {error}"),
            );
        }
    };
    let query_vector = match create_embedding(config, query).await {
        Ok(vector) => vector,
        Err(error) => {
            return with_semantic_status(
                keyword_response,
                &format!("embedding_query_error: {error}"),
            );
        }
    };
    let article_index = match load_index(paths) {
        Ok(index) => index,
        Err(error) => {
            return with_semantic_status(
                keyword_response,
                &format!("article_index_error: {error}"),
            );
        }
    };

    let mut hits = keyword_response.hits;
    for hit in &mut hits {
        hit.combined_score = Some(hit.score as f32);
    }

    for vector_record in &embedding_index.vectors {
        let semantic_score = cosine_similarity(&query_vector, &vector_record.vector);
        if semantic_score <= 0.0 {
            continue;
        }
        if let Some(existing) = hits
            .iter_mut()
            .find(|hit| hit.id == vector_record.article_id)
        {
            existing.semantic_score = Some(semantic_score);
            existing.combined_score = Some(existing.score as f32 + semantic_score * 10.0);
            if !existing
                .matched_fields
                .iter()
                .any(|field| field == "embedding")
            {
                existing.matched_fields.push("embedding".to_string());
            }
            continue;
        }
        let Some(article) = article_index
            .articles
            .iter()
            .find(|article| article.id == vector_record.article_id)
        else {
            continue;
        };
        let mut hit = record_to_search_hit(paths, article);
        hit.semantic_score = Some(semantic_score);
        hit.combined_score = Some(semantic_score * 10.0);
        hit.matched_fields.push("embedding".to_string());
        hits.push(hit);
    }

    hits.sort_by(compare_hybrid_hits);
    let total_hits = hits.len();
    hits.truncate(normalize_limit(limit));
    ArticleMemorySearchResponse {
        status: if total_hits == 0 { "empty" } else { "ok" }.to_string(),
        query: query.trim().to_string(),
        search_mode: "hybrid".to_string(),
        returned: hits.len(),
        total_hits,
        hits,
        semantic_status: Some("ok".to_string()),
        message: None,
    }
}

fn ensure_article_memory_dirs(paths: &RuntimePaths) -> Result<()> {
    fs::create_dir_all(paths.article_memory_articles_dir()).with_context(|| {
        format!(
            "failed to create {}",
            paths.article_memory_articles_dir().display()
        )
    })?;
    fs::create_dir_all(paths.article_memory_reports_dir()).with_context(|| {
        format!(
            "failed to create {}",
            paths.article_memory_reports_dir().display()
        )
    })?;
    fs::create_dir_all(paths.article_memory_clean_reports_dir()).with_context(|| {
        format!(
            "failed to create {}",
            paths.article_memory_clean_reports_dir().display()
        )
    })?;
    fs::create_dir_all(paths.article_memory_value_reports_dir()).with_context(|| {
        format!(
            "failed to create {}",
            paths.article_memory_value_reports_dir().display()
        )
    })?;
    fs::create_dir_all(paths.article_memory_strategy_reports_dir()).with_context(|| {
        format!(
            "failed to create {}",
            paths.article_memory_strategy_reports_dir().display()
        )
    })?;
    fs::create_dir_all(paths.article_memory_implementation_requests_dir()).with_context(|| {
        format!(
            "failed to create {}",
            paths.article_memory_implementation_requests_dir().display()
        )
    })?;
    Ok(())
}

fn load_index(paths: &RuntimePaths) -> Result<ArticleMemoryIndex> {
    let raw = fs::read_to_string(paths.article_memory_index_path()).with_context(|| {
        format!(
            "failed to read {}",
            paths.article_memory_index_path().display()
        )
    })?;
    let mut index = serde_json::from_str::<ArticleMemoryIndex>(&raw).with_context(|| {
        format!(
            "failed to parse {}",
            paths.article_memory_index_path().display()
        )
    })?;
    if index.version != ARTICLE_MEMORY_INDEX_VERSION {
        return Err(anyhow!(
            "unsupported article memory index version: {}",
            index.version
        ));
    }
    index.articles.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(index)
}

fn load_embedding_index(paths: &RuntimePaths) -> Result<ArticleMemoryEmbeddingIndex> {
    let raw = fs::read_to_string(paths.article_memory_embeddings_path()).with_context(|| {
        format!(
            "failed to read {}",
            paths.article_memory_embeddings_path().display()
        )
    })?;
    let index = serde_json::from_str::<ArticleMemoryEmbeddingIndex>(&raw).with_context(|| {
        format!(
            "failed to parse {}",
            paths.article_memory_embeddings_path().display()
        )
    })?;
    if index.version != ARTICLE_MEMORY_EMBEDDINGS_VERSION {
        return Err(anyhow!(
            "unsupported article memory embeddings version: {}",
            index.version
        ));
    }
    Ok(index)
}

fn write_index(paths: &RuntimePaths, index: &ArticleMemoryIndex) -> Result<()> {
    ensure_article_memory_dirs(paths)?;
    let index_path = paths.article_memory_index_path();
    let parent = index_path.parent().ok_or_else(|| {
        anyhow!(
            "invalid article memory index path: {}",
            index_path.display()
        )
    })?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let temp_path = index_path.with_extension("json.tmp");
    let raw = serde_json::to_vec_pretty(index)?;
    fs::write(&temp_path, raw)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    fs::rename(&temp_path, &index_path)
        .with_context(|| format!("failed to replace {}", index_path.display()))?;
    Ok(())
}

fn write_embedding_index(paths: &RuntimePaths, index: &ArticleMemoryEmbeddingIndex) -> Result<()> {
    ensure_article_memory_dirs(paths)?;
    let index_path = paths.article_memory_embeddings_path();
    let parent = index_path
        .parent()
        .ok_or_else(|| anyhow!("invalid embeddings path: {}", index_path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let temp_path = index_path.with_extension("json.tmp");
    let raw = serde_json::to_vec_pretty(index)?;
    fs::write(&temp_path, raw)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    fs::rename(&temp_path, &index_path)
        .with_context(|| format!("failed to replace {}", index_path.display()))?;
    Ok(())
}

fn build_missing_status(paths: &RuntimePaths) -> ArticleMemoryStatusResponse {
    ArticleMemoryStatusResponse {
        status: "missing".to_string(),
        root: paths.article_memory_dir().display().to_string(),
        index_path: paths.article_memory_index_path().display().to_string(),
        articles_dir: paths.article_memory_articles_dir().display().to_string(),
        reports_dir: paths.article_memory_reports_dir().display().to_string(),
        total_articles: 0,
        saved_articles: 0,
        candidate_articles: 0,
        rejected_articles: 0,
        archived_articles: 0,
        languages: Vec::new(),
        tags: Vec::new(),
        last_updated_at: None,
        message: Some("article memory is not initialized".to_string()),
    }
}

fn build_error_status(paths: &RuntimePaths, message: String) -> ArticleMemoryStatusResponse {
    ArticleMemoryStatusResponse {
        message: Some(message),
        status: "error".to_string(),
        ..build_missing_status(paths)
    }
}

fn build_status_response(
    paths: &RuntimePaths,
    status: &str,
    index: &ArticleMemoryIndex,
    message: Option<String>,
) -> ArticleMemoryStatusResponse {
    let mut languages = BTreeSet::new();
    let mut tags = BTreeSet::new();
    let mut saved_articles = 0;
    let mut candidate_articles = 0;
    let mut rejected_articles = 0;
    let mut archived_articles = 0;

    for article in &index.articles {
        if let Some(language) = article.language.as_deref().and_then(clean_optional) {
            languages.insert(language);
        }
        tags.extend(article.tags.iter().cloned());
        match article.status {
            ArticleMemoryRecordStatus::Saved => saved_articles += 1,
            ArticleMemoryRecordStatus::Candidate => candidate_articles += 1,
            ArticleMemoryRecordStatus::Rejected => rejected_articles += 1,
            ArticleMemoryRecordStatus::Archived => archived_articles += 1,
        }
    }

    ArticleMemoryStatusResponse {
        status: status.to_string(),
        root: paths.article_memory_dir().display().to_string(),
        index_path: paths.article_memory_index_path().display().to_string(),
        articles_dir: paths.article_memory_articles_dir().display().to_string(),
        reports_dir: paths.article_memory_reports_dir().display().to_string(),
        total_articles: index.articles.len(),
        saved_articles,
        candidate_articles,
        rejected_articles,
        archived_articles,
        languages: languages.into_iter().collect(),
        tags: tags.into_iter().collect(),
        last_updated_at: Some(index.updated_at.clone()),
        message,
    }
}

fn score_record(
    paths: &RuntimePaths,
    record: &ArticleMemoryRecord,
    query: &str,
) -> Option<ArticleMemorySearchHit> {
    let needle = query.to_lowercase();
    let mut score = 0;
    let mut matched_fields = Vec::new();
    let mut snippet = None;

    if contains_case_insensitive(&record.title, &needle) {
        score += 10;
        matched_fields.push("title".to_string());
    }
    if record
        .url
        .as_deref()
        .is_some_and(|url| contains_case_insensitive(url, &needle))
    {
        score += 6;
        matched_fields.push("url".to_string());
    }
    if contains_case_insensitive(&record.source, &needle) {
        score += 3;
        matched_fields.push("source".to_string());
    }
    if record
        .tags
        .iter()
        .any(|tag| contains_case_insensitive(tag, &needle))
    {
        score += 7;
        matched_fields.push("tags".to_string());
    }
    if record
        .notes
        .as_deref()
        .is_some_and(|notes| contains_case_insensitive(notes, &needle))
    {
        score += 3;
        matched_fields.push("notes".to_string());
    }

    for (field, relative_path, field_score) in [
        ("content", Some(record.content_path.as_str()), 2),
        ("summary", record.summary_path.as_deref(), 5),
        ("translation", record.translation_path.as_deref(), 4),
    ] {
        let Some(relative_path) = relative_path else {
            continue;
        };
        let text = read_article_text(paths, relative_path).unwrap_or_default();
        if contains_case_insensitive(&text, &needle) {
            score += field_score;
            matched_fields.push(field.to_string());
            if snippet.is_none() {
                snippet = Some(make_snippet(&text, query));
            }
        }
    }

    if score == 0 {
        return None;
    }

    Some(ArticleMemorySearchHit {
        id: record.id.clone(),
        title: record.title.clone(),
        url: record.url.clone(),
        source: record.source.clone(),
        language: record.language.clone(),
        tags: record.tags.clone(),
        status: record.status.clone(),
        value_score: record.value_score,
        captured_at: record.captured_at.clone(),
        score,
        semantic_score: None,
        combined_score: None,
        matched_fields,
        snippet,
        content_path: resolve_article_path(paths, &record.content_path)
            .display()
            .to_string(),
        summary_path: record
            .summary_path
            .as_deref()
            .map(|path| resolve_article_path(paths, path).display().to_string()),
        translation_path: record
            .translation_path
            .as_deref()
            .map(|path| resolve_article_path(paths, path).display().to_string()),
    })
}

fn compare_hits(a: &ArticleMemorySearchHit, b: &ArticleMemorySearchHit) -> Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| {
            b.value_score
                .partial_cmp(&a.value_score)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| b.captured_at.cmp(&a.captured_at))
        .then_with(|| a.id.cmp(&b.id))
}

fn compare_hybrid_hits(a: &ArticleMemorySearchHit, b: &ArticleMemorySearchHit) -> Ordering {
    b.combined_score
        .partial_cmp(&a.combined_score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| compare_hits(a, b))
}

fn record_to_search_hit(
    paths: &RuntimePaths,
    record: &ArticleMemoryRecord,
) -> ArticleMemorySearchHit {
    ArticleMemorySearchHit {
        id: record.id.clone(),
        title: record.title.clone(),
        url: record.url.clone(),
        source: record.source.clone(),
        language: record.language.clone(),
        tags: record.tags.clone(),
        status: record.status.clone(),
        value_score: record.value_score,
        captured_at: record.captured_at.clone(),
        score: 0,
        semantic_score: None,
        combined_score: None,
        matched_fields: Vec::new(),
        snippet: None,
        content_path: resolve_article_path(paths, &record.content_path)
            .display()
            .to_string(),
        summary_path: record
            .summary_path
            .as_deref()
            .map(|path| resolve_article_path(paths, path).display().to_string()),
        translation_path: record
            .translation_path
            .as_deref()
            .map(|path| resolve_article_path(paths, path).display().to_string()),
    }
}

fn with_semantic_status(
    mut response: ArticleMemorySearchResponse,
    semantic_status: &str,
) -> ArticleMemorySearchResponse {
    response.semantic_status = Some(semantic_status.to_string());
    response
}

fn article_embedding_text(
    paths: &RuntimePaths,
    article: &ArticleMemoryRecord,
    max_chars: usize,
) -> Result<String> {
    let mut parts = Vec::new();
    parts.push(format!("Title: {}", article.title));
    if let Some(url) = &article.url {
        parts.push(format!("URL: {url}"));
    }
    if !article.tags.is_empty() {
        parts.push(format!("Tags: {}", article.tags.join(", ")));
    }
    if let Some(notes) = &article.notes {
        parts.push(format!("Notes: {notes}"));
    }
    if let Some(path) = &article.summary_path {
        if let Ok(summary) = read_article_text(paths, path) {
            parts.push(format!("Summary:\n{summary}"));
        }
    }
    if let Some(path) = &article.translation_path {
        if let Ok(translation) = read_article_text(paths, path) {
            parts.push(format!("Translation:\n{translation}"));
        }
    }
    if let Ok(content) = read_article_text(paths, &article.content_path) {
        parts.push(format!("Content:\n{content}"));
    }
    Ok(truncate_chars(&parts.join("\n\n"), max_chars))
}

async fn create_embedding(
    config: &ResolvedArticleEmbeddingConfig,
    input: &str,
) -> Result<Vec<f32>> {
    let endpoint = format!("{}/embeddings", config.base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let mut payload = json!({
        "model": config.model,
        "input": input,
        "encoding_format": "float"
    });
    if config.dimensions > 0 {
        payload["dimensions"] = json!(config.dimensions);
    }
    let response = client
        .post(endpoint)
        .bearer_auth(&config.api_key)
        .json(&payload)
        .send()
        .await
        .context("embedding request failed")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| String::from("<failed to read response>"));
    if !status.is_success() {
        bail!("embedding request failed with HTTP {status}: {body}");
    }
    let value: serde_json::Value =
        serde_json::from_str(&body).context("embedding response was not valid JSON")?;
    let embedding = value
        .get("data")
        .and_then(|data| data.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.get("embedding"))
        .and_then(|embedding| embedding.as_array())
        .ok_or_else(|| anyhow!("embedding response did not contain data[0].embedding"))?;
    let vector = embedding
        .iter()
        .map(|value| {
            value
                .as_f64()
                .map(|number| number as f32)
                .ok_or_else(|| anyhow!("embedding vector contained a non-number value"))
        })
        .collect::<Result<Vec<_>>>()?;
    if vector.is_empty() {
        bail!("embedding response returned an empty vector");
    }
    Ok(vector)
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut a_norm = 0.0;
    let mut b_norm = 0.0;
    for (left, right) in a.iter().zip(b.iter()) {
        dot += left * right;
        a_norm += left * left;
        b_norm += right * right;
    }
    if a_norm == 0.0 || b_norm == 0.0 {
        return 0.0;
    }
    dot / (a_norm.sqrt() * b_norm.sqrt())
}

fn first_non_empty(primary: &str, fallback: &str) -> String {
    if !primary.trim().is_empty() {
        primary.trim().to_string()
    } else {
        fallback.trim().to_string()
    }
}

fn text_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read_article_text(paths: &RuntimePaths, relative_path: &str) -> Result<String> {
    fs::read_to_string(resolve_article_path(paths, relative_path))
        .with_context(|| format!("failed to read article text: {relative_path}"))
}

fn read_article_raw_text(paths: &RuntimePaths, article: &ArticleMemoryRecord) -> Result<String> {
    if let Some(raw_path) = &article.raw_path {
        if let Ok(text) = read_article_text(paths, raw_path) {
            return Ok(text);
        }
    }
    read_article_text(paths, &article.content_path)
}

fn resolve_article_path(paths: &RuntimePaths, relative_path: &str) -> PathBuf {
    paths.article_memory_dir().join(relative_path)
}

fn article_id(title: &str, url: Option<&str>, captured_at: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(title.as_bytes());
    hasher.update(b"\n");
    hasher.update(url.unwrap_or_default().as_bytes());
    hasher.update(b"\n");
    hasher.update(captured_at.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn clean_required(field: &str, value: &str) -> Result<String> {
    clean_optional(value).ok_or_else(|| anyhow!("{field} is required"))
}

fn clean_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    tags.into_iter()
        .filter_map(|tag| clean_optional(&tag))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize_score(score: Option<f32>) -> Result<Option<f32>> {
    match score {
        Some(value) if !(0.0..=1.0).contains(&value) => {
            bail!("value_score must be between 0.0 and 1.0")
        }
        value => Ok(value),
    }
}

fn normalize_limit(limit: usize) -> usize {
    limit.clamp(1, 100)
}

fn contains_case_insensitive(value: &str, needle_lowercase: &str) -> bool {
    value.to_lowercase().contains(needle_lowercase)
}

fn make_snippet(text: &str, query: &str) -> String {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = cleaned.to_lowercase();
    let query_lower = query.to_lowercase();
    let start = lower
        .find(&query_lower)
        .map(|index| {
            cleaned
                .char_indices()
                .take_while(|(byte_index, _)| *byte_index < index)
                .count()
                .saturating_sub(60)
        })
        .unwrap_or(0);
    cleaned.chars().skip(start).take(240).collect()
}

#[derive(Debug, Clone)]
struct PreparedArticleText {
    text: String,
    removed_start_chars: usize,
    removed_end_chars: usize,
    removed_start_preview: Option<String>,
    removed_end_preview: Option<String>,
}

#[derive(Debug, Clone)]
struct NormalizedArticleText {
    markdown: String,
    prepared_chars: usize,
    removed_start_chars: usize,
    removed_end_chars: usize,
    removed_start_preview: Option<String>,
    removed_end_preview: Option<String>,
    noise_lines_removed: usize,
    removed_lines_sample: Vec<String>,
    leftover_noise_candidates: Vec<String>,
}

fn load_article_cleaning_config(paths: &RuntimePaths) -> Result<ArticleCleaningConfig> {
    let path = paths.article_cleaning_config_path();
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            BUILTIN_ARTICLE_MEMORY_POLICY_CONFIG.to_string()
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read article cleaning config: {}", path.display())
            })
        }
    };
    let mut config: ArticleCleaningConfig = toml::from_str(&raw).with_context(|| {
        format!(
            "failed to parse article cleaning config: {}",
            path.display()
        )
    })?;
    normalize_article_cleaning_config(&mut config)?;
    normalize_article_value_config(&mut config.value);
    Ok(config)
}

fn normalize_article_cleaning_config(config: &mut ArticleCleaningConfig) -> Result<()> {
    normalize_cleaning_defaults(&mut config.defaults);
    let mut seen = BTreeSet::new();
    for site in &mut config.sites {
        site.name = site.name.trim().to_string();
        if site.name.is_empty() {
            bail!("article cleaning site name is required");
        }
        if !seen.insert(site.name.clone()) {
            bail!("duplicate article cleaning site strategy: {}", site.name);
        }
        if site.version == 0 {
            site.version = default_cleaning_strategy_version();
        }
        site.status = clean_optional(&site.status).unwrap_or_else(default_cleaning_strategy_status);
        site.url_patterns = normalize_string_list(std::mem::take(&mut site.url_patterns));
        site.source_patterns = normalize_string_list(std::mem::take(&mut site.source_patterns));
        site.preferred_selectors =
            normalize_string_list(std::mem::take(&mut site.preferred_selectors));
        site.start_markers = normalize_string_list(std::mem::take(&mut site.start_markers));
        site.end_markers = normalize_string_list(std::mem::take(&mut site.end_markers));
        site.exact_noise_lines = normalize_string_list(std::mem::take(&mut site.exact_noise_lines));
        site.contains_noise_lines =
            normalize_string_list(std::mem::take(&mut site.contains_noise_lines));
        site.line_suffix_noise = normalize_string_list(std::mem::take(&mut site.line_suffix_noise));
    }
    Ok(())
}

fn normalize_cleaning_defaults(defaults: &mut ArticleCleaningDefaults) {
    if defaults.min_kept_ratio <= 0.0 || defaults.min_kept_ratio > 1.0 {
        defaults.min_kept_ratio = default_cleaning_min_kept_ratio();
    }
    if defaults.max_kept_ratio <= 0.0 || defaults.max_kept_ratio > 1.0 {
        defaults.max_kept_ratio = default_cleaning_max_kept_ratio();
    }
    if defaults.min_normalized_chars == 0 {
        defaults.min_normalized_chars = default_cleaning_min_normalized_chars();
    }
    defaults.exact_noise_lines =
        normalize_string_list(std::mem::take(&mut defaults.exact_noise_lines));
    defaults.contains_noise_lines =
        normalize_string_list(std::mem::take(&mut defaults.contains_noise_lines));
}

fn normalize_article_value_config(value: &mut ArticleValueConfig) {
    value.provider = value.provider.trim().to_string();
    value.api_key = value.api_key.trim().to_string();
    value.base_url = value.base_url.trim().trim_end_matches('/').to_string();
    value.model = value.model.trim().to_string();
    value.target_topics = normalize_string_list(std::mem::take(&mut value.target_topics));
    if value.max_input_chars == 0 {
        value.max_input_chars = default_value_max_input_chars();
    }
    if value.min_normalized_chars == 0 {
        value.min_normalized_chars = default_value_min_normalized_chars();
    }
    if value.save_threshold <= 0.0 || value.save_threshold > 1.0 {
        value.save_threshold = default_value_save_threshold();
    }
    if value.candidate_threshold <= 0.0 || value.candidate_threshold > value.save_threshold {
        value.candidate_threshold = default_value_candidate_threshold();
    }
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .filter_map(|value| clean_optional(&value))
        .collect()
}

fn resolve_article_cleaning_strategy(
    config: &ArticleCleaningConfig,
    article: &ArticleMemoryRecord,
) -> ResolvedArticleCleaningStrategy {
    if let Some(site) = config
        .sites
        .iter()
        .find(|strategy| article_matches_strategy(article, strategy))
    {
        return ResolvedArticleCleaningStrategy {
            name: site.name.clone(),
            version: site.version,
            source: "config/davis/article_memory.toml".to_string(),
            min_kept_ratio: config.defaults.min_kept_ratio,
            max_kept_ratio: config.defaults.max_kept_ratio,
            min_normalized_chars: config.defaults.min_normalized_chars,
            start_markers: site.start_markers.clone(),
            end_markers: site.end_markers.clone(),
            exact_noise_lines: merged_lines(
                &config.defaults.exact_noise_lines,
                &site.exact_noise_lines,
            ),
            contains_noise_lines: merged_lines(
                &config.defaults.contains_noise_lines,
                &site.contains_noise_lines,
            ),
            line_suffix_noise: site.line_suffix_noise.clone(),
        };
    }

    ResolvedArticleCleaningStrategy {
        name: "generic".to_string(),
        version: 1,
        source: "config/davis/article_memory.toml".to_string(),
        min_kept_ratio: config.defaults.min_kept_ratio,
        max_kept_ratio: config.defaults.max_kept_ratio,
        min_normalized_chars: config.defaults.min_normalized_chars,
        start_markers: Vec::new(),
        end_markers: Vec::new(),
        exact_noise_lines: config.defaults.exact_noise_lines.clone(),
        contains_noise_lines: config.defaults.contains_noise_lines.clone(),
        line_suffix_noise: Vec::new(),
    }
}

fn article_matches_strategy(
    article: &ArticleMemoryRecord,
    strategy: &ArticleCleaningSiteStrategy,
) -> bool {
    let url = article.url.as_deref().unwrap_or_default().to_lowercase();
    let haystack = format!("{} {}", article.source, article.title).to_lowercase();
    strategy
        .url_patterns
        .iter()
        .any(|pattern| wildcard_match(&url, &pattern.to_lowercase()))
        || strategy
            .source_patterns
            .iter()
            .any(|pattern| haystack.contains(&pattern.to_lowercase()))
}

fn wildcard_match(value: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return value.contains(pattern);
    }
    let parts = pattern
        .split('*')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return true;
    }
    let mut offset = 0;
    for part in &parts {
        let Some(found) = value[offset..].find(part) else {
            return false;
        };
        offset += found + part.len();
    }
    true
}

fn merged_lines(defaults: &[String], site: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    defaults
        .iter()
        .chain(site.iter())
        .filter_map(|item| {
            if seen.insert(item.to_lowercase()) {
                Some(item.clone())
            } else {
                None
            }
        })
        .collect()
}

fn normalize_article_text(
    article: &ArticleMemoryRecord,
    raw_text: &str,
    strategy: &ResolvedArticleCleaningStrategy,
) -> NormalizedArticleText {
    let mut lines = Vec::new();
    let mut seen = BTreeSet::new();
    let mut noise_lines_removed = 0;
    let mut removed_lines_sample = Vec::new();
    let prepared = prepare_raw_text_for_normalization(raw_text, strategy);
    for raw_line in raw_text_units(&prepared.text) {
        let line = normalize_line(raw_line);
        if line.is_empty() {
            if !lines.last().is_some_and(|item: &String| item.is_empty()) {
                lines.push(String::new());
            }
            continue;
        }
        if is_noise_line(&line, strategy) {
            noise_lines_removed += 1;
            if removed_lines_sample.len() < 20 {
                removed_lines_sample.push(line);
            }
            continue;
        }
        if line.chars().count() <= 3 {
            continue;
        }
        let dedupe_key = line.to_lowercase();
        if line.chars().count() < 80 && !seen.insert(dedupe_key) {
            continue;
        }
        lines.push(line);
    }
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    let mut output = Vec::new();
    output.push("---".to_string());
    output.push(format!("title: {}", yaml_scalar(&article.title)));
    if let Some(url) = &article.url {
        output.push(format!("url: {}", yaml_scalar(url)));
    }
    output.push(format!("source: {}", yaml_scalar(&article.source)));
    if let Some(language) = &article.language {
        output.push(format!("language: {}", yaml_scalar(language)));
    }
    output.push(format!(
        "captured_at: {}",
        yaml_scalar(&article.captured_at)
    ));
    output.push(format!(
        "status: {}",
        yaml_scalar(&article.status.to_string())
    ));
    if !article.tags.is_empty() {
        output.push(format!(
            "tags: [{}]",
            article
                .tags
                .iter()
                .map(|tag| yaml_scalar(tag))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    output.push("---".to_string());
    output.push(String::new());
    output.push(format!("# {}", article.title.trim()));
    output.push(String::new());
    output.extend(lines);
    let markdown = output.join("\n");
    let leftover_noise_candidates = detect_leftover_noise(&markdown, strategy);
    NormalizedArticleText {
        markdown,
        prepared_chars: prepared.text.chars().count(),
        removed_start_chars: prepared.removed_start_chars,
        removed_end_chars: prepared.removed_end_chars,
        removed_start_preview: prepared.removed_start_preview,
        removed_end_preview: prepared.removed_end_preview,
        noise_lines_removed,
        removed_lines_sample,
        leftover_noise_candidates,
    }
}

fn prepare_raw_text_for_normalization(
    raw_text: &str,
    strategy: &ResolvedArticleCleaningStrategy,
) -> PreparedArticleText {
    let mut text = raw_text.replace('\u{00a0}', " ");
    let mut removed_start = String::new();
    let mut removed_end = String::new();
    for marker in &strategy.start_markers {
        if let Some(position) = text.find(marker) {
            removed_start = text[..position + marker.len()].trim().to_string();
            text = text[position + marker.len()..].trim().to_string();
            break;
        }
    }
    if let Some(end_position) = strategy
        .end_markers
        .iter()
        .filter_map(|marker| text.find(marker))
        .min()
    {
        removed_end = text[end_position..].trim().to_string();
        text.truncate(end_position);
    }
    PreparedArticleText {
        text,
        removed_start_chars: removed_start.chars().count(),
        removed_end_chars: removed_end.chars().count(),
        removed_start_preview: preview_text(&removed_start),
        removed_end_preview: preview_text(&removed_end),
    }
}

fn raw_text_units(raw_text: &str) -> Vec<String> {
    raw_text
        .lines()
        .flat_map(|line| {
            if line.chars().count() > 800 {
                split_long_raw_line(line)
            } else {
                vec![line.to_string()]
            }
        })
        .collect()
}

fn split_long_raw_line(line: &str) -> Vec<String> {
    let normalized = normalize_line(line);
    let mut units = Vec::new();
    let mut current = String::new();
    for character in normalized.chars() {
        current.push(character);
        let current_len = current.chars().count();
        if is_sentence_boundary(character) && current_len >= 24 {
            units.push(current.trim().to_string());
            current.clear();
        } else if character == ' ' && current_len >= 220 {
            units.push(current.trim().to_string());
            current.clear();
        }
    }
    if !current.trim().is_empty() {
        units.push(current.trim().to_string());
    }
    units
}

fn is_sentence_boundary(character: char) -> bool {
    matches!(
        character,
        '。' | '！' | '？' | '；' | '…' | '.' | '!' | '?' | ';'
    )
}

fn normalize_line(line: impl AsRef<str>) -> String {
    let line = line.as_ref();
    line.replace('\u{00a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn is_noise_line(line: &str, strategy: &ResolvedArticleCleaningStrategy) -> bool {
    let lowered = line.to_lowercase();
    if strategy
        .exact_noise_lines
        .iter()
        .any(|needle| lowered == needle.to_lowercase())
        || strategy
            .contains_noise_lines
            .iter()
            .any(|needle| lowered.contains(&needle.to_lowercase()))
        || strategy
            .line_suffix_noise
            .iter()
            .any(|needle| lowered.ends_with(&needle.to_lowercase()))
    {
        return true;
    }
    if line.starts_with('(') && line.contains("封私信") && line.contains("条消息") {
        return true;
    }
    false
}

fn detect_leftover_noise(
    markdown: &str,
    strategy: &ResolvedArticleCleaningStrategy,
) -> Vec<String> {
    let lowered = markdown.to_lowercase();
    strategy
        .exact_noise_lines
        .iter()
        .chain(strategy.contains_noise_lines.iter())
        .filter(|needle| lowered.contains(&needle.to_lowercase()))
        .take(20)
        .cloned()
        .collect()
}

fn preview_text(text: &str) -> Option<String> {
    let cleaned = normalize_line(text);
    (!cleaned.is_empty()).then(|| cleaned.chars().take(240).collect())
}

fn yaml_scalar(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

async fn polish_markdown(
    config: &ResolvedArticleNormalizeConfig,
    markdown: &str,
) -> Result<String> {
    let system = "You format extracted article text into faithful Markdown. Do not add facts, opinions, explanations, or new content. Preserve the source language, commands, URLs, code, names, and all substantive claims. Remove leftover UI/navigation noise only when obvious.";
    let user = format!(
        "Format this article as clean Markdown. Preserve meaning and substance. Return only Markdown.\n\n{markdown}"
    );
    create_chat_completion(config, system, &user, 12_000).await
}

async fn summarize_markdown(
    config: &ResolvedArticleNormalizeConfig,
    markdown: &str,
) -> Result<String> {
    let system = "You write concise Chinese summaries of saved articles. Do not invent facts. Mention uncertainty and caveats when present.";
    let user = format!(
        "请基于下面文章生成中文摘要，固定使用这个 Markdown 结构：\n# Summary\n\n## One-Sentence Takeaway\n\n...\n\n## Key Points\n\n- ...\n\n## Practical Value\n\n...\n\n## Caveats\n\n...\n\n文章：\n\n{markdown}"
    );
    create_chat_completion(config, system, &user, 2_000).await
}

async fn create_chat_completion(
    config: &ResolvedArticleNormalizeConfig,
    system: &str,
    user: &str,
    max_tokens: usize,
) -> Result<String> {
    let endpoint = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let payload = json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "temperature": 0.1,
        "max_tokens": max_tokens
    });
    let response = client
        .post(endpoint)
        .bearer_auth(&config.api_key)
        .json(&payload)
        .send()
        .await
        .context("chat completion request failed")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| String::from("<failed to read response>"));
    if !status.is_success() {
        bail!("chat completion failed with HTTP {status}: {body}");
    }
    let value: serde_json::Value =
        serde_json::from_str(&body).context("chat completion response was not valid JSON")?;
    value
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(|content| content.trim().to_string())
        .filter(|content| !content.is_empty())
        .ok_or_else(|| anyhow!("chat completion response did not contain message content"))
}

async fn create_chat_completion_for_value(
    config: &ResolvedArticleValueConfig,
    system: &str,
    user: &str,
    max_tokens: usize,
) -> Result<String> {
    let endpoint = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let payload = json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ],
        "temperature": 0.0,
        "max_tokens": max_tokens
    });
    let response = client
        .post(endpoint)
        .bearer_auth(&config.api_key)
        .json(&payload)
        .send()
        .await
        .context("value judge request failed")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| String::from("<failed to read response>"));
    if !status.is_success() {
        bail!("value judge failed with HTTP {status}: {body}");
    }
    let value: serde_json::Value =
        serde_json::from_str(&body).context("value judge response was not valid JSON")?;
    value
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(|content| content.trim().to_string())
        .filter(|content| !content.is_empty())
        .ok_or_else(|| anyhow!("value judge response did not contain message content"))
}

fn polished_is_valid(normalized: &str, polished: &str, fallback_min_ratio: f32) -> bool {
    let normalized_chars = normalized.chars().count() as f32;
    let polished_chars = polished.chars().count() as f32;
    if normalized_chars < 1.0 || polished_chars < 1.0 {
        return false;
    }
    polished_chars / normalized_chars >= fallback_min_ratio
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_add_and_search_article_memory() {
        let paths = test_paths("init_add_and_search_article_memory");
        let status = init_article_memory(&paths).unwrap();
        assert_eq!(status.status, "ok");
        assert_eq!(status.total_articles, 0);

        let record = add_article_memory(
            &paths,
            ArticleMemoryAddRequest {
                title: "A useful agent memory article".to_string(),
                url: Some("https://example.com/agent-memory".to_string()),
                source: "manual".to_string(),
                language: Some("en".to_string()),
                tags: vec!["agent".to_string(), "memory".to_string()],
                content: "This article explains durable memory for agents.".to_string(),
                summary: Some("Durable agent memory patterns.".to_string()),
                translation: None,
                status: ArticleMemoryRecordStatus::Saved,
                value_score: Some(0.9),
                notes: Some("Keep for Davis article memory tests.".to_string()),
            },
        )
        .unwrap();

        assert_eq!(record.title, "A useful agent memory article");
        assert!(paths
            .article_memory_dir()
            .join(&record.content_path)
            .is_file());

        let search = search_article_memory(&paths, "durable", 10);
        assert_eq!(search.status, "ok");
        assert_eq!(search.total_hits, 1);
        assert_eq!(search.hits[0].title, "A useful agent memory article");
        assert!(search.hits[0]
            .matched_fields
            .iter()
            .any(|field| field == "content" || field == "summary"));

        let _ = fs::remove_dir_all(paths.repo_root);
    }

    #[tokio::test]
    async fn normalize_article_memory_writes_raw_normalized_and_final_files() {
        let paths = test_paths("normalize_article_memory_writes_files");
        init_article_memory(&paths).unwrap();

        let record = add_article_memory(
            &paths,
            ArticleMemoryAddRequest {
                title: "知乎 Claude Code 入门".to_string(),
                url: Some("https://www.zhihu.com/question/1/answer/2".to_string()),
                source: "browser".to_string(),
                language: Some("zh-CN".to_string()),
                tags: vec!["agent".to_string()],
                content: "知乎\n登录\n\nClaude Code 可以通过反复实践学习。\nClaude Code 可以通过反复实践学习。\n\n保留这一段重要内容。".to_string(),
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Candidate,
                value_score: None,
                notes: None,
            },
        )
        .unwrap();

        let response = normalize_article_memory(&paths, None, None, &record.id)
            .await
            .unwrap();

        assert_eq!(response.status, "ok");
        assert_eq!(response.clean_profile, "zhihu");
        assert_eq!(response.clean_status, "ok");
        assert!(std::path::Path::new(&response.raw_path).is_file());
        assert!(std::path::Path::new(&response.normalized_path).is_file());
        assert!(std::path::Path::new(&response.content_path).is_file());
        assert!(std::path::Path::new(&response.clean_report_path).is_file());

        let normalized = fs::read_to_string(&response.normalized_path).unwrap();
        assert!(normalized.contains("title: \"知乎 Claude Code 入门\""));
        assert!(normalized.contains("# 知乎 Claude Code 入门"));
        assert!(normalized.contains("Claude Code 可以通过反复实践学习。"));
        assert!(normalized.contains("保留这一段重要内容。"));
        assert!(!normalized.contains("\n登录\n"));

        let index = load_index(&paths).unwrap();
        let updated = index
            .articles
            .iter()
            .find(|article| article.id == record.id)
            .unwrap();
        let expected_raw_path = format!("articles/{}.raw.txt", record.id);
        let expected_normalized_path = format!("articles/{}.normalized.md", record.id);
        assert_eq!(
            updated.raw_path.as_deref(),
            Some(expected_raw_path.as_str())
        );
        assert_eq!(
            updated.normalized_path.as_deref(),
            Some(expected_normalized_path.as_str())
        );
        assert_eq!(updated.clean_status.as_deref(), Some("ok"));
        assert_eq!(updated.clean_profile.as_deref(), Some("zhihu"));

        let report: ArticleCleanReport =
            serde_json::from_str(&fs::read_to_string(&response.clean_report_path).unwrap())
                .unwrap();
        assert_eq!(report.article_id, record.id);
        assert_eq!(report.strategy_name, "zhihu");
        assert_eq!(report.noise_lines_removed, 2);

        let _ = fs::remove_dir_all(paths.repo_root);
    }

    #[tokio::test]
    async fn normalize_article_memory_splits_long_single_line_browser_text() {
        let paths = test_paths("normalize_article_memory_splits_long_line");
        init_article_memory(&paths).unwrap();
        let repeated_body = (0..40)
            .map(|index| {
                format!(
                    "Claude Code 的第 {index} 个学习要点是让 agent 能直接理解项目、修改文件并运行验证。"
                )
            })
            .collect::<Vec<_>>()
            .join("");
        let content = format!(
            "知乎 登录 分享 初学者如何快速入门学会Claude Code ？ 关注问题 45 人赞同了该回答 目录 收起 {repeated_body} 所属专栏 AI大模型实用手册 更多回答 这不是当前回答"
        );

        let record = add_article_memory(
            &paths,
            ArticleMemoryAddRequest {
                title: "初学者如何快速入门学会Claude Code ？".to_string(),
                url: Some("https://www.zhihu.com/question/1/answer/2".to_string()),
                source: "知乎回答".to_string(),
                language: Some("zh".to_string()),
                tags: Vec::new(),
                content,
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Candidate,
                value_score: None,
                notes: None,
            },
        )
        .unwrap();

        let response = normalize_article_memory(&paths, None, None, &record.id)
            .await
            .unwrap();

        assert_ne!(response.clean_status, "fallback_raw");
        assert!(response.normalized_chars > 1_000);
        assert!(std::path::Path::new(&response.clean_report_path).is_file());
        let normalized = fs::read_to_string(&response.normalized_path).unwrap();
        assert!(normalized.contains("Claude Code 的第 0 个学习要点"));
        assert!(!normalized.contains("关注问题"));
        assert!(!normalized.contains("所属专栏"));
        assert!(!normalized.contains("这不是当前回答"));

        let _ = fs::remove_dir_all(paths.repo_root);
    }

    #[tokio::test]
    async fn value_judge_rejects_off_topic_articles_before_polish() {
        let paths = test_paths("value_judge_rejects_off_topic_articles");
        init_article_memory(&paths).unwrap();
        let record = add_article_memory(
            &paths,
            ArticleMemoryAddRequest {
                title: "一篇厨房收纳技巧".to_string(),
                url: Some("https://example.com/kitchen".to_string()),
                source: "manual".to_string(),
                language: Some("zh".to_string()),
                tags: Vec::new(),
                content:
                    "这篇文章讨论厨房抽屉收纳、标签分类和餐具摆放。内容很完整，但和智能体学习无关。"
                        .repeat(20),
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Candidate,
                value_score: None,
                notes: None,
            },
        )
        .unwrap();
        let value_config = ResolvedArticleValueConfig {
            provider: "test".to_string(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            llm_judge: false,
            max_input_chars: 2000,
            min_normalized_chars: 20,
            save_threshold: 0.75,
            candidate_threshold: 0.45,
            target_topics: vec!["AI agent".to_string(), "MCP".to_string()],
        };

        let response = normalize_article_memory(&paths, None, Some(&value_config), &record.id)
            .await
            .unwrap();

        assert_eq!(response.value_decision.as_deref(), Some("reject"));
        assert_eq!(response.clean_status, "rejected");
        assert!(response
            .value_report_path
            .as_deref()
            .is_some_and(|path| { std::path::Path::new(path).is_file() }));
        let index = load_index(&paths).unwrap();
        let updated = index
            .articles
            .iter()
            .find(|article| article.id == record.id)
            .unwrap();
        assert_eq!(updated.status, ArticleMemoryRecordStatus::Rejected);

        let _ = fs::remove_dir_all(paths.repo_root);
    }

    #[tokio::test]
    async fn strategy_review_input_writes_bounded_agent_context() {
        let paths = test_paths("strategy_review_input_writes_context");
        init_article_memory(&paths).unwrap();
        let record = add_article_memory(
            &paths,
            ArticleMemoryAddRequest {
                title: "Claude Code agent workflow".to_string(),
                url: Some("https://example.com/agent-workflow".to_string()),
                source: "manual".to_string(),
                language: Some("en".to_string()),
                tags: vec!["agent".to_string()],
                content: "Claude Code agent workflow with memory and tool use. ".repeat(30),
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Candidate,
                value_score: None,
                notes: None,
            },
        )
        .unwrap();
        let value_config = ResolvedArticleValueConfig {
            provider: "test".to_string(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            llm_judge: false,
            max_input_chars: 2000,
            min_normalized_chars: 20,
            save_threshold: 0.75,
            candidate_threshold: 0.45,
            target_topics: vec!["agent".to_string(), "memory".to_string()],
        };
        normalize_article_memory(&paths, None, Some(&value_config), &record.id)
            .await
            .unwrap();

        let response = build_article_strategy_review_input(&paths, 5).unwrap();

        assert!(std::path::Path::new(&response.report_path).is_file());
        assert!(paths.article_memory_implementation_requests_dir().is_dir());
        assert!(response.markdown.contains("You may edit only"));
        assert!(response.markdown.contains("Do not edit Rust source"));
        assert!(response.markdown.contains(&record.id));
        assert!(response
            .markdown
            .contains("Article Memory Strategy Review Input"));

        let _ = fs::remove_dir_all(paths.repo_root);
    }

    #[test]
    fn check_article_cleaning_loads_builtin_strategy_when_config_is_missing() {
        let paths = test_paths("check_article_cleaning_loads_builtin_strategy");
        let response = check_article_cleaning(&paths).unwrap();

        assert_eq!(response.status, "ok");
        assert!(response.sites.iter().any(|site| site == "zhihu"));

        let _ = fs::remove_dir_all(paths.repo_root);
    }

    #[test]
    fn rejects_invalid_value_score() {
        let paths = test_paths("rejects_invalid_value_score");
        init_article_memory(&paths).unwrap();
        let error = add_article_memory(
            &paths,
            ArticleMemoryAddRequest {
                title: "Bad score".to_string(),
                url: None,
                source: "manual".to_string(),
                language: None,
                tags: Vec::new(),
                content: "content".to_string(),
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Saved,
                value_score: Some(1.5),
                notes: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("value_score"));

        let _ = fs::remove_dir_all(paths.repo_root);
    }

    #[test]
    fn resolves_embedding_config_from_provider() {
        let embedding = ArticleMemoryEmbeddingConfig {
            enabled: true,
            provider: "siliconflow".to_string(),
            api_key: String::new(),
            base_url: String::new(),
            model: "Qwen/Qwen3-Embedding-8B".to_string(),
            dimensions: 1024,
            max_input_chars: 12000,
        };
        let providers = vec![ModelProviderConfig {
            name: "siliconflow".to_string(),
            api_key: "test-key".to_string(),
            base_url: "https://api.siliconflow.cn/v1".to_string(),
            allowed_models: vec!["some-chat-model".to_string()],
        }];

        let resolved = resolve_article_embedding_config(&embedding, &providers)
            .unwrap()
            .unwrap();

        assert_eq!(resolved.provider, "siliconflow");
        assert_eq!(resolved.api_key, "test-key");
        assert_eq!(resolved.base_url, "https://api.siliconflow.cn/v1");
        assert_eq!(resolved.model, "Qwen/Qwen3-Embedding-8B");
    }

    fn test_paths(name: &str) -> RuntimePaths {
        let root = std::env::temp_dir().join(format!(
            "daviszeroclaw-article-memory-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        RuntimePaths {
            repo_root: root.clone(),
            runtime_dir: root.join("runtime"),
        }
    }
}
