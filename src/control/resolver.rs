use crate::{
    fetch_all_states_typed, normalize_text, Candidate, ControlAction, ControlConfig,
    ControlResolution, FailureReason, HaClient, HaState, ProxyError, CONTROL_DOMAINS,
};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone)]
struct QueryVariant {
    raw: String,
    normalized: String,
    label: &'static str,
}

fn alias_list_for_entity<'a>(entity_id: &str, config: &'a ControlConfig) -> Vec<&'a str> {
    config
        .entity_aliases
        .get(entity_id)
        .map(|items| items.iter().map(String::as_str).collect())
        .unwrap_or_default()
}

fn trim_query_text(text: &str) -> String {
    text.trim()
        .trim_matches(|ch: char| {
            matches!(
                ch,
                ' ' | '，' | ',' | '。' | '.' | '！' | '!' | '？' | '?' | '：' | ':'
            )
        })
        .to_string()
}

fn strip_common_query_noise(query: &str, action: &ControlAction) -> String {
    let mut value = trim_query_text(query);
    let leading_phrases = [
        "请帮我把",
        "帮我把",
        "麻烦把",
        "请帮我",
        "请把",
        "帮忙把",
        "帮忙",
        "麻烦",
        "给我把",
        "给我",
        "请",
        "把",
        "将",
    ];
    loop {
        let mut changed = false;
        for phrase in &leading_phrases {
            if value.starts_with(phrase) {
                value = value[phrase.len()..].to_string();
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
        value = trim_query_text(&value);
    }

    let mut replacements = vec!["一下子", "一下", "帮我", "帮忙", "麻烦"];
    match action {
        ControlAction::TurnOn => replacements.extend(["打开一下", "开一下", "打开", "开启"]),
        ControlAction::TurnOff => {
            replacements.extend(["关闭一下", "关掉一下", "关一下", "关闭", "关掉", "关上"])
        }
        ControlAction::Toggle => replacements.extend(["切换一下", "切一下", "切换"]),
        ControlAction::SetBrightness => replacements.extend([
            "亮度调到",
            "亮度调成",
            "亮度设为",
            "亮度改到",
            "亮度改成",
            "调到",
            "调成",
            "设为",
            "改到",
            "改成",
            "调亮",
            "调暗",
            "亮一点",
            "暗一点",
            "亮一些",
            "暗一些",
            "亮度",
            "百分之",
        ]),
        ControlAction::QueryState => replacements.extend(["现在", "当前", "目前"]),
        ControlAction::Unknown => {}
    }
    for phrase in replacements {
        value = value.replace(phrase, "");
    }

    if matches!(action, ControlAction::SetBrightness)
        || value.contains('%')
        || value.contains('％')
        || value.contains("百分")
    {
        value = value
            .chars()
            .filter(|ch| !ch.is_ascii_digit() && !matches!(ch, '%' | '％'))
            .collect();
    }

    trim_query_text(&value)
}

fn query_variants(query: &str, action: &ControlAction) -> Vec<QueryVariant> {
    let raw = trim_query_text(query);
    let mut variants = Vec::new();
    let mut seen = HashSet::new();
    for (label, value) in [
        ("raw", raw.clone()),
        ("focused", strip_common_query_noise(&raw, action)),
    ] {
        let trimmed = trim_query_text(&value);
        if trimmed.is_empty() || !seen.insert(trimmed.clone()) {
            continue;
        }
        variants.push(QueryVariant {
            normalized: normalize_text(&trimmed),
            raw: trimmed,
            label,
        });
    }
    if let Some(focused) = variants.iter().find(|variant| variant.label == "focused") {
        let compact = focused.raw.replace('的', "");
        let compact = trim_query_text(&compact);
        if !compact.is_empty() && seen.insert(compact.clone()) {
            variants.push(QueryVariant {
                normalized: normalize_text(&compact),
                raw: compact,
                label: "focused_compact",
            });
        }
    }
    variants
}

fn room_matches(
    query_entity: &str,
    friendly_name: &str,
    aliases: &[&str],
    config: &ControlConfig,
) -> Vec<String> {
    let query_norm = normalize_text(query_entity);
    config
        .area_aliases
        .iter()
        .filter_map(|(canonical, raw_aliases)| {
            let mut terms = vec![canonical.as_str()];
            terms.extend(raw_aliases.iter().map(String::as_str));
            let found_in_query = terms
                .iter()
                .map(|term| normalize_text(term))
                .any(|norm| !norm.is_empty() && query_norm.contains(&norm));
            if !found_in_query {
                return None;
            }
            let mut targets = vec![friendly_name];
            targets.extend(aliases.iter().copied());
            let canonical_norm = normalize_text(canonical);
            let found_in_targets = targets
                .into_iter()
                .map(normalize_text)
                .any(|value| value.contains(&canonical_norm));
            if found_in_targets {
                Some(canonical.clone())
            } else {
                None
            }
        })
        .collect()
}

fn preferred_domains(
    query_entity: &str,
    action: &ControlAction,
    config: &ControlConfig,
) -> Vec<String> {
    let mut domains = Vec::new();
    for (domain, keywords) in &config.domain_preferences {
        if keywords
            .iter()
            .any(|keyword| query_entity.contains(keyword))
        {
            domains.push(domain.clone());
        }
    }
    if *action == ControlAction::SetBrightness && !domains.iter().any(|item| item == "light") {
        domains.insert(0, "light".to_string());
    }
    domains
}

fn score_entity_candidate(
    query_entity: &str,
    action: &ControlAction,
    state: &HaState,
    config: &ControlConfig,
) -> Option<Candidate> {
    let entity_id = state.entity_id.clone();
    let domain = state.domain();
    if !CONTROL_DOMAINS.iter().any(|item| *item == domain) {
        return None;
    }
    if config
        .ignored_entities
        .iter()
        .any(|item| item == &entity_id)
    {
        return None;
    }
    let friendly_name = state.friendly_name();
    let suffix = state.suffix();
    let aliases = alias_list_for_entity(&entity_id, config);
    let raw = query_entity.trim();
    let alias_norms: Vec<String> = aliases.iter().map(|alias| normalize_text(alias)).collect();
    let entity_norm = normalize_text(&entity_id);
    let suffix_norm = normalize_text(&suffix);
    let name_norm = normalize_text(&friendly_name);
    let variants = query_variants(raw, action);
    let mut matched = None;
    for variant in variants {
        let prefix = match variant.label {
            "raw" => "",
            "focused" => "focused_",
            "focused_compact" => "focused_compact_",
            _ => "",
        };
        let current = if variant.raw == entity_id {
            Some((100, format!("{prefix}exact_entity_id")))
        } else if aliases.iter().any(|alias| *alias == variant.raw) {
            Some((98, format!("{prefix}exact_entity_alias")))
        } else if variant.raw == suffix {
            Some((95, format!("{prefix}exact_suffix")))
        } else if variant.raw == friendly_name {
            Some((90, format!("{prefix}exact_friendly_name")))
        } else if alias_norms.iter().any(|alias| alias == &variant.normalized) {
            Some((88, format!("{prefix}normalized_entity_alias")))
        } else if variant.normalized == suffix_norm {
            Some((85, format!("{prefix}normalized_suffix")))
        } else if variant.normalized == name_norm {
            Some((80, format!("{prefix}normalized_friendly_name")))
        } else if variant.normalized == entity_norm {
            Some((75, format!("{prefix}normalized_entity_id")))
        } else if alias_norms
            .iter()
            .any(|alias| !variant.normalized.is_empty() && alias.contains(&variant.normalized))
        {
            Some((45, format!("{prefix}partial_entity_alias")))
        } else if !variant.normalized.is_empty()
            && (suffix_norm.contains(&variant.normalized)
                || name_norm.contains(&variant.normalized))
        {
            Some((40, format!("{prefix}partial_match")))
        } else {
            None
        };
        if current.is_some() {
            matched = current;
            break;
        }
    }
    let (mut score, matched_by) = matched?;
    let mut reasons = vec![matched_by.clone()];
    let preferred = preferred_domains(query_entity, action, config);
    if let Some(rank) = preferred.iter().position(|item| item == &domain) {
        score += if rank == 0 {
            8
        } else {
            std::cmp::max(1, 5 - rank as i64)
        };
        reasons.push(format!("domain_preference:{domain}"));
    } else if config
        .domain_preferences
        .get("light")
        .map(|keywords| {
            keywords
                .iter()
                .any(|keyword| query_entity.contains(keyword))
        })
        .unwrap_or(false)
        && domain == "switch"
    {
        score += 1;
        reasons.push("light_like_switch".to_string());
    }
    let matched_rooms = room_matches(query_entity, &friendly_name, &aliases, config);
    if !matched_rooms.is_empty() {
        score += 12;
        reasons.push(format!("room_match:{}", matched_rooms.join(",")));
    }
    if state.current_state().as_deref() == Some("unavailable") {
        score -= 10;
        reasons.push("unavailable".to_string());
    }
    Some(Candidate {
        entity_id,
        friendly_name,
        domain,
        score,
        matched_by,
        reasons,
    })
}

fn confidence_from_candidates(candidates: &[Candidate]) -> String {
    if candidates.is_empty() {
        return "low".to_string();
    }
    let top = candidates[0].score;
    let second = candidates.get(1).map(|item| item.score);
    if top >= 90 && second.map(|value| top - value >= 10).unwrap_or(true) {
        "high".to_string()
    } else if top >= 75 {
        "medium".to_string()
    } else {
        "low".to_string()
    }
}

fn match_group(
    query_entity: &str,
    action: &ControlAction,
    config: &ControlConfig,
    all_states: &[HaState],
) -> Option<ControlResolution> {
    let raw = query_entity.trim();
    let state_by_id: HashSet<String> = all_states
        .iter()
        .map(|state| state.entity_id.clone())
        .collect();
    let variants = query_variants(raw, action);
    for (group_name, group) in &config.groups {
        let mut aliases = vec![group_name.as_str()];
        aliases.extend(group.aliases.iter().map(String::as_str));
        let matched_by = variants.iter().find_map(|variant| {
            let prefix = match variant.label {
                "raw" => "",
                "focused" => "focused_",
                "focused_compact" => "focused_compact_",
                _ => "",
            };
            aliases.iter().find_map(|alias| {
                if variant.raw == *alias {
                    Some(format!("{prefix}exact_group_alias"))
                } else if variant.normalized == normalize_text(alias) {
                    Some(format!("{prefix}normalized_group_alias"))
                } else {
                    None
                }
            })
        });
        let Some(matched_by) = matched_by else {
            continue;
        };
        let entity_ids: Vec<String> = group
            .entities
            .iter()
            .filter(|entity_id| state_by_id.contains(*entity_id))
            .cloned()
            .collect();
        let missing_targets: Vec<String> = group
            .entities
            .iter()
            .filter(|entity_id| !state_by_id.contains(*entity_id))
            .cloned()
            .collect();
        if entity_ids.is_empty() && missing_targets.is_empty() {
            continue;
        }
        if !missing_targets.is_empty() {
            return Some(ControlResolution {
                status: "config_issue".to_string(),
                query_entity: raw.to_string(),
                action: ControlAction::Unknown,
                reason: Some(FailureReason::GroupMembersMissing),
                resolution_type: Some("group".to_string()),
                resolved_targets: entity_ids,
                missing_targets,
                friendly_names: Vec::new(),
                matched_by: Some(matched_by.clone()),
                confidence: Some("low".to_string()),
                best_guess_used: Some(false),
                candidate_count: Some(group.entities.len()),
                second_best_gap: None,
                candidates: Vec::new(),
                suggestions: Vec::new(),
            });
        }
        let names = entity_ids
            .iter()
            .filter_map(|resolved_id| {
                all_states
                    .iter()
                    .find(|state| state.entity_id == *resolved_id)
            })
            .map(HaState::friendly_name)
            .collect();
        return Some(ControlResolution {
            status: "ok".to_string(),
            query_entity: raw.to_string(),
            action: ControlAction::Unknown,
            reason: None,
            resolution_type: Some("group".to_string()),
            resolved_targets: entity_ids.clone(),
            missing_targets: Vec::new(),
            friendly_names: names,
            matched_by: Some(matched_by),
            confidence: Some("high".to_string()),
            best_guess_used: Some(false),
            candidate_count: Some(entity_ids.len()),
            second_best_gap: None,
            candidates: Vec::new(),
            suggestions: Vec::new(),
        });
    }
    None
}

pub(crate) fn resolve_control_target_with_typed_states(
    query_entity: &str,
    action: &ControlAction,
    all_states: &[HaState],
    config: &ControlConfig,
) -> ControlResolution {
    let raw = query_entity.trim().to_string();
    if raw.is_empty() {
        return ControlResolution {
            status: "not_found".to_string(),
            query_entity: raw,
            action: action.clone(),
            reason: Some(FailureReason::ResolutionNotFound),
            ..Default::default()
        };
    }
    if let Some(mut group_match) = match_group(&raw, action, config, all_states) {
        group_match.action = action.clone();
        return group_match;
    }
    let mut candidates: Vec<Candidate> = all_states
        .iter()
        .filter_map(|state| score_entity_candidate(&raw, action, state, config))
        .collect();
    if candidates.is_empty() {
        let query_norms: Vec<String> = query_variants(&raw, action)
            .into_iter()
            .map(|variant| variant.normalized)
            .filter(|value| !value.is_empty())
            .collect();
        let suggestions = all_states
            .iter()
            .filter_map(|state| {
                let haystack =
                    normalize_text(&format!("{} {}", state.entity_id, state.friendly_name()));
                if query_norms
                    .iter()
                    .any(|query_norm| haystack.contains(query_norm))
                {
                    Some(state.entity_id.clone())
                } else {
                    None
                }
            })
            .take(5)
            .collect();
        return ControlResolution {
            status: "not_found".to_string(),
            query_entity: raw,
            action: action.clone(),
            reason: Some(FailureReason::ResolutionNotFound),
            suggestions,
            ..Default::default()
        };
    }
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.entity_id.cmp(&right.entity_id))
    });
    let top = &candidates[0];
    let second_best_gap = candidates.get(1).map(|item| top.score - item.score);
    let confidence = confidence_from_candidates(&candidates);
    if candidates.len() > 1 && confidence != "high" {
        return ControlResolution {
            status: "ambiguous".to_string(),
            query_entity: raw,
            action: action.clone(),
            reason: Some(FailureReason::ResolutionAmbiguous),
            matched_by: Some(top.matched_by.clone()),
            confidence: Some(confidence),
            best_guess_used: Some(false),
            candidate_count: Some(candidates.len()),
            second_best_gap,
            suggestions: candidates
                .iter()
                .take(5)
                .map(|item| item.entity_id.clone())
                .collect(),
            candidates: candidates.into_iter().take(5).collect(),
            ..Default::default()
        };
    }
    ControlResolution {
        status: "ok".to_string(),
        query_entity: raw,
        action: action.clone(),
        reason: None,
        resolution_type: Some("entity".to_string()),
        resolved_targets: vec![top.entity_id.clone()],
        missing_targets: Vec::new(),
        friendly_names: vec![top.friendly_name.clone()],
        matched_by: Some(top.matched_by.clone()),
        confidence: Some(confidence.clone()),
        best_guess_used: Some(confidence != "high" || candidates.len() > 1),
        candidate_count: Some(candidates.len()),
        second_best_gap,
        suggestions: candidates
            .iter()
            .take(5)
            .map(|item| item.entity_id.clone())
            .collect(),
        candidates: candidates.into_iter().take(5).collect(),
    }
}

pub fn resolve_control_target_with_states(
    query_entity: &str,
    action: &str,
    all_states: &[Value],
    config: &ControlConfig,
) -> ControlResolution {
    let typed_states: Vec<HaState> = all_states
        .iter()
        .cloned()
        .filter_map(|value| serde_json::from_value::<HaState>(value).ok())
        .collect();
    resolve_control_target_with_typed_states(
        query_entity,
        &ControlAction::from_query(action),
        &typed_states,
        config,
    )
}

pub async fn resolve_control_target(
    client: &HaClient,
    query_entity: &str,
    action: &str,
    config: &ControlConfig,
) -> std::result::Result<ControlResolution, ProxyError> {
    let states = fetch_all_states_typed(client).await?;
    Ok(resolve_control_target_with_typed_states(
        query_entity,
        &ControlAction::from_query(action),
        &states,
        config,
    ))
}
