use crate::support::{isoformat, now_utc};
use crate::RuntimePaths;
use anyhow::{bail, Context, Result};
use std::fs;

const ARTICLE_MEMORY_INDEX_VERSION: u32 = 1;
const ARTICLE_MEMORY_EMBEDDINGS_VERSION: u32 = 1;
const BUILTIN_ARTICLE_MEMORY_POLICY_CONFIG: &str =
    include_str!("../../config/davis/article_memory.toml");

mod types;
pub use types::*;

mod config;
pub use config::*;

pub fn init_article_memory(paths: &RuntimePaths) -> Result<ArticleMemoryStatusResponse> {
    ensure_article_memory_dirs(paths)?;
    if !paths.article_memory_index_path().is_file() {
        write_index(paths, &ArticleMemoryIndex::new())?;
    }
    migrate_urls_to_normalized(paths)?;
    merge_duplicate_urls(paths)?;
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
    // Phase 1: per-site [[sites]] strategies were deleted. This call now
    // just validates that the config file parses and the [defaults] block
    // is present. Warnings stay as an open-ended channel for future
    // config-level checks.
    let _config = load_article_cleaning_config(paths)?;
    let warnings: Vec<String> = Vec::new();
    Ok(ArticleCleaningCheckResponse {
        status: if warnings.is_empty() { "ok" } else { "warn" }.to_string(),
        config_path: paths.article_cleaning_config_path().display().to_string(),
        warnings,
    })
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

/// Update an existing article record in place, reusing `override_id`.
/// Writes new content/summary/translation files under the same id, then
/// atomically rewrites the index. Fails if `override_id` is not present
/// in the index.
pub fn add_article_memory_override(
    paths: &RuntimePaths,
    request: ArticleMemoryAddRequest,
    override_id: &str,
) -> Result<ArticleMemoryRecord> {
    ensure_article_memory_dirs(paths)?;

    let title = clean_required("title", &request.title)?;
    let content = clean_required("content", &request.content)?;
    let source = clean_optional(&request.source).unwrap_or_else(|| "manual".to_string());
    let now = isoformat(now_utc());

    let content_path = format!("articles/{override_id}.md");
    let raw_path = format!("articles/{override_id}.raw.txt");
    let normalized_path = format!("articles/{override_id}.normalized.md");
    let summary_path = request
        .summary
        .as_deref()
        .and_then(clean_optional)
        .map(|_| format!("articles/{override_id}.summary.md"));
    let translation_path = request
        .translation
        .as_deref()
        .and_then(clean_optional)
        .map(|_| format!("articles/{override_id}.translation.md"));

    fs::write(resolve_article_path(paths, &raw_path), &content)
        .with_context(|| format!("failed to write article raw content for {override_id}"))?;
    fs::write(resolve_article_path(paths, &normalized_path), &content)
        .with_context(|| format!("failed to write article normalized content for {override_id}"))?;
    fs::write(resolve_article_path(paths, &content_path), &content)
        .with_context(|| format!("failed to write article content for {override_id}"))?;
    if let (Some(summary), Some(path)) = (request.summary.as_deref(), summary_path.as_deref()) {
        fs::write(resolve_article_path(paths, path), summary.trim())
            .with_context(|| format!("failed to write article summary for {override_id}"))?;
    }
    if let (Some(translation), Some(path)) =
        (request.translation.as_deref(), translation_path.as_deref())
    {
        fs::write(resolve_article_path(paths, path), translation.trim())
            .with_context(|| format!("failed to write article translation for {override_id}"))?;
    }

    let mut index = internals::load_index(paths)?;
    let idx = index
        .articles
        .iter()
        .position(|r| r.id == override_id)
        .ok_or_else(|| anyhow::anyhow!("article_id {override_id} not in index"))?;

    let replacement = ArticleMemoryRecord {
        id: override_id.to_string(),
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
    index.articles[idx] = replacement.clone();
    index.updated_at = isoformat(now_utc());
    internals::write_index(paths, &index)?;
    Ok(replacement)
}

/// One-time startup pass: rewrites `record.url` to its normalized form so
/// `find_article_by_normalized_url` can match it. Idempotent — running it
/// twice is a no-op because `normalize_url` is a fixpoint. Returns the
/// number of records that changed.
pub fn migrate_urls_to_normalized(paths: &RuntimePaths) -> Result<usize> {
    let mut index = internals::load_index(paths)?;
    let mut changed = 0usize;
    for article in &mut index.articles {
        let Some(url) = article.url.as_ref() else {
            continue;
        };
        let normalized = normalize_url(url).unwrap_or_else(|_| url.clone());
        if normalized != *url {
            article.url = Some(normalized);
            changed += 1;
        }
    }
    if changed > 0 {
        index.updated_at = isoformat(now_utc());
        internals::write_index(paths, &index)?;
        tracing::info!(count = changed, "migrated article URLs to normalized form");
    }
    Ok(changed)
}

/// Merge duplicate article records sharing the same `url`. Winner selection:
/// (1) higher `value_score`, (2) later `captured_at`, (3) first-seen. Loser
/// record is removed from the index and its on-disk content/summary/raw/
/// normalized/translation/embedding files are deleted. No backup is kept —
/// one-way cleanup per Q11 D+A policy.
pub fn merge_duplicate_urls(paths: &RuntimePaths) -> Result<usize> {
    use std::collections::HashMap;

    let mut index = internals::load_index(paths)?;
    if index.articles.is_empty() {
        return Ok(0);
    }

    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, article) in index.articles.iter().enumerate() {
        if let Some(url) = &article.url {
            groups.entry(url.clone()).or_default().push(i);
        }
    }

    let mut losers: Vec<usize> = Vec::new();
    for indices in groups.values() {
        if indices.len() < 2 {
            continue;
        }
        let mut best = indices[0];
        for &candidate in &indices[1..] {
            let best_rec = &index.articles[best];
            let cand_rec = &index.articles[candidate];
            let best_score = best_rec.value_score.unwrap_or(0.0);
            let cand_score = cand_rec.value_score.unwrap_or(0.0);
            let pick_candidate = if cand_score > best_score {
                true
            } else if cand_score < best_score {
                false
            } else {
                cand_rec.captured_at > best_rec.captured_at
            };
            if pick_candidate {
                best = candidate;
            }
        }
        for &i in indices {
            if i != best {
                losers.push(i);
            }
        }
    }

    if losers.is_empty() {
        return Ok(0);
    }

    let articles_dir = paths.runtime_dir.join("article-memory").join("articles");
    let embeddings_dir = paths.runtime_dir.join("article-memory").join("embeddings");

    let loser_ids: Vec<String> = losers
        .iter()
        .map(|&i| index.articles[i].id.clone())
        .collect();

    let mut sorted_losers = losers;
    sorted_losers.sort_unstable_by(|a, b| b.cmp(a));
    for i in sorted_losers {
        let dropped = index.articles.remove(i);
        tracing::info!(
            dropped_id = %dropped.id,
            url = %dropped.url.unwrap_or_default(),
            "merging duplicate article: dropping loser"
        );
    }

    index.updated_at = isoformat(now_utc());
    internals::write_index(paths, &index)?;

    for id in &loser_ids {
        for ext in [
            "md",
            "raw.txt",
            "normalized.md",
            "summary.md",
            "translation.md",
        ] {
            let p = articles_dir.join(format!("{id}.{ext}"));
            let _ = fs::remove_file(&p);
        }
        let _ = fs::remove_file(embeddings_dir.join(format!("{id}.bin")));
    }

    Ok(loser_ids.len())
}

/// Linear-scan lookup by canonical URL. Returns the first record whose
/// `url` field equals `normalized_url`.
///
/// Callers normalize via `ingest::host_profile::normalize_url` before
/// calling — storing a mix of raw and normalized URLs in the index would
/// cause dedup to miss (same logical URL, different string bytes).
pub fn find_article_by_normalized_url(
    paths: &RuntimePaths,
    normalized_url: &str,
) -> Result<Option<ArticleMemoryRecord>> {
    let index = internals::load_index(paths)?;
    Ok(index
        .articles
        .into_iter()
        .find(|r| r.url.as_deref() == Some(normalized_url)))
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

mod reports;
pub use reports::*;

mod pipeline;
pub use pipeline::*;

mod embedding;
pub use embedding::*;

pub(crate) mod internals;
use internals::*;

// Public wrappers around the crate-private `internals::{load_index, write_index}`
// so integration tests (tests/rust/topic_crawl_translate.rs) can seed the
// article memory index from outside the crate without promoting the raw
// internals to `pub`. Keeping `internals` at `pub(crate)` preserves the
// intentional encapsulation for non-test callers.
pub fn load_article_index(paths: &RuntimePaths) -> Result<ArticleMemoryIndex> {
    internals::load_index(paths)
}

pub fn save_article_index(paths: &RuntimePaths, index: &ArticleMemoryIndex) -> Result<()> {
    internals::write_index(paths, index)
}

pub(crate) mod llm_client;

mod cleaning_internals;
use cleaning_internals::*;

pub mod discovery;
pub(crate) mod ingest;
pub(crate) mod mempalace_projection;
mod pii_scrub;
pub mod refresh;
pub mod translate;
// Consumed starting Task 4; remove allow once consumers land.
#[allow(unused_imports)]
pub use ingest::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_config::{ArticleMemoryEmbeddingConfig, ModelProviderConfig};

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
        assert_eq!(response.clean_profile, "default");
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
        assert_eq!(updated.clean_profile.as_deref(), Some("default"));

        let report: ArticleCleanReport =
            serde_json::from_str(&fs::read_to_string(&response.clean_report_path).unwrap())
                .unwrap();
        assert_eq!(report.article_id, record.id);
        assert_eq!(report.strategy_name, "default");
        // With site-specific rules gone, only `登录` in defaults.exact_noise_lines
        // matches; the duplicated sentence is removed by dedup, not noise.
        assert_eq!(report.noise_lines_removed, 1);

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
        // Core guarantee we still care about: the single long line is split
        // into sentence-sized units so the body text survives normalization.
        assert!(normalized.contains("Claude Code 的第 0 个学习要点"));
        assert!(normalized.contains("Claude Code 的第 39 个学习要点"));
        // Phase 1: zhihu-specific start/end markers and suffix-noise rules
        // are gone. Preamble/trailing noise (`关注问题`, `所属专栏`, `这不是当前回答`)
        // is no longer trimmed from this input; the engine ladder is now
        // responsible for stripping site chrome upstream. We intentionally
        // do not assert their absence here.

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
    fn check_article_cleaning_loads_builtin_config_when_config_is_missing() {
        // Phase 1: per-site [[sites]] strategies were deleted. This test
        // now just verifies that when no config file exists on disk, the
        // builtin article_memory.toml is parsed successfully and produces
        // an "ok" status. Warnings are reserved for future checks.
        let paths = test_paths("check_article_cleaning_loads_builtin_config");
        let response = check_article_cleaning(&paths).unwrap();

        assert_eq!(response.status, "ok");
        assert!(response.warnings.is_empty());
        assert!(response.config_path.ends_with("article_memory.toml"));

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

#[cfg(test)]
mod find_by_url_tests {
    use super::*;
    use crate::article_memory::types::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
    use tempfile::TempDir;

    fn mk_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn mk_record(id: &str, url: Option<&str>) -> ArticleMemoryRecord {
        ArticleMemoryRecord {
            id: id.into(),
            title: format!("T {id}"),
            url: url.map(String::from),
            source: "test".into(),
            language: None,
            tags: vec![],
            status: ArticleMemoryRecordStatus::Saved,
            value_score: Some(0.9),
            captured_at: "2026-04-24T00:00:00Z".into(),
            updated_at: "2026-04-24T00:00:00Z".into(),
            content_path: format!("articles/{id}.md"),
            raw_path: None,
            normalized_path: None,
            summary_path: None,
            translation_path: None,
            notes: None,
            clean_status: None,
            clean_profile: None,
        }
    }

    fn seed(paths: &RuntimePaths, records: Vec<ArticleMemoryRecord>) {
        init_article_memory(paths).unwrap();
        let mut index = crate::article_memory::internals::load_index(paths).unwrap();
        index.articles = records;
        crate::article_memory::internals::write_index(paths, &index).unwrap();
    }

    #[test]
    fn returns_none_when_index_empty() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        init_article_memory(&paths).unwrap();
        let hit = find_article_by_normalized_url(&paths, "https://example.com/").unwrap();
        assert!(hit.is_none());
    }

    #[test]
    fn returns_record_when_normalized_url_matches() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![mk_record("aaa", Some("https://example.com/"))]);
        let hit = find_article_by_normalized_url(&paths, "https://example.com/")
            .unwrap()
            .expect("record should be found");
        assert_eq!(hit.id, "aaa");
    }

    #[test]
    fn returns_none_when_only_non_matching_urls() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![mk_record("aaa", Some("https://other.com/"))]);
        let hit = find_article_by_normalized_url(&paths, "https://example.com/").unwrap();
        assert!(hit.is_none());
    }

    #[test]
    fn skips_records_with_missing_url() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(
            &paths,
            vec![
                mk_record("aaa", None),
                mk_record("bbb", Some("https://example.com/")),
            ],
        );
        let hit = find_article_by_normalized_url(&paths, "https://example.com/")
            .unwrap()
            .expect("bbb should be found");
        assert_eq!(hit.id, "bbb");
    }

    #[test]
    fn returns_first_hit_when_multiple_match() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(
            &paths,
            vec![
                mk_record("aaa", Some("https://example.com/")),
                mk_record("bbb", Some("https://example.com/")),
            ],
        );
        let hit = find_article_by_normalized_url(&paths, "https://example.com/")
            .unwrap()
            .expect("should find first");
        assert_eq!(hit.id, "aaa");
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;
    use crate::article_memory::types::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
    use tempfile::TempDir;

    fn mk_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn seed_one(paths: &RuntimePaths, id: &str, url: &str) {
        init_article_memory(paths).unwrap();
        let mut index = internals::load_index(paths).unwrap();
        index.articles.push(ArticleMemoryRecord {
            id: id.into(),
            title: "OLD TITLE".into(),
            url: Some(url.into()),
            source: "test".into(),
            language: None,
            tags: vec![],
            status: ArticleMemoryRecordStatus::Saved,
            value_score: Some(0.5),
            captured_at: "2026-04-01T00:00:00Z".into(),
            updated_at: "2026-04-01T00:00:00Z".into(),
            content_path: format!("articles/{id}.md"),
            raw_path: Some(format!("articles/{id}.raw.txt")),
            normalized_path: Some(format!("articles/{id}.normalized.md")),
            summary_path: None,
            translation_path: None,
            notes: None,
            clean_status: Some("raw".into()),
            clean_profile: None,
        });
        internals::write_index(paths, &index).unwrap();
        let articles_dir = paths.runtime_dir.join("article-memory").join("articles");
        std::fs::create_dir_all(&articles_dir).unwrap();
        std::fs::write(articles_dir.join(format!("{id}.md")), "OLD CONTENT").unwrap();
        std::fs::write(articles_dir.join(format!("{id}.raw.txt")), "OLD CONTENT").unwrap();
        std::fs::write(
            articles_dir.join(format!("{id}.normalized.md")),
            "OLD CONTENT",
        )
        .unwrap();
    }

    #[test]
    fn override_reuses_id_and_does_not_append() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed_one(&paths, "aaa", "https://example.com/");

        let _updated = add_article_memory_override(
            &paths,
            ArticleMemoryAddRequest {
                title: "NEW TITLE".into(),
                url: Some("https://example.com/".into()),
                source: "web".into(),
                language: None,
                tags: vec![],
                content: "NEW CONTENT".into(),
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Saved,
                value_score: Some(0.95),
                notes: None,
            },
            "aaa",
        )
        .unwrap();

        let index = internals::load_index(&paths).unwrap();
        assert_eq!(index.articles.len(), 1, "no append");
        assert_eq!(index.articles[0].id, "aaa");
        assert_eq!(index.articles[0].title, "NEW TITLE");
        assert_eq!(index.articles[0].value_score, Some(0.95));
    }

    #[test]
    fn override_overwrites_on_disk_content_files() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed_one(&paths, "aaa", "https://example.com/");

        add_article_memory_override(
            &paths,
            ArticleMemoryAddRequest {
                title: "T".into(),
                url: Some("https://example.com/".into()),
                source: "web".into(),
                language: None,
                tags: vec![],
                content: "NEW CONTENT".into(),
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Saved,
                value_score: None,
                notes: None,
            },
            "aaa",
        )
        .unwrap();

        let articles_dir = paths.runtime_dir.join("article-memory").join("articles");
        let md = std::fs::read_to_string(articles_dir.join("aaa.md")).unwrap();
        assert_eq!(md, "NEW CONTENT");
    }

    #[test]
    fn override_errors_when_id_not_in_index() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        init_article_memory(&paths).unwrap();

        let err = add_article_memory_override(
            &paths,
            ArticleMemoryAddRequest {
                title: "T".into(),
                url: Some("https://example.com/".into()),
                source: "web".into(),
                language: None,
                tags: vec![],
                content: "X".into(),
                summary: None,
                translation: None,
                status: ArticleMemoryRecordStatus::Saved,
                value_score: None,
                notes: None,
            },
            "missing",
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing"));
    }
}

#[cfg(test)]
mod migrate_tests {
    use super::*;
    use crate::article_memory::types::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
    use tempfile::TempDir;

    fn mk_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn seed(paths: &RuntimePaths, records: Vec<(&str, Option<&str>)>) {
        init_article_memory(paths).unwrap();
        let mut index = internals::load_index(paths).unwrap();
        index.articles = records
            .into_iter()
            .map(|(id, url)| ArticleMemoryRecord {
                id: id.into(),
                title: format!("T {id}"),
                url: url.map(String::from),
                source: "test".into(),
                language: None,
                tags: vec![],
                status: ArticleMemoryRecordStatus::Saved,
                value_score: Some(0.5),
                captured_at: "2026-04-01T00:00:00Z".into(),
                updated_at: "2026-04-01T00:00:00Z".into(),
                content_path: format!("articles/{id}.md"),
                raw_path: None,
                normalized_path: None,
                summary_path: None,
                translation_path: None,
                notes: None,
                clean_status: None,
                clean_profile: None,
            })
            .collect();
        internals::write_index(paths, &index).unwrap();
    }

    #[test]
    fn migration_normalizes_trailing_slash_and_fragment() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![("aaa", Some("https://example.com/p#frag"))]);
        let changed = migrate_urls_to_normalized(&paths).unwrap();
        assert!(changed >= 1, "expected at least one URL rewritten");
        let index = internals::load_index(&paths).unwrap();
        assert!(
            !index.articles[0].url.as_ref().unwrap().contains('#'),
            "fragment should be stripped"
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![("aaa", Some("https://example.com/p#frag"))]);
        migrate_urls_to_normalized(&paths).unwrap();
        let changed_second = migrate_urls_to_normalized(&paths).unwrap();
        assert_eq!(changed_second, 0, "second run should be no-op");
    }

    #[test]
    fn migration_skips_records_with_no_url() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(&paths, vec![("aaa", None)]);
        let changed = migrate_urls_to_normalized(&paths).unwrap();
        assert_eq!(changed, 0);
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;
    use crate::article_memory::types::{ArticleMemoryRecord, ArticleMemoryRecordStatus};
    use tempfile::TempDir;

    fn mk_paths(tmp: &TempDir) -> RuntimePaths {
        RuntimePaths {
            repo_root: tmp.path().to_path_buf(),
            runtime_dir: tmp.path().join("runtime"),
        }
    }

    fn mk_record(id: &str, url: &str, score: f32, captured_at: &str) -> ArticleMemoryRecord {
        ArticleMemoryRecord {
            id: id.into(),
            title: format!("T {id}"),
            url: Some(url.into()),
            source: "test".into(),
            language: None,
            tags: vec![],
            status: ArticleMemoryRecordStatus::Saved,
            value_score: Some(score),
            captured_at: captured_at.into(),
            updated_at: captured_at.into(),
            content_path: format!("articles/{id}.md"),
            raw_path: Some(format!("articles/{id}.raw.txt")),
            normalized_path: Some(format!("articles/{id}.normalized.md")),
            summary_path: None,
            translation_path: None,
            notes: None,
            clean_status: None,
            clean_profile: None,
        }
    }

    fn seed(paths: &RuntimePaths, records: Vec<ArticleMemoryRecord>) {
        // Skip init_article_memory; it runs migration+merge and would destroy
        // the duplicates these tests want to observe. Set up dirs manually.
        ensure_article_memory_dirs(paths).unwrap();
        internals::write_index(paths, &ArticleMemoryIndex::new()).unwrap();
        let mut index = internals::load_index(paths).unwrap();
        index.articles = records;
        internals::write_index(paths, &index).unwrap();
        let articles_dir = paths.runtime_dir.join("article-memory").join("articles");
        std::fs::create_dir_all(&articles_dir).unwrap();
        for record in &index.articles {
            std::fs::write(articles_dir.join(format!("{}.md", record.id)), "c").unwrap();
            std::fs::write(articles_dir.join(format!("{}.raw.txt", record.id)), "r").unwrap();
            std::fs::write(
                articles_dir.join(format!("{}.normalized.md", record.id)),
                "n",
            )
            .unwrap();
        }
    }

    #[test]
    fn merge_keeps_higher_value_score_and_deletes_loser_files() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(
            &paths,
            vec![
                mk_record(
                    "lower",
                    "https://example.com/p",
                    0.7,
                    "2026-04-20T00:00:00Z",
                ),
                mk_record(
                    "higher",
                    "https://example.com/p",
                    0.9,
                    "2026-04-10T00:00:00Z",
                ),
            ],
        );

        let merged = merge_duplicate_urls(&paths).unwrap();
        assert_eq!(merged, 1);

        let index = internals::load_index(&paths).unwrap();
        assert_eq!(index.articles.len(), 1);
        assert_eq!(index.articles[0].id, "higher");

        let articles_dir = paths.runtime_dir.join("article-memory").join("articles");
        assert!(!articles_dir.join("lower.md").exists());
        assert!(!articles_dir.join("lower.raw.txt").exists());
        assert!(!articles_dir.join("lower.normalized.md").exists());
        assert!(articles_dir.join("higher.md").exists());
    }

    #[test]
    fn merge_tiebreaks_by_captured_at_when_scores_equal() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(
            &paths,
            vec![
                mk_record(
                    "older",
                    "https://example.com/p",
                    0.5,
                    "2026-04-01T00:00:00Z",
                ),
                mk_record(
                    "newer",
                    "https://example.com/p",
                    0.5,
                    "2026-04-20T00:00:00Z",
                ),
            ],
        );

        merge_duplicate_urls(&paths).unwrap();

        let index = internals::load_index(&paths).unwrap();
        assert_eq!(index.articles.len(), 1);
        assert_eq!(index.articles[0].id, "newer");
    }

    #[test]
    fn merge_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        seed(
            &paths,
            vec![mk_record(
                "a",
                "https://example.com/p",
                0.5,
                "2026-04-01T00:00:00Z",
            )],
        );
        merge_duplicate_urls(&paths).unwrap();
        let second = merge_duplicate_urls(&paths).unwrap();
        assert_eq!(second, 0);
    }

    #[test]
    fn merge_leaves_records_without_url_untouched() {
        let tmp = TempDir::new().unwrap();
        let paths = mk_paths(&tmp);
        let mut r1 = mk_record("a", "https://example.com/p", 0.5, "2026-04-01T00:00:00Z");
        r1.url = None;
        let mut r2 = mk_record("b", "https://example.com/p", 0.5, "2026-04-01T00:00:00Z");
        r2.url = None;
        seed(&paths, vec![r1, r2]);
        let merged = merge_duplicate_urls(&paths).unwrap();
        assert_eq!(merged, 0);
        let index = internals::load_index(&paths).unwrap();
        assert_eq!(index.articles.len(), 2);
    }
}
