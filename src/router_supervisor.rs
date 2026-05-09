//! Router DHCP keeper supervisor: spawns `router_adapter/` as a one-shot
//! subprocess every tick, parses its final-line JSON, returns a typed
//! outcome. Runs no scheduling logic itself — that lives in
//! `router_worker.rs`. See
//! `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md`.

use serde::Deserialize;

/// Closed enum of stages the Python adapter can report when it self-fails.
/// String-typed (not enum) on the Rust side because we only echo the
/// value back into diary lines and tracing — no behavior keys off it.
pub type ReportedStage = String;

/// What the action did during a successful tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterAction {
    /// DHCP was already off — no change made.
    None,
    /// DHCP was on — the adapter clicked it off.
    Disabled,
}

/// One tick's outcome. Four variants cover every observable case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterCheckOutcome {
    /// Adapter ran cleanly and reported `{"status":"ok",...}`.
    Ok {
        action: RouterAction,
        dhcp_was_enabled: bool,
        duration_ms: u64,
    },
    /// Adapter ran but reported `{"status":"error",...}` — a self-detected
    /// failure (selector miss, login timeout, etc.) within the closed
    /// `stage` enum.
    Reported {
        stage: ReportedStage,
        reason: String,
        duration_ms: u64,
    },
    /// Adapter died without printing the final JSON line, or the tick
    /// timeout expired and we killed the child.
    Crashed {
        exit_code: Option<i32>,
        stderr_tail: String,
    },
    /// We never even got the child started (e.g., python binary missing).
    SpawnFailed { reason: String },
}

/// Discriminant kind used by the dedupe state machine in `router_worker.rs`.
/// `Reported` carries the stage so "login failure" and "iframe failure"
/// dedupe independently. Other variants don't carry detail because they
/// already represent a single failure shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutcomeKind {
    OkNone,
    OkDisabled,
    Reported(ReportedStage),
    Crashed,
    SpawnFailed,
}

impl RouterCheckOutcome {
    pub fn kind(&self) -> OutcomeKind {
        match self {
            Self::Ok {
                action: RouterAction::None,
                ..
            } => OutcomeKind::OkNone,
            Self::Ok {
                action: RouterAction::Disabled,
                ..
            } => OutcomeKind::OkDisabled,
            Self::Reported { stage, .. } => OutcomeKind::Reported(stage.clone()),
            Self::Crashed { .. } => OutcomeKind::Crashed,
            Self::SpawnFailed { .. } => OutcomeKind::SpawnFailed,
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }
}

/// Internal shape of the trailing JSON line. Private to this module; not
/// exposed because callers only need `RouterCheckOutcome`.
#[derive(Debug, Deserialize)]
struct AdapterStatus {
    status: String,
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    dhcp_was_enabled: Option<bool>,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

/// Pure: derive a `RouterCheckOutcome` from raw subprocess output.
///
/// Logic:
/// 1. If `stdout` last non-empty line parses as JSON with `status="ok"`:
///    return `Ok { ... }`.
/// 2. If it parses as JSON with `status="error"`: return `Reported { ... }`.
/// 3. Anything else (no JSON last line, malformed JSON, missing required
///    fields, unknown status string) → `Crashed { exit_code, stderr_tail }`.
///    The exit code might still be 0 in pathological cases (Python prints
///    nothing then exits cleanly) — we treat that as crashed too.
pub fn parse_outcome(stdout: &str, exit_code: Option<i32>, stderr: &str) -> RouterCheckOutcome {
    let stderr_tail = stderr_tail(stderr, 256);
    let last_line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let Ok(parsed) = serde_json::from_str::<AdapterStatus>(last_line.trim()) else {
        return RouterCheckOutcome::Crashed {
            exit_code,
            stderr_tail,
        };
    };
    match parsed.status.as_str() {
        "ok" => {
            let action = match parsed.action.as_deref() {
                Some("none") => RouterAction::None,
                Some("disabled") => RouterAction::Disabled,
                _ => {
                    return RouterCheckOutcome::Crashed {
                        exit_code,
                        stderr_tail,
                    }
                }
            };
            let Some(dhcp_was_enabled) = parsed.dhcp_was_enabled else {
                return RouterCheckOutcome::Crashed {
                    exit_code,
                    stderr_tail,
                };
            };
            RouterCheckOutcome::Ok {
                action,
                dhcp_was_enabled,
                duration_ms: parsed.duration_ms.unwrap_or(0),
            }
        }
        "error" => {
            let stage = parsed.stage.unwrap_or_else(|| "unhandled".to_string());
            let reason = parsed.reason.unwrap_or_default();
            RouterCheckOutcome::Reported {
                stage,
                reason,
                duration_ms: parsed.duration_ms.unwrap_or(0),
            }
        }
        _ => RouterCheckOutcome::Crashed {
            exit_code,
            stderr_tail,
        },
    }
}

/// Take the last `n` chars of `stderr` (not bytes — string slicing on a
/// byte boundary panics on multi-byte UTF-8). Used to surface adapter
/// crashes in a bounded way without dragging the whole stderr buffer
/// into diary entries.
fn stderr_tail(stderr: &str, n: usize) -> String {
    if stderr.chars().count() <= n {
        return stderr.to_string();
    }
    let skip = stderr.chars().count() - n;
    stderr.chars().skip(skip).collect()
}

use crate::{RouterDhcpConfig, RuntimePaths};
use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

/// One tick of work. Implementations spawn the Python adapter (prod) or
/// return canned outcomes (test).
#[async_trait]
pub trait RouterChecker: Send + Sync {
    async fn check_once(&self) -> RouterCheckOutcome;
}

/// Production implementation: spawns `python -m router_adapter` as a
/// one-shot subprocess.
pub struct PythonRouterChecker {
    paths: RuntimePaths,
    config: RouterDhcpConfig,
}

impl PythonRouterChecker {
    /// Construct from config. Returns `None` when credentials are missing —
    /// caller (`RouterWorker`) interprets this as the credential-gate failure
    /// described in the spec.
    pub fn from_config(paths: RuntimePaths, config: RouterDhcpConfig) -> Option<Self> {
        if config.username.is_empty() || config.password.is_empty() {
            return None;
        }
        Some(Self { paths, config })
    }

    fn python_path(&self) -> PathBuf {
        self.paths.router_adapter_python_path()
    }

    fn playwright_browsers_path(&self) -> PathBuf {
        self.paths.playwright_browsers_path()
    }
}

#[async_trait]
impl RouterChecker for PythonRouterChecker {
    async fn check_once(&self) -> RouterCheckOutcome {
        let python = self.python_path();
        if !python.is_file() {
            return RouterCheckOutcome::SpawnFailed {
                reason: format!(
                    "router-adapter python not found at {} — run `daviszeroclaw router-dhcp install`",
                    python.display()
                ),
            };
        }

        let mut cmd = Command::new(&python);
        cmd.arg("-m")
            .arg("router_adapter")
            .env("ROUTER_URL", &self.config.url)
            .env("ROUTER_USERNAME", &self.config.username)
            .env("ROUTER_PASSWORD", &self.config.password)
            .env("PLAYWRIGHT_BROWSERS_PATH", self.playwright_browsers_path())
            .env("PYTHONPATH", self.paths.repo_root.display().to_string())
            .current_dir(&self.paths.repo_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                return RouterCheckOutcome::SpawnFailed {
                    reason: format!("spawn: {err}"),
                };
            }
        };

        let limit = Duration::from_secs(self.config.tick_timeout_secs);
        let output = match timeout(limit, child.wait_with_output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(err)) => {
                return RouterCheckOutcome::Crashed {
                    exit_code: None,
                    stderr_tail: format!("wait_with_output error: {err}"),
                };
            }
            Err(_elapsed) => {
                return RouterCheckOutcome::Crashed {
                    exit_code: None,
                    stderr_tail: format!(
                        "<tick exceeded {}s; child killed via kill_on_drop>",
                        self.config.tick_timeout_secs
                    ),
                };
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        parse_outcome(&stdout, output.status.code(), &stderr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ok_action_none() {
        let stdout = "some log\n{\"status\":\"ok\",\"action\":\"none\",\"dhcp_was_enabled\":false,\"duration_ms\":12000}\n";
        let got = parse_outcome(stdout, Some(0), "");
        assert_eq!(
            got,
            RouterCheckOutcome::Ok {
                action: RouterAction::None,
                dhcp_was_enabled: false,
                duration_ms: 12000,
            }
        );
    }

    #[test]
    fn parses_ok_action_disabled() {
        let stdout =
            r#"{"status":"ok","action":"disabled","dhcp_was_enabled":true,"duration_ms":31000}"#;
        let got = parse_outcome(stdout, Some(0), "");
        assert_eq!(
            got,
            RouterCheckOutcome::Ok {
                action: RouterAction::Disabled,
                dhcp_was_enabled: true,
                duration_ms: 31000,
            }
        );
    }

    #[test]
    fn parses_reported_with_stage_and_reason() {
        let stdout =
            r#"{"status":"error","stage":"login","reason":"selector.timeout","duration_ms":5000}"#;
        let got = parse_outcome(stdout, Some(1), "");
        assert_eq!(
            got,
            RouterCheckOutcome::Reported {
                stage: "login".to_string(),
                reason: "selector.timeout".to_string(),
                duration_ms: 5000,
            }
        );
    }

    #[test]
    fn empty_stdout_yields_crashed() {
        let got = parse_outcome("", Some(139), "Segmentation fault");
        assert_eq!(
            got,
            RouterCheckOutcome::Crashed {
                exit_code: Some(139),
                stderr_tail: "Segmentation fault".to_string(),
            }
        );
    }

    #[test]
    fn malformed_json_last_line_yields_crashed() {
        let got = parse_outcome("not json\n", Some(0), "");
        assert!(matches!(got, RouterCheckOutcome::Crashed { .. }));
    }

    #[test]
    fn missing_required_field_yields_crashed() {
        let stdout = r#"{"status":"ok","action":"none"}"#;
        let got = parse_outcome(stdout, Some(0), "");
        assert!(matches!(got, RouterCheckOutcome::Crashed { .. }));
    }

    #[test]
    fn unknown_status_yields_crashed() {
        let stdout = r#"{"status":"weird","action":"none","dhcp_was_enabled":false}"#;
        let got = parse_outcome(stdout, Some(0), "");
        assert!(matches!(got, RouterCheckOutcome::Crashed { .. }));
    }

    #[test]
    fn unknown_action_yields_crashed() {
        let stdout = r#"{"status":"ok","action":"banana","dhcp_was_enabled":false}"#;
        let got = parse_outcome(stdout, Some(0), "");
        assert!(matches!(got, RouterCheckOutcome::Crashed { .. }));
    }

    #[test]
    fn stderr_tail_handles_multibyte() {
        let s = "前面无关的文字 末尾错误信息";
        let tail = stderr_tail(s, 6);
        assert_eq!(tail.chars().count(), 6);
        assert_eq!(tail, "末尾错误信息");
    }

    #[test]
    fn stderr_tail_returns_full_when_short() {
        assert_eq!(stderr_tail("hi", 256), "hi");
    }

    #[test]
    fn outcome_kind_distinguishes_reported_stages() {
        let login = RouterCheckOutcome::Reported {
            stage: "login".into(),
            reason: "x".into(),
            duration_ms: 0,
        };
        let iframe = RouterCheckOutcome::Reported {
            stage: "iframe".into(),
            reason: "x".into(),
            duration_ms: 0,
        };
        assert_ne!(login.kind(), iframe.kind());
    }

    #[test]
    fn outcome_kind_disabled_vs_none_distinct() {
        let none = RouterCheckOutcome::Ok {
            action: RouterAction::None,
            dhcp_was_enabled: false,
            duration_ms: 0,
        };
        let disabled = RouterCheckOutcome::Ok {
            action: RouterAction::Disabled,
            dhcp_was_enabled: true,
            duration_ms: 0,
        };
        assert_ne!(none.kind(), disabled.kind());
    }
}
