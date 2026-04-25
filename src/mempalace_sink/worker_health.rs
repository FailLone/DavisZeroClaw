//! Phase 5 — debounced worker-health / component-reachability projection.
//!
//! Both predicates share the same shape: "component X transitioned from
//! healthy to degraded after sustained failures, or recovered after
//! sustained successes". Centralize the state machine here so both
//! WorkerHealth and ComponentReachability callers get the same debouncing
//! without re-implementing it each time.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::predicate::{Predicate, TripleId};
use super::MempalaceEmitter;

/// State we track per tracked key (worker name, component name, …).
#[derive(Debug)]
struct KeySlot {
    /// True once we've emitted an "unhealthy" kg_add for this key — tells
    /// the next recovery to emit kg_invalidate.
    emitted_unhealthy: bool,
    /// Count of consecutive failing samples since the last healthy one.
    consecutive_failures: u32,
    /// Count of consecutive healthy samples since the last failing one.
    consecutive_successes: u32,
    /// Instant the most recent failure window started; used by the
    /// time-based flavour.
    first_failure_at: Option<Instant>,
    /// Instant the most recent healthy window started; used by the
    /// time-based flavour to time recovery.
    first_healthy_at: Option<Instant>,
}

impl KeySlot {
    fn new() -> Self {
        Self {
            emitted_unhealthy: false,
            consecutive_failures: 0,
            consecutive_successes: 0,
            first_failure_at: None,
            first_healthy_at: None,
        }
    }
}

/// Count-based debounce: flip to unhealthy after `flip_after` consecutive
/// failing samples, recover after `flip_after` consecutive healthy samples.
/// Use this for the ingest worker where we already observe one sample at
/// the end of each job.
#[derive(Debug)]
pub struct SampleDebouncer {
    flip_after: u32,
    slots: Mutex<HashMap<String, KeySlot>>,
    emits: AtomicU32,
}

impl SampleDebouncer {
    pub fn new(flip_after: u32) -> Self {
        Self {
            flip_after: flip_after.max(1),
            slots: Mutex::new(HashMap::new()),
            emits: AtomicU32::new(0),
        }
    }

    pub fn record(
        &self,
        key: &str,
        subject: &TripleId,
        predicate: Predicate,
        object: &TripleId,
        unhealthy: bool,
        emitter: &dyn MempalaceEmitter,
    ) {
        let mut guard = self.slots.lock().expect("SampleDebouncer lock poisoned");
        let slot = guard.entry(key.to_string()).or_insert_with(KeySlot::new);
        if unhealthy {
            slot.consecutive_failures = slot.consecutive_failures.saturating_add(1);
            slot.consecutive_successes = 0;
            if !slot.emitted_unhealthy && slot.consecutive_failures >= self.flip_after {
                slot.emitted_unhealthy = true;
                self.emits.fetch_add(1, Ordering::Relaxed);
                emitter.kg_add(subject.clone(), predicate, object.clone());
            }
        } else {
            slot.consecutive_successes = slot.consecutive_successes.saturating_add(1);
            slot.consecutive_failures = 0;
            if slot.emitted_unhealthy && slot.consecutive_successes >= self.flip_after {
                slot.emitted_unhealthy = false;
                self.emits.fetch_add(1, Ordering::Relaxed);
                emitter.kg_invalidate(subject.clone(), predicate, object.clone());
            }
        }
    }

    #[cfg(test)]
    pub fn total_emits(&self) -> u32 {
        self.emits.load(Ordering::Relaxed)
    }
}

/// Time-based debounce: flip to unhealthy after `sustained` wall-clock time
/// of failing samples, recover after `sustained` of healthy samples. Use
/// this for component reachability where failures come as bursts (a single
/// failed probe shouldn't flip; 30s of failed probes should).
#[derive(Debug)]
pub struct TimeDebouncer {
    sustained: Duration,
    slots: Mutex<HashMap<String, KeySlot>>,
}

impl TimeDebouncer {
    pub fn new(sustained: Duration) -> Self {
        Self {
            sustained,
            slots: Mutex::new(HashMap::new()),
        }
    }

    pub fn record(
        &self,
        key: &str,
        subject: &TripleId,
        predicate: Predicate,
        object: &TripleId,
        unhealthy: bool,
        now: Instant,
        emitter: &dyn MempalaceEmitter,
    ) {
        let mut guard = self.slots.lock().expect("TimeDebouncer lock poisoned");
        let slot = guard.entry(key.to_string()).or_insert_with(KeySlot::new);
        if unhealthy {
            // Entering or continuing a failure window.
            if slot.first_failure_at.is_none() {
                slot.first_failure_at = Some(now);
            }
            slot.first_healthy_at = None;
            let failed_for = slot
                .first_failure_at
                .map(|t| now.saturating_duration_since(t))
                .unwrap_or_default();
            if !slot.emitted_unhealthy && failed_for >= self.sustained {
                slot.emitted_unhealthy = true;
                emitter.kg_add(subject.clone(), predicate, object.clone());
            }
        } else {
            // Entering or continuing a healthy window.
            if slot.first_healthy_at.is_none() {
                slot.first_healthy_at = Some(now);
            }
            slot.first_failure_at = None;
            let healthy_for = slot
                .first_healthy_at
                .map(|t| now.saturating_duration_since(t))
                .unwrap_or_default();
            if slot.emitted_unhealthy && healthy_for >= self.sustained {
                slot.emitted_unhealthy = false;
                emitter.kg_invalidate(subject.clone(), predicate, object.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mempalace_sink::SpySink;

    fn worker_subject() -> TripleId {
        TripleId::worker("ingest")
    }
    fn backlogged_object() -> TripleId {
        TripleId::entity("state.backlogged")
    }

    #[test]
    fn sample_debouncer_flips_only_after_threshold() {
        let debouncer = SampleDebouncer::new(3);
        let spy = SpySink::default();
        for _ in 0..2 {
            debouncer.record(
                "ingest",
                &worker_subject(),
                Predicate::WorkerHealth,
                &backlogged_object(),
                true,
                &spy,
            );
        }
        assert_eq!(debouncer.total_emits(), 0);
        assert!(spy.kg_adds().is_empty());

        debouncer.record(
            "ingest",
            &worker_subject(),
            Predicate::WorkerHealth,
            &backlogged_object(),
            true,
            &spy,
        );
        assert_eq!(debouncer.total_emits(), 1);
        assert_eq!(spy.kg_adds().len(), 1);

        for _ in 0..5 {
            debouncer.record(
                "ingest",
                &worker_subject(),
                Predicate::WorkerHealth,
                &backlogged_object(),
                true,
                &spy,
            );
        }
        assert_eq!(spy.kg_adds().len(), 1, "further failures must not re-emit");
    }

    #[test]
    fn sample_debouncer_recovers_after_threshold_healthy_samples() {
        let debouncer = SampleDebouncer::new(2);
        let spy = SpySink::default();
        for _ in 0..2 {
            debouncer.record(
                "ingest",
                &worker_subject(),
                Predicate::WorkerHealth,
                &backlogged_object(),
                true,
                &spy,
            );
        }
        assert_eq!(spy.kg_adds().len(), 1);
        assert!(spy.kg_invalidates().is_empty());
        debouncer.record(
            "ingest",
            &worker_subject(),
            Predicate::WorkerHealth,
            &backlogged_object(),
            false,
            &spy,
        );
        assert!(spy.kg_invalidates().is_empty());
        debouncer.record(
            "ingest",
            &worker_subject(),
            Predicate::WorkerHealth,
            &backlogged_object(),
            false,
            &spy,
        );
        assert_eq!(spy.kg_invalidates().len(), 1);
    }

    #[test]
    fn sample_debouncer_keys_tracked_independently() {
        let debouncer = SampleDebouncer::new(2);
        let spy = SpySink::default();
        let a = TripleId::worker("a");
        let b = TripleId::worker("b");
        let obj = backlogged_object();
        debouncer.record("a", &a, Predicate::WorkerHealth, &obj, true, &spy);
        debouncer.record("b", &b, Predicate::WorkerHealth, &obj, true, &spy);
        assert!(spy.kg_adds().is_empty());
        debouncer.record("a", &a, Predicate::WorkerHealth, &obj, true, &spy);
        assert_eq!(spy.kg_adds().len(), 1);
        debouncer.record("b", &b, Predicate::WorkerHealth, &obj, true, &spy);
        assert_eq!(spy.kg_adds().len(), 2);
    }

    fn component_subject() -> TripleId {
        TripleId::component("zeroclaw-daemon")
    }
    fn unreachable_object() -> TripleId {
        TripleId::entity("state.unreachable")
    }

    #[test]
    fn time_debouncer_requires_sustained_failure() {
        let debouncer = TimeDebouncer::new(Duration::from_secs(30));
        let spy = SpySink::default();
        let t0 = Instant::now();
        debouncer.record(
            "zeroclaw-daemon",
            &component_subject(),
            Predicate::ComponentReachability,
            &unreachable_object(),
            true,
            t0,
            &spy,
        );
        debouncer.record(
            "zeroclaw-daemon",
            &component_subject(),
            Predicate::ComponentReachability,
            &unreachable_object(),
            true,
            t0 + Duration::from_secs(10),
            &spy,
        );
        assert!(spy.kg_adds().is_empty());
        debouncer.record(
            "zeroclaw-daemon",
            &component_subject(),
            Predicate::ComponentReachability,
            &unreachable_object(),
            true,
            t0 + Duration::from_secs(30),
            &spy,
        );
        assert_eq!(spy.kg_adds().len(), 1);
    }

    #[test]
    fn time_debouncer_single_healthy_sample_resets_failure_window() {
        let debouncer = TimeDebouncer::new(Duration::from_secs(30));
        let spy = SpySink::default();
        let t0 = Instant::now();
        debouncer.record(
            "ha-mcp",
            &TripleId::component("ha-mcp"),
            Predicate::ComponentReachability,
            &unreachable_object(),
            true,
            t0,
            &spy,
        );
        debouncer.record(
            "ha-mcp",
            &TripleId::component("ha-mcp"),
            Predicate::ComponentReachability,
            &unreachable_object(),
            false,
            t0 + Duration::from_secs(20),
            &spy,
        );
        debouncer.record(
            "ha-mcp",
            &TripleId::component("ha-mcp"),
            Predicate::ComponentReachability,
            &unreachable_object(),
            true,
            t0 + Duration::from_secs(25),
            &spy,
        );
        debouncer.record(
            "ha-mcp",
            &TripleId::component("ha-mcp"),
            Predicate::ComponentReachability,
            &unreachable_object(),
            true,
            t0 + Duration::from_secs(40),
            &spy,
        );
        assert!(
            spy.kg_adds().is_empty(),
            "15s after the reset must not flip: {:?}",
            spy.kg_adds()
        );
    }

    #[test]
    fn time_debouncer_recovers_after_sustained_healthy() {
        let debouncer = TimeDebouncer::new(Duration::from_secs(30));
        let spy = SpySink::default();
        let t0 = Instant::now();
        // Fail for 30s — flip.
        debouncer.record(
            "ha-mcp",
            &TripleId::component("ha-mcp"),
            Predicate::ComponentReachability,
            &unreachable_object(),
            true,
            t0,
            &spy,
        );
        debouncer.record(
            "ha-mcp",
            &TripleId::component("ha-mcp"),
            Predicate::ComponentReachability,
            &unreachable_object(),
            true,
            t0 + Duration::from_secs(30),
            &spy,
        );
        assert_eq!(spy.kg_adds().len(), 1);
        // Healthy for 20s — not enough.
        debouncer.record(
            "ha-mcp",
            &TripleId::component("ha-mcp"),
            Predicate::ComponentReachability,
            &unreachable_object(),
            false,
            t0 + Duration::from_secs(40),
            &spy,
        );
        assert!(spy.kg_invalidates().is_empty());
        // Healthy for 30s — recover.
        debouncer.record(
            "ha-mcp",
            &TripleId::component("ha-mcp"),
            Predicate::ComponentReachability,
            &unreachable_object(),
            false,
            t0 + Duration::from_secs(70),
            &spy,
        );
        assert_eq!(spy.kg_invalidates().len(), 1);
    }
}
