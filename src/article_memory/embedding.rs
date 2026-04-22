use super::*;
use crate::support::{isoformat, now_utc};
use crate::RuntimePaths;
use anyhow::Result;

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
