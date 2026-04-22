use super::*;
use crate::RuntimePaths;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;

pub(super) fn load_article_cleaning_config(paths: &RuntimePaths) -> Result<ArticleCleaningConfig> {
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

pub(super) fn normalize_article_cleaning_config(config: &mut ArticleCleaningConfig) -> Result<()> {
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

pub(super) fn normalize_cleaning_defaults(defaults: &mut ArticleCleaningDefaults) {
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

pub(super) fn normalize_article_value_config(value: &mut ArticleValueConfig) {
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

pub(super) fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .filter_map(|value| clean_optional(&value))
        .collect()
}

pub(super) fn resolve_article_cleaning_strategy(
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

pub(super) fn article_matches_strategy(
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

pub(super) fn wildcard_match(value: &str, pattern: &str) -> bool {
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

pub(super) fn merged_lines(defaults: &[String], site: &[String]) -> Vec<String> {
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

pub(super) fn normalize_article_text(
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

pub(super) fn prepare_raw_text_for_normalization(
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

pub(super) fn raw_text_units(raw_text: &str) -> Vec<String> {
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

pub(super) fn split_long_raw_line(line: &str) -> Vec<String> {
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

pub(super) fn is_sentence_boundary(character: char) -> bool {
    matches!(
        character,
        '。' | '！' | '？' | '；' | '…' | '.' | '!' | '?' | ';'
    )
}

pub(super) fn normalize_line(line: impl AsRef<str>) -> String {
    let line = line.as_ref();
    line.replace('\u{00a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

pub(super) fn is_noise_line(line: &str, strategy: &ResolvedArticleCleaningStrategy) -> bool {
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

pub(super) fn detect_leftover_noise(
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

pub(super) fn preview_text(text: &str) -> Option<String> {
    let cleaned = normalize_line(text);
    (!cleaned.is_empty()).then(|| cleaned.chars().take(240).collect())
}

pub(super) fn yaml_scalar(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(super) async fn polish_markdown(
    config: &ResolvedArticleNormalizeConfig,
    markdown: &str,
) -> Result<String> {
    let system = "You format extracted article text into faithful Markdown. Do not add facts, opinions, explanations, or new content. Preserve the source language, commands, URLs, code, names, and all substantive claims. Remove leftover UI/navigation noise only when obvious.";
    let user = format!(
        "Format this article as clean Markdown. Preserve meaning and substance. Return only Markdown.\n\n{markdown}"
    );
    create_chat_completion(config, system, &user, 12_000).await
}

pub(super) async fn summarize_markdown(
    config: &ResolvedArticleNormalizeConfig,
    markdown: &str,
) -> Result<String> {
    let system = "You write concise Chinese summaries of saved articles. Do not invent facts. Mention uncertainty and caveats when present.";
    let user = format!(
        "请基于下面文章生成中文摘要，固定使用这个 Markdown 结构：\n# Summary\n\n## One-Sentence Takeaway\n\n...\n\n## Key Points\n\n- ...\n\n## Practical Value\n\n...\n\n## Caveats\n\n...\n\n文章：\n\n{markdown}"
    );
    create_chat_completion(config, system, &user, 2_000).await
}

pub(super) async fn create_chat_completion(
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

pub(super) async fn create_chat_completion_for_value(
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

pub(super) fn polished_is_valid(normalized: &str, polished: &str, fallback_min_ratio: f32) -> bool {
    let normalized_chars = normalized.chars().count() as f32;
    let polished_chars = polished.chars().count() as f32;
    if normalized_chars < 1.0 || polished_chars < 1.0 {
        return false;
    }
    polished_chars / normalized_chars >= fallback_min_ratio
}

pub(super) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}
