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
                    .join("control_aliases.json")
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

    pub fn browser_profiles_root(&self) -> PathBuf {
        self.runtime_dir.join("browser-profiles")
    }

    pub fn express_cache_path(&self, source: &str) -> PathBuf {
        self.state_dir()
            .join(format!("express_{source}_cache.json"))
    }

    pub fn browser_bridge_status_path(&self) -> PathBuf {
        self.state_dir().join("browser_bridge_status.json")
    }

    pub fn browser_actions_log_path(&self) -> PathBuf {
        self.state_dir().join("browser_actions.jsonl")
    }

    pub fn browser_confirmations_log_path(&self) -> PathBuf {
        self.state_dir().join("browser_confirmations.jsonl")
    }

    pub fn browser_screenshots_dir(&self) -> PathBuf {
        self.runtime_dir.join("browser-screenshots")
    }

    pub fn browser_worker_script_path(&self) -> PathBuf {
        self.repo_root.join("browser-worker").join("server.mjs")
    }

    pub fn browser_worker_log_path(&self) -> PathBuf {
        self.runtime_dir.join("browser_worker.log")
    }

    pub fn browser_worker_pid_path(&self) -> PathBuf {
        self.runtime_dir.join("browser_worker.pid")
    }

    pub fn workspace_dir(&self) -> PathBuf {
        self.runtime_dir.join("workspace")
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
