//! Fire-and-forget projection driver.
//!
//! The public `MemPalaceSink` never blocks caller paths. Events land in a
//! bounded `mpsc::channel`; a background task drains them and invokes the
//! MCP tools on a long-running child process. Failures are counted, not
//! retried — Davis must stay up even if MemPalace is unreachable.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};

use super::mcp_stdio::{InitializeParams, McpStdioClient};
use super::predicate::{Predicate, TripleId};
use crate::runtime_paths::RuntimePaths;

/// Upper bound on the sink's in-flight event queue. When full, new events
/// are dropped rather than blocking the producer.
const CHANNEL_CAPACITY: usize = 1024;

/// Connection failures in a row that cause the driver to go silent for a
/// cool-off window. Log spam avoidance.
const FAILURE_SILENCE_THRESHOLD: u32 = 5;

/// How long the driver stays silent after crossing the failure threshold.
const SILENCE_DURATION: Duration = Duration::from_secs(300);

/// Minimum delay between reconnect attempts after a child crash.
const RECONNECT_MIN_BACKOFF: Duration = Duration::from_secs(1);

/// Maximum delay between reconnect attempts (exponential ceiling).
const RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Policy knobs the driver loop reads. Production uses `::default()`;
/// tests override to collapse backoff windows.
#[derive(Debug, Clone, Copy)]
struct DriverTimings {
    failure_silence_threshold: u32,
    silence_duration: Duration,
    reconnect_min_backoff: Duration,
    reconnect_max_backoff: Duration,
}

impl Default for DriverTimings {
    fn default() -> Self {
        Self {
            failure_silence_threshold: FAILURE_SILENCE_THRESHOLD,
            silence_duration: SILENCE_DURATION,
            reconnect_min_backoff: RECONNECT_MIN_BACKOFF,
            reconnect_max_backoff: RECONNECT_MAX_BACKOFF,
        }
    }
}

/// Internal event shape sent over the mpsc channel.
#[derive(Debug)]
enum SinkEvent {
    AddDrawer {
        wing: String,
        room: String,
        content: String,
    },
    KgAdd {
        subject: TripleId,
        predicate: Predicate,
        object: TripleId,
    },
    KgInvalidate {
        subject: TripleId,
        predicate: Predicate,
        object: TripleId,
    },
    DiaryWrite {
        wing: String,
        entry: String,
    },
}

/// Observable counters for `/health`. Atomics so metrics don't contend with
/// the driver task.
#[derive(Debug, Default)]
struct SinkMetricsInner {
    sent: AtomicU64,
    dropped: AtomicU64,
    failed: AtomicU64,
    child_restarts: AtomicU64,
    last_error: Mutex<Option<String>>,
    silenced_until: Mutex<Option<Instant>>,
}

/// Snapshot used by `/health` handlers; cloneable and Send.
#[derive(Debug, Clone)]
pub struct SinkMetrics {
    pub sent: u64,
    pub dropped: u64,
    pub failed: u64,
    pub child_restarts: u64,
    pub last_error: Option<String>,
    pub silenced: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Live,
    Disabled,
}

/// Davis → MemPalace projection sink. Clone-friendly; all clones share the
/// same underlying mpsc queue and metrics.
#[derive(Clone)]
pub struct MemPalaceSink {
    tx: Option<mpsc::Sender<SinkEvent>>,
    metrics: Arc<SinkMetricsInner>,
    mode: Mode,
}

impl MemPalaceSink {
    /// Launch the driver against the user's MemPalace venv. Never fails —
    /// if the venv or MCP server is unavailable, the sink starts in a
    /// degraded state that drops events silently after the backoff window.
    pub fn spawn(paths: &RuntimePaths) -> Self {
        let (program, args) = paths.mempalace_mcp_server_cmd();
        Self::spawn_with_command(move || {
            let mut cmd = Command::new(&program);
            cmd.args(&args);
            cmd
        })
    }

    /// Construct a sink that discards every event. Useful in tests and when
    /// a user explicitly disables MemPalace integration.
    pub fn disabled() -> Self {
        Self {
            tx: None,
            metrics: Arc::new(SinkMetricsInner::default()),
            mode: Mode::Disabled,
        }
    }

    /// Test-only constructor: every child spawn attempt fails immediately.
    /// Uses collapsed backoff so tests finish in milliseconds.
    #[cfg(test)]
    pub fn for_test_missing_child() -> Self {
        Self::spawn_with_command_and_timings(
            || {
                let mut cmd = Command::new("/nonexistent/binary-for-davis-sink-test");
                cmd.arg("--unused");
                cmd
            },
            DriverTimings {
                failure_silence_threshold: 3,
                silence_duration: Duration::from_secs(30),
                reconnect_min_backoff: Duration::from_millis(10),
                reconnect_max_backoff: Duration::from_millis(50),
            },
        )
    }

    fn spawn_with_command<F>(builder: F) -> Self
    where
        F: Fn() -> Command + Send + Sync + 'static,
    {
        Self::spawn_with_command_and_timings(builder, DriverTimings::default())
    }

    fn spawn_with_command_and_timings<F>(builder: F, timings: DriverTimings) -> Self
    where
        F: Fn() -> Command + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel::<SinkEvent>(CHANNEL_CAPACITY);
        let metrics = Arc::new(SinkMetricsInner::default());
        let driver_metrics = Arc::clone(&metrics);
        tokio::spawn(async move {
            driver_loop(rx, driver_metrics, builder, timings).await;
        });
        Self {
            tx: Some(tx),
            metrics,
            mode: Mode::Live,
        }
    }

    /// Fire-and-forget: file a verbatim drawer. Dropped if the queue is full.
    pub fn add_drawer(&self, wing: &str, room: &str, content: &str) {
        self.enqueue(SinkEvent::AddDrawer {
            wing: wing.to_string(),
            room: room.to_string(),
            content: content.to_string(),
        });
    }

    /// Fire-and-forget: record a KG triple that becomes valid at `now`.
    pub fn kg_add(&self, subject: TripleId, predicate: Predicate, object: TripleId) {
        self.enqueue(SinkEvent::KgAdd {
            subject,
            predicate,
            object,
        });
    }

    /// Fire-and-forget: mark a previously-added triple as no longer valid.
    pub fn kg_invalidate(&self, subject: TripleId, predicate: Predicate, object: TripleId) {
        self.enqueue(SinkEvent::KgInvalidate {
            subject,
            predicate,
            object,
        });
    }

    /// Fire-and-forget: append an entry to a per-agent diary wing.
    pub fn diary_write(&self, wing: &str, entry: &str) {
        self.enqueue(SinkEvent::DiaryWrite {
            wing: wing.to_string(),
            entry: entry.to_string(),
        });
    }

    /// Snapshot current counters. Safe to call from the HTTP thread pool.
    pub async fn metrics(&self) -> SinkMetrics {
        let last_error = self.metrics.last_error.lock().await.clone();
        let silenced = self
            .metrics
            .silenced_until
            .lock()
            .await
            .map(|deadline| Instant::now() < deadline)
            .unwrap_or(false);
        SinkMetrics {
            sent: self.metrics.sent.load(Ordering::Relaxed),
            dropped: self.metrics.dropped.load(Ordering::Relaxed),
            failed: self.metrics.failed.load(Ordering::Relaxed),
            child_restarts: self.metrics.child_restarts.load(Ordering::Relaxed),
            last_error,
            silenced,
            enabled: self.mode == Mode::Live,
        }
    }

    fn enqueue(&self, event: SinkEvent) {
        let Some(tx) = &self.tx else {
            self.metrics.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        match tx.try_send(event) {
            Ok(()) => {}
            Err(_) => {
                self.metrics.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

async fn driver_loop<F>(
    mut rx: mpsc::Receiver<SinkEvent>,
    metrics: Arc<SinkMetricsInner>,
    builder: F,
    timings: DriverTimings,
) where
    F: Fn() -> Command + Send + Sync + 'static,
{
    let mut client: Option<Arc<McpStdioClient>> = None;
    let mut consecutive_failures: u32 = 0;
    let mut reconnect_backoff = timings.reconnect_min_backoff;

    while let Some(event) = rx.recv().await {
        if is_silenced(&metrics).await {
            metrics.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        if client.is_none() {
            match connect(&builder).await {
                Ok(fresh) => {
                    if consecutive_failures > 0 {
                        metrics.child_restarts.fetch_add(1, Ordering::Relaxed);
                    }
                    client = Some(Arc::new(fresh));
                    consecutive_failures = 0;
                    reconnect_backoff = timings.reconnect_min_backoff;
                }
                Err(err) => {
                    consecutive_failures += 1;
                    record_failure(&metrics, &err).await;
                    metrics.dropped.fetch_add(1, Ordering::Relaxed);
                    if consecutive_failures >= timings.failure_silence_threshold {
                        silence(&metrics, timings.silence_duration).await;
                        consecutive_failures = 0;
                        reconnect_backoff = timings.reconnect_min_backoff;
                    } else {
                        tokio::time::sleep(reconnect_backoff).await;
                        reconnect_backoff =
                            (reconnect_backoff * 2).min(timings.reconnect_max_backoff);
                    }
                    continue;
                }
            }
        }

        let Some(active) = client.clone() else {
            metrics.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        };

        match dispatch(&active, event).await {
            Ok(()) => {
                metrics.sent.fetch_add(1, Ordering::Relaxed);
                consecutive_failures = 0;
            }
            Err(err) => {
                metrics.failed.fetch_add(1, Ordering::Relaxed);
                record_failure(&metrics, &err).await;
                consecutive_failures += 1;
                // Drop the client — reconnect on the next event.
                if let Some(c) = client.take() {
                    tokio::spawn(async move {
                        c.shutdown().await;
                    });
                }
                if consecutive_failures >= timings.failure_silence_threshold {
                    silence(&metrics, timings.silence_duration).await;
                    consecutive_failures = 0;
                }
            }
        }
    }

    if let Some(c) = client {
        c.shutdown().await;
    }
}

async fn connect<F>(builder: &F) -> anyhow::Result<McpStdioClient>
where
    F: Fn() -> Command + Send + Sync + 'static,
{
    let cmd = builder();
    let client = McpStdioClient::spawn(cmd).await?;
    let info = client
        .initialize(&InitializeParams {
            client_name: "davis".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        })
        .await?;
    let server_name = info
        .server_info
        .as_ref()
        .and_then(|s| s.name.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let server_version = info
        .server_info
        .as_ref()
        .and_then(|s| s.version.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let protocol = info.protocol_version.clone().unwrap_or_default();
    tracing::info!(
        target: "mempalace_sink",
        server = %server_name,
        version = %server_version,
        protocol = %protocol,
        "connected to MemPalace MCP server",
    );
    Ok(client)
}

async fn dispatch(client: &McpStdioClient, event: SinkEvent) -> anyhow::Result<()> {
    let (tool, args) = event_to_call(event);
    client.call_tool(&tool, args).await.map(|_| ())
}

fn event_to_call(event: SinkEvent) -> (String, Value) {
    match event {
        SinkEvent::AddDrawer {
            wing,
            room,
            content,
        } => (
            "mempalace_add_drawer".to_string(),
            json!({
                "wing": wing,
                "room": room,
                "content": content,
                "added_by": "davis",
            }),
        ),
        SinkEvent::KgAdd {
            subject,
            predicate,
            object,
        } => (
            "mempalace_kg_add".to_string(),
            json!({
                "subject": subject.as_str(),
                "predicate": predicate.as_str(),
                "object": object.as_str(),
            }),
        ),
        SinkEvent::KgInvalidate {
            subject,
            predicate,
            object,
        } => (
            "mempalace_kg_invalidate".to_string(),
            json!({
                "subject": subject.as_str(),
                "predicate": predicate.as_str(),
                "object": object.as_str(),
            }),
        ),
        SinkEvent::DiaryWrite { wing, entry } => (
            "mempalace_diary_write".to_string(),
            json!({
                "agent_name": wing,
                "entry": entry,
            }),
        ),
    }
}

async fn is_silenced(metrics: &SinkMetricsInner) -> bool {
    let mut guard = metrics.silenced_until.lock().await;
    match *guard {
        Some(deadline) if Instant::now() < deadline => true,
        Some(_) => {
            *guard = None;
            false
        }
        None => false,
    }
}

async fn silence(metrics: &SinkMetricsInner, duration: Duration) {
    let mut guard = metrics.silenced_until.lock().await;
    *guard = Some(Instant::now() + duration);
}

async fn record_failure(metrics: &SinkMetricsInner, err: &anyhow::Error) {
    let mut slot = metrics.last_error.lock().await;
    *slot = Some(err.to_string());
    tracing::warn!(target: "mempalace_sink", error = %err, "sink failure");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn disabled_sink_drops_every_event() {
        let sink = MemPalaceSink::disabled();
        for _ in 0..100 {
            sink.add_drawer("davis:test", "r", "c");
            sink.kg_add(
                TripleId::entity("a"),
                Predicate::EntityHasState,
                TripleId::entity("b"),
            );
            sink.diary_write("davis:agent:x", "hello");
        }
        let m = sink.metrics().await;
        assert_eq!(m.sent, 0);
        assert_eq!(m.failed, 0);
        assert!(m.dropped >= 300, "expected drops, got {m:?}");
        assert!(!m.enabled);
    }

    #[tokio::test]
    async fn missing_child_records_failures_without_panic() {
        let sink = MemPalaceSink::for_test_missing_child();
        sink.add_drawer("davis:test", "r", "c");
        // Give driver a chance to attempt + record the failure.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let m = sink.metrics().await;
        assert_eq!(m.sent, 0);
        assert!(m.dropped >= 1, "{m:?}");
        assert!(m.last_error.is_some(), "{m:?}");
        assert!(m.enabled);
    }

    #[tokio::test]
    async fn silenced_state_flips_after_threshold_failures() {
        let sink = MemPalaceSink::for_test_missing_child();
        // Drive past the threshold. Each event triggers a connect attempt
        // because the driver retries every event until silenced.
        for _ in 0..12 {
            sink.add_drawer("davis:test", "r", "c");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Allow some backoff time.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let m = sink.metrics().await;
        assert!(m.silenced, "sink should be silenced by now: {m:?}");
    }

    #[tokio::test]
    async fn queue_full_increments_drop_counter() {
        // Build a sink whose driver never runs so the channel fills.
        let (tx, _rx) = mpsc::channel::<SinkEvent>(4);
        let sink = MemPalaceSink {
            tx: Some(tx),
            metrics: Arc::new(SinkMetricsInner::default()),
            mode: Mode::Live,
        };
        for _ in 0..50 {
            sink.add_drawer("davis:test", "r", "c");
        }
        let m = sink.metrics().await;
        assert!(m.dropped > 0, "{m:?}");
    }
}
