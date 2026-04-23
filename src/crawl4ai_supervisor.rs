//! Long-lived Python crawl4ai adapter supervised by the daviszeroclaw daemon.
//!
//! On daemon start, `Crawl4aiSupervisor::start` spawns `python -m
//! crawl4ai_adapter.server_main`, probes `/health` until it responds, and
//! returns once ready. A background task watches the child: if it exits,
//! it restarts with exponential backoff (1s → 2s → 4s → ... capped at 30s).
//! Five consecutive failures within the backoff window surfaces a
//! non-recoverable error to the daemon.

use crate::{Crawl4aiConfig, Crawl4aiError, RuntimePaths};
use reqwest::Client;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::time::sleep;

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

const HEALTH_PATH: &str = "/health";
const STARTUP_PROBE_INTERVAL: Duration = Duration::from_millis(200);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const RESTART_BUDGET: u32 = 5;

#[derive(Clone)]
pub struct Crawl4aiSupervisor {
    inner: Arc<Mutex<SupervisorInner>>,
    health_url: String,
    http: Client,
}

struct SupervisorInner {
    child: Option<Child>,
    paths: RuntimePaths,
    // Retained for Task 10 (per-request crawl options) and Task 14
    // (for_test constructor). Not read by Task 9 itself.
    #[allow(dead_code)]
    config: Crawl4aiConfig,
    python: PathBuf,
    port: u16,
}

impl Crawl4aiSupervisor {
    /// Spawn the adapter, probe /health until ready, return handle.
    pub async fn start(paths: RuntimePaths, config: Crawl4aiConfig) -> Result<Self, Crawl4aiError> {
        if !config.enabled {
            return Err(Crawl4aiError::Disabled);
        }
        let python = resolve_python_binary(&paths, &config)?;
        let port = parse_port(&config.base_url)?;
        let http = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs.saturating_add(10)))
            .build()
            .map_err(|err| Crawl4aiError::ServerUnavailable {
                details: format!("build reqwest client: {err}"),
            })?;
        let inner = SupervisorInner {
            child: None,
            paths,
            config,
            python,
            port,
        };
        let supervisor = Self {
            inner: Arc::new(Mutex::new(inner)),
            health_url: format!("http://127.0.0.1:{port}{HEALTH_PATH}"),
            http,
        };
        supervisor.spawn_child().await?;
        supervisor.wait_until_healthy().await?;
        supervisor.clone().spawn_restart_loop();
        Ok(supervisor)
    }

    /// Returns the URL callers should POST /crawl to (e.g. http://127.0.0.1:11235).
    pub async fn base_url(&self) -> String {
        let guard = self.inner.lock().await;
        format!("http://127.0.0.1:{}", guard.port)
    }

    /// Shared HTTP client for callers. Connection pool is reused.
    pub fn http_client(&self) -> Client {
        self.http.clone()
    }

    /// Probes /health and, on success, returns the body JSON so callers can
    /// inspect the `versions` map the adapter publishes.
    async fn probe_health(&self) -> Result<Value, Crawl4aiError> {
        let resp = self
            .http
            .get(&self.health_url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .map_err(|err| Crawl4aiError::ServerUnavailable {
                details: err.to_string(),
            })?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(body)
        } else {
            Err(Crawl4aiError::ServerUnavailable {
                details: format!("health returned {}: {}", status, compact_json(&body)),
            })
        }
    }

    pub async fn is_healthy(&self) -> bool {
        self.probe_health().await.is_ok()
    }

    async fn spawn_child(&self) -> Result<(), Crawl4aiError> {
        let mut guard = self.inner.lock().await;
        let log_path = guard.paths.crawl4ai_log_path();
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| Crawl4aiError::LocalIo {
                details: format!("create log dir: {err}"),
            })?;
        }
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|err| Crawl4aiError::LocalIo {
                details: format!("open crawl4ai log {}: {err}", log_path.display()),
            })?;
        let log_stderr = log_file.try_clone().map_err(|err| Crawl4aiError::LocalIo {
            details: format!("dup crawl4ai log handle: {err}"),
        })?;

        let child = Command::new(&guard.python)
            .arg("-m")
            .arg("crawl4ai_adapter.server_main")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(guard.port.to_string())
            .arg("--runtime-dir")
            .arg(guard.paths.runtime_dir.display().to_string())
            .current_dir(&guard.paths.repo_root)
            .env("PYTHONPATH", guard.paths.repo_root.display().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_stderr))
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| Crawl4aiError::ServerUnavailable {
                details: format!("spawn crawl4ai_adapter.server_main: {err}"),
            })?;

        if let Some(pid) = child.id() {
            let pid_path = guard.paths.crawl4ai_pid_path();
            let _ = std::fs::write(&pid_path, pid.to_string());
        }
        guard.child = Some(child);
        tracing::info!(port = guard.port, "crawl4ai adapter server spawned");
        Ok(())
    }

    async fn wait_until_healthy(&self) -> Result<(), Crawl4aiError> {
        let start = std::time::Instant::now();
        loop {
            if let Ok(body) = self.probe_health().await {
                // Emit a single-line summary with the adapter's package versions.
                // Unpinned-by-design (see Task 5 rationale): if a crawl breaks
                // next week, `grep 'crawl4ai adapter ready' daemon.log` tells
                // you which versions the daemon booted with.
                let versions = body.get("versions").cloned().unwrap_or(Value::Null);
                tracing::info!(
                    versions = %compact_json(&versions),
                    "crawl4ai adapter ready",
                );
                return Ok(());
            }
            if start.elapsed() > STARTUP_TIMEOUT {
                return Err(Crawl4aiError::ServerUnavailable {
                    details: format!(
                        "adapter did not become healthy within {:?}",
                        STARTUP_TIMEOUT
                    ),
                });
            }
            sleep(STARTUP_PROBE_INTERVAL).await;
        }
    }

    fn spawn_restart_loop(self) {
        tokio::spawn(async move {
            let mut consecutive_failures: u32 = 0;
            let mut backoff = Duration::from_secs(1);
            loop {
                let child_opt = {
                    let mut guard = self.inner.lock().await;
                    guard.child.take()
                };
                let Some(mut child) = child_opt else {
                    break;
                };
                let status = match child.wait().await {
                    Ok(status) => status,
                    Err(err) => {
                        tracing::error!(?err, "crawl4ai adapter wait() failed");
                        break;
                    }
                };
                tracing::warn!(?status, "crawl4ai adapter exited; restarting");
                sleep(backoff).await;
                match self.spawn_child().await {
                    Ok(()) => match self.wait_until_healthy().await {
                        Ok(()) => {
                            consecutive_failures = 0;
                            backoff = Duration::from_secs(1);
                        }
                        Err(err) => {
                            tracing::warn!(error = %err, "crawl4ai adapter restart health probe failed");
                            consecutive_failures += 1;
                        }
                    },
                    Err(err) => {
                        tracing::error!(error = %err, "crawl4ai adapter respawn failed");
                        consecutive_failures += 1;
                    }
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                if consecutive_failures >= RESTART_BUDGET {
                    tracing::error!(
                        "crawl4ai adapter failed to stay up after {RESTART_BUDGET} attempts; giving up"
                    );
                    break;
                }
            }
        });
    }
}

fn resolve_python_binary(
    paths: &RuntimePaths,
    config: &Crawl4aiConfig,
) -> Result<PathBuf, Crawl4aiError> {
    if !config.python.is_empty() {
        return Ok(PathBuf::from(&config.python));
    }
    let candidate = paths.crawl4ai_python_path();
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(Crawl4aiError::ServerUnavailable {
        details: format!(
            "crawl4ai venv python not found at {}. Run `daviszeroclaw crawl install`.",
            candidate.display()
        ),
    })
}

fn parse_port(base_url: &str) -> Result<u16, Crawl4aiError> {
    let url = url::Url::parse(base_url).map_err(|err| Crawl4aiError::ServerUnavailable {
        details: format!("parse base_url {base_url}: {err}"),
    })?;
    url.port_or_known_default()
        .ok_or_else(|| Crawl4aiError::ServerUnavailable {
            details: format!("no port derivable from {base_url}"),
        })
}
