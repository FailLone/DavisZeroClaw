//! Task-local context used by the ingest worker to pass engine telemetry
//! into `build_clean_report` without threading two extra parameters
//! through 4 callers.
//!
//! Safe across `.await` boundaries (unlike `thread_local!`) because
//! `tokio::task_local!` is scoped per-future, not per-thread.
//!
//! All non-ingest callers (CLI replay, LLM polish, judge) run outside
//! the scope and see `current() == None`, producing an empty chain /
//! `final_engine = None` — equivalent to the pre-T13 behavior.

use tokio::task_local;

#[derive(Debug, Clone)]
pub struct EngineReportContext {
    pub engine_chain: Vec<String>,
    pub final_engine: Option<String>,
}

task_local! {
    pub(crate) static CONTEXT: EngineReportContext;
}

/// Run the given future with `ctx` installed in the task-local.
pub async fn with_context<F, R>(ctx: EngineReportContext, fut: F) -> R
where
    F: std::future::Future<Output = R>,
{
    CONTEXT.scope(ctx, fut).await
}

/// Read the currently installed context, if any.
pub fn current() -> Option<EngineReportContext> {
    CONTEXT.try_with(|c| c.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn current_returns_none_outside_scope() {
        assert!(current().is_none());
    }

    #[tokio::test]
    async fn current_returns_some_inside_scope() {
        let ctx = EngineReportContext {
            engine_chain: vec!["trafilatura".to_string(), "openrouter-llm".to_string()],
            final_engine: Some("openrouter-llm".to_string()),
        };
        let ctx_clone = ctx.clone();
        with_context(ctx, async move {
            let got = current().expect("context present");
            assert_eq!(got.engine_chain, ctx_clone.engine_chain);
            assert_eq!(got.final_engine, ctx_clone.final_engine);
        })
        .await;
    }

    #[tokio::test]
    async fn context_isolated_across_concurrent_tasks() {
        // Two tasks run concurrently with different contexts; neither
        // should see the other's.
        let a = tokio::spawn(async move {
            with_context(
                EngineReportContext {
                    engine_chain: vec!["a".to_string()],
                    final_engine: Some("a".to_string()),
                },
                async {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    current().unwrap().final_engine
                },
            )
            .await
        });
        let b = tokio::spawn(async move {
            with_context(
                EngineReportContext {
                    engine_chain: vec!["b".to_string()],
                    final_engine: Some("b".to_string()),
                },
                async {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    current().unwrap().final_engine
                },
            )
            .await
        });
        let (ra, rb) = tokio::join!(a, b);
        assert_eq!(ra.unwrap(), Some("a".to_string()));
        assert_eq!(rb.unwrap(), Some("b".to_string()));
    }
}
