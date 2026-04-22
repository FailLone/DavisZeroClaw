use super::*;
use crate::{
    add_article_memory, check_article_cleaning, check_article_memory, check_local_config,
    hybrid_search_article_memory, init_article_memory, judge_all_article_value_memory,
    judge_article_value_memory, list_article_clean_reports, list_article_memory,
    list_article_value_reports, normalize_all_article_memory, normalize_article_memory,
    rebuild_article_memory_embeddings, replay_article_cleaning, resolve_article_embedding_config,
    resolve_article_normalize_config, resolve_article_value_config, search_article_memory,
    upsert_article_memory_embedding, ArticleMemoryAddRequest, RuntimePaths,
};
use anyhow::{anyhow, bail, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(super) struct ArticleCliAdd {
    pub(super) title: String,
    pub(super) url: Option<String>,
    pub(super) source: String,
    pub(super) language: Option<String>,
    pub(super) tags: Vec<String>,
    pub(super) content_file: PathBuf,
    pub(super) summary_file: Option<PathBuf>,
    pub(super) translation_file: Option<PathBuf>,
    pub(super) score: Option<f32>,
    pub(super) status: ArticleStatusArg,
    pub(super) notes: Option<String>,
}

pub(super) fn init_articles(paths: &RuntimePaths) -> Result<()> {
    let status = init_article_memory(paths)?;
    println!("Article memory initialized.");
    print_article_status(&status);
    println!("Next: daviszeroclaw articles add --title <title> --content-file <file>");
    Ok(())
}

pub(super) fn check_articles(paths: &RuntimePaths) -> Result<()> {
    let status = check_article_memory(paths)?;
    println!("Article memory ok.");
    print_article_status(&status);
    Ok(())
}

pub(super) async fn add_article(paths: &RuntimePaths, input: ArticleCliAdd) -> Result<()> {
    let content = fs::read_to_string(&input.content_file)
        .with_context(|| format!("failed to read {}", input.content_file.display()))?;
    let summary = read_optional_text_file(input.summary_file.as_deref())?;
    let translation = read_optional_text_file(input.translation_file.as_deref())?;
    let record = add_article_memory(
        paths,
        ArticleMemoryAddRequest {
            title: input.title,
            url: input.url,
            source: input.source,
            language: input.language,
            tags: input.tags,
            content,
            summary,
            translation,
            status: input.status.into(),
            value_score: input.score,
            notes: input.notes,
        },
    )?;
    println!("Article stored.");
    println!("- id: {}", record.id);
    println!("- title: {}", record.title);
    println!("- status: {}", record.status);
    println!(
        "- content: {}",
        paths
            .article_memory_dir()
            .join(&record.content_path)
            .display()
    );
    let config = check_local_config(paths)?;
    let normalize_config =
        resolve_article_normalize_config(&config.article_memory.normalize, &config.providers)?;
    let value_config = resolve_article_value_config(paths, &config.providers)?;
    let normalize_response = normalize_article_memory(
        paths,
        normalize_config.as_ref(),
        value_config.as_ref(),
        &record.id,
    )
    .await?;
    println!(
        "- normalize: {} ({})",
        normalize_response.clean_status, normalize_response.clean_profile
    );
    match (
        normalize_response.value_decision.as_deref(),
        resolve_article_embedding_config(&config.article_memory.embedding, &config.providers)?,
    ) {
        (Some("reject"), _) => println!("- embedding: skipped (value rejected)"),
        (_, Some(embedding_config)) => {
            upsert_article_memory_embedding(paths, &embedding_config, &record).await?;
            println!("- embedding: indexed");
        }
        (_, None) => println!("- embedding: disabled"),
    }
    Ok(())
}

pub(super) fn list_articles(paths: &RuntimePaths, limit: usize) -> Result<()> {
    let response = list_article_memory(paths, limit);
    if response.status != "ok" {
        bail!(
            "{}",
            response
                .message
                .unwrap_or_else(|| format!("article memory {}", response.status))
        );
    }
    println!(
        "Article memory records: {} of {}",
        response.returned, response.total_articles
    );
    for article in response.articles {
        println!(
            "- {} | {} | {} | {}",
            article.id, article.status, article.captured_at, article.title
        );
    }
    Ok(())
}

pub(super) async fn search_articles(
    paths: &RuntimePaths,
    query: &str,
    limit: usize,
    keyword_only: bool,
) -> Result<()> {
    let config = if keyword_only {
        None
    } else {
        let config = check_local_config(paths)?;
        resolve_article_embedding_config(&config.article_memory.embedding, &config.providers)?
    };
    let response = if keyword_only {
        search_article_memory(paths, query, limit)
    } else {
        hybrid_search_article_memory(paths, config.as_ref(), query, limit).await
    };
    match response.status.as_str() {
        "ok" | "empty" => {}
        _ => bail!(
            "{}",
            response
                .message
                .unwrap_or_else(|| format!("article memory {}", response.status))
        ),
    }
    println!(
        "Article memory search: {} hit(s), showing {} ({})",
        response.total_hits, response.returned, response.search_mode
    );
    if let Some(semantic_status) = response.semantic_status {
        println!("Semantic index: {semantic_status}");
    }
    for hit in response.hits {
        println!(
            "- {} | keyword={} | semantic={} | {} | {}",
            hit.id,
            hit.score,
            hit.semantic_score
                .map(|score| format!("{score:.3}"))
                .unwrap_or_else(|| "n/a".to_string()),
            hit.status,
            hit.title
        );
        if let Some(url) = hit.url {
            println!("  url: {url}");
        }
        if let Some(snippet) = hit.snippet {
            println!("  snippet: {snippet}");
        }
    }
    Ok(())
}

pub(super) async fn normalize_articles(
    paths: &RuntimePaths,
    id: Option<String>,
    all: bool,
    no_llm: bool,
) -> Result<()> {
    let config = check_local_config(paths)?;
    let normalize_config = if no_llm {
        None
    } else {
        resolve_article_normalize_config(&config.article_memory.normalize, &config.providers)?
    };
    let value_config = if no_llm {
        None
    } else {
        resolve_article_value_config(paths, &config.providers)?
    };
    let responses = if all {
        normalize_all_article_memory(paths, normalize_config.as_ref(), value_config.as_ref())
            .await?
    } else {
        let id = id.ok_or_else(|| anyhow!("provide --id <article-id> or --all"))?;
        vec![
            normalize_article_memory(paths, normalize_config.as_ref(), value_config.as_ref(), &id)
                .await?,
        ]
    };
    println!("Article normalization complete.");
    for response in responses {
        println!(
            "- {} | {} | profile={} | raw={} normalized={} final={} polished={} summary={}",
            response.article_id,
            response.clean_status,
            response.clean_profile,
            response.raw_chars,
            response.normalized_chars,
            response.final_chars,
            response.polished,
            response.summary_generated
        );
        if let Some(message) = response.message {
            println!("  note: {message}");
        }
        println!("  clean_report: {}", response.clean_report_path);
        if let Some(decision) = response.value_decision {
            println!(
                "  value: {} ({})",
                decision,
                response
                    .value_score
                    .map(|score| format!("{score:.2}"))
                    .unwrap_or_else(|| "n/a".to_string())
            );
        }
        if let Some(path) = response.value_report_path {
            println!("  value_report: {path}");
        }
    }
    Ok(())
}

pub(super) fn check_article_cleaning_cli(paths: &RuntimePaths) -> Result<()> {
    let response = check_article_cleaning(paths)?;
    println!("Article cleaning strategy {}.", response.status);
    println!("- config: {}", response.config_path);
    println!("- sites: {}", response.sites.join(", "));
    for warning in response.warnings {
        println!("WARN: {warning}");
    }
    Ok(())
}

pub(super) fn clean_audit_articles(paths: &RuntimePaths, recent: usize) -> Result<()> {
    let response = list_article_clean_reports(paths, recent)?;
    println!(
        "Article clean reports: {} report(s), status={}",
        response.returned, response.status
    );
    for report in response.reports {
        println!(
            "- {} | strategy={}@{} | clean={} | raw={} normalized={} final={} kept={:.2} | risks={}",
            report.article_id,
            report.strategy_name,
            report.strategy_version,
            report.clean_status,
            report.raw_chars,
            report.normalized_chars,
            report.final_chars,
            report.kept_ratio,
            if report.risk_flags.is_empty() {
                "none".to_string()
            } else {
                report.risk_flags.join(",")
            }
        );
        if let Some(url) = report.url {
            println!("  url: {url}");
        }
        if !report.leftover_noise_candidates.is_empty() {
            println!(
                "  leftover_noise: {}",
                report.leftover_noise_candidates.join(", ")
            );
        }
    }
    Ok(())
}

pub(super) async fn judge_articles(
    paths: &RuntimePaths,
    id: Option<String>,
    all: bool,
    no_llm: bool,
) -> Result<()> {
    let config = check_local_config(paths)?;
    let value_config = if no_llm {
        let mut resolved = resolve_article_value_config(paths, &config.providers)?
            .ok_or_else(|| anyhow!("article value judging is disabled"))?;
        resolved.llm_judge = false;
        Some(resolved)
    } else {
        resolve_article_value_config(paths, &config.providers)?
    };
    let reports = if all {
        judge_all_article_value_memory(
            paths,
            value_config
                .as_ref()
                .ok_or_else(|| anyhow!("article value judging is disabled"))?,
        )
        .await?
    } else {
        let id = id.ok_or_else(|| anyhow!("provide --id <article-id> or --all"))?;
        vec![
            judge_article_value_memory(
                paths,
                value_config
                    .as_ref()
                    .ok_or_else(|| anyhow!("article value judging is disabled"))?,
                &id,
            )
            .await?,
        ]
    };
    println!("Article value judging complete.");
    for report in reports {
        println!(
            "- {} | value={} | score={:.2} | topics={} | risks={}",
            report.article_id,
            report.decision,
            report.value_score,
            if report.topic_tags.is_empty() {
                "none".to_string()
            } else {
                report.topic_tags.join(",")
            },
            if report.risk_flags.is_empty() {
                "none".to_string()
            } else {
                report.risk_flags.join(",")
            }
        );
        for reason in report.reasons.iter().take(3) {
            println!("  reason: {reason}");
        }
    }
    Ok(())
}

pub(super) fn value_audit_articles(paths: &RuntimePaths, recent: usize) -> Result<()> {
    let response = list_article_value_reports(paths, recent)?;
    println!(
        "Article value reports: {} report(s), status={}",
        response.returned, response.status
    );
    for report in response.reports {
        println!(
            "- {} | decision={} | score={:.2} | topics={} | risks={}",
            report.article_id,
            report.decision,
            report.value_score,
            if report.topic_tags.is_empty() {
                "none".to_string()
            } else {
                report.topic_tags.join(",")
            },
            if report.risk_flags.is_empty() {
                "none".to_string()
            } else {
                report.risk_flags.join(",")
            }
        );
        for reason in report.reasons.iter().take(3) {
            println!("  reason: {reason}");
        }
    }
    Ok(())
}

pub(super) fn review_article_strategy_input(paths: &RuntimePaths, recent: usize) -> Result<()> {
    let response = build_article_strategy_review_input(paths, recent)?;
    println!("Article strategy review input generated.");
    println!("- status: {}", response.status);
    println!("- report: {}", response.report_path);
    println!("- editable config: {}", response.config_path);
    println!(
        "- implementation requests: {}",
        response.implementation_requests_dir
    );
    println!();
    println!("{}", response.markdown);
    Ok(())
}

pub(super) async fn replay_cleaning_articles(
    paths: &RuntimePaths,
    id: Option<String>,
    all: bool,
) -> Result<()> {
    if !all && id.is_none() {
        bail!("provide --id <article-id> or --all");
    }
    let response = if all {
        replay_article_cleaning(paths, None)?
    } else {
        replay_article_cleaning(paths, id.as_deref())?
    };
    println!("Article deterministic cleaning replay complete.");
    for report in response.reports {
        println!(
            "- {} | {} | strategy={}@{} | raw={} normalized={} kept={:.2} risks={}",
            report.article_id,
            report.clean_status,
            report.strategy_name,
            report.strategy_version,
            report.raw_chars,
            report.normalized_chars,
            report.kept_ratio,
            if report.risk_flags.is_empty() {
                "none".to_string()
            } else {
                report.risk_flags.join(",")
            }
        );
    }
    Ok(())
}

pub(super) async fn index_articles(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let Some(embedding_config) =
        resolve_article_embedding_config(&config.article_memory.embedding, &config.providers)?
    else {
        bail!("article_memory.embedding is disabled. Enable it in config/davis/local.toml first");
    };
    let response = rebuild_article_memory_embeddings(paths, &embedding_config).await?;
    println!("Article memory semantic index rebuilt.");
    println!("- provider: {}", response.provider);
    println!("- model: {}", response.model);
    println!("- dimensions: {}", response.dimensions);
    println!("- indexed: {}", response.indexed);
    println!("- skipped: {}", response.skipped);
    println!("- index: {}", response.index_path);
    Ok(())
}

pub(super) fn read_optional_text_file(path: Option<&Path>) -> Result<Option<String>> {
    path.map(|path| {
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
    })
    .transpose()
}

pub(super) fn print_article_status(status: &crate::ArticleMemoryStatusResponse) {
    println!("- root: {}", status.root);
    println!("- index: {}", status.index_path);
    println!("- articles: {}", status.total_articles);
    println!("- saved: {}", status.saved_articles);
    println!("- candidates: {}", status.candidate_articles);
    println!("- rejected: {}", status.rejected_articles);
    println!("- archived: {}", status.archived_articles);
}
