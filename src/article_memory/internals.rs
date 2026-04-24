use super::cleaning_internals::truncate_chars;
use super::types::*;
use super::{ARTICLE_MEMORY_EMBEDDINGS_VERSION, ARTICLE_MEMORY_INDEX_VERSION};
use crate::RuntimePaths;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

pub(super) fn ensure_article_memory_dirs(paths: &RuntimePaths) -> Result<()> {
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

pub(crate) fn load_index(paths: &RuntimePaths) -> Result<ArticleMemoryIndex> {
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

pub(super) fn load_embedding_index(paths: &RuntimePaths) -> Result<ArticleMemoryEmbeddingIndex> {
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

pub(crate) fn write_index(paths: &RuntimePaths, index: &ArticleMemoryIndex) -> Result<()> {
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

pub(super) fn write_embedding_index(
    paths: &RuntimePaths,
    index: &ArticleMemoryEmbeddingIndex,
) -> Result<()> {
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

pub(super) fn build_missing_status(paths: &RuntimePaths) -> ArticleMemoryStatusResponse {
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

pub(super) fn build_error_status(
    paths: &RuntimePaths,
    message: String,
) -> ArticleMemoryStatusResponse {
    ArticleMemoryStatusResponse {
        message: Some(message),
        status: "error".to_string(),
        ..build_missing_status(paths)
    }
}

pub(super) fn build_status_response(
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

pub(super) fn score_record(
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

pub(super) fn compare_hits(a: &ArticleMemorySearchHit, b: &ArticleMemorySearchHit) -> Ordering {
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

pub(super) fn compare_hybrid_hits(
    a: &ArticleMemorySearchHit,
    b: &ArticleMemorySearchHit,
) -> Ordering {
    b.combined_score
        .partial_cmp(&a.combined_score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| compare_hits(a, b))
}

pub(super) fn record_to_search_hit(
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

pub(super) fn with_semantic_status(
    mut response: ArticleMemorySearchResponse,
    semantic_status: &str,
) -> ArticleMemorySearchResponse {
    response.semantic_status = Some(semantic_status.to_string());
    response
}

pub(super) fn article_embedding_text(
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

pub(super) async fn create_embedding(
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

pub(super) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
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

pub(super) fn first_non_empty(primary: &str, fallback: &str) -> String {
    if !primary.trim().is_empty() {
        primary.trim().to_string()
    } else {
        fallback.trim().to_string()
    }
}

pub(super) fn text_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn read_article_text(paths: &RuntimePaths, relative_path: &str) -> Result<String> {
    fs::read_to_string(resolve_article_path(paths, relative_path))
        .with_context(|| format!("failed to read article text: {relative_path}"))
}

pub(super) fn read_article_raw_text(
    paths: &RuntimePaths,
    article: &ArticleMemoryRecord,
) -> Result<String> {
    if let Some(raw_path) = &article.raw_path {
        if let Ok(text) = read_article_text(paths, raw_path) {
            return Ok(text);
        }
    }
    read_article_text(paths, &article.content_path)
}

pub(super) fn resolve_article_path(paths: &RuntimePaths, relative_path: &str) -> PathBuf {
    paths.article_memory_dir().join(relative_path)
}

pub(super) fn article_id(title: &str, url: Option<&str>, captured_at: &str) -> String {
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

pub(super) fn clean_required(field: &str, value: &str) -> Result<String> {
    clean_optional(value).ok_or_else(|| anyhow!("{field} is required"))
}

pub(super) fn clean_optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(super) fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    tags.into_iter()
        .filter_map(|tag| clean_optional(&tag))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn normalize_score(score: Option<f32>) -> Result<Option<f32>> {
    match score {
        Some(value) if !(0.0..=1.0).contains(&value) => {
            bail!("value_score must be between 0.0 and 1.0")
        }
        value => Ok(value),
    }
}

pub(super) fn normalize_limit(limit: usize) -> usize {
    limit.clamp(1, 100)
}

pub(super) fn contains_case_insensitive(value: &str, needle_lowercase: &str) -> bool {
    value.to_lowercase().contains(needle_lowercase)
}

pub(super) fn make_snippet(text: &str, query: &str) -> String {
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
pub(super) struct PreparedArticleText {
    pub(super) text: String,
    pub(super) removed_start_chars: usize,
    pub(super) removed_end_chars: usize,
    pub(super) removed_start_preview: Option<String>,
    pub(super) removed_end_preview: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct NormalizedArticleText {
    pub(super) markdown: String,
    pub(super) prepared_chars: usize,
    pub(super) removed_start_chars: usize,
    pub(super) removed_end_chars: usize,
    pub(super) removed_start_preview: Option<String>,
    pub(super) removed_end_preview: Option<String>,
    pub(super) noise_lines_removed: usize,
    pub(super) removed_lines_sample: Vec<String>,
    pub(super) leftover_noise_candidates: Vec<String>,
}
