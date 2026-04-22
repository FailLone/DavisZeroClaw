use crate::{
    build_failure_summary, fetch_all_states_typed, isoformat, now_utc,
    refine_live_context_report_with_typed_states, AdvancedOpportunity, AssistEntitySuggestion,
    ConfigMigrationSuggestion, ConfigReport, ConfigReportCounts, ConfigReportFindings,
    ConfigReportSuggestions, ControlConfig, CrossDomainConflictFinding, CustomSentenceSuggestion,
    DuplicateFriendlyNameFinding, EntityAliasSuggestion, FailureSummary, GroupSuggestion, HaClient,
    HaMcpClient, HaMcpLiveContextReport, HaState, MissingRoomSemanticFinding, ProxyError,
    ReplacementCandidateReview, ReplacementCandidatesReport, RuntimePaths, CONTROL_DOMAINS,
    ROOM_LIGHT_KEYWORDS,
};
use anyhow::Result;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

static CONFIG_REPORT_TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn write_report_cache_atomic(path: &std::path::Path, report: &ConfigReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        CONFIG_REPORT_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&temp_path, serde_json::to_vec_pretty(report)?)?;
    fs::rename(&temp_path, path).inspect_err(|_err| {
        let _ = fs::remove_file(&temp_path);
    })?;
    Ok(())
}

fn infer_group_suggestions(states: &[HaState], config: &ControlConfig) -> Vec<GroupSuggestion> {
    let mut groups = Vec::new();
    let mut seen = HashSet::new();
    for room in &config.room_tokens {
        let entity_ids: Vec<String> = states
            .iter()
            .filter(|state| {
                let domain = state.domain();
                (domain == "light" || domain == "switch")
                    && state.friendly_name().contains(room)
                    && ROOM_LIGHT_KEYWORDS
                        .iter()
                        .any(|keyword| state.friendly_name().contains(keyword))
            })
            .map(|state| state.entity_id.clone())
            .collect();
        if entity_ids.len() < 2 {
            continue;
        }
        let group_name = format!("{room}的灯");
        if !seen.insert(group_name.clone()) {
            continue;
        }
        groups.push(GroupSuggestion {
            group_name,
            entities: entity_ids,
            aliases: vec![format!("{room}灯"), format!("{room}灯光")],
        });
    }
    groups
}

fn infer_assist_entities(states: &[HaState]) -> Vec<AssistEntitySuggestion> {
    states
        .iter()
        .filter_map(|state| {
            let entity_id = state.entity_id.clone();
            let domain = state.domain();
            if !["light", "switch", "cover", "climate", "fan", "lock"].contains(&domain.as_str()) {
                return None;
            }
            let friendly_name = state.friendly_name();
            if ["指示灯", "儿童锁", "勿扰", "物理控制锁", "场景虚拟按钮"]
                .iter()
                .any(|token| friendly_name.contains(token))
            {
                return None;
            }
            Some(AssistEntitySuggestion {
                entity_id,
                friendly_name,
                domain,
            })
        })
        .take(40)
        .collect()
}

fn infer_custom_sentence_suggestions(
    config: &ControlConfig,
    group_suggestions: &[GroupSuggestion],
) -> Vec<CustomSentenceSuggestion> {
    group_suggestions
        .iter()
        .map(|group| CustomSentenceSuggestion {
            group_name: group.group_name.clone(),
            sentences: group
                .aliases
                .iter()
                .map(|alias| format!("打开{alias}"))
                .collect(),
            config_room_tokens: config.room_tokens.iter().take(5).cloned().collect(),
        })
        .take(20)
        .collect()
}

fn build_migration_suggestions(
    states: &[HaState],
    config: &ControlConfig,
    replacement_candidates: &[ReplacementCandidateReview],
) -> Vec<ConfigMigrationSuggestion> {
    let mut suggestions = Vec::new();
    for candidate in replacement_candidates {
        let Some(unavailable_state) =
            find_replacement_state(states, &candidate.domain, &candidate.unavailable_name, true)
        else {
            continue;
        };
        let Some(replacement_state) = find_replacement_state(
            states,
            &candidate.domain,
            &candidate.replacement_name,
            false,
        ) else {
            continue;
        };

        if unavailable_state.entity_id == replacement_state.entity_id {
            continue;
        }

        let old_aliases = config
            .entity_aliases
            .get(&unavailable_state.entity_id)
            .cloned()
            .unwrap_or_default();
        let mut merged_aliases = config
            .entity_aliases
            .get(&replacement_state.entity_id)
            .cloned()
            .unwrap_or_default();
        for alias in old_aliases.iter().cloned().chain(
            (candidate.unavailable_name != candidate.replacement_name)
                .then(|| candidate.unavailable_name.clone()),
        ) {
            if !alias.trim().is_empty() && !merged_aliases.iter().any(|item| item == &alias) {
                merged_aliases.push(alias);
            }
        }

        if !merged_aliases.is_empty() {
            suggestions.push(ConfigMigrationSuggestion {
                suggestion_type: "entity_alias_migration".to_string(),
                reason: format!(
                    "{} replacement candidate: '{}' -> '{}'",
                    candidate.confidence, candidate.unavailable_name, candidate.replacement_name
                ),
                target: replacement_state.entity_id.clone(),
                current: old_aliases.clone(),
                recommended: merged_aliases.clone(),
                snippet: serde_json::to_string_pretty(&json!({
                    "entity_aliases": {
                        replacement_state.entity_id.clone(): merged_aliases
                    }
                }))
                .unwrap_or_default(),
                requires_confirmation: true,
            });
        }

        for (group_name, group) in &config.groups {
            if !group
                .entities
                .iter()
                .any(|entity_id| entity_id == &unavailable_state.entity_id)
            {
                continue;
            }
            let mut entities = group.entities.clone();
            for entity_id in &mut entities {
                if entity_id == &unavailable_state.entity_id {
                    *entity_id = replacement_state.entity_id.clone();
                }
            }
            dedupe_preserving_order(&mut entities);
            suggestions.push(ConfigMigrationSuggestion {
                suggestion_type: "group_member_migration".to_string(),
                reason: format!(
                    "group '{}' still references unavailable replacement source '{}'",
                    group_name, unavailable_state.entity_id
                ),
                target: group_name.clone(),
                current: group.entities.clone(),
                recommended: entities.clone(),
                snippet: serde_json::to_string_pretty(&json!({
                    "groups": {
                        group_name: {
                            "entities": entities,
                            "aliases": group.aliases
                        }
                    }
                }))
                .unwrap_or_default(),
                requires_confirmation: true,
            });
        }
    }

    suggestions.truncate(20);
    suggestions
}

fn find_replacement_state<'a>(
    states: &'a [HaState],
    domain: &str,
    friendly_name: &str,
    prefer_unavailable: bool,
) -> Option<&'a HaState> {
    let normalized_name = crate::normalize_text(friendly_name);
    states
        .iter()
        .filter(|state| state.domain() == domain)
        .filter(|state| crate::normalize_text(&state.friendly_name()) == normalized_name)
        .min_by(|left, right| {
            replacement_state_rank(left, prefer_unavailable)
                .cmp(&replacement_state_rank(right, prefer_unavailable))
                .then_with(|| left.entity_id.cmp(&right.entity_id))
        })
}

fn replacement_state_rank(state: &HaState, prefer_unavailable: bool) -> usize {
    let state = state.current_state().unwrap_or_default();
    if state.eq_ignore_ascii_case("unavailable") {
        if prefer_unavailable {
            0
        } else {
            2
        }
    } else if state.eq_ignore_ascii_case("unknown") {
        1
    } else if prefer_unavailable {
        2
    } else {
        0
    }
}

fn dedupe_preserving_order(items: &mut Vec<String>) {
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.clone()));
}

fn detect_cross_domain_conflicts(
    duplicate_rows: &[DuplicateFriendlyNameFinding],
) -> Vec<CrossDomainConflictFinding> {
    duplicate_rows
        .iter()
        .filter_map(|row| {
            let domains: HashSet<String> = row
                .entities
                .iter()
                .map(|entity_id| crate::entity_domain(entity_id))
                .collect();
            if domains.len() > 1 {
                let mut domains: Vec<String> = domains.into_iter().collect();
                domains.sort();
                Some(CrossDomainConflictFinding {
                    friendly_name: row.friendly_name.clone(),
                    entities: row.entities.clone(),
                    domains,
                })
            } else {
                None
            }
        })
        .collect()
}

fn infer_advanced_opportunities(
    states: &[HaState],
    duplicate_rows: &[DuplicateFriendlyNameFinding],
    group_suggestions: &[GroupSuggestion],
    replacement_candidate_count: usize,
) -> Vec<AdvancedOpportunity> {
    let mut rows = Vec::new();
    if !duplicate_rows.is_empty() {
        rows.push(AdvancedOpportunity {
            opportunity_type: "rename_or_alias_cleanup".to_string(),
            reason: "duplicate_friendly_names".to_string(),
            count: duplicate_rows.len(),
        });
    }
    if !group_suggestions.is_empty() {
        rows.push(AdvancedOpportunity {
            opportunity_type: "room_grouping".to_string(),
            reason: "multi-light_rooms_without_groups".to_string(),
            count: group_suggestions.len(),
        });
    }
    if replacement_candidate_count > 0 {
        rows.push(AdvancedOpportunity {
            opportunity_type: "entity_reconciliation_review".to_string(),
            reason: "possible_replacements_detected".to_string(),
            count: replacement_candidate_count,
        });
    }
    let climate_count = states
        .iter()
        .filter(|state| state.domain() == "climate")
        .count();
    if climate_count > 0 {
        rows.push(AdvancedOpportunity {
            opportunity_type: "climate_review".to_string(),
            reason: "climate_entities_present".to_string(),
            count: climate_count,
        });
    }
    rows
}

fn generate_config_report_with_typed_states(
    paths: &RuntimePaths,
    all_states: &[HaState],
    config: &ControlConfig,
    failure_summary: &FailureSummary,
    mut ha_mcp_live_context: Option<HaMcpLiveContextReport>,
) -> Result<Value> {
    let control_states: Vec<HaState> = all_states
        .iter()
        .filter(|state| CONTROL_DOMAINS.iter().any(|item| *item == state.domain()))
        .cloned()
        .collect();
    let mut name_to_entities: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut missing_room_semantic = Vec::new();
    for state in &control_states {
        let entity_id = state.entity_id.clone();
        let friendly_name = state.friendly_name();
        name_to_entities
            .entry(friendly_name.clone())
            .or_default()
            .push(entity_id.clone());
        let aliases = config
            .entity_aliases
            .get(&entity_id)
            .map(|items| items.iter().map(String::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        let has_room = config.room_tokens.iter().any(|room| {
            friendly_name.contains(room) || aliases.iter().any(|alias| alias.contains(room))
        });
        if !has_room {
            missing_room_semantic.push(MissingRoomSemanticFinding {
                entity_id: entity_id.clone(),
                friendly_name: friendly_name.clone(),
                domain: state.domain(),
            });
        }
    }
    let mut duplicate_rows: Vec<DuplicateFriendlyNameFinding> = name_to_entities
        .into_iter()
        .filter(|(_, entities)| entities.len() > 1)
        .map(|(friendly_name, entities)| DuplicateFriendlyNameFinding {
            friendly_name,
            entities,
        })
        .collect();
    duplicate_rows.sort_by_key(|row| std::cmp::Reverse(row.entities.len()));
    let cross_domain_conflicts = detect_cross_domain_conflicts(&duplicate_rows);
    let group_suggestions = infer_group_suggestions(&control_states, config);
    if let Some(report) = ha_mcp_live_context.as_mut() {
        refine_live_context_report_with_typed_states(report, all_states);
    }
    let replacement_candidate_reviews = ha_mcp_live_context
        .as_ref()
        .map(build_replacement_candidate_reviews)
        .unwrap_or_default();
    let migration_suggestions =
        build_migration_suggestions(all_states, config, &replacement_candidate_reviews);
    let report = ConfigReport {
        status: "ok".to_string(),
        generated_at: isoformat(now_utc()),
        counts: ConfigReportCounts {
            controllable_entities: control_states.len(),
            duplicate_friendly_names: duplicate_rows.len(),
            cross_domain_conflicts: cross_domain_conflicts.len(),
            missing_room_semantic: missing_room_semantic.len(),
            recent_failures: failure_summary.failure_count as u64,
        },
        recent_failures: failure_summary.clone(),
        findings: ConfigReportFindings {
            duplicate_friendly_names: duplicate_rows.iter().take(30).cloned().collect(),
            cross_domain_conflicts: cross_domain_conflicts.iter().take(30).cloned().collect(),
            missing_room_semantic: missing_room_semantic.iter().take(40).cloned().collect(),
        },
        suggestions: ConfigReportSuggestions {
            entity_aliases: missing_room_semantic
                .iter()
                .take(20)
                .map(|row| EntityAliasSuggestion {
                    entity_id: row.entity_id.clone(),
                    friendly_name: row.friendly_name.clone(),
                    recommended_aliases: Vec::new(),
                })
                .collect(),
            groups: group_suggestions.clone(),
            assist_entities: infer_assist_entities(&control_states),
            custom_sentences: infer_custom_sentence_suggestions(config, &group_suggestions),
            migration_suggestions,
            replacement_candidates: replacement_candidate_reviews.clone(),
        },
        advanced_opportunities: infer_advanced_opportunities(
            &control_states,
            &duplicate_rows,
            &group_suggestions,
            replacement_candidate_reviews.len(),
        ),
        ha_mcp_live_context,
    };
    let cache_path = paths.config_report_cache_path();
    write_report_cache_atomic(&cache_path, &report)?;
    serde_json::to_value(&report).map_err(Into::into)
}

pub fn generate_config_report_with_states(
    paths: &RuntimePaths,
    all_states: &[Value],
    config: &ControlConfig,
    failure_summary: &FailureSummary,
) -> Result<Value> {
    let typed_states: Vec<HaState> = all_states
        .iter()
        .cloned()
        .filter_map(|value| serde_json::from_value::<HaState>(value).ok())
        .collect();
    generate_config_report_with_typed_states(paths, &typed_states, config, failure_summary, None)
}

pub async fn generate_config_report(
    client: &HaClient,
    mcp_client: &HaMcpClient,
    paths: &RuntimePaths,
    config: &ControlConfig,
) -> std::result::Result<Value, ProxyError> {
    let states = fetch_all_states_typed(client).await?;
    let failure_summary = build_failure_summary(paths);
    let ha_mcp_live_context = mcp_client.live_context_report().await.ok();
    generate_config_report_with_typed_states(
        paths,
        &states,
        config,
        &failure_summary,
        ha_mcp_live_context,
    )
    .map_err(|err| ProxyError::Invalid(err.to_string()))
}

pub fn build_replacement_candidates_report(
    live_context: &HaMcpLiveContextReport,
) -> ReplacementCandidatesReport {
    let candidates = build_replacement_candidate_reviews(live_context);
    let high_confidence_count = candidates
        .iter()
        .filter(|candidate| candidate.confidence == "high_confidence")
        .count();
    let needs_review_count = candidates.len().saturating_sub(high_confidence_count);
    ReplacementCandidatesReport {
        status: "ok".to_string(),
        generated_at: isoformat(now_utc()),
        candidate_count: candidates.len(),
        high_confidence_count,
        needs_review_count,
        candidates,
    }
}

fn build_replacement_candidate_reviews(
    live_context: &HaMcpLiveContextReport,
) -> Vec<ReplacementCandidateReview> {
    let mut candidates: Vec<ReplacementCandidateReview> = live_context
        .findings
        .possible_replacements
        .iter()
        .map(|candidate| {
            let confidence = classify_replacement_confidence(candidate.score, &candidate.reasons);
            ReplacementCandidateReview {
                unavailable_name: candidate.unavailable_name.clone(),
                replacement_name: candidate.replacement_name.clone(),
                domain: candidate.domain.clone(),
                score: candidate.score,
                confidence: confidence.to_string(),
                reasons: candidate.reasons.clone(),
                unavailable_areas: candidate.unavailable_areas.clone(),
                replacement_areas: candidate.replacement_areas.clone(),
                suggested_actions: suggested_replacement_actions(candidate, confidence),
            }
        })
        .collect();
    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.unavailable_name.cmp(&right.unavailable_name))
    });
    candidates
}

fn classify_replacement_confidence(score: i32, reasons: &[String]) -> &'static str {
    if score >= 8
        && reasons
            .iter()
            .any(|reason| reason == "same_normalized_base_name")
        && reasons.iter().any(|reason| reason == "same_area")
    {
        "high_confidence"
    } else {
        "needs_review"
    }
}

fn suggested_replacement_actions(
    candidate: &crate::ha_mcp::HaMcpPossibleReplacementFinding,
    confidence: &str,
) -> Vec<String> {
    let mut actions = vec![format!(
        "Review whether '{}' should replace '{}' in aliases and groups",
        candidate.replacement_name, candidate.unavailable_name
    )];
    if confidence == "needs_review" {
        actions.push(
            "Verify both entities are not just duplicate exposures from the same gateway"
                .to_string(),
        );
    }
    if candidate.unavailable_areas.is_empty() || candidate.replacement_areas.is_empty() {
        actions.push(
            "Confirm the target area assignment before migrating voice-facing aliases".to_string(),
        );
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ha_mcp::{HaMcpLiveContextFindings, HaMcpPossibleReplacementFinding};
    use crate::GroupConfig;

    #[test]
    fn migration_suggestions_include_aliases_and_group_member_replacements() {
        let states = vec![
            HaState {
                entity_id: "light.living_main_old".to_string(),
                state: Some("unavailable".to_string()),
                attributes: crate::HaStateAttributes {
                    friendly_name: Some("客厅主灯 2".to_string()),
                    ..crate::HaStateAttributes::default()
                },
                ..HaState::default()
            },
            HaState {
                entity_id: "light.living_main".to_string(),
                state: Some("off".to_string()),
                attributes: crate::HaStateAttributes {
                    friendly_name: Some("客厅主灯".to_string()),
                    ..crate::HaStateAttributes::default()
                },
                ..HaState::default()
            },
        ];
        let mut config = ControlConfig::default();
        config.entity_aliases.insert(
            "light.living_main_old".to_string(),
            vec!["客厅大灯".to_string()],
        );
        config.groups.insert(
            "客厅灯光".to_string(),
            GroupConfig {
                entities: vec![
                    "light.living_main_old".to_string(),
                    "light.living_accent".to_string(),
                ],
                aliases: vec!["客厅灯".to_string()],
            },
        );
        let live_context = HaMcpLiveContextReport {
            findings: HaMcpLiveContextFindings {
                possible_replacements: vec![HaMcpPossibleReplacementFinding {
                    unavailable_name: "客厅主灯 2".to_string(),
                    replacement_name: "客厅主灯".to_string(),
                    domain: "light".to_string(),
                    score: 8,
                    reasons: vec![
                        "same_normalized_base_name".to_string(),
                        "same_area".to_string(),
                    ],
                    ..HaMcpPossibleReplacementFinding::default()
                }],
                ..HaMcpLiveContextFindings::default()
            },
            ..HaMcpLiveContextReport::default()
        };
        let reviews = build_replacement_candidate_reviews(&live_context);

        let suggestions = build_migration_suggestions(&states, &config, &reviews);

        assert!(suggestions.iter().any(|suggestion| {
            suggestion.suggestion_type == "entity_alias_migration"
                && suggestion.target == "light.living_main"
                && suggestion.recommended.contains(&"客厅大灯".to_string())
        }));
        assert!(suggestions.iter().any(|suggestion| {
            suggestion.suggestion_type == "group_member_migration"
                && suggestion.target == "客厅灯光"
                && suggestion
                    .recommended
                    .contains(&"light.living_main".to_string())
                && !suggestion
                    .recommended
                    .contains(&"light.living_main_old".to_string())
        }));
        assert!(suggestions
            .iter()
            .all(|suggestion| suggestion.requires_confirmation));
    }
}
