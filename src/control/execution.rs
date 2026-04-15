use crate::{
    build_issue, entity_domain, fetch_all_states_typed, ControlAction, ControlConfig,
    ControlResolution, ControlTargetState, ExecuteControlRequest, ExecuteControlResponse,
    ExecutionError, FailureDetails, FailureReason, HaClient, HaState, ProxyError, RuntimePaths,
    ServiceExecution,
};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use super::failures::{maybe_consume_advisor_suggestion, record_control_failure};
use super::resolver::resolve_control_target_with_typed_states;

#[derive(Debug, Clone)]
enum ParsedServiceData {
    Empty,
    Object(BTreeMap<String, Value>),
    Brightness(BTreeMap<String, Value>),
}

fn summarize_target_states(
    entity_ids: &[String],
    all_states: &[HaState],
) -> Vec<ControlTargetState> {
    entity_ids
        .iter()
        .map(|target_id| {
            let state = all_states
                .iter()
                .find(|state| state.entity_id == *target_id);
            ControlTargetState {
                entity_id: target_id.clone(),
                friendly_name: state
                    .map(HaState::friendly_name)
                    .unwrap_or_else(|| target_id.clone()),
                domain: Some(entity_domain(target_id)),
                state: state.and_then(HaState::current_state),
                brightness_pct: state.and_then(HaState::brightness_pct),
            }
        })
        .collect()
}

fn parse_service_data(action: &ControlAction, service_data: &Value) -> Result<ParsedServiceData> {
    match action {
        ControlAction::SetBrightness => {
            let Some(object) = service_data.as_object() else {
                return Err(anyhow!("set_brightness 的 service_data 必须是 object"));
            };
            if object
                .get("brightness_pct")
                .and_then(Value::as_u64)
                .is_none()
            {
                return Err(anyhow!("set_brightness 缺少 brightness_pct"));
            }
            Ok(ParsedServiceData::Brightness(
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ))
        }
        _ if service_data.is_null() => Ok(ParsedServiceData::Empty),
        _ => {
            let Some(object) = service_data.as_object() else {
                return Err(anyhow!("service_data 必须是 object 或 null"));
            };
            Ok(ParsedServiceData::Object(
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ))
        }
    }
}

fn action_to_service(
    action: &ControlAction,
    domain: &str,
    service_data: &ParsedServiceData,
) -> Result<(String, Value)> {
    match action {
        ControlAction::TurnOn => Ok((
            "turn_on".to_string(),
            match service_data {
                ParsedServiceData::Empty => Value::Null,
                ParsedServiceData::Object(object) | ParsedServiceData::Brightness(object) => {
                    Value::Object(object.clone().into_iter().collect())
                }
            },
        )),
        ControlAction::TurnOff => Ok((
            "turn_off".to_string(),
            match service_data {
                ParsedServiceData::Empty => Value::Null,
                ParsedServiceData::Object(object) | ParsedServiceData::Brightness(object) => {
                    Value::Object(object.clone().into_iter().collect())
                }
            },
        )),
        ControlAction::Toggle => Ok((
            "toggle".to_string(),
            match service_data {
                ParsedServiceData::Empty => Value::Null,
                ParsedServiceData::Object(object) | ParsedServiceData::Brightness(object) => {
                    Value::Object(object.clone().into_iter().collect())
                }
            },
        )),
        ControlAction::SetBrightness if domain == "light" => match service_data {
            ParsedServiceData::Brightness(object) => Ok((
                "turn_on".to_string(),
                Value::Object(object.clone().into_iter().collect()),
            )),
            _ => Err(anyhow!("set_brightness 缺少 brightness_pct")),
        },
        ControlAction::SetBrightness => Err(anyhow!("set_brightness 只支持 light")),
        other => Err(anyhow!("不支持的 action: {}", other.as_str())),
    }
}

fn attach_entity_ids(entity_ids: &[String], payload_data: Value) -> Value {
    if payload_data.is_null() {
        json!({"entity_id": if entity_ids.len() == 1 { Value::String(entity_ids[0].clone()) } else { json!(entity_ids.to_vec()) }})
    } else if let Some(map) = payload_data.as_object() {
        let mut object = map.clone();
        object.insert(
            "entity_id".to_string(),
            if entity_ids.len() == 1 {
                Value::String(entity_ids[0].clone())
            } else {
                json!(entity_ids.to_vec())
            },
        );
        Value::Object(object)
    } else {
        json!({"entity_id": entity_ids.to_vec()})
    }
}

fn build_control_speech(
    action: &ControlAction,
    resolution: &ControlResolution,
    targets: &[ControlTargetState],
    requested_brightness_pct: Option<u8>,
) -> String {
    let target_name = if resolution.resolution_type.as_deref() == Some("entity") {
        targets
            .first()
            .map(|item| item.friendly_name.clone())
            .unwrap_or_else(|| resolution.query_entity.clone())
    } else {
        resolution.query_entity.clone()
    };
    match action {
        ControlAction::TurnOn => format!("{target_name}已打开。"),
        ControlAction::TurnOff => format!("{target_name}已关闭。"),
        ControlAction::Toggle => format!("{target_name}已切换。"),
        ControlAction::SetBrightness => {
            if let Some(brightness_pct) = requested_brightness_pct
                .or_else(|| targets.first().and_then(|target| target.brightness_pct))
            {
                format!("{target_name}亮度已调到{brightness_pct}%。")
            } else {
                format!("{target_name}亮度已调整。")
            }
        }
        ControlAction::QueryState => {
            if targets.is_empty() {
                return format!("{target_name}当前状态未知。");
            }
            let parts: Vec<String> = targets
                .iter()
                .map(|target| {
                    let state_text = match target.state.as_deref() {
                        Some("on") => match target.brightness_pct {
                            Some(brightness_pct) if target.domain.as_deref() == Some("light") => {
                                format!("已打开，亮度{brightness_pct}%")
                            }
                            _ => "已打开".to_string(),
                        },
                        Some("off") => "已关闭".to_string(),
                        Some("unavailable") => "当前不可用".to_string(),
                        Some(state) => format!("当前状态是{state}"),
                        None => "当前状态未知".to_string(),
                    };
                    format!("{}{}", target.friendly_name, state_text)
                })
                .collect();
            format!("{}。", parts.join("；"))
        }
        _ => format!("{target_name}已执行。"),
    }
}

fn extract_requested_brightness_pct(service_data: &Value) -> Option<u8> {
    service_data
        .as_object()
        .and_then(|object| object.get("brightness_pct"))
        .and_then(Value::as_u64)
        .map(|value| value.min(100) as u8)
}

pub async fn execute_control(
    client: &HaClient,
    paths: &RuntimePaths,
    config: &ControlConfig,
    request: ExecuteControlRequest,
) -> ExecuteControlResponse {
    if request.action == ControlAction::Unknown {
        return ExecuteControlResponse {
            status: "failed".to_string(),
            error: Some(FailureReason::MissingAction),
            reason: Some(FailureReason::MissingAction),
            issue: Some(build_issue("bad_request", &request.query, vec![])),
            ..Default::default()
        };
    }
    let all_states = match fetch_all_states_typed(client).await {
        Ok(states) => states,
        Err(ProxyError::MissingCredentials) => {
            return ExecuteControlResponse {
                status: "failed".to_string(),
                error: Some(FailureReason::MissingCredentials),
                reason: Some(FailureReason::MissingCredentials),
                issue: Some(build_issue("missing_credentials", &request.query, vec![])),
                ..Default::default()
            };
        }
        Err(ProxyError::AuthFailed) => {
            return ExecuteControlResponse {
                status: "failed".to_string(),
                error: Some(FailureReason::HaAuthFailed),
                reason: Some(FailureReason::HaAuthFailed),
                issue: Some(build_issue("ha_auth_failed", &request.query, vec![])),
                ..Default::default()
            };
        }
        Err(_) => {
            return ExecuteControlResponse {
                status: "failed".to_string(),
                error: Some(FailureReason::HaUnreachable),
                reason: Some(FailureReason::HaUnreachable),
                issue: Some(build_issue("ha_unreachable", &request.query, vec![])),
                ..Default::default()
            };
        }
    };
    let query_entity = if !request.query.trim().is_empty() {
        request.query.trim().to_string()
    } else if !request.query_entity.trim().is_empty() {
        request.query_entity.trim().to_string()
    } else {
        request.raw_text.trim().to_string()
    };
    let resolution = if !request.targets.is_empty() {
        ControlResolution {
            status: "ok".to_string(),
            query_entity: query_entity.clone(),
            action: request.action.clone(),
            resolution_type: Some(
                if request.targets.len() == 1 {
                    "entity"
                } else {
                    "group"
                }
                .to_string(),
            ),
            resolved_targets: request.targets.clone(),
            missing_targets: Vec::new(),
            matched_by: Some("explicit_targets".to_string()),
            confidence: Some("high".to_string()),
            best_guess_used: Some(false),
            candidate_count: Some(request.targets.len()),
            ..Default::default()
        }
    } else {
        resolve_control_target_with_typed_states(
            &query_entity,
            &request.action,
            &all_states,
            config,
        )
    };
    if resolution.status == "ambiguous" {
        let _ = record_control_failure(
            paths,
            &query_entity,
            request.action.as_str(),
            FailureReason::ResolutionAmbiguous,
            Some(FailureDetails::Resolution {
                resolution: Box::new(resolution.clone()),
            }),
        );
        return ExecuteControlResponse {
            status: "failed".to_string(),
            error: Some(FailureReason::ResolutionAmbiguous),
            reason: Some(FailureReason::ResolutionAmbiguous),
            issue: Some(build_issue(
                "entity_ambiguous",
                &query_entity,
                resolution.suggestions.clone(),
            )),
            resolution: Some(resolution),
            advisor_suggestion: maybe_consume_advisor_suggestion(paths).ok().flatten(),
            ..Default::default()
        };
    }
    if resolution.status != "ok" {
        let failure_reason = resolution
            .reason
            .clone()
            .unwrap_or(FailureReason::ResolutionFailed);
        let _ = record_control_failure(
            paths,
            &query_entity,
            request.action.as_str(),
            failure_reason.clone(),
            Some(FailureDetails::Resolution {
                resolution: Box::new(resolution.clone()),
            }),
        );
        return ExecuteControlResponse {
            status: "failed".to_string(),
            error: Some(failure_reason.clone()),
            reason: Some(failure_reason.clone()),
            issue: if failure_reason == FailureReason::GroupMembersMissing {
                Some(build_issue(
                    "group_members_missing",
                    &query_entity,
                    resolution.missing_targets.clone(),
                ))
            } else {
                None
            },
            resolution: Some(resolution),
            advisor_suggestion: maybe_consume_advisor_suggestion(paths).ok().flatten(),
            ..Default::default()
        };
    }
    let targets = resolution.resolved_targets.clone();
    let requested_brightness_pct = extract_requested_brightness_pct(&request.service_data);
    if request.action == ControlAction::QueryState {
        let rows = summarize_target_states(&targets, &all_states);
        return ExecuteControlResponse {
            status: "success".to_string(),
            action: Some(request.action.clone()),
            speech: Some(build_control_speech(
                &request.action,
                &resolution,
                &rows,
                requested_brightness_pct,
            )),
            resolution: Some(resolution),
            targets: rows,
            ..Default::default()
        };
    }
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entity_id in &targets {
        grouped
            .entry(entity_domain(entity_id))
            .or_default()
            .push(entity_id.clone());
    }
    let mut executed_services = Vec::new();
    let mut errors = Vec::new();
    let parsed_service_data = match parse_service_data(&request.action, &request.service_data) {
        Ok(data) => Some(data),
        Err(err) => {
            for (domain, entity_ids) in &grouped {
                errors.push(ExecutionError {
                    error: err.to_string(),
                    domain: Some(domain.clone()),
                    entity_ids: entity_ids.clone(),
                });
            }
            None
        }
    };
    for (domain, entity_ids) in grouped {
        let Some(parsed_service_data) = parsed_service_data.as_ref() else {
            continue;
        };
        match action_to_service(&request.action, &domain, parsed_service_data) {
            Ok((service, payload_data)) => {
                let payload = attach_entity_ids(&entity_ids, payload_data);
                match client
                    .post_value(&format!("/api/services/{domain}/{service}"), payload)
                    .await
                {
                    Ok(_) => executed_services.push(ServiceExecution {
                        domain,
                        service,
                        entity_ids,
                    }),
                    Err(err) => errors.push(ExecutionError {
                        error: format!("{err:?}"),
                        domain: None,
                        entity_ids: Vec::new(),
                    }),
                }
            }
            Err(err) => errors.push(ExecutionError {
                error: err.to_string(),
                domain: Some(domain),
                entity_ids,
            }),
        }
    }
    let refreshed_states = fetch_all_states_typed(client).await.unwrap_or(all_states);
    let target_rows = summarize_target_states(&targets, &refreshed_states);
    if !executed_services.is_empty() && errors.is_empty() {
        ExecuteControlResponse {
            status: "success".to_string(),
            action: Some(request.action.clone()),
            speech: Some(build_control_speech(
                &request.action,
                &resolution,
                &target_rows,
                requested_brightness_pct,
            )),
            resolution: Some(resolution),
            executed_services,
            targets: target_rows,
            ..Default::default()
        }
    } else if !executed_services.is_empty() {
        ExecuteControlResponse {
            status: "partial_success".to_string(),
            action: Some(request.action.clone()),
            speech: Some(build_control_speech(
                &request.action,
                &resolution,
                &target_rows,
                requested_brightness_pct,
            )),
            resolution: Some(resolution),
            executed_services,
            errors,
            targets: target_rows,
            ..Default::default()
        }
    } else {
        let _ = record_control_failure(
            paths,
            &query_entity,
            request.action.as_str(),
            FailureReason::ExecutionFailed,
            Some(FailureDetails::ExecutionErrors {
                errors: errors.clone(),
            }),
        );
        ExecuteControlResponse {
            status: "failed".to_string(),
            error: Some(FailureReason::ExecutionFailed),
            reason: Some(FailureReason::ExecutionFailed),
            action: Some(request.action.clone()),
            resolution: Some(resolution),
            errors,
            targets: target_rows,
            advisor_suggestion: maybe_consume_advisor_suggestion(paths).ok().flatten(),
            ..Default::default()
        }
    }
}
