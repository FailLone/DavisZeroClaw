use crate::{
    isoformat, now_utc, parse_time, AdvisorSuggestion, FailureDetails, FailureEvent, FailureReason,
    FailureState, FailureSummary, RuntimePaths, TopFailedQuery, CONTROL_FAILURE_THRESHOLD,
    CONTROL_FAILURE_WINDOW_HOURS,
};
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

static FAILURE_STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static FAILURE_STATE_TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn failure_state_lock() -> &'static Mutex<()> {
    FAILURE_STATE_LOCK.get_or_init(|| Mutex::new(()))
}

fn load_failure_state_unlocked(paths: &RuntimePaths) -> FailureState {
    let path = paths.failure_state_path();
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<FailureState>(&content).ok())
        .unwrap_or_default()
}

fn write_failure_state_atomic(path: &Path, state: &FailureState) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        FAILURE_STATE_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&temp_path, serde_json::to_vec_pretty(state)?)?;
    fs::rename(&temp_path, path).inspect_err(|_err| {
        let _ = fs::remove_file(&temp_path);
    })?;
    Ok(())
}

pub fn load_failure_state(paths: &RuntimePaths) -> FailureState {
    load_failure_state_unlocked(paths)
}

pub fn prune_failure_state(state: &mut FailureState, now: DateTime<Utc>) {
    let cutoff = now - Duration::hours(CONTROL_FAILURE_WINDOW_HOURS);
    state.events.retain(|event| {
        parse_time(&event.time)
            .map(|time| time >= cutoff)
            .unwrap_or(false)
    });
}

pub fn save_failure_state(paths: &RuntimePaths, state: &FailureState) -> Result<()> {
    let _guard = failure_state_lock().lock().unwrap();
    let path = paths.failure_state_path();
    write_failure_state_atomic(&path, state)?;
    Ok(())
}

pub fn record_control_failure(
    paths: &RuntimePaths,
    query_entity: &str,
    action: &str,
    reason: FailureReason,
    details: Option<FailureDetails>,
) -> Result<()> {
    let _guard = failure_state_lock().lock().unwrap();
    let mut state = load_failure_state_unlocked(paths);
    prune_failure_state(&mut state, now_utc());
    state.events.push(FailureEvent {
        time: isoformat(now_utc()),
        query_entity: query_entity.to_string(),
        action: action.to_string(),
        reason,
        details,
    });
    let path = paths.failure_state_path();
    write_failure_state_atomic(&path, &state)
}

pub fn build_failure_summary(paths: &RuntimePaths) -> FailureSummary {
    let mut state = load_failure_state(paths);
    prune_failure_state(&mut state, now_utc());
    let mut counts_by_reason: BTreeMap<FailureReason, usize> = BTreeMap::new();
    let mut counts_by_query: HashMap<String, usize> = HashMap::new();
    for event in &state.events {
        *counts_by_reason.entry(event.reason.clone()).or_insert(0) += 1;
        if !event.query_entity.is_empty() {
            *counts_by_query
                .entry(event.query_entity.clone())
                .or_insert(0) += 1;
        }
    }
    let mut top_failed_queries: Vec<_> = counts_by_query.into_iter().collect();
    top_failed_queries
        .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let suggestion_due = if state.events.len() < CONTROL_FAILURE_THRESHOLD {
        false
    } else if let Some(last) = state.last_suggested_at.as_deref().and_then(parse_time) {
        last < now_utc() - Duration::hours(CONTROL_FAILURE_WINDOW_HOURS)
    } else {
        true
    };
    FailureSummary {
        status: "ok".to_string(),
        window_hours: CONTROL_FAILURE_WINDOW_HOURS,
        threshold: CONTROL_FAILURE_THRESHOLD,
        failure_count: state.events.len(),
        counts_by_reason,
        top_failed_queries: top_failed_queries
            .into_iter()
            .take(10)
            .map(|(query_entity, count)| TopFailedQuery {
                query_entity,
                count,
            })
            .collect(),
        events: state.events,
        suggestion_due,
        last_suggested_at: state.last_suggested_at,
    }
}

pub fn build_failure_summary_payload(paths: &RuntimePaths) -> Value {
    serde_json::to_value(build_failure_summary(paths)).unwrap_or_else(|_| {
        serde_json::json!({
            "status":"ok",
            "window_hours": CONTROL_FAILURE_WINDOW_HOURS,
            "threshold": CONTROL_FAILURE_THRESHOLD,
            "failure_count": 0,
            "counts_by_reason": {},
            "top_failed_queries": [],
            "events": [],
            "suggestion_due": false,
        })
    })
}

pub fn maybe_consume_advisor_suggestion(paths: &RuntimePaths) -> Result<Option<AdvisorSuggestion>> {
    let _guard = failure_state_lock().lock().unwrap();
    let mut state = load_failure_state_unlocked(paths);
    prune_failure_state(&mut state, now_utc());
    let suggestion_due = if state.events.len() < CONTROL_FAILURE_THRESHOLD {
        false
    } else if let Some(last) = state.last_suggested_at.as_deref().and_then(parse_time) {
        last < now_utc() - Duration::hours(CONTROL_FAILURE_WINDOW_HOURS)
    } else {
        true
    };
    if !suggestion_due {
        return Ok(None);
    }
    let failure_count = state.events.len();
    state.last_suggested_at = Some(isoformat(now_utc()));
    let path = paths.failure_state_path();
    write_failure_state_atomic(&path, &state)?;
    Ok(Some(AdvisorSuggestion {
        skill: "ha-config-advisor".to_string(),
        message: "最近 24 小时内多次出现实体解析失败，建议运行 ha-config-advisor，整理命名、区域别名和分组配置。".to_string(),
        reason: "repeated_control_resolution_failures".to_string(),
        failure_count,
        window_hours: CONTROL_FAILURE_WINDOW_HOURS,
    }))
}
