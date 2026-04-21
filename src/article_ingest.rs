use crate::{
    add_article_memory, article_cleaning_preferred_selectors, browser_evaluate, browser_open,
    browser_wait, hybrid_search_article_memory, normalize_article_memory,
    resolve_article_embedding_config, resolve_article_normalize_config,
    resolve_article_value_config, upsert_article_memory_embedding, ArticleMemoryAddRequest,
    ArticleMemoryConfig, ArticleMemoryRecord, ArticleMemoryRecordStatus, BrowserBridgeConfig,
    BrowserEvaluateRequest, BrowserOpenRequest, BrowserWaitRequest, ModelProviderConfig,
    RuntimePaths,
};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ARTICLE_EXTRACTION_SCRIPT_TEMPLATE: &str = r##"
JSON.stringify((() => {
  const clean = (value) => String(value || "").replace(/\s+/g, " ").trim();
  const meta = (...names) => {
    for (const name of names) {
      const escaped = String(name).replace(/"/g, '\\"');
      const node = document.querySelector(`meta[name="${escaped}"],meta[property="${escaped}"]`);
      const content = clean(node && node.getAttribute("content"));
      if (content) return content;
    }
    return "";
  };
  const textOf = (node) => clean(node && node.innerText);
  const selectors = __DAVIS_ARTICLE_SELECTORS__;
  const candidates = selectors
    .flatMap((selector, index) => Array.from(document.querySelectorAll(selector)).slice(0, 5).map((node) => ({ selector, index, node })))
    .map((item) => ({ selector: item.selector, score: (selectors.length - item.index) * 100000, text: textOf(item.node) }))
    .filter((item) => item.text.length > 0)
    .sort((a, b) => (b.score + b.text.length) - (a.score + a.text.length));
  const best = candidates[0] || { selector: "body", text: textOf(document.body) };
  const canonical = document.querySelector('link[rel="canonical"]');
  return {
    title: clean(meta("og:title", "twitter:title") || document.title),
    url: clean((canonical && canonical.href) || location.href),
    browser_url: clean(location.href),
    language: clean(document.documentElement.lang || meta("language")),
    author: clean(meta("author", "article:author", "og:article:author")),
    published_at: clean(meta("article:published_time", "date", "pubdate", "publishdate")),
    site_name: clean(meta("og:site_name") || location.hostname),
    description: clean(meta("description", "og:description", "twitter:description")),
    extraction_selector: best.selector,
    content_length: best.text.length,
    content: best.text
  };
})())
"##;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArticleMemoryIngestRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    #[serde(default)]
    pub new_tab: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_ingest_status")]
    pub status: ArticleMemoryRecordStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_score: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArticleMemoryIngestResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub article: Option<ArticleMemoryRecord>,
    pub extraction: ArticleExtraction,
    pub duplicate_count: usize,
    pub embedding_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArticleExtraction {
    pub title: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_selector: Option<String>,
    #[serde(default)]
    pub content_length: usize,
    pub content: String,
}

pub async fn ingest_article_from_browser(
    paths: &RuntimePaths,
    browser_config: &BrowserBridgeConfig,
    article_config: &ArticleMemoryConfig,
    providers: &[ModelProviderConfig],
    request: ArticleMemoryIngestRequest,
) -> Result<ArticleMemoryIngestResponse> {
    let profile = request.profile.clone();
    let mut tab_id = request.tab_id.clone();
    if let Some(url) = request
        .url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        let opened = browser_open(
            browser_config.clone(),
            BrowserOpenRequest {
                profile: profile.clone(),
                url: url.to_string(),
                new_tab: request.new_tab,
            },
        )
        .await;
        if opened.status != "ok" {
            bail!(
                "browser open failed: {}",
                opened
                    .message
                    .unwrap_or_else(|| "browser returned non-ok status".to_string())
            );
        }
        tab_id = opened.tab_id.or(tab_id);
        if opened.profile.as_deref() == Some("managed") {
            let _ = browser_wait(
                browser_config.clone(),
                BrowserWaitRequest {
                    profile: opened.profile.clone(),
                    tab_id: tab_id.clone(),
                    timeout_ms: Some(10_000),
                    ..Default::default()
                },
            )
            .await;
        }
    }

    let selectors = article_cleaning_preferred_selectors(paths)?;
    let selectors_json = serde_json::to_string(&selectors)?;
    let extraction_script =
        ARTICLE_EXTRACTION_SCRIPT_TEMPLATE.replace("__DAVIS_ARTICLE_SELECTORS__", &selectors_json);
    let evaluated = browser_evaluate(
        browser_config.clone(),
        BrowserEvaluateRequest {
            profile,
            tab_id,
            script: extraction_script,
            mode: Some("read".to_string()),
        },
    )
    .await;
    if evaluated.status != "ok" {
        bail!(
            "browser extraction failed: {}",
            evaluated
                .message
                .unwrap_or_else(|| "browser returned non-ok status".to_string())
        );
    }

    let extraction = parse_extraction(evaluated.data)?;
    if extraction.title.trim().is_empty() {
        bail!("article title could not be extracted");
    }
    if extraction.content.trim().chars().count() < 20 {
        bail!("article content was too short to ingest");
    }

    let duplicate_count = duplicate_count(paths, &extraction).await;
    if duplicate_count > 0 {
        return Ok(ArticleMemoryIngestResponse {
            status: "duplicate".to_string(),
            article: None,
            extraction,
            duplicate_count,
            embedding_status: "skipped".to_string(),
            message: Some("a matching article already exists in article memory".to_string()),
        });
    }

    let source = request
        .source
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| extraction.site_name.clone())
        .unwrap_or_else(|| "browser".to_string());
    let language = request
        .language
        .clone()
        .or_else(|| extraction.language.clone());
    let notes = build_ingest_notes(&request, &extraction);
    let article = add_article_memory(
        paths,
        ArticleMemoryAddRequest {
            title: extraction.title.clone(),
            url: Some(extraction.url.clone()),
            source,
            language,
            tags: request.tags,
            content: extraction.content.clone(),
            summary: None,
            translation: None,
            status: request.status,
            value_score: request.value_score,
            notes,
        },
    )?;
    let normalize_config = resolve_article_normalize_config(&article_config.normalize, providers)?;
    let value_config = resolve_article_value_config(paths, providers)?;
    let normalize_response = normalize_article_memory(
        paths,
        normalize_config.as_ref(),
        value_config.as_ref(),
        &article.id,
    )
    .await?;

    let embedding_status = if normalize_response.value_decision.as_deref() == Some("reject") {
        "skipped_value_rejected".to_string()
    } else {
        match resolve_article_embedding_config(&article_config.embedding, providers)? {
            Some(config) => match upsert_article_memory_embedding(paths, &config, &article).await {
                Ok(()) => "ok".to_string(),
                Err(error) => format!("error: {error}"),
            },
            None => "disabled".to_string(),
        }
    };

    Ok(ArticleMemoryIngestResponse {
        status: "ok".to_string(),
        article: Some(article),
        extraction,
        duplicate_count,
        embedding_status,
        message: None,
    })
}

fn parse_extraction(data: Value) -> Result<ArticleExtraction> {
    let value = match data {
        Value::String(raw) => serde_json::from_str::<Value>(&raw)
            .map_err(|err| anyhow!("browser extraction returned invalid JSON string: {err}"))?,
        value => value,
    };
    let extraction = serde_json::from_value::<ArticleExtraction>(value)
        .map_err(|err| anyhow!("browser extraction returned invalid shape: {err}"))?;
    Ok(ArticleExtraction {
        title: extraction.title.trim().to_string(),
        url: extraction.url.trim().to_string(),
        browser_url: clean_optional(extraction.browser_url),
        language: clean_optional(extraction.language),
        author: clean_optional(extraction.author),
        published_at: clean_optional(extraction.published_at),
        site_name: clean_optional(extraction.site_name),
        description: clean_optional(extraction.description),
        extraction_selector: clean_optional(extraction.extraction_selector),
        content_length: extraction.content.chars().count(),
        content: extraction.content.trim().to_string(),
    })
}

async fn duplicate_count(paths: &RuntimePaths, extraction: &ArticleExtraction) -> usize {
    let mut count = 0;
    for query in [&extraction.url, &extraction.title] {
        let response = hybrid_search_article_memory(paths, None, query, 10).await;
        count += response
            .hits
            .iter()
            .filter(|hit| {
                hit.url.as_deref() == Some(extraction.url.as_str())
                    || hit.title.eq_ignore_ascii_case(&extraction.title)
            })
            .count();
    }
    count
}

fn build_ingest_notes(
    request: &ArticleMemoryIngestRequest,
    extraction: &ArticleExtraction,
) -> Option<String> {
    let mut notes = Vec::new();
    notes.push("ingested_from=browser".to_string());
    if let Some(author) = &extraction.author {
        notes.push(format!("author={author}"));
    }
    if let Some(published_at) = &extraction.published_at {
        notes.push(format!("published_at={published_at}"));
    }
    if let Some(description) = &extraction.description {
        notes.push(format!("description={description}"));
    }
    if let Some(selector) = &extraction.extraction_selector {
        notes.push(format!("selector={selector}"));
    }
    if let Some(user_notes) = request
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        notes.push(user_notes.to_string());
    }
    Some(notes.join("; "))
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn default_ingest_status() -> ArticleMemoryRecordStatus {
    ArticleMemoryRecordStatus::Candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_extraction_accepts_json_string_from_chrome() {
        let extraction = parse_extraction(Value::String(
            json!({
                "title": " Example ",
                "url": "https://example.com/article",
                "language": "en",
                "content": "A useful article body with enough text to ingest."
            })
            .to_string(),
        ))
        .unwrap();

        assert_eq!(extraction.title, "Example");
        assert_eq!(extraction.url, "https://example.com/article");
        assert_eq!(extraction.language.as_deref(), Some("en"));
        assert_eq!(
            extraction.content,
            "A useful article body with enough text to ingest."
        );
    }
}
