//! Tick loop, dedupe state machine, and AAAK diary formatting for the
//! router DHCP keeper. See
//! `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md`
//! §"Worker dedupe state machine" for the table this implements.

use crate::router_supervisor::{OutcomeKind, RouterAction, RouterCheckOutcome};
use chrono::{DateTime, Utc};

const DIARY_WING: &str = "davis.agent.router-keeper";

/// Worker decision: did this tick warrant a diary entry?
#[derive(Debug, Clone, PartialEq, Eq)]
enum DiaryDecision {
    Write(String),
    Skip,
}

/// Mutable per-worker state. Lives behind a Mutex; tick logic is async.
#[derive(Debug, Default)]
pub struct WorkerState {
    last_kind: Option<OutcomeKind>,
    consecutive_failures: u32,
    last_run: Option<DateTime<Utc>>,
    last_outcome_label: Option<&'static str>,
}

/// Decision rules from the spec, evaluated top-down (first match wins):
/// 1. First ever tick → write.
/// 2. Failure → success → write RECOVERED.
/// 3. Success+Disabled action (regardless of prior state) → write.
/// 4. Same failure kind as last → skip.
/// 5. Different failure kind → write.
/// 6. Success+None when prior was Success+None → skip.
fn decide_and_advance(
    state: &mut WorkerState,
    outcome: &RouterCheckOutcome,
    now: DateTime<Utc>,
) -> DiaryDecision {
    let kind = outcome.kind();
    let prior = state.last_kind.clone();
    let is_first_ever = prior.is_none();
    let prior_was_failure = matches!(
        prior,
        Some(OutcomeKind::Reported(_))
            | Some(OutcomeKind::Crashed)
            | Some(OutcomeKind::SpawnFailed)
    );
    let now_is_success = outcome.is_success();

    // Update fail counter BEFORE deciding so RECOVERED line can read it.
    let prior_failures = state.consecutive_failures;
    state.consecutive_failures = if now_is_success {
        0
    } else {
        prior_failures + 1
    };
    state.last_run = Some(now);
    state.last_outcome_label = Some(outcome_label(outcome));

    let decision = if is_first_ever {
        DiaryDecision::Write(format_tick_line(outcome, now, None))
    } else if prior_was_failure && now_is_success {
        DiaryDecision::Write(format_tick_line(outcome, now, Some(prior_failures)))
    } else if matches!(kind, OutcomeKind::OkDisabled) {
        DiaryDecision::Write(format_tick_line(outcome, now, None))
    } else if Some(&kind) == prior.as_ref() {
        DiaryDecision::Skip
    } else if !now_is_success {
        DiaryDecision::Write(format_tick_line(outcome, now, None))
    } else {
        // Success+None → Success+None
        DiaryDecision::Skip
    };

    state.last_kind = Some(kind);
    decision
}

fn format_tick_line(
    outcome: &RouterCheckOutcome,
    now: DateTime<Utc>,
    recovered_after: Option<u32>,
) -> String {
    let ts = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let recovered_segment = recovered_after
        .map(|n| format!("|RECOVERED|prev.failed.{n}"))
        .unwrap_or_default();
    match outcome {
        RouterCheckOutcome::Ok {
            action: RouterAction::None,
            duration_ms,
            ..
        } => format!(
            "TICK:{ts}|router.dhcp{recovered_segment}|action=none|dur.{}s|✓",
            duration_ms / 1000
        ),
        RouterCheckOutcome::Ok {
            action: RouterAction::Disabled,
            duration_ms,
            ..
        } => format!(
            "TICK:{ts}|router.dhcp{recovered_segment}|action=disabled|was.on|dur.{}s|★",
            duration_ms / 1000
        ),
        RouterCheckOutcome::Reported {
            stage,
            reason,
            duration_ms,
        } => format!(
            "TICK:{ts}|router.dhcp|stage.{stage}|reason={reason}|dur.{}s|⚠️",
            duration_ms / 1000
        ),
        RouterCheckOutcome::Crashed {
            exit_code,
            stderr_tail,
        } => {
            let code = exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none".to_string());
            // Truncate stderr_tail in the diary line — full tail still
            // visible via tracing.
            let snippet: String = stderr_tail.chars().take(80).collect();
            format!("TICK:{ts}|router.dhcp|crash|exit.{code}|err={snippet}|⚠️")
        }
        RouterCheckOutcome::SpawnFailed { reason } => {
            format!("TICK:{ts}|router.dhcp|spawn.fail|reason={reason}|⚠️")
        }
    }
}

fn outcome_label(outcome: &RouterCheckOutcome) -> &'static str {
    match outcome {
        RouterCheckOutcome::Ok { .. } => "ok",
        RouterCheckOutcome::Reported { .. } => "reported",
        RouterCheckOutcome::Crashed { .. } => "crashed",
        RouterCheckOutcome::SpawnFailed { .. } => "spawn_failed",
    }
}

/// Read-only snapshot of worker health for the daemon's `/health` route.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RouterHealthSnapshot {
    pub enabled: bool,
    pub last_run: Option<DateTime<Utc>>,
    pub last_outcome: Option<&'static str>,
    pub consecutive_failures: u32,
}

/// Periodic worker. Holds a checker (prod or test), a MemPalace sink,
/// the config, and mutable state behind a Mutex.
pub struct RouterWorker {
    config: crate::RouterDhcpConfig,
    checker: std::sync::Arc<dyn crate::router_supervisor::RouterChecker>,
    sink: std::sync::Arc<dyn crate::mempalace_sink::MempalaceEmitter>,
    state: tokio::sync::Mutex<WorkerState>,
    /// `false` ⇒ creds were absent at construction; we never call
    /// `checker.check_once()` and ticks are no-ops.
    creds_present: bool,
}

impl RouterWorker {
    pub fn new(
        config: crate::RouterDhcpConfig,
        checker: std::sync::Arc<dyn crate::router_supervisor::RouterChecker>,
        sink: std::sync::Arc<dyn crate::mempalace_sink::MempalaceEmitter>,
        creds_present: bool,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            config,
            checker,
            sink,
            state: tokio::sync::Mutex::new(WorkerState::default()),
            creds_present,
        })
    }

    /// Run exactly one tick. Returns immediately. No retries.
    pub async fn run_one_tick(&self) {
        if !self.creds_present {
            return;
        }
        let outcome = self.checker.check_once().await;
        self.record(&outcome).await;
    }

    /// Long-running loop. Spawned by the daemon. Bails out immediately
    /// when `creds_present == false` after writing one INIT diary line.
    pub async fn run_loop(self: std::sync::Arc<Self>) {
        if !self.creds_present {
            let now = Utc::now();
            let line = format!(
                "INIT:{}|router.dhcp|disabled.no.creds|⚠️",
                now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            );
            tracing::warn!("router-dhcp: required env vars unset; worker self-disabled");
            self.sink.diary_write(DIARY_WING, &line);
            return;
        }
        tracing::info!(
            interval_secs = self.config.interval_secs,
            "router-dhcp worker starting tick loop"
        );
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(self.config.interval_secs));
        // Skip the immediate-fire tick to give Davis a quiet boot.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            self.run_one_tick().await;
        }
    }

    /// Snapshot for `/health` JSON.
    pub async fn health_snapshot(&self) -> RouterHealthSnapshot {
        let state = self.state.lock().await;
        RouterHealthSnapshot {
            enabled: self.config.enabled,
            last_run: state.last_run,
            last_outcome: state.last_outcome_label,
            consecutive_failures: state.consecutive_failures,
        }
    }

    async fn record(&self, outcome: &RouterCheckOutcome) {
        let now = Utc::now();
        // Hold the state lock only long enough to advance the dedupe FSM.
        // Diary write happens outside the lock so the (synchronous, internal-
        // mutex-acquiring) sink path does not nest under our async lock.
        let decision = {
            let mut state = self.state.lock().await;
            decide_and_advance(&mut state, outcome, now)
        };
        match outcome {
            RouterCheckOutcome::Ok {
                action,
                duration_ms,
                ..
            } => {
                tracing::debug!(?action, duration_ms, "router-dhcp tick ok");
            }
            RouterCheckOutcome::Reported {
                stage,
                reason,
                duration_ms,
            } => {
                tracing::warn!(%stage, %reason, duration_ms, "router-dhcp tick failed (reported)");
            }
            RouterCheckOutcome::Crashed {
                exit_code,
                stderr_tail,
            } => {
                tracing::error!(?exit_code, stderr_tail = %stderr_tail, "router-dhcp tick crashed");
            }
            RouterCheckOutcome::SpawnFailed { reason } => {
                tracing::error!(%reason, "router-dhcp tick spawn failed");
            }
        }
        if let DiaryDecision::Write(line) = decision {
            self.sink.diary_write(DIARY_WING, &line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mempalace_sink::testing::NoopSink;
    use async_trait::async_trait;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn ok_none() -> RouterCheckOutcome {
        RouterCheckOutcome::Ok {
            action: RouterAction::None,
            dhcp_was_enabled: false,
            duration_ms: 28000,
        }
    }
    fn ok_disabled() -> RouterCheckOutcome {
        RouterCheckOutcome::Ok {
            action: RouterAction::Disabled,
            dhcp_was_enabled: true,
            duration_ms: 31000,
        }
    }
    fn reported(stage: &str) -> RouterCheckOutcome {
        RouterCheckOutcome::Reported {
            stage: stage.into(),
            reason: "selector.timeout".into(),
            duration_ms: 12000,
        }
    }

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[test]
    fn first_tick_always_writes() {
        let mut state = WorkerState::default();
        let d = decide_and_advance(&mut state, &ok_none(), ts(1_700_000_000));
        assert!(matches!(d, DiaryDecision::Write(_)));
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn second_ok_none_after_first_ok_none_is_skipped() {
        let mut state = WorkerState::default();
        let _ = decide_and_advance(&mut state, &ok_none(), ts(1));
        let d = decide_and_advance(&mut state, &ok_none(), ts(2));
        assert_eq!(d, DiaryDecision::Skip);
    }

    #[test]
    fn ok_disabled_always_writes() {
        let mut state = WorkerState::default();
        let _ = decide_and_advance(&mut state, &ok_none(), ts(1));
        let d = decide_and_advance(&mut state, &ok_disabled(), ts(2));
        assert!(matches!(d, DiaryDecision::Write(s) if s.contains("action=disabled")));
    }

    #[test]
    fn three_identical_failures_dedupe_to_one_diary() {
        let mut state = WorkerState::default();
        let mut writes = 0;
        for _ in 0..3 {
            if matches!(
                decide_and_advance(&mut state, &reported("login"), ts(1)),
                DiaryDecision::Write(_)
            ) {
                writes += 1;
            }
        }
        assert_eq!(writes, 1);
        assert_eq!(state.consecutive_failures, 3);
    }

    #[test]
    fn different_failure_stage_writes_new_entry() {
        let mut state = WorkerState::default();
        let _ = decide_and_advance(&mut state, &reported("login"), ts(1));
        let d = decide_and_advance(&mut state, &reported("iframe"), ts(2));
        assert!(matches!(d, DiaryDecision::Write(s) if s.contains("stage.iframe")));
    }

    #[test]
    fn failure_counter_increments_even_on_dedupe_skip() {
        // Pin the invariant the recovery test depends on: when rule 4 (same
        // failure kind → skip) fires, the consecutive-failure counter still
        // advances. Otherwise RECOVERED|prev.failed.N would be wrong.
        let mut state = WorkerState::default();
        let d1 = decide_and_advance(&mut state, &reported("login"), ts(1));
        assert!(matches!(d1, DiaryDecision::Write(_)));
        assert_eq!(state.consecutive_failures, 1);
        let d2 = decide_and_advance(&mut state, &reported("login"), ts(2));
        assert_eq!(d2, DiaryDecision::Skip);
        assert_eq!(state.consecutive_failures, 2);
        let d3 = decide_and_advance(&mut state, &reported("login"), ts(3));
        assert_eq!(d3, DiaryDecision::Skip);
        assert_eq!(state.consecutive_failures, 3);
    }

    #[test]
    fn recovery_writes_with_prev_failed_count() {
        let mut state = WorkerState::default();
        for _ in 0..3 {
            let _ = decide_and_advance(&mut state, &reported("login"), ts(1));
        }
        let d = decide_and_advance(&mut state, &ok_none(), ts(2));
        match d {
            DiaryDecision::Write(s) => {
                assert!(s.contains("RECOVERED"), "got: {s}");
                assert!(s.contains("prev.failed.3"), "got: {s}");
            }
            DiaryDecision::Skip => panic!("recovery must write"),
        }
        assert_eq!(state.consecutive_failures, 0);
    }

    #[test]
    fn snapshot_reflects_state() {
        let mut state = WorkerState::default();
        let _ = decide_and_advance(&mut state, &reported("login"), ts(1_700_000_000));
        let snap = RouterHealthSnapshot {
            enabled: true,
            last_run: state.last_run,
            last_outcome: state.last_outcome_label,
            consecutive_failures: state.consecutive_failures,
        };
        assert_eq!(snap.last_outcome, Some("reported"));
        assert_eq!(snap.consecutive_failures, 1);
        assert!(snap.last_run.is_some());
    }

    /// Test double: records call count, returns canned outcomes from a Vec.
    struct FakeChecker {
        outcomes: tokio::sync::Mutex<Vec<RouterCheckOutcome>>,
        call_count: AtomicUsize,
    }

    impl FakeChecker {
        fn new(outcomes: Vec<RouterCheckOutcome>) -> Self {
            Self {
                outcomes: tokio::sync::Mutex::new(outcomes),
                call_count: AtomicUsize::new(0),
            }
        }
        fn calls(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl crate::router_supervisor::RouterChecker for FakeChecker {
        async fn check_once(&self) -> RouterCheckOutcome {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            let mut q = self.outcomes.lock().await;
            q.remove(0)
        }
    }

    fn cfg_enabled() -> crate::RouterDhcpConfig {
        crate::RouterDhcpConfig {
            enabled: true,
            interval_secs: 600,
            tick_timeout_secs: 90,
            url: "http://example".into(),
            username_env: "ROUTER_USERNAME".into(),
            password_env: "ROUTER_PASSWORD".into(),
        }
    }

    #[tokio::test]
    async fn worker_skips_calls_when_creds_self_disabled() {
        let checker = std::sync::Arc::new(FakeChecker::new(vec![ok_none(); 3]));
        let sink: std::sync::Arc<dyn crate::mempalace_sink::MempalaceEmitter> =
            std::sync::Arc::new(NoopSink);
        let worker = RouterWorker::new(
            cfg_enabled(),
            checker.clone(),
            sink,
            /* creds_present = */ false,
        );
        for _ in 0..3 {
            worker.run_one_tick().await;
        }
        assert_eq!(checker.calls(), 0);
        assert!(worker.health_snapshot().await.last_run.is_none());
    }

    #[tokio::test]
    async fn worker_calls_checker_when_creds_present() {
        let checker = std::sync::Arc::new(FakeChecker::new(vec![ok_none()]));
        let sink: std::sync::Arc<dyn crate::mempalace_sink::MempalaceEmitter> =
            std::sync::Arc::new(NoopSink);
        let worker = RouterWorker::new(
            cfg_enabled(),
            checker.clone(),
            sink,
            /* creds_present = */ true,
        );
        worker.run_one_tick().await;
        assert_eq!(checker.calls(), 1);
        let snap = worker.health_snapshot().await;
        assert_eq!(snap.last_outcome, Some("ok"));
    }
}
