use super::*;
use crate::support::{isoformat, now_utc};
use crate::RuntimePaths;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

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

pub(super) fn build_clean_report(
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

pub(super) fn deterministic_clean_status(
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
