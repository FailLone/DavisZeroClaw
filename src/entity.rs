use crate::{
    build_issue, fetch_all_states_typed, normalize_text, EntityBasicResolution, HaClient, HaState,
    ProxyError, ResolveEntityPayload,
};
use serde_json::{json, Value};

pub fn resolve_entity_basic(query_entity: &str, all_states: &[HaState]) -> EntityBasicResolution {
    let raw = query_entity.trim();
    let raw_norm = normalize_text(raw);
    if raw_norm.is_empty() {
        return EntityBasicResolution {
            status: "not_found".to_string(),
            suggestions: Vec::new(),
            ..Default::default()
        };
    }
    let mut scored: Vec<(i64, String, HaState, String)> = Vec::new();
    for state in all_states {
        let entity_id = state.entity_id.clone();
        let friendly_name = state.friendly_name();
        let suffix = state.suffix();
        let entity_norm = normalize_text(&entity_id);
        let suffix_norm = normalize_text(&suffix);
        let name_norm = normalize_text(&friendly_name);
        let found = if raw == entity_id {
            Some((100, "exact_entity_id"))
        } else if raw == suffix {
            Some((95, "exact_suffix"))
        } else if raw == friendly_name {
            Some((90, "exact_friendly_name"))
        } else if raw_norm == suffix_norm {
            Some((85, "normalized_suffix"))
        } else if raw_norm == name_norm {
            Some((80, "normalized_friendly_name"))
        } else if raw_norm == entity_norm {
            Some((75, "normalized_entity_id"))
        } else if !raw_norm.is_empty()
            && (suffix_norm.contains(&raw_norm) || name_norm.contains(&raw_norm))
        {
            Some((40, "partial_match"))
        } else {
            None
        };
        if let Some((score, matched_by)) = found {
            scored.push((score, entity_id, state.clone(), matched_by.to_string()));
        }
    }
    if scored.is_empty() {
        let suggestions: Vec<String> = all_states
            .iter()
            .filter_map(|state| {
                let haystack =
                    format!("{} {}", state.entity_id, state.friendly_name()).to_lowercase();
                if haystack.contains(&raw.to_lowercase()) {
                    Some(state.entity_id.clone())
                } else {
                    None
                }
            })
            .take(5)
            .collect();
        return EntityBasicResolution {
            status: "not_found".to_string(),
            suggestions,
            ..Default::default()
        };
    }
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    let best_score = scored[0].0;
    let best: Vec<_> = scored.iter().filter(|item| item.0 == best_score).collect();
    if best.len() > 1 && best_score < 95 {
        return EntityBasicResolution {
            status: "ambiguous".to_string(),
            suggestions: best.iter().take(5).map(|item| item.1.clone()).collect(),
            ..Default::default()
        };
    }
    let (_, entity_id, state, matched_by) = scored[0].clone();
    EntityBasicResolution {
        status: "ok".to_string(),
        entity_id: Some(entity_id),
        state: Some(state),
        matched_by: Some(matched_by),
        suggestions: scored.iter().take(5).map(|item| item.1.clone()).collect(),
    }
}

pub async fn resolve_entity_payload(client: &HaClient, query_entity: &str) -> Value {
    match fetch_all_states_typed(client).await {
        Ok(states) => {
            let resolution = resolve_entity_basic(query_entity, &states);
            let payload = match resolution.status.as_str() {
                "ok" => {
                    let entity_id = resolution.entity_id.clone();
                    let state = resolution.state.clone().unwrap_or_default();
                    let friendly_name = state.friendly_name();
                    ResolveEntityPayload {
                        status: "ok".to_string(),
                        query_entity: query_entity.to_string(),
                        resolved_entity_id: entity_id.clone(),
                        matched_by: resolution.matched_by.clone(),
                        friendly_name: (!friendly_name.is_empty()).then_some(friendly_name),
                        domain: entity_id.as_ref().map(|_| state.domain()),
                        current_state: state.current_state(),
                        related_entity_ids: entity_id
                            .as_deref()
                            .map(|id| related_entity_ids(id, &states))
                            .unwrap_or_default(),
                        suggestions: resolution.suggestions.clone(),
                        issue: None,
                    }
                }
                "ambiguous" => ResolveEntityPayload {
                    status: "ambiguous".to_string(),
                    query_entity: query_entity.to_string(),
                    suggestions: resolution.suggestions.clone(),
                    ..Default::default()
                },
                _ => ResolveEntityPayload {
                    status: "not_found".to_string(),
                    query_entity: query_entity.to_string(),
                    suggestions: resolution.suggestions.clone(),
                    ..Default::default()
                },
            };
            serde_json::to_value(payload).unwrap_or_else(|_| json!({"status":"not_found"}))
        }
        Err(ProxyError::MissingCredentials) => serde_json::to_value(ResolveEntityPayload {
            status: "config_issue".to_string(),
            query_entity: query_entity.to_string(),
            issue: Some(build_issue("missing_credentials", query_entity, vec![])),
            ..Default::default()
        })
        .unwrap_or_else(|_| json!({"status":"config_issue"})),
        Err(ProxyError::AuthFailed) => serde_json::to_value(ResolveEntityPayload {
            status: "config_issue".to_string(),
            query_entity: query_entity.to_string(),
            issue: Some(build_issue("ha_auth_failed", query_entity, vec![])),
            ..Default::default()
        })
        .unwrap_or_else(|_| json!({"status":"config_issue"})),
        Err(_) => serde_json::to_value(ResolveEntityPayload {
            status: "config_issue".to_string(),
            query_entity: query_entity.to_string(),
            issue: Some(build_issue("ha_unreachable", query_entity, vec![])),
            ..Default::default()
        })
        .unwrap_or_else(|_| json!({"status":"config_issue"})),
    }
}

pub fn related_entity_ids(primary_entity_id: &str, all_states: &[HaState]) -> Vec<String> {
    let ids: std::collections::HashSet<String> = all_states
        .iter()
        .map(|state| state.entity_id.clone())
        .collect();
    let mut related = Vec::new();
    if primary_entity_id.starts_with("binary_sensor.") && primary_entity_id.ends_with("_on_off") {
        let stem = primary_entity_id
            .trim_start_matches("binary_sensor.")
            .trim_end_matches("_on_off");
        let climate_id = format!("climate.{stem}");
        if ids.contains(&climate_id) {
            related.push(climate_id);
        }
    }
    if primary_entity_id.starts_with("climate.") {
        let stem = primary_entity_id.trim_start_matches("climate.");
        let binary_id = format!("binary_sensor.{stem}_on_off");
        if ids.contains(&binary_id) {
            related.push(binary_id);
        }
    }
    related
}
