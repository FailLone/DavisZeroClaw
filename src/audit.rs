use crate::{
    build_issue, fetch_all_states_typed, isoformat, parse_time, related_entity_ids,
    resolve_entity_basic, AuditActor, AuditConfigIssueResult, AuditCounts, AuditEntityRow,
    AuditEvidenceResult, AuditFindings, AuditNoEvidenceResult, AuditSource, AuditSourceObservation,
    AuditTimelineEntry, HaClient, HaState, ProxyError, DEFAULT_WINDOW_MINUTES,
};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Deserialize, Default)]
struct HistoryAttributes {
    #[serde(default)]
    friendly_name: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct HistoryRow {
    #[serde(default)]
    last_changed: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    attributes: Option<HistoryAttributes>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct LogbookRow {
    #[serde(default)]
    when: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    entity_id: Option<String>,
    #[serde(default)]
    context_entity_id: Option<String>,
    #[serde(default)]
    context_state: Option<String>,
    #[serde(default)]
    context_user_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct AuditConfig {
    #[serde(default)]
    components: Vec<String>,
}

pub fn parse_window(params: &HashMap<String, String>) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let start = if let Some(value) = params.get("start") {
        parse_time(value).ok_or_else(|| anyhow!("invalid_start"))?
    } else {
        crate::now_utc()
            - Duration::minutes(
                params
                    .get("window_minutes")
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(DEFAULT_WINDOW_MINUTES),
            )
    };
    let end = if let Some(value) = params.get("end") {
        parse_time(value).ok_or_else(|| anyhow!("invalid_end"))?
    } else {
        crate::now_utc()
    };
    if start >= end {
        return Err(anyhow!("start must be before end"));
    }
    Ok((start, end))
}

async fn fetch_history_rows(
    client: &HaClient,
    entity_id: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> std::result::Result<Vec<HistoryRow>, ProxyError> {
    let path = format!(
        "/api/history/period/{}?end_time={}&filter_entity_id={}",
        urlencoding::encode(&isoformat(start)),
        urlencoding::encode(&isoformat(end)),
        urlencoding::encode(entity_id),
    );
    let rows: Vec<Vec<HistoryRow>> = client.get_json(&path).await?;
    Ok(rows.into_iter().next().unwrap_or_default())
}

async fn fetch_logbook_rows(
    client: &HaClient,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> std::result::Result<Vec<LogbookRow>, ProxyError> {
    let global_path = format!(
        "/api/logbook/{}?end_time={}",
        urlencoding::encode(&isoformat(start)),
        urlencoding::encode(&isoformat(end))
    );
    client.get_json(&global_path).await
}

async fn fetch_direct_logbook_rows(
    client: &HaClient,
    entity_id: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> std::result::Result<Vec<LogbookRow>, ProxyError> {
    let direct_path = format!(
        "/api/logbook/{}?end_time={}&entity={}",
        urlencoding::encode(&isoformat(start)),
        urlencoding::encode(&isoformat(end)),
        urlencoding::encode(entity_id)
    );
    client.get_json(&direct_path).await
}

fn duplicate_friendly_names(states: &[HaState]) -> HashSet<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for state in states {
        let name = state.friendly_name();
        if !name.is_empty() {
            *counts.entry(name).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(name, _)| name)
        .collect()
}

fn filter_logbook_rows(
    entity_id: &str,
    friendly_name: &str,
    direct_rows: Vec<LogbookRow>,
    global_rows: &[LogbookRow],
    allow_friendly_name_match: bool,
) -> Vec<LogbookRow> {
    let mut merged: BTreeMap<String, LogbookRow> = BTreeMap::new();
    for row in direct_rows.into_iter().chain(global_rows.iter().cloned()) {
        let message = row.message.as_deref().unwrap_or_default();
        let name = row.name.as_deref().unwrap_or_default();
        let entity_match = row
            .entity_id
            .as_deref()
            .map(|value| value == entity_id)
            .unwrap_or(false)
            || row
                .context_entity_id
                .as_deref()
                .map(|value| value == entity_id)
                .unwrap_or(false)
            || (allow_friendly_name_match
                && !friendly_name.is_empty()
                && message.contains(friendly_name))
            || (allow_friendly_name_match
                && !friendly_name.is_empty()
                && name == "HomeKit"
                && message.contains(friendly_name));
        if entity_match {
            let key = format!(
                "{}|{}|{}|{}|{}",
                row.when.as_deref().unwrap_or_default(),
                row.entity_id.as_deref().unwrap_or_default(),
                name,
                message,
                row.context_entity_id.as_deref().unwrap_or_default()
            );
            merged.insert(key, row);
        }
    }
    merged.into_values().collect()
}

fn build_timeline(
    entity_id: &str,
    history_rows: &[HistoryRow],
    logbook_rows: &[LogbookRow],
) -> Vec<AuditTimelineEntry> {
    let mut timeline: Vec<AuditTimelineEntry> = logbook_rows
        .iter()
        .map(|row| AuditTimelineEntry {
            time: row.when.clone(),
            entity_id: entity_id.to_string(),
            source: "logbook".to_string(),
            name: row.name.clone(),
            message: row.message.clone(),
            context_entity_id: row.context_entity_id.clone(),
            context_state: row.context_state.clone(),
            state: None,
            friendly_name: None,
            upstream_source: None,
        })
        .collect();
    timeline.extend(history_rows.iter().map(|row| {
        AuditTimelineEntry {
            time: row.last_changed.clone(),
            entity_id: entity_id.to_string(),
            source: "history".to_string(),
            name: None,
            message: None,
            context_entity_id: None,
            context_state: None,
            state: row.state.clone(),
            friendly_name: row
                .attributes
                .as_ref()
                .and_then(|attrs| attrs.friendly_name.clone()),
            upstream_source: row
                .attributes
                .as_ref()
                .and_then(|attrs| attrs.source.clone()),
        }
    }));
    timeline.sort_by(|left, right| left.time.as_deref().cmp(&right.time.as_deref()));
    timeline
}

fn collect_actor(logbook_rows: &[LogbookRow]) -> AuditActor {
    for row in logbook_rows {
        if let Some(context_user_id) = row.context_user_id.as_deref() {
            return AuditActor {
                actor_type: "user".to_string(),
                id: Some(context_user_id.to_string()),
                name: row.name.clone(),
            };
        }
    }
    AuditActor {
        actor_type: "unknown".to_string(),
        id: None,
        name: None,
    }
}

fn collect_source(history_rows: &[HistoryRow], logbook_rows: &[LogbookRow]) -> AuditSource {
    let mut source_counts: HashMap<String, usize> = HashMap::new();
    for row in history_rows {
        if let Some(source) = row
            .attributes
            .as_ref()
            .and_then(|attrs| attrs.source.as_deref())
        {
            *source_counts.entry(source.to_string()).or_insert(0) += 1;
        }
    }
    let primary_source = source_counts
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1))
        .map(|item| item.0);
    let observations: Vec<AuditSourceObservation> = logbook_rows
        .iter()
        .filter_map(|row| {
            if row.name.as_deref() == Some("HomeKit") {
                Some(AuditSourceObservation {
                    observation_type: "integration_command".to_string(),
                    integration: "HomeKit".to_string(),
                    time: row.when.clone(),
                    message: row.message.clone(),
                })
            } else {
                None
            }
        })
        .collect();
    if primary_source.is_some() || !observations.is_empty() {
        AuditSource {
            source_type: "integration_signal".to_string(),
            id: primary_source,
            observations,
        }
    } else {
        AuditSource {
            source_type: "unknown".to_string(),
            id: None,
            observations: Vec::new(),
        }
    }
}

fn confidence_for(actor: &AuditActor, source: &AuditSource) -> &'static str {
    if actor.actor_type == "user" {
        "high"
    } else if source.id.is_some() || !source.observations.is_empty() {
        "medium"
    } else {
        "low"
    }
}

pub async fn audit_entity(
    client: &HaClient,
    query_entity: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Value {
    fn audit_config_issue(issue_type: &str, query_entity: &str, suggestions: Vec<String>) -> Value {
        serde_json::to_value(AuditConfigIssueResult {
            result_type: "config_issue".to_string(),
            issue: build_issue(issue_type, query_entity, suggestions),
        })
        .unwrap_or_else(|_| json!({"result_type":"config_issue"}))
    }

    let config = match client.get_json::<AuditConfig>("/api/config").await {
        Ok(value) => value,
        Err(ProxyError::MissingCredentials) => {
            return audit_config_issue("missing_credentials", query_entity, vec![])
        }
        Err(ProxyError::AuthFailed) => {
            return audit_config_issue("ha_auth_failed", query_entity, vec![])
        }
        Err(_) => return audit_config_issue("ha_unreachable", query_entity, vec![]),
    };
    let all_states = match fetch_all_states_typed(client).await {
        Ok(states) => states,
        Err(ProxyError::MissingCredentials) => {
            return audit_config_issue("missing_credentials", query_entity, vec![])
        }
        Err(ProxyError::AuthFailed) => {
            return audit_config_issue("ha_auth_failed", query_entity, vec![])
        }
        Err(_) => return audit_config_issue("ha_unreachable", query_entity, vec![]),
    };
    let components: HashSet<String> = config.components.into_iter().collect();
    if !components.contains("recorder")
        || !components.contains("history")
        || !components.contains("logbook")
    {
        return audit_config_issue("recorder_not_enabled", query_entity, vec![]);
    }
    let resolution = resolve_entity_basic(query_entity, &all_states);
    match resolution.status.as_str() {
        "not_found" => {
            return audit_config_issue(
                "entity_not_found",
                query_entity,
                resolution.suggestions.clone(),
            );
        }
        "ambiguous" => {
            return audit_config_issue(
                "entity_ambiguous",
                query_entity,
                resolution.suggestions.clone(),
            );
        }
        _ => {}
    }
    let primary_entity_id = resolution.entity_id.clone().unwrap_or_default();
    let primary_state = resolution.state.clone().unwrap_or_default();
    let related_ids = related_entity_ids(&primary_entity_id, &all_states);
    let duplicate_names = duplicate_friendly_names(&all_states);
    let mut audit_ids = vec![primary_entity_id.clone()];
    audit_ids.extend(
        related_ids
            .iter()
            .filter(|item| *item != &primary_entity_id)
            .cloned(),
    );
    let global_logbook_rows = match fetch_logbook_rows(client, start, end).await {
        Ok(rows) => rows,
        Err(ProxyError::AuthFailed) => {
            return audit_config_issue("ha_auth_failed", query_entity, vec![])
        }
        Err(_) => return audit_config_issue("ha_unreachable", query_entity, vec![]),
    };

    let mut entity_audits = Vec::new();
    let mut all_history_rows = Vec::new();
    let mut all_logbook_rows = Vec::new();
    let mut primary_history_rows = Vec::new();

    for entity_id in &audit_ids {
        let state = all_states
            .iter()
            .find(|state| state.entity_id == *entity_id)
            .cloned()
            .unwrap_or_default();
        let friendly_name = state.friendly_name();
        let history_rows = match fetch_history_rows(client, entity_id, start, end).await {
            Ok(rows) => rows,
            Err(ProxyError::AuthFailed) => {
                return audit_config_issue("ha_auth_failed", query_entity, vec![])
            }
            Err(_) => return audit_config_issue("ha_unreachable", query_entity, vec![]),
        };
        let direct_logbook_rows =
            match fetch_direct_logbook_rows(client, entity_id, start, end).await {
                Ok(rows) => rows,
                Err(ProxyError::AuthFailed) => {
                    return audit_config_issue("ha_auth_failed", query_entity, vec![])
                }
                Err(_) => return audit_config_issue("ha_unreachable", query_entity, vec![]),
            };
        let logbook_rows = filter_logbook_rows(
            entity_id,
            &friendly_name,
            direct_logbook_rows,
            &global_logbook_rows,
            !duplicate_names.contains(&friendly_name),
        );
        if entity_id == &primary_entity_id {
            primary_history_rows = history_rows.clone();
        }
        entity_audits.push(AuditEntityRow {
            entity_id: entity_id.clone(),
            friendly_name,
            current_state: state.current_state(),
            history_count: history_rows.len(),
            logbook_count: logbook_rows.len(),
            timeline: build_timeline(entity_id, &history_rows, &logbook_rows),
        });
        all_history_rows.extend(history_rows);
        all_logbook_rows.extend(logbook_rows);
    }

    if all_history_rows.is_empty() && all_logbook_rows.is_empty() {
        return serde_json::to_value(AuditNoEvidenceResult {
            result_type: "no_evidence".to_string(),
            query_entity: query_entity.to_string(),
            resolved_entity_id: primary_entity_id,
            related_entity_ids: related_ids,
            window_start: isoformat(start),
            window_end: isoformat(end),
            current_state: primary_state.current_state(),
            queried_sources: vec!["logbook".to_string(), "history".to_string()],
            missing_evidence_types: vec![
                "state_changes".to_string(),
                "logbook_entries".to_string(),
                "actor_context".to_string(),
            ],
            possible_reasons: vec![
                "no_matching_activity_in_window".to_string(),
                "recorder_gap_or_exclusion".to_string(),
                "history_purged".to_string(),
            ],
            confidence: "low".to_string(),
        })
        .unwrap_or_else(|_| json!({"result_type":"no_evidence"}));
    }
    let actor = collect_actor(&all_logbook_rows);
    let source = collect_source(&all_history_rows, &all_logbook_rows);
    serde_json::to_value(AuditEvidenceResult {
        result_type: "evidence".to_string(),
        query_entity: query_entity.to_string(),
        resolved_entity_id: primary_entity_id,
        matched_by: resolution.matched_by.clone(),
        related_entity_ids: related_ids.clone(),
        window_start: isoformat(start),
        window_end: isoformat(end),
        current_state: primary_state.current_state(),
        actor: actor.clone(),
        source: source.clone(),
        confidence: confidence_for(&actor, &source).to_string(),
        findings: AuditFindings {
            primary_transition_count: primary_history_rows.len().saturating_sub(1),
            actor_identified: actor.actor_type != "unknown",
            upstream_source_identified: source.id.is_some(),
            integration_observation_count: source.observations.len(),
            related_entity_count: related_ids.len(),
        },
        counts: AuditCounts {
            entities: entity_audits.len(),
            history: all_history_rows.len(),
            logbook: all_logbook_rows.len(),
        },
        entities: entity_audits,
    })
    .unwrap_or_else(|_| json!({"result_type":"evidence"}))
}
