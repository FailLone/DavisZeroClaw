use crate::app_config::TranslateConfig;
use crate::article_memory::translate::prompt::{user_block, SYSTEM};
use crate::article_memory::translate::remote_chat::{RemoteChat, RemoteChatError};
use std::sync::Arc;

#[derive(Clone)]
pub struct TranslateWorkerDeps {
    pub config: Arc<TranslateConfig>,
    pub http: reqwest::Client,
    pub paths: crate::RuntimePaths,
    pub mempalace_sink: Arc<dyn crate::mempalace_sink::MempalaceEmitter>,
}

pub struct TranslateWorker;

impl TranslateWorker {
    pub fn spawn(deps: TranslateWorkerDeps) {
        // Placeholder — real implementation lands in Task 16. See the plan at
        // docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md §Task 16.
        //
        // This guard exits immediately for disabled configs (the default) so
        // no cycle ever runs. The downstream call below exists to keep the
        // private `remote_chat` surface reachable from the crate-level build
        // graph ahead of Task 16 wiring — avoids `#[allow(dead_code)]` during
        // the interim commit.
        if !deps.config.enabled {
            return;
        }
        tokio::spawn(run_once_stub(deps));
    }
}

async fn run_once_stub(deps: TranslateWorkerDeps) -> Result<(), RemoteChatError> {
    let client = RemoteChat::new(&deps.config, deps.http.clone());
    let _ = client.translate_to_zh(SYSTEM, &user_block("")).await?;
    Ok(())
}
