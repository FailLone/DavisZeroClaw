use super::fixtures::{
    sample_broken_group_config, sample_config, sample_failure_summary, sample_paths, sample_states,
    sample_states_with_brightness, sample_typed_states, write_control_config,
};
use super::support::{
    audit_config_handler, audit_history_handler, audit_history_with_events_handler,
    audit_logbook_handler, audit_logbook_with_events_handler, spawn_test_client,
    spawn_upstream_client, test_states_handler, TestServerState,
};
use crate::*;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn load_control_config_returns_error_for_missing_file() {
    let paths = sample_paths();
    let error = load_control_config(&paths).unwrap_err();
    assert!(error.to_string().contains("control config not found"));
}

#[test]
fn load_control_config_returns_error_for_invalid_toml() {
    let paths = sample_paths();
    write_control_config(&paths, "= not valid toml");
    let error = load_control_config(&paths).unwrap_err();
    assert!(error.to_string().contains("invalid control config"));
}

#[test]
fn load_control_config_parses_shipped_toml_file() {
    let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let paths = RuntimePaths {
        runtime_dir: repo_root.join(".runtime").join("davis"),
        repo_root,
    };
    let config = load_control_config(&paths).unwrap();
    assert_eq!(
        config.area_aliases.get("父母间").map(Vec::as_slice),
        Some(["次卧".to_string(), "爸妈房".to_string()].as_slice())
    );
    assert!(config.domain_preferences.contains_key("light"));
    assert!(config.room_tokens.contains(&"客厅".to_string()));
}

#[test]
fn resolve_control_target_prefers_alias_and_friendly_name() {
    let states = sample_states();
    let config = sample_config();
    let result = resolve_control_target_with_states("书房灯带", "turn_on", &states, &config);
    assert_eq!(result.status, "ok");
    assert_eq!(
        result.resolved_targets,
        vec!["light.study_strip".to_string()]
    );

    let alias_result = resolve_control_target_with_states("书房灯条", "turn_on", &states, &config);
    assert_eq!(alias_result.status, "ok");
    assert_eq!(
        alias_result.resolved_targets,
        vec!["light.study_strip".to_string()]
    );

    let natural_result =
        resolve_control_target_with_states("请把书房灯带打开一下", "turn_on", &states, &config);
    assert_eq!(natural_result.status, "ok");
    assert_eq!(
        natural_result.resolved_targets,
        vec!["light.study_strip".to_string()]
    );
}

#[test]
fn resolve_control_target_supports_group_alias() {
    let states = sample_states();
    let config = sample_config();
    let result = resolve_control_target_with_states("书房的灯", "turn_on", &states, &config);
    assert_eq!(result.status, "ok");
    assert_eq!(result.resolution_type.as_deref(), Some("group"));
    assert_eq!(
        result.resolved_targets,
        vec![
            "light.study_strip".to_string(),
            "light.study_main".to_string()
        ]
    );

    let natural_group =
        resolve_control_target_with_states("请把书房的灯打开", "turn_on", &states, &config);
    assert_eq!(natural_group.status, "ok");
    assert_eq!(natural_group.resolution_type.as_deref(), Some("group"));
}

#[test]
fn resolve_control_target_marks_duplicate_friendly_name_as_ambiguous() {
    let states = sample_states();
    let config = sample_config();
    let result = resolve_control_target_with_states("父母间吊灯", "turn_on", &states, &config);
    assert_eq!(result.status, "ambiguous");
    assert_eq!(result.reason, Some(FailureReason::ResolutionAmbiguous));
    assert_eq!(result.resolved_targets, Vec::<String>::new());
    assert_eq!(result.candidate_count, Some(2));
    assert_eq!(
        result.suggestions,
        vec![
            "switch.parents_chandelier".to_string(),
            "switch.parents_chandelier_2".to_string()
        ]
    );
}

#[test]
fn resolve_control_target_marks_incomplete_group_as_config_issue() {
    let states = sample_states();
    let config = sample_broken_group_config();
    let result = resolve_control_target_with_states("坏书房灯", "turn_on", &states, &config);
    assert_eq!(result.status, "config_issue");
    assert_eq!(result.reason, Some(FailureReason::GroupMembersMissing));
    assert_eq!(
        result.resolved_targets,
        vec!["light.study_strip".to_string()]
    );
    assert_eq!(
        result.missing_targets,
        vec!["light.study_missing".to_string()]
    );
}

#[test]
fn resolve_entity_basic_returns_typed_resolution() {
    let states = sample_typed_states();
    let resolution = resolve_entity_basic("书房灯带", &states);
    assert_eq!(resolution.status, "ok");
    assert_eq!(resolution.entity_id.as_deref(), Some("light.study_strip"));
    assert_eq!(
        resolution.matched_by.as_deref(),
        Some("exact_friendly_name")
    );
    assert_eq!(
        resolution
            .state
            .as_ref()
            .map(HaState::friendly_name)
            .as_deref(),
        Some("书房灯带")
    );
}

#[test]
fn failure_summary_requires_threshold_and_only_suggests_once() {
    let paths = sample_paths();
    let _ = record_control_failure(
        &paths,
        "父母间吊灯",
        "turn_on",
        FailureReason::ResolutionFailed,
        None,
    );
    let _ = record_control_failure(
        &paths,
        "父母间吊灯",
        "turn_on",
        FailureReason::ResolutionFailed,
        None,
    );
    let _ = record_control_failure(
        &paths,
        "父母间吊灯",
        "turn_on",
        FailureReason::ResolutionFailed,
        None,
    );
    let summary = build_failure_summary_payload(&paths);
    assert_eq!(
        summary.get("failure_count").and_then(Value::as_u64),
        Some(3)
    );
    assert_eq!(
        summary.get("suggestion_due").and_then(Value::as_bool),
        Some(true)
    );

    let suggestion = maybe_consume_advisor_suggestion(&paths).unwrap();
    assert!(suggestion.is_some());

    let summary_after = build_failure_summary_payload(&paths);
    assert_eq!(
        summary_after.get("suggestion_due").and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn record_control_failure_is_safe_under_concurrent_writes() {
    let paths = sample_paths();
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for index in 0..8 {
        let paths = paths.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            record_control_failure(
                &paths,
                &format!("并发设备{index}"),
                "turn_on",
                FailureReason::ResolutionFailed,
                None,
            )
            .unwrap();
        }));
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let summary = build_failure_summary_payload(&paths);
    assert_eq!(
        summary.get("failure_count").and_then(Value::as_u64),
        Some(8)
    );
}

#[test]
fn generate_config_report_detects_duplicates() {
    let states = sample_states();
    let config = sample_config();
    let paths = sample_paths();
    let failure_summary = sample_failure_summary(2);
    let report =
        generate_config_report_with_states(&paths, &states, &config, &failure_summary).unwrap();
    assert_eq!(report.get("status").and_then(Value::as_str), Some("ok"));
    assert!(
        report
            .get("counts")
            .and_then(|value| value.get("duplicate_friendly_names"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            >= 1
    );
    assert!(report
        .get("suggestions")
        .and_then(|value| value.get("groups"))
        .is_some());
}

#[test]
fn generate_config_report_writes_valid_cache_file() {
    let states = sample_states();
    let config = sample_config();
    let paths = sample_paths();
    let failure_summary = sample_failure_summary(1);
    let report =
        generate_config_report_with_states(&paths, &states, &config, &failure_summary).unwrap();
    let raw = std::fs::read_to_string(paths.config_report_cache_path()).unwrap();
    let cached: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(cached.get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(cached, report);
}

#[test]
fn generate_config_report_serializes_typed_failure_summary() {
    let states = sample_states();
    let config = sample_config();
    let paths = sample_paths();
    let failure_summary = sample_failure_summary(3);
    let report =
        generate_config_report_with_states(&paths, &states, &config, &failure_summary).unwrap();
    let typed: ConfigReport = serde_json::from_value(report).unwrap();
    assert_eq!(typed.recent_failures.failure_count, 3);
    assert_eq!(
        typed.recent_failures.top_failed_queries[0].query_entity,
        "书房灯带"
    );
}

#[tokio::test]
async fn execute_control_returns_ambiguous_failure_records_it_and_skips_service_call() {
    let paths = sample_paths();
    let config = sample_config();
    let (client, service_calls) = spawn_test_client(sample_states()).await;

    let response = execute_control(
        &client,
        &paths,
        &config,
        ExecuteControlRequest {
            query: "父母间吊灯".to_string(),
            action: ControlAction::TurnOn,
            ..Default::default()
        },
    )
    .await;

    assert_eq!(response.status, "failed");
    assert_eq!(response.error, Some(FailureReason::ResolutionAmbiguous));
    assert_eq!(response.reason, Some(FailureReason::ResolutionAmbiguous));
    assert_eq!(
        response
            .issue
            .as_ref()
            .map(|issue| issue.issue_type.as_str()),
        Some("entity_ambiguous")
    );
    assert_eq!(service_calls.load(Ordering::Relaxed), 0);

    let summary = build_failure_summary_payload(&paths);
    assert_eq!(
        summary.get("failure_count").and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        summary
            .get("counts_by_reason")
            .and_then(|value: &Value| value.get("resolution_ambiguous"))
            .and_then(Value::as_u64),
        Some(1)
    );
}

#[tokio::test]
async fn audit_entity_skips_global_name_match_for_duplicate_friendly_names() {
    let upstream = spawn_upstream_client(
        Router::new()
            .route("/api/config", get(audit_config_handler))
            .route("/api/states", get(test_states_handler))
            .route("/api/history/period/:start", get(audit_history_handler))
            .route("/api/logbook/:start", get(audit_logbook_handler))
            .with_state(TestServerState {
                states: Arc::new(sample_states()),
                service_calls: Arc::new(AtomicUsize::new(0)),
            }),
    )
    .await;

    let start = DateTime::parse_from_rfc3339("2026-03-29T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let end = DateTime::parse_from_rfc3339("2026-03-29T23:59:59Z")
        .unwrap()
        .with_timezone(&Utc);

    let result = audit_entity(&upstream, "switch.parents_chandelier", start, end).await;
    assert_eq!(
        result.get("result_type").and_then(Value::as_str),
        Some("no_evidence")
    );
}

#[tokio::test]
async fn audit_entity_serializes_typed_timeline_entries_in_evidence_mode() {
    let upstream = spawn_upstream_client(
        Router::new()
            .route("/api/config", get(audit_config_handler))
            .route("/api/states", get(test_states_handler))
            .route(
                "/api/history/period/:start",
                get(audit_history_with_events_handler),
            )
            .route(
                "/api/logbook/:start",
                get(audit_logbook_with_events_handler),
            )
            .with_state(TestServerState {
                states: Arc::new(sample_states()),
                service_calls: Arc::new(AtomicUsize::new(0)),
            }),
    )
    .await;

    let start = DateTime::parse_from_rfc3339("2026-03-29T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let end = DateTime::parse_from_rfc3339("2026-03-29T23:59:59Z")
        .unwrap()
        .with_timezone(&Utc);

    let result = audit_entity(&upstream, "light.study_strip", start, end).await;
    let typed: AuditEvidenceResult = serde_json::from_value(result).unwrap();
    assert_eq!(typed.result_type, "evidence");
    assert_eq!(typed.actor.actor_type, "user");
    assert_eq!(typed.source.id.as_deref(), Some("HomeKit Bridge"));
    assert_eq!(typed.entities.len(), 1);
    assert_eq!(typed.entities[0].timeline.len(), 2);
    assert_eq!(typed.entities[0].timeline[0].source, "history");
    assert_eq!(
        typed.entities[0].timeline[1].context_entity_id.as_deref(),
        Some("light.study_strip")
    );
}

#[tokio::test]
async fn execute_control_returns_group_config_issue_and_skips_service_call() {
    let paths = sample_paths();
    let config = sample_broken_group_config();
    let (client, service_calls) = spawn_test_client(sample_states()).await;

    let response = execute_control(
        &client,
        &paths,
        &config,
        ExecuteControlRequest {
            query: "坏书房灯".to_string(),
            action: ControlAction::TurnOn,
            ..Default::default()
        },
    )
    .await;

    assert_eq!(response.status, "failed");
    assert_eq!(response.reason, Some(FailureReason::GroupMembersMissing));
    assert_eq!(
        response
            .issue
            .as_ref()
            .map(|issue| issue.issue_type.as_str()),
        Some("group_members_missing")
    );
    assert_eq!(service_calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        response
            .resolution
            .as_ref()
            .map(|resolution| resolution.missing_targets.clone()),
        Some(vec!["light.study_missing".to_string()])
    );

    let summary = build_failure_summary(&paths);
    assert!(matches!(
        summary.events[0].details.as_ref(),
        Some(FailureDetails::Resolution { resolution })
            if resolution.reason == Some(FailureReason::GroupMembersMissing)
    ));
}

#[tokio::test]
async fn execute_control_returns_typed_execution_errors_and_records_them() {
    let paths = sample_paths();
    let config = sample_config();
    let (client, service_calls) = spawn_test_client(sample_states()).await;

    let response = execute_control(
        &client,
        &paths,
        &config,
        ExecuteControlRequest {
            query: "书房灯带".to_string(),
            action: ControlAction::SetBrightness,
            service_data: json!({}),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(response.status, "failed");
    assert_eq!(response.reason, Some(FailureReason::ExecutionFailed));
    assert_eq!(service_calls.load(Ordering::Relaxed), 0);
    assert_eq!(response.errors.len(), 1);
    assert_eq!(response.errors[0].domain.as_deref(), Some("light"));
    assert_eq!(
        response.errors[0].entity_ids,
        vec!["light.study_strip".to_string()]
    );

    let summary = build_failure_summary(&paths);
    assert!(matches!(
        summary.events[0].details.as_ref(),
        Some(FailureDetails::ExecutionErrors { errors })
            if errors.len() == 1
                && errors[0].domain.as_deref() == Some("light")
                && errors[0].entity_ids == vec!["light.study_strip".to_string()]
    ));
}

#[tokio::test]
async fn execute_control_accepts_typed_brightness_service_data() {
    let paths = sample_paths();
    let config = sample_config();
    let (client, service_calls) = spawn_test_client(sample_states()).await;

    let response = execute_control(
        &client,
        &paths,
        &config,
        ExecuteControlRequest {
            query: "书房灯带".to_string(),
            action: ControlAction::SetBrightness,
            service_data: json!({"brightness_pct": 55}),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(response.status, "success");
    assert_eq!(service_calls.load(Ordering::Relaxed), 1);
    assert_eq!(response.executed_services.len(), 1);
    assert_eq!(response.executed_services[0].service, "turn_on");
    assert_eq!(response.speech.as_deref(), Some("书房灯带亮度已调到55%。"));
}

#[tokio::test]
async fn execute_control_rejects_non_object_service_data() {
    let paths = sample_paths();
    let config = sample_config();
    let (client, service_calls) = spawn_test_client(sample_states()).await;

    let response = execute_control(
        &client,
        &paths,
        &config,
        ExecuteControlRequest {
            query: "书房灯带".to_string(),
            action: ControlAction::TurnOn,
            service_data: json!("invalid"),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(response.status, "failed");
    assert_eq!(response.reason, Some(FailureReason::ExecutionFailed));
    assert_eq!(service_calls.load(Ordering::Relaxed), 0);
    assert_eq!(response.errors.len(), 1);
    assert_eq!(
        response.errors[0].error,
        "service_data 必须是 object 或 null"
    );
}

#[tokio::test]
async fn execute_control_query_state_formats_siri_friendly_brightness() {
    let paths = sample_paths();
    let config = sample_config();
    let (client, _service_calls) = spawn_test_client(sample_states_with_brightness()).await;

    let response = execute_control(
        &client,
        &paths,
        &config,
        ExecuteControlRequest {
            query: "书房灯带".to_string(),
            action: ControlAction::QueryState,
            ..Default::default()
        },
    )
    .await;

    assert_eq!(response.status, "success");
    assert_eq!(
        response.speech.as_deref(),
        Some("书房灯带已打开，亮度51%。")
    );
    assert_eq!(response.targets[0].brightness_pct, Some(51));
}
