use super::*;
use crate::support::{isoformat, now_utc};
use crate::RuntimePaths;
use anyhow::{Context, Result};
use std::fs;

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
