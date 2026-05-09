use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub repo_root: PathBuf,
    pub runtime_dir: PathBuf,
}

impl RuntimePaths {
    pub fn from_env() -> Self {
        let repo_root = std::env::var("DAVIS_REPO_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let runtime_dir = std::env::var("DAVIS_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| repo_root.join(".runtime").join("davis"));
        Self {
            repo_root,
            runtime_dir,
        }
    }

    pub fn control_aliases_path(&self) -> PathBuf {
        std::env::var("DAVIS_CONTROL_ALIASES_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                self.repo_root
                    .join("config")
                    .join("davis")
                    .join("control_aliases.toml")
            })
    }

    pub fn config_template_path(&self) -> PathBuf {
        self.repo_root
            .join("config")
            .join("davis")
            .join("config.toml")
    }

    pub fn local_config_path(&self) -> PathBuf {
        self.repo_root
            .join("config")
            .join("davis")
            .join("local.toml")
    }

    pub fn local_config_example_path(&self) -> PathBuf {
        self.repo_root
            .join("config")
            .join("davis")
            .join("local.example.toml")
    }

    pub fn article_cleaning_config_path(&self) -> PathBuf {
        self.repo_root
            .join("config")
            .join("davis")
            .join("article_memory.toml")
    }

    pub fn runtime_config_path(&self) -> PathBuf {
        self.runtime_dir.join("config.toml")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.runtime_dir.join("state")
    }

    pub fn failure_state_path(&self) -> PathBuf {
        self.state_dir().join("control_failures.json")
    }

    pub fn config_report_cache_path(&self) -> PathBuf {
        self.state_dir().join("ha_config_advisor_report.json")
    }

    pub fn model_scorecard_path(&self) -> PathBuf {
        self.state_dir().join("model_scorecard.json")
    }

    pub fn model_route_plan_path(&self) -> PathBuf {
        self.state_dir().join("model_route_plan.json")
    }

    pub fn model_route_history_path(&self) -> PathBuf {
        self.state_dir().join("model_route_history.jsonl")
    }

    pub fn model_runtime_observations_path(&self) -> PathBuf {
        self.state_dir().join("model_runtime_observations.json")
    }

    pub fn zeroclaw_runtime_trace_path(&self) -> PathBuf {
        self.state_dir().join("runtime-trace.jsonl")
    }

    pub fn ha_mcp_capabilities_path(&self) -> PathBuf {
        self.state_dir().join("ha_mcp_capabilities.json")
    }

    pub fn ha_mcp_live_context_path(&self) -> PathBuf {
        self.state_dir().join("ha_mcp_live_context.json")
    }

    pub fn crawl4ai_home_dir(&self) -> PathBuf {
        self.runtime_dir.join(".crawl4ai")
    }

    pub fn crawl4ai_profiles_root(&self) -> PathBuf {
        self.crawl4ai_home_dir().join("profiles")
    }

    pub fn crawl4ai_legacy_profiles_root(&self) -> PathBuf {
        self.runtime_dir.join("crawl4ai").join("profiles")
    }

    pub fn crawl4ai_adapter_dir(&self) -> PathBuf {
        self.repo_root.join("crawl4ai_adapter")
    }

    pub fn crawl4ai_pid_path(&self) -> PathBuf {
        self.runtime_dir.join("crawl4ai.pid")
    }

    pub fn crawl4ai_log_path(&self) -> PathBuf {
        self.runtime_dir.join("crawl4ai.log")
    }

    pub fn express_cache_path(&self, source: &str) -> PathBuf {
        self.state_dir()
            .join(format!("express_{source}_cache.json"))
    }

    pub fn local_proxy_log_path(&self) -> PathBuf {
        self.runtime_dir.join("local_proxy.log")
    }

    pub fn local_proxy_pid_path(&self) -> PathBuf {
        self.runtime_dir.join("local_proxy.pid")
    }

    pub fn legacy_local_proxy_log_path(&self) -> PathBuf {
        self.runtime_dir.join("ha_audit_proxy.log")
    }

    pub fn legacy_local_proxy_pid_path(&self) -> PathBuf {
        self.runtime_dir.join("ha_audit_proxy.pid")
    }

    pub fn mempalace_venv_dir(&self) -> PathBuf {
        self.runtime_dir.join("mempalace-venv")
    }

    pub fn mempalace_python_path(&self) -> PathBuf {
        self.mempalace_venv_dir().join("bin").join("python")
    }

    pub fn crawl4ai_venv_dir(&self) -> PathBuf {
        self.runtime_dir.join("crawl4ai-venv")
    }

    pub fn crawl4ai_python_path(&self) -> PathBuf {
        self.crawl4ai_venv_dir().join("bin").join("python")
    }

    pub fn router_adapter_venv_dir(&self) -> PathBuf {
        self.runtime_dir.join("router-adapter-venv")
    }

    pub fn router_adapter_python_path(&self) -> PathBuf {
        self.router_adapter_venv_dir().join("bin").join("python")
    }

    pub fn router_adapter_dir(&self) -> PathBuf {
        self.repo_root.join("router_adapter")
    }

    /// Shared Playwright browser cache for ALL Python adapters that drive
    /// Chromium. Both `crawl4ai_adapter/` and `router_adapter/` MUST point
    /// here via `PLAYWRIGHT_BROWSERS_PATH=…` so we have exactly one
    /// Chromium binary on disk. See
    /// `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md`
    /// "Open risks" section.
    pub fn playwright_browsers_path(&self) -> PathBuf {
        self.runtime_dir.join("playwright-browsers")
    }

    pub fn mempalace_palace_dir(&self) -> PathBuf {
        self.runtime_dir.join("mempalace")
    }

    /// Program path + args for launching the MemPalace MCP server that the
    /// Davis sink talks to. Returns `(program, args)` so the caller can feed
    /// them straight into `tokio::process::Command`.
    ///
    /// The `--palace <dir>` argument is CRITICAL: without it MemPalace falls
    /// back to `~/.mempalace/palace`, which is the user's personal MemPalace.
    /// Davis must never write its automated projections into the user's
    /// palace — it uses its own dir under `{runtime}/mempalace/`.
    pub fn mempalace_mcp_server_cmd(&self) -> (PathBuf, Vec<String>) {
        let palace = self.mempalace_palace_dir();
        (
            self.mempalace_python_path(),
            vec![
                "-m".to_string(),
                "mempalace.mcp_server".to_string(),
                "--palace".to_string(),
                palace.to_string_lossy().into_owned(),
            ],
        )
    }

    pub fn article_memory_dir(&self) -> PathBuf {
        self.runtime_dir.join("article-memory")
    }

    pub fn article_memory_index_path(&self) -> PathBuf {
        self.article_memory_dir().join("index.json")
    }

    pub fn article_memory_embeddings_path(&self) -> PathBuf {
        self.article_memory_dir().join("embeddings.json")
    }

    pub fn article_memory_articles_dir(&self) -> PathBuf {
        self.article_memory_dir().join("articles")
    }

    pub fn article_memory_reports_dir(&self) -> PathBuf {
        self.article_memory_dir().join("reports")
    }

    pub fn article_memory_clean_reports_dir(&self) -> PathBuf {
        self.article_memory_reports_dir().join("clean")
    }

    pub fn article_memory_value_reports_dir(&self) -> PathBuf {
        self.article_memory_reports_dir().join("value")
    }

    pub fn article_memory_strategy_reports_dir(&self) -> PathBuf {
        self.article_memory_reports_dir().join("strategy")
    }

    pub fn article_memory_implementation_requests_dir(&self) -> PathBuf {
        self.article_memory_reports_dir()
            .join("implementation-requests")
    }

    pub fn article_memory_ingest_jobs_path(&self) -> PathBuf {
        self.article_memory_dir().join("ingest_jobs.json")
    }

    pub fn workspace_dir(&self) -> PathBuf {
        self.runtime_dir.join("workspace")
    }

    pub fn workspace_skills_dir(&self) -> PathBuf {
        self.workspace_dir().join("skills")
    }

    pub fn workspace_sops_dir(&self) -> PathBuf {
        self.workspace_dir().join("sops")
    }

    pub fn workspace_sessions_dir(&self) -> PathBuf {
        self.workspace_dir().join("sessions")
    }

    pub fn workspace_costs_path(&self) -> PathBuf {
        self.workspace_dir().join("state").join("costs.jsonl")
    }

    pub fn daemon_pid_path(&self) -> PathBuf {
        self.runtime_dir.join("daemon.pid")
    }

    pub fn daemon_log_path(&self) -> PathBuf {
        self.runtime_dir.join("daemon.log")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_jobs_path_nests_under_article_memory_dir() {
        let paths = RuntimePaths {
            repo_root: std::path::PathBuf::from("/tmp/repo"),
            runtime_dir: std::path::PathBuf::from("/tmp/runtime"),
        };
        let got = paths.article_memory_ingest_jobs_path();
        assert_eq!(got, paths.article_memory_dir().join("ingest_jobs.json"));
    }

    #[test]
    fn router_adapter_paths_are_under_runtime_dir() {
        let paths = RuntimePaths {
            repo_root: std::path::PathBuf::from("/tmp/repo"),
            runtime_dir: std::path::PathBuf::from("/tmp/runtime"),
        };
        assert_eq!(
            paths.router_adapter_venv_dir(),
            std::path::PathBuf::from("/tmp/runtime/router-adapter-venv")
        );
        assert_eq!(
            paths.router_adapter_python_path(),
            std::path::PathBuf::from("/tmp/runtime/router-adapter-venv/bin/python")
        );
        assert_eq!(
            paths.router_adapter_dir(),
            std::path::PathBuf::from("/tmp/repo/router_adapter")
        );
        assert_eq!(
            paths.playwright_browsers_path(),
            std::path::PathBuf::from("/tmp/runtime/playwright-browsers")
        );
    }

    #[test]
    fn mempalace_mcp_server_cmd_points_into_venv_python_and_isolates_palace() {
        let paths = RuntimePaths {
            repo_root: std::path::PathBuf::from("/tmp/repo"),
            runtime_dir: std::path::PathBuf::from("/tmp/runtime"),
        };
        let (program, args) = paths.mempalace_mcp_server_cmd();
        assert_eq!(program, paths.mempalace_python_path());
        assert!(program.ends_with("mempalace-venv/bin/python"));
        // --palace is REQUIRED — without it MemPalace writes into the user's
        // personal palace at ~/.mempalace/palace.
        let expected_palace = paths.mempalace_palace_dir();
        assert_eq!(
            args,
            vec![
                "-m".to_string(),
                "mempalace.mcp_server".to_string(),
                "--palace".to_string(),
                expected_palace.to_string_lossy().into_owned(),
            ],
        );
    }
}
