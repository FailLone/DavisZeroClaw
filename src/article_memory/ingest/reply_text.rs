use super::types::{IngestJob, IngestJobStatus};

/// Returns an empty string for non-terminal job states (caller should
/// treat empty as "don't send"). Consumer: worker notify hook.
pub fn build_reply_text(job: &IngestJob, resolved_title: Option<&str>) -> String {
    match job.status {
        IngestJobStatus::Saved => {
            let title = resolved_title.unwrap_or(job.url.as_str());
            format!("已保存《{title}》")
        }
        IngestJobStatus::Rejected => "内容价值不高，已略过".to_string(),
        IngestJobStatus::Failed => {
            let reason = humanize_issue_type(
                job.error
                    .as_ref()
                    .map(|e| e.issue_type.as_str())
                    .unwrap_or(""),
            );
            format!("抓取失败：{reason}\n{url}", url = job.url)
        }
        _ => String::new(),
    }
}

/// Map stable `issue_type` strings to user-facing Chinese phrases.
/// Unknown types fall through to a generic hint.
pub fn humanize_issue_type(issue_type: &str) -> &'static str {
    match issue_type {
        "crawl4ai_unavailable" => "抓取服务暂时不可用，请稍后再试",
        "auth_required" => "需要登录才能访问，请登录后再发",
        "site_changed" => "页面结构无法识别，可能需要更新策略",
        "empty_content" => "抓到的内容太短（可能是登录墙或 404）",
        "pipeline_error" => "内部处理出错",
        _ => "未知错误：请查看 articles ingest show",
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::IngestJobError;
    use super::*;

    fn base_job() -> IngestJob {
        IngestJob {
            id: "job1".into(),
            url: "https://example.com/a".into(),
            normalized_url: "https://example.com/a".into(),
            title_override: None,
            tags: vec![],
            force: false,
            source_hint: None,
            reply_handle: None,
            profile_name: "articles-generic".into(),
            resolved_source: None,
            status: IngestJobStatus::Pending,
            article_id: None,
            outcome: None,
            error: None,
            warnings: vec![],
            submitted_at: "t".into(),
            started_at: None,
            finished_at: None,
            attempts: 1,
        }
    }

    #[test]
    fn saved_uses_resolved_title() {
        let mut job = base_job();
        job.status = IngestJobStatus::Saved;
        let txt = build_reply_text(&job, Some("Real Title"));
        assert_eq!(txt, "已保存《Real Title》");
    }

    #[test]
    fn saved_falls_back_to_url_when_no_title() {
        let mut job = base_job();
        job.status = IngestJobStatus::Saved;
        let txt = build_reply_text(&job, None);
        assert_eq!(txt, "已保存《https://example.com/a》");
    }

    #[test]
    fn rejected_has_fixed_phrase() {
        let mut job = base_job();
        job.status = IngestJobStatus::Rejected;
        assert_eq!(build_reply_text(&job, None), "内容价值不高，已略过");
    }

    #[test]
    fn failed_includes_url_on_second_line() {
        let mut job = base_job();
        job.status = IngestJobStatus::Failed;
        job.error = Some(IngestJobError {
            issue_type: "auth_required".into(),
            message: "login wall".into(),
            stage: "fetching".into(),
        });
        let txt = build_reply_text(&job, None);
        assert!(txt.starts_with("抓取失败：需要登录才能访问"));
        assert!(txt.ends_with("\nhttps://example.com/a"));
    }

    #[test]
    fn failed_unknown_issue_type_uses_fallback() {
        let mut job = base_job();
        job.status = IngestJobStatus::Failed;
        job.error = Some(IngestJobError {
            issue_type: "something_new".into(),
            message: "x".into(),
            stage: "fetching".into(),
        });
        let txt = build_reply_text(&job, None);
        assert!(txt.contains("未知错误"));
    }

    #[test]
    fn non_terminal_returns_empty_string() {
        let mut job = base_job();
        for status in [
            IngestJobStatus::Pending,
            IngestJobStatus::Fetching,
            IngestJobStatus::Cleaning,
            IngestJobStatus::Judging,
            IngestJobStatus::Embedding,
        ] {
            job.status = status;
            assert!(build_reply_text(&job, None).is_empty());
        }
    }

    #[test]
    fn humanize_covers_all_stable_types() {
        for t in [
            "crawl4ai_unavailable",
            "auth_required",
            "site_changed",
            "empty_content",
            "pipeline_error",
        ] {
            assert_ne!(
                humanize_issue_type(t),
                "未知错误：请查看 articles ingest show"
            );
        }
    }
}
