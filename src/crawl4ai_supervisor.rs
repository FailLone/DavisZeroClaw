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
/// After `/health` first returns OK, keep the just-spawned child alive for a
/// short settle window before declaring it ready. This prevents a port-squatter
/// false positive: the probe can hit an older adapter on the same port while
/// the new child is still in the process of failing its bind and exiting.
const STARTUP_STABILITY_GRACE: Duration = Duration::from_secs(1);
/// Grace window before a structured 503 from `/health` short-circuits the
/// startup probe. Uvicorn can respond with 503 while the FastAPI app is
/// still wiring up its lifespan; waiting ~5s is enough to rule out
/// transient boot noise. Anything past this window is almost always a
/// broken venv / ImportError that no amount of waiting will fix.
const STARTUP_UNHEALTHY_GRACE: Duration = Duration::from_secs(5);
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
    /// Consumed by `base_url()` — trimming trailing slashes off this field
    /// gives the URL callers POST /crawl to. In production, `start()` sets
    /// this from `local.toml` (default `http://127.0.0.1:11235`). In tests
    /// `for_test()` sets it to the ephemeral-port axum mock's base URL, so
    /// the same accessor works uniformly.
    config: Crawl4aiConfig,
    python: PathBuf,
    port: u16,
    /// Flips to `true` once the restart loop has exhausted its budget. A
    /// future `/health` endpoint (or CLI subcommand) can consume this via
    /// `Crawl4aiSupervisor::is_abandoned()` to distinguish "currently
    /// restarting" from "gave up, will never retry." Plain `bool` because
    /// the surrounding Mutex already serializes every read/write; adding
    /// `AtomicBool` here would just be noise.
    gave_up: bool,
}

impl Crawl4aiSupervisor {
    /// Spawn the adapter, probe /health until ready, return handle.
    pub async fn start(paths: RuntimePaths, config: Crawl4aiConfig) -> Result<Self, Crawl4aiError> {
        if !config.enabled {
            return Err(Crawl4aiError::Disabled);
        }
        let python = resolve_python_binary(&paths)?;
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
            gave_up: false,
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

    /// Placeholder used when `[crawl4ai].enabled = false` or the supervisor
    /// failed to start at daemon boot. Any `crawl4ai_crawl` call routed
    /// through this instance trips the `config.enabled` check in
    /// `crawl4ai_crawl` and surfaces `Crawl4aiError::Disabled`. The
    /// `base_url` / `http_client` accessors remain callable so calling
    /// code does not need to branch, but the stub never hits the network.
    ///
    /// Takes `paths` explicitly rather than calling `RuntimePaths::from_env()`
    /// so every supervisor instance shares the daemon's single source of
    /// truth for filesystem layout (guards against env/cwd drift between
    /// construction points).
    pub fn disabled(paths: RuntimePaths) -> Self {
        let inner = SupervisorInner {
            child: None,
            paths,
            config: Crawl4aiConfig {
                enabled: false,
                ..Crawl4aiConfig::default()
            },
            python: PathBuf::new(),
            port: 0,
            gave_up: false,
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
            health_url: String::new(),
            http: Client::new(),
        }
    }

    /// `true` once `spawn_restart_loop` has exhausted `RESTART_BUDGET`.
    /// Intended for future `/health` / CLI surface; no downstream consumer
    /// wired yet. See SupervisorInner::gave_up for the rationale on using
    /// plain `bool` rather than `AtomicBool`.
    pub async fn is_abandoned(&self) -> bool {
        let guard = self.inner.lock().await;
        guard.gave_up
    }

    /// Returns the URL callers should POST /crawl to (e.g. `http://127.0.0.1:11235`).
    ///
    /// Reads from `config.base_url` (trimmed of trailing slashes) rather than
    /// reconstructing `http://127.0.0.1:{port}`. In production, `start()`
    /// sets `config.base_url` from `local.toml`, and the default there is
    /// `http://127.0.0.1:11235` — so this returns the same string the old
    /// code did for any default-configured deployment. The explicit choice
    /// to honor the configured host (rather than hardcoding 127.0.0.1) also
    /// lets `Crawl4aiSupervisor::for_test(paths, base_url)` point at a
    /// random-port axum mock without having to teach the supervisor about
    /// "test mode."
    pub async fn base_url(&self) -> String {
        let guard = self.inner.lock().await;
        guard.config.base_url.trim_end_matches('/').to_string()
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

    async fn has_spawned_child(&self) -> bool {
        let guard = self.inner.lock().await;
        guard.child.is_some()
    }

    async fn ensure_spawned_child_alive(&self) -> Result<(), Crawl4aiError> {
        let mut guard = self.inner.lock().await;
        let Some(child) = guard.child.as_mut() else {
            return Ok(());
        };
        match child.try_wait() {
            Ok(Some(status)) => Err(Crawl4aiError::ServerUnavailable {
                details: format!("adapter process exited during startup: {status:?}"),
            }),
            Ok(None) => Ok(()),
            Err(err) => Err(Crawl4aiError::ServerUnavailable {
                details: format!("adapter process status check failed: {err}"),
            }),
        }
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
        self.wait_until_healthy_with(STARTUP_TIMEOUT, STARTUP_UNHEALTHY_GRACE)
            .await
    }

    /// Probe `/health` on a loop with injectable budgets. Broken out of
    /// `wait_until_healthy` so tests can drive the three-way branch logic
    /// (connection refused → keep polling, 503 past grace → bail verbatim,
    /// outer timeout → generic error) in ~300 ms instead of 30 s.
    ///
    /// Reasoning for the three-case split:
    /// 1. `Err(_)` from reqwest = uvicorn hasn't bound the port yet. Normal
    ///    during boot; keep polling until `startup_timeout`.
    /// 2. 503 with a structured body = uvicorn is up (it responded) but the
    ///    app's lifespan reports it cannot serve (typically
    ///    `crawl4ai_import_failed`). After `unhealthy_grace`, return the
    ///    body verbatim so operators see the real reason in daemon.log
    ///    instead of a generic 30s timeout.
    /// 3. Other non-2xx (e.g. a transient 500 from FastAPI mid-boot) = keep
    ///    polling; those genuinely can clear on their own.
    ///
    /// `pub(crate)` so in-crate integration tests under `tests/rust/` can
    /// pin down the 503 short-circuit contract without waiting out the
    /// production 30 s `STARTUP_TIMEOUT`.
    pub(crate) async fn wait_until_healthy_with(
        &self,
        startup_timeout: Duration,
        unhealthy_grace: Duration,
    ) -> Result<(), Crawl4aiError> {
        let start = std::time::Instant::now();
        loop {
            self.ensure_spawned_child_alive().await?;
            match self
                .http
                .get(&self.health_url)
                .timeout(Duration::from_secs(2))
                .send()
                .await
            {
                Ok(resp) => {
                    let status = resp.status();
                    // Only parse the body for statuses we actually read from
                    // (2xx → versions map, 503 past grace → verbatim error).
                    // Transient 500s mid-boot are logged by status alone and
                    // skip the JSON allocation.
                    if status.is_success() {
                        let body: Value = resp.json().await.unwrap_or(Value::Null);
                        if self.has_spawned_child().await {
                            self.ensure_spawned_child_alive().await?;
                            sleep(STARTUP_STABILITY_GRACE).await;
                            self.ensure_spawned_child_alive().await?;
                        }
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
                    if status == reqwest::StatusCode::SERVICE_UNAVAILABLE
                        && start.elapsed() > unhealthy_grace
                    {
                        // uvicorn is up (it responded with a structured 503)
                        // but the app's lifespan reports it cannot serve.
                        // Bail verbatim with the body so operators see
                        // "crawl4ai_import_failed: ModuleNotFoundError..."
                        // in daemon.log instead of a generic 30s timeout.
                        let body: Value = resp.json().await.unwrap_or(Value::Null);
                        return Err(Crawl4aiError::ServerUnavailable {
                            details: format!("adapter reports unhealthy: {}", compact_json(&body)),
                        });
                    }
                    // 503 inside the grace window, or any other non-2xx
                    // (e.g. a transient 500 mid-boot) — fall through to
                    // the poll/sleep path and retry.
                }
                Err(_) => {
                    // Connection refused / reqwest timeout — uvicorn is
                    // still binding its listener. Keep polling.
                }
            }
            if start.elapsed() > startup_timeout {
                return Err(Crawl4aiError::ServerUnavailable {
                    details: format!("adapter did not become healthy within {startup_timeout:?}"),
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
                tracing::warn!(
                    ?status,
                    consecutive_failures,
                    backoff_ms = backoff.as_millis() as u64,
                    "crawl4ai adapter exited; restarting"
                );
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
                    // Flip the abandoned flag so callers (CLI / future
                    // /health surface) can tell this apart from a normal
                    // restart in progress.
                    let mut guard = self.inner.lock().await;
                    guard.gave_up = true;
                    break;
                }
            }
        });
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Crawl4aiSupervisor {
    /// Test constructor: skips spawning a child, points `base_url()` and the
    /// shared `reqwest::Client` at the caller-supplied address (typically an
    /// ephemeral-port axum mock router). Lets integration tests drive
    /// `crawl4ai_crawl` / `express_auth_status` end-to-end through a fake
    /// `/crawl` + `/health` HTTP surface without a real Python process.
    ///
    /// Gated behind `#[cfg(any(test, feature = "test-util"))]`; the
    /// `test-util` feature is reserved for future external test harnesses and
    /// is not yet declared in `Cargo.toml`. In-crate `cargo test` picks this
    /// up via the `test` cfg.
    pub fn for_test(paths: RuntimePaths, base_url: impl Into<String>) -> Self {
        let base = base_url.into();
        let parsed = url::Url::parse(&base).expect("for_test requires a parseable base_url");
        let port = parsed.port_or_known_default().unwrap_or(0);
        let http = Client::new();
        let health_url = format!("{}/health", base.trim_end_matches('/'));
        let config = Crawl4aiConfig {
            enabled: true,
            base_url: base,
            ..Crawl4aiConfig::default()
        };
        Self {
            inner: Arc::new(Mutex::new(SupervisorInner {
                child: None,
                paths,
                config,
                python: PathBuf::new(),
                port,
                gave_up: false,
            })),
            health_url,
            http,
        }
    }
}

fn resolve_python_binary(paths: &RuntimePaths) -> Result<PathBuf, Crawl4aiError> {
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
