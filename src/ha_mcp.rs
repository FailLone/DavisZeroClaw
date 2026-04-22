use crate::{
    ha_client::normalize_ha_url, normalize_text, parse_time, HaState, ProxyError, USER_AGENT,
};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(not(test))]
use std::fs;
#[cfg(not(test))]
use std::path::PathBuf;

const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpPrompt {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpCapabilities {
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    #[serde(default)]
    pub tools: Vec<HaMcpTool>,
    #[serde(default)]
    pub prompts: Vec<HaMcpPrompt>,
    pub supports_live_context: bool,
    pub supports_control: bool,
    pub supports_audit_history: bool,
    #[serde(default)]
    pub missing_audit_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpLiveContextReport {
    pub status: String,
    pub endpoint: String,
    pub source_tool: String,
    pub characters: usize,
    pub line_count: usize,
    pub entity_count: usize,
    pub area_count: usize,
    #[serde(default)]
    pub domain_counts: BTreeMap<String, usize>,
    #[serde(default)]
    pub top_areas: Vec<HaMcpAreaCount>,
    pub unavailable_count: usize,
    pub unknown_count: usize,
    #[serde(default)]
    pub attention_entities: Vec<HaMcpAttentionEntity>,
    #[serde(default)]
    pub findings: HaMcpLiveContextFindings,
    #[serde(default)]
    pub observations: Vec<HaMcpEntityObservation>,
    pub preview: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpAreaCount {
    pub area: String,
    pub entities: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpAttentionEntity {
    pub name: String,
    pub domain: String,
    pub state: String,
    #[serde(default)]
    pub areas: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpEntityObservation {
    pub signature: String,
    pub name: String,
    pub domain: String,
    pub state: String,
    #[serde(default)]
    pub areas: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpLiveContextFindings {
    #[serde(default)]
    pub bad_names: Vec<HaMcpBadNameFinding>,
    #[serde(default)]
    pub exposed_duplicate_names: Vec<HaMcpDuplicateNameFinding>,
    #[serde(default)]
    pub exposed_cross_domain_conflicts: Vec<HaMcpDuplicateNameFinding>,
    #[serde(default)]
    pub missing_area_exposure: Vec<HaMcpMissingAreaExposureFinding>,
    #[serde(default)]
    pub unavailable_or_unknown_entities: Vec<HaMcpAttentionEntity>,
    #[serde(default)]
    pub possible_replacements: Vec<HaMcpPossibleReplacementFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpBadNameFinding {
    pub name: String,
    pub domain: String,
    #[serde(default)]
    pub areas: Vec<String>,
    #[serde(default)]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpDuplicateNameFinding {
    pub name: String,
    pub count: usize,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub areas: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpMissingAreaExposureFinding {
    pub name: String,
    pub domain: String,
    pub state: String,
    #[serde(default)]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaMcpPossibleReplacementFinding {
    pub unavailable_name: String,
    pub replacement_name: String,
    pub domain: String,
    #[serde(default)]
    pub unavailable_areas: Vec<String>,
    #[serde(default)]
    pub replacement_areas: Vec<String>,
    pub score: i32,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub time_signals: Vec<String>,
}

impl HaMcpCapabilities {
    fn from_parts(
        endpoint: String,
        initialize: InitializeResult,
        tools: Vec<HaMcpTool>,
        prompts: Vec<HaMcpPrompt>,
    ) -> Self {
        let supports_live_context = tools.iter().any(|tool| tool.name == "GetLiveContext");
        let supports_control = tools.iter().any(|tool| is_control_tool(&tool.name));
        let supports_audit_history = tools.iter().any(|tool| is_audit_tool(&tool.name))
            || prompts.iter().any(|prompt| is_audit_tool(&prompt.name));
        let missing_audit_capabilities = if supports_audit_history {
            Vec::new()
        } else {
            vec![
                "history or logbook MCP tool".to_string(),
                "recorder/timeline MCP prompt".to_string(),
            ]
        };

        Self {
            endpoint,
            server_name: initialize.server_info.name,
            server_version: initialize.server_info.version,
            protocol_version: initialize.protocol_version,
            tools,
            prompts,
            supports_live_context,
            supports_control,
            supports_audit_history,
            missing_audit_capabilities,
        }
    }
}

#[derive(Clone)]
pub struct HaMcpClient {
    client: Client,
    endpoint: String,
    token: String,
}

impl HaMcpClient {
    pub fn from_credentials(ha_url: &str, token: &str) -> std::result::Result<Self, ProxyError> {
        let endpoint = derive_ha_mcp_endpoint(ha_url).map_err(ProxyError::Invalid)?;
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|err| ProxyError::Invalid(err.to_string()))?;
        Ok(Self {
            client,
            endpoint,
            token: token.to_string(),
        })
    }

    async fn rpc<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: Value,
    ) -> std::result::Result<T, ProxyError> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|_| ProxyError::Unreachable)?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(ProxyError::AuthFailed);
        }
        if !status.is_success() {
            return Err(ProxyError::Unreachable);
        }
        let body: RpcEnvelope<T> = response
            .json()
            .await
            .map_err(|err| ProxyError::Invalid(err.to_string()))?;
        if let Some(error) = body.error {
            return Err(ProxyError::Invalid(format!(
                "ha mcp {} failed: {}",
                method, error.message
            )));
        }
        body.result
            .ok_or_else(|| ProxyError::Invalid(format!("ha mcp {} returned no result", method)))
    }

    pub async fn capabilities(&self) -> std::result::Result<HaMcpCapabilities, ProxyError> {
        let initialize: InitializeResult = self
            .rpc(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "davis-local-proxy",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            )
            .await?;
        let tools: ToolsListResult = self.rpc("tools/list", json!({})).await?;
        let prompts: PromptsListResult = self
            .rpc("prompts/list", json!({}))
            .await
            .unwrap_or_default();

        Ok(HaMcpCapabilities::from_parts(
            self.endpoint.clone(),
            initialize,
            tools.tools,
            prompts.prompts,
        ))
    }

    pub async fn live_context_report(
        &self,
    ) -> std::result::Result<HaMcpLiveContextReport, ProxyError> {
        let capabilities = self.capabilities().await?;
        if !capabilities.supports_live_context {
            return Err(ProxyError::Invalid(
                "ha mcp does not expose GetLiveContext".to_string(),
            ));
        }
        let raw_text = self.call_tool_text("GetLiveContext", json!({})).await?;
        Ok(build_live_context_report(&self.endpoint, &raw_text))
    }

    #[cfg(test)]
    pub(crate) fn from_parts(client: Client, endpoint: String, token: String) -> Self {
        Self {
            client,
            endpoint,
            token,
        }
    }

    async fn call_tool_text(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> std::result::Result<String, ProxyError> {
        let response: ToolCallResult = self
            .rpc(
                "tools/call",
                json!({
                    "name": tool_name,
                    "arguments": arguments,
                }),
            )
            .await?;
        let text = response
            .content
            .into_iter()
            .find_map(|item| item.text)
            .ok_or_else(|| {
                ProxyError::Invalid(format!("ha mcp tool {} returned no text", tool_name))
            })?;
        Ok(extract_live_context_text(&text))
    }
}

pub fn derive_ha_mcp_endpoint(ha_url: &str) -> std::result::Result<String, String> {
    let normalized = normalize_ha_url(ha_url)?;
    let mut parsed =
        url::Url::parse(&normalized).map_err(|_| "home_assistant.url 不是合法 URL".to_string())?;
    let path = parsed.path().trim();
    if path.is_empty() || path == "/" {
        parsed.set_path("/api/mcp");
    }
    Ok(parsed.to_string())
}

#[derive(Debug, Deserialize)]
struct RpcEnvelope<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    message: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct InitializeResult {
    #[serde(default)]
    protocol_version: Option<String>,
    #[serde(default)]
    server_info: ServerInfo,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ServerInfo {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ToolsListResult {
    #[serde(default)]
    tools: Vec<HaMcpTool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PromptsListResult {
    #[serde(default)]
    prompts: Vec<HaMcpPrompt>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ToolCallResult {
    #[serde(default)]
    content: Vec<ToolCallContent>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ToolCallContent {
    #[serde(default)]
    text: Option<String>,
}

fn is_control_tool(name: &str) -> bool {
    name.starts_with("Hass") && !is_audit_tool(name) && name != "GetLiveContext"
}

fn is_audit_tool(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    ["history", "logbook", "recorder", "timeline", "event"]
        .iter()
        .any(|needle| lowered.contains(needle))
}

fn extract_live_context_text(raw_text: &str) -> String {
    serde_json::from_str::<Value>(raw_text)
        .ok()
        .and_then(|value| {
            value
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| raw_text.to_string())
}

fn build_live_context_report(endpoint: &str, raw_text: &str) -> HaMcpLiveContextReport {
    #[cfg(not(test))]
    let previous_snapshot = load_previous_live_context_report();
    #[cfg(test)]
    let previous_snapshot = None;
    build_live_context_report_with_previous(endpoint, raw_text, previous_snapshot.as_ref())
}

fn build_live_context_report_with_previous(
    endpoint: &str,
    raw_text: &str,
    previous_snapshot: Option<&HaMcpLiveContextReport>,
) -> HaMcpLiveContextReport {
    let normalized = extract_live_context_text(raw_text);
    let parsed_entities = parse_live_context_entities(&normalized);
    let lines: Vec<&str> = normalized.lines().collect();
    let preview_lines = lines.iter().take(18).copied().collect::<Vec<_>>();
    let preview = preview_lines.join("\n");
    let mut domain_counts = BTreeMap::new();
    let mut area_counts = BTreeMap::new();
    let mut areas_seen = BTreeSet::new();
    let mut attention_entities = Vec::new();
    let mut bad_names = Vec::new();
    let mut missing_area_exposure = Vec::new();
    let mut unavailable_count = 0;
    let mut unknown_count = 0;
    let mut duplicate_name_groups: BTreeMap<String, Vec<&ParsedLiveContextEntity>> =
        BTreeMap::new();
    let mut current_observations = Vec::new();
    let mut current_signature_counts: BTreeMap<String, SignatureActivity> = BTreeMap::new();

    for entity in &parsed_entities {
        let signature = entity_signature(entity);
        current_observations.push(HaMcpEntityObservation {
            signature: signature.clone(),
            name: entity.name.clone(),
            domain: entity.domain.clone(),
            state: entity.state.clone(),
            areas: entity.areas.clone(),
        });
        current_signature_counts
            .entry(signature)
            .or_default()
            .observe(&entity.state);
        duplicate_name_groups
            .entry(entity.name.clone())
            .or_default()
            .push(entity);
        *domain_counts.entry(entity.domain.clone()).or_insert(0) += 1;
        for area in &entity.areas {
            areas_seen.insert(area.clone());
            *area_counts.entry(area.clone()).or_insert(0) += 1;
        }
        let reasons = detect_bad_name_reasons(&entity.name);
        if !reasons.is_empty() && bad_names.len() < 20 {
            bad_names.push(HaMcpBadNameFinding {
                name: entity.name.clone(),
                domain: entity.domain.clone(),
                areas: entity.areas.clone(),
                reasons,
            });
        }
        let missing_area_reasons = detect_missing_area_reasons(entity);
        if !missing_area_reasons.is_empty() && missing_area_exposure.len() < 20 {
            missing_area_exposure.push(HaMcpMissingAreaExposureFinding {
                name: entity.name.clone(),
                domain: entity.domain.clone(),
                state: entity.state.clone(),
                reasons: missing_area_reasons,
            });
        }
        if entity.state.eq_ignore_ascii_case("unavailable") {
            unavailable_count += 1;
            if attention_entities.len() < 20 {
                attention_entities.push(HaMcpAttentionEntity {
                    name: entity.name.clone(),
                    domain: entity.domain.clone(),
                    state: entity.state.clone(),
                    areas: entity.areas.clone(),
                });
            }
        } else if entity.state.eq_ignore_ascii_case("unknown") {
            unknown_count += 1;
            if attention_entities.len() < 20 {
                attention_entities.push(HaMcpAttentionEntity {
                    name: entity.name.clone(),
                    domain: entity.domain.clone(),
                    state: entity.state.clone(),
                    areas: entity.areas.clone(),
                });
            }
        }
    }

    let mut top_areas: Vec<HaMcpAreaCount> = area_counts
        .into_iter()
        .map(|(area, entities)| HaMcpAreaCount { area, entities })
        .collect();
    top_areas.sort_by(|left, right| {
        right
            .entities
            .cmp(&left.entities)
            .then_with(|| left.area.cmp(&right.area))
    });
    top_areas.truncate(10);

    let mut exposed_duplicate_names: Vec<HaMcpDuplicateNameFinding> = duplicate_name_groups
        .into_iter()
        .filter(|(name, rows)| !name.is_empty() && rows.len() > 1)
        .map(|(name, rows)| {
            let mut domains = rows
                .iter()
                .map(|row| row.domain.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            let mut areas = rows
                .iter()
                .flat_map(|row| row.areas.iter().cloned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            domains.sort();
            areas.sort();
            HaMcpDuplicateNameFinding {
                name,
                count: rows.len(),
                domains,
                areas,
            }
        })
        .collect();
    exposed_duplicate_names.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name.cmp(&right.name))
    });
    exposed_duplicate_names.truncate(20);
    let mut exposed_cross_domain_conflicts = exposed_duplicate_names
        .iter()
        .filter(|finding| finding.domains.len() > 1)
        .cloned()
        .collect::<Vec<_>>();
    exposed_cross_domain_conflicts.truncate(20);

    let previous_signature_counts = previous_snapshot
        .map(|snapshot| signature_activity_from_observations(&snapshot.observations))
        .unwrap_or_default();
    let unavailable_entities: Vec<&ParsedLiveContextEntity> = parsed_entities
        .iter()
        .filter(|entity| entity.state.eq_ignore_ascii_case("unavailable"))
        .collect();
    let available_entities: Vec<&ParsedLiveContextEntity> = parsed_entities
        .iter()
        .filter(|entity| {
            !entity.state.eq_ignore_ascii_case("unavailable")
                && !entity.state.eq_ignore_ascii_case("unknown")
        })
        .collect();
    let mut possible_replacements = infer_possible_replacements(
        &unavailable_entities,
        &available_entities,
        &current_signature_counts,
        &previous_signature_counts,
    );
    possible_replacements.truncate(20);

    HaMcpLiveContextReport {
        status: "ok".to_string(),
        endpoint: endpoint.to_string(),
        source_tool: "GetLiveContext".to_string(),
        characters: normalized.chars().count(),
        line_count: lines.len(),
        entity_count: parsed_entities.len(),
        area_count: areas_seen.len(),
        domain_counts,
        top_areas,
        unavailable_count,
        unknown_count,
        attention_entities: attention_entities.clone(),
        findings: HaMcpLiveContextFindings {
            bad_names,
            exposed_duplicate_names,
            exposed_cross_domain_conflicts,
            missing_area_exposure,
            unavailable_or_unknown_entities: attention_entities,
            possible_replacements,
        },
        observations: current_observations,
        preview,
        truncated: lines.len() > preview_lines.len(),
    }
}

pub fn refine_live_context_report_with_typed_states(
    report: &mut HaMcpLiveContextReport,
    states: &[HaState],
) {
    for candidate in &mut report.findings.possible_replacements {
        let unavailable_states = matching_typed_states(
            states,
            &candidate.domain,
            &candidate.unavailable_name,
            &candidate.unavailable_areas,
            true,
        );
        let replacement_states = matching_typed_states(
            states,
            &candidate.domain,
            &candidate.replacement_name,
            &candidate.replacement_areas,
            false,
        );
        if unavailable_states.is_empty() || replacement_states.is_empty() {
            continue;
        }

        push_unique(&mut candidate.reasons, "typed_state_linked");
        candidate.score += 1;

        let unavailable = unavailable_states[0];
        let replacement = replacement_states[0];
        if same_entity_shape(unavailable, replacement) {
            push_unique(&mut candidate.reasons, "typed_state_shape_match");
            candidate.score += 1;
        } else {
            push_unique(&mut candidate.reasons, "typed_state_shape_mismatch");
            candidate.score -= 2;
        }

        match typed_state_change_delta_hours(unavailable, replacement) {
            Some(hours) if hours <= 24 => {
                push_unique(&mut candidate.reasons, "typed_state_change_window_match");
                push_unique(
                    &mut candidate.time_signals,
                    "typed_state_change_window_match",
                );
                candidate.score += 2;
            }
            Some(hours) if hours > 24 * 7 => {
                push_unique(&mut candidate.reasons, "typed_state_change_window_mismatch");
                push_unique(
                    &mut candidate.time_signals,
                    "typed_state_change_window_mismatch",
                );
                candidate.score -= 3;
            }
            Some(_) => {
                push_unique(
                    &mut candidate.time_signals,
                    "typed_state_change_window_neutral",
                );
            }
            None => {}
        }

        if same_source(unavailable, replacement)
            && same_entity_shape(unavailable, replacement)
            && candidate.reasons.iter().any(|reason| {
                reason == "typed_state_change_window_mismatch"
                    || reason == "persistent_dual_active_duplicate_exposure"
            })
        {
            push_unique(
                &mut candidate.reasons,
                "typed_state_persistent_duplicate_shape",
            );
            candidate.score -= 4;
        }
    }

    report.findings.possible_replacements.retain(|candidate| {
        candidate.score >= 5
            && !candidate
                .reasons
                .iter()
                .any(|reason| reason == "typed_state_persistent_duplicate_shape")
    });
    report
        .findings
        .possible_replacements
        .sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.unavailable_name.cmp(&right.unavailable_name))
        });
}

#[derive(Debug, Clone, Default)]
struct ParsedLiveContextEntity {
    name: String,
    domain: String,
    state: String,
    areas: Vec<String>,
}

fn parse_live_context_entities(text: &str) -> Vec<ParsedLiveContextEntity> {
    let mut entities = Vec::new();
    let mut current: Option<ParsedLiveContextEntity> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("- names: ") {
            if let Some(entity) = current.take() {
                if !entity.name.is_empty() {
                    entities.push(entity);
                }
            }
            current = Some(ParsedLiveContextEntity {
                name: name.trim().to_string(),
                ..ParsedLiveContextEntity::default()
            });
            continue;
        }

        let Some(entity) = current.as_mut() else {
            continue;
        };

        if let Some(domain) = trimmed.strip_prefix("domain: ") {
            entity.domain = trim_wrapped_value(domain);
        } else if let Some(state) = trimmed.strip_prefix("state: ") {
            entity.state = trim_wrapped_value(state);
        } else if let Some(areas) = trimmed.strip_prefix("areas: ") {
            entity.areas = areas
                .split(',')
                .map(trim_wrapped_value)
                .filter(|item| !item.is_empty())
                .collect();
        }
    }

    if let Some(entity) = current {
        if !entity.name.is_empty() {
            entities.push(entity);
        }
    }

    entities
}

fn trim_wrapped_value(raw: &str) -> String {
    raw.trim()
        .trim_matches('\'')
        .trim_matches('"')
        .trim()
        .to_string()
}

fn infer_possible_replacements(
    unavailable_entities: &[&ParsedLiveContextEntity],
    available_entities: &[&ParsedLiveContextEntity],
    current_signature_counts: &BTreeMap<String, SignatureActivity>,
    previous_signature_counts: &BTreeMap<String, SignatureActivity>,
) -> Vec<HaMcpPossibleReplacementFinding> {
    let mut findings = Vec::new();

    for unavailable in unavailable_entities {
        let mut best: Option<(i32, Vec<String>, Vec<String>, &ParsedLiveContextEntity)> = None;
        for candidate in available_entities {
            if unavailable.domain != candidate.domain {
                continue;
            }
            let (score, reasons, time_signals, suppress) = replacement_score(
                unavailable,
                candidate,
                current_signature_counts,
                previous_signature_counts,
            );
            if suppress {
                continue;
            }
            if score < 5 {
                continue;
            }
            let should_replace = match &best {
                Some((best_score, _, _, best_candidate)) => {
                    score > *best_score
                        || (score == *best_score
                            && candidate.name.len() < best_candidate.name.len())
                }
                None => true,
            };
            if should_replace {
                best = Some((score, reasons, time_signals, candidate));
            }
        }

        if let Some((score, reasons, time_signals, candidate)) = best {
            findings.push(HaMcpPossibleReplacementFinding {
                unavailable_name: unavailable.name.clone(),
                replacement_name: candidate.name.clone(),
                domain: unavailable.domain.clone(),
                unavailable_areas: unavailable.areas.clone(),
                replacement_areas: candidate.areas.clone(),
                score,
                reasons,
                time_signals,
            });
        }
    }

    findings.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.unavailable_name.cmp(&right.unavailable_name))
    });
    findings
}

fn replacement_score(
    unavailable: &ParsedLiveContextEntity,
    candidate: &ParsedLiveContextEntity,
    current_signature_counts: &BTreeMap<String, SignatureActivity>,
    previous_signature_counts: &BTreeMap<String, SignatureActivity>,
) -> (i32, Vec<String>, Vec<String>, bool) {
    let mut score = 0;
    let mut reasons = Vec::new();
    let mut time_signals = Vec::new();
    let mut suppress = false;

    let unavailable_norm = normalize_text(&unavailable.name);
    let candidate_norm = normalize_text(&candidate.name);
    let unavailable_base = trim_numeric_suffix(&unavailable_norm);
    let candidate_base = trim_numeric_suffix(&candidate_norm);
    let unavailable_signature = entity_signature(unavailable);
    let candidate_signature = entity_signature(candidate);
    let current_unavailable_activity = current_signature_counts
        .get(&unavailable_signature)
        .cloned()
        .unwrap_or_default();
    let current_candidate_activity = current_signature_counts
        .get(&candidate_signature)
        .cloned()
        .unwrap_or_default();
    let previous_unavailable_activity = previous_signature_counts
        .get(&unavailable_signature)
        .cloned()
        .unwrap_or_default();
    let previous_candidate_activity = previous_signature_counts
        .get(&candidate_signature)
        .cloned()
        .unwrap_or_default();

    if !unavailable_base.is_empty() && unavailable_base == candidate_base {
        score += 4;
        reasons.push("same_normalized_base_name".to_string());
    } else if !unavailable_base.is_empty()
        && !candidate_base.is_empty()
        && (unavailable_base.contains(&candidate_base)
            || candidate_base.contains(&unavailable_base))
    {
        score += 2;
        reasons.push("base_name_contains_match".to_string());
    }

    let unavailable_tokens = name_tokens(&unavailable.name);
    let candidate_tokens = name_tokens(&candidate.name);
    let overlap = unavailable_tokens.intersection(&candidate_tokens).count();
    if overlap >= 2 {
        score += 2;
        reasons.push("token_overlap".to_string());
    } else if overlap == 1 {
        score += 1;
        reasons.push("partial_token_overlap".to_string());
    }

    let unavailable_areas: BTreeSet<String> = unavailable.areas.iter().cloned().collect();
    let candidate_areas: BTreeSet<String> = candidate.areas.iter().cloned().collect();
    let area_overlap = unavailable_areas.intersection(&candidate_areas).count();
    if area_overlap > 0 {
        score += 2;
        reasons.push("same_area".to_string());
    } else if unavailable.areas.is_empty() || candidate.areas.is_empty() {
        score += 1;
        reasons.push("area_missing_on_one_side".to_string());
    }

    if unavailable_signature == candidate_signature {
        if current_unavailable_activity.dual_active() {
            score -= 2;
            reasons.push("current_dual_active_duplicate_exposure".to_string());
            time_signals.push("current_dual_active_duplicate_exposure".to_string());
        }
        if previous_unavailable_activity.dual_active() {
            score -= 3;
            reasons.push("persistent_dual_active_duplicate_exposure".to_string());
            time_signals.push("persistent_dual_active_duplicate_exposure".to_string());
            if current_unavailable_activity.dual_active() {
                suppress = true;
            }
        }
    } else {
        if current_candidate_activity.observed > 0 && previous_candidate_activity.observed == 0 {
            score += 1;
            reasons.push("recently_seen_since_previous_snapshot".to_string());
            time_signals.push("recently_seen_since_previous_snapshot".to_string());
        }
        if current_unavailable_activity.observed > 0 && previous_unavailable_activity.observed == 0
        {
            score += 1;
            reasons.push("recently_unavailable_since_previous_snapshot".to_string());
            time_signals.push("recently_unavailable_since_previous_snapshot".to_string());
        }
    }

    (score, reasons, time_signals, suppress)
}

#[derive(Debug, Clone, Default)]
struct SignatureActivity {
    observed: usize,
    available: usize,
    unavailable: usize,
    unknown: usize,
}

impl SignatureActivity {
    fn observe(&mut self, state: &str) {
        self.observed += 1;
        if state.eq_ignore_ascii_case("unavailable") {
            self.unavailable += 1;
        } else if state.eq_ignore_ascii_case("unknown") {
            self.unknown += 1;
        } else {
            self.available += 1;
        }
    }

    fn dual_active(&self) -> bool {
        self.available > 0 && self.unavailable > 0
    }
}

fn signature_activity_from_observations(
    observations: &[HaMcpEntityObservation],
) -> BTreeMap<String, SignatureActivity> {
    let mut counts: BTreeMap<String, SignatureActivity> = BTreeMap::new();
    for observation in observations {
        counts
            .entry(observation.signature.clone())
            .or_default()
            .observe(&observation.state);
    }
    counts
}

fn entity_signature(entity: &ParsedLiveContextEntity) -> String {
    let mut areas = entity
        .areas
        .iter()
        .map(|area| normalize_text(area))
        .filter(|area| !area.is_empty())
        .collect::<Vec<_>>();
    areas.sort();
    format!(
        "{}|{}|{}",
        entity.domain.trim().to_ascii_lowercase(),
        trim_numeric_suffix(&normalize_text(&entity.name)),
        areas.join("|")
    )
}

fn matching_typed_states<'a>(
    states: &'a [HaState],
    domain: &str,
    name: &str,
    areas: &[String],
    prefer_unavailable: bool,
) -> Vec<&'a HaState> {
    let normalized_name = normalize_text(name);
    let normalized_areas = areas
        .iter()
        .map(|area| normalize_text(area))
        .collect::<Vec<_>>();
    let mut matches = states
        .iter()
        .filter(|state| state.domain() == domain)
        .filter(|state| normalize_text(&state.friendly_name()) == normalized_name)
        .collect::<Vec<_>>();

    if matches.len() > 1 && !normalized_areas.is_empty() {
        let area_matches = matches
            .iter()
            .copied()
            .filter(|state| {
                let haystack = normalize_text(&format!("{} {}", state.entity_id, state.suffix()));
                normalized_areas
                    .iter()
                    .any(|area| !area.is_empty() && haystack.contains(area))
            })
            .collect::<Vec<_>>();
        if !area_matches.is_empty() {
            matches = area_matches;
        }
    }

    matches.sort_by(|left, right| {
        let left_rank = state_availability_rank(left, prefer_unavailable);
        let right_rank = state_availability_rank(right, prefer_unavailable);
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.entity_id.cmp(&right.entity_id))
    });
    matches
}

fn state_availability_rank(state: &HaState, prefer_unavailable: bool) -> usize {
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

fn same_entity_shape(left: &HaState, right: &HaState) -> bool {
    entity_shape_key(left) == entity_shape_key(right)
}

fn entity_shape_key(state: &HaState) -> String {
    trim_numeric_suffix(&normalize_text(&state.suffix()))
}

fn same_source(left: &HaState, right: &HaState) -> bool {
    match (&left.attributes.source, &right.attributes.source) {
        (Some(left), Some(right)) => normalize_text(left) == normalize_text(right),
        _ => false,
    }
}

fn typed_state_change_delta_hours(left: &HaState, right: &HaState) -> Option<i64> {
    let left = state_changed_at(left)?;
    let right = state_changed_at(right)?;
    Some((left - right).num_hours().abs())
}

fn state_changed_at(state: &HaState) -> Option<chrono::DateTime<chrono::Utc>> {
    state
        .last_changed
        .as_deref()
        .or(state.last_updated.as_deref())
        .and_then(parse_time)
}

fn push_unique(items: &mut Vec<String>, value: &str) {
    if !items.iter().any(|item| item == value) {
        items.push(value.to_string());
    }
}

#[cfg(not(test))]
#[allow(dead_code)]
fn load_previous_live_context_report() -> Option<HaMcpLiveContextReport> {
    let path = live_context_state_path();
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str::<HaMcpLiveContextReport>(&raw).ok()
}

#[cfg(test)]
#[allow(dead_code)]
fn load_previous_live_context_report() -> Option<HaMcpLiveContextReport> {
    None
}

#[cfg(not(test))]
fn live_context_state_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("DAVIS_RUNTIME_DIR") {
        return PathBuf::from(runtime_dir)
            .join("state")
            .join("ha_mcp_live_context.json");
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".runtime")
        .join("davis")
        .join("state")
        .join("ha_mcp_live_context.json")
}

fn trim_numeric_suffix(value: &str) -> String {
    value
        .trim_end_matches(|ch: char| ch.is_ascii_digit())
        .to_string()
}

fn name_tokens(value: &str) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    for segment in value
        .replace(['_', '-'], " ")
        .split_whitespace()
        .map(|segment| segment.trim_matches(|ch: char| !ch.is_alphanumeric() && !is_cjk(ch)))
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_lowercase())
        .filter(|segment| segment.chars().count() >= 2)
    {
        tokens.insert(segment.clone());
        if segment.chars().any(is_cjk) {
            let chars = segment.chars().collect::<Vec<_>>();
            for window in chars.windows(2) {
                tokens.insert(window.iter().collect::<String>());
            }
        }
    }
    tokens
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x4E00..=0x9FFF)
}

fn detect_bad_name_reasons(name: &str) -> Vec<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return vec!["empty_name".to_string()];
    }

    let mut reasons = Vec::new();
    if trimmed.contains('_') {
        reasons.push("contains_underscore".to_string());
    }
    if trimmed.contains("  ") {
        reasons.push("contains_repeated_spaces".to_string());
    }
    if trimmed
        .chars()
        .next()
        .map(|ch| ch.is_ascii_lowercase())
        .unwrap_or(false)
        && !contains_cjk(trimmed)
    {
        reasons.push("starts_with_lowercase_ascii".to_string());
    }
    if trimmed
        .split_whitespace()
        .last()
        .map(|segment| segment.chars().all(|ch| ch.is_ascii_digit()))
        .unwrap_or(false)
    {
        reasons.push("ends_with_numeric_suffix".to_string());
    }
    if !contains_cjk(trimmed) && ascii_letter_ratio(trimmed) > 0.7 {
        reasons.push("mostly_ascii_technical_name".to_string());
    }
    reasons
}

fn detect_missing_area_reasons(entity: &ParsedLiveContextEntity) -> Vec<String> {
    if !entity.areas.is_empty() {
        return Vec::new();
    }

    let mut reasons = vec!["no_area_exposed".to_string()];
    if !contains_room_hint(&entity.name) {
        reasons.push("name_lacks_room_semantic".to_string());
    }
    reasons
}

fn ascii_letter_ratio(value: &str) -> f32 {
    let mut total = 0usize;
    let mut ascii_letters = 0usize;
    for ch in value.chars() {
        if ch.is_whitespace() {
            continue;
        }
        total += 1;
        if ch.is_ascii_alphabetic() {
            ascii_letters += 1;
        }
    }
    if total == 0 {
        return 0.0;
    }
    ascii_letters as f32 / total as f32
}

fn contains_cjk(value: &str) -> bool {
    value.chars().any(|ch| matches!(ch as u32, 0x4E00..=0x9FFF))
}

fn contains_room_hint(value: &str) -> bool {
    let lowered = normalize_text(value);
    let chinese_hints = [
        "书房",
        "客厅",
        "卧室",
        "主卧",
        "次卧",
        "父母间",
        "儿童房",
        "厨房",
        "餐厅",
        "卫生间",
        "浴室",
        "阳台",
        "玄关",
        "走廊",
        "车库",
    ];
    let ascii_hints = [
        "living", "bedroom", "study", "office", "kitchen", "bath", "bathroom", "garage", "hall",
        "balcony", "dining",
    ];

    chinese_hints.iter().any(|hint| value.contains(hint))
        || ascii_hints.iter().any(|hint| lowered.contains(hint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_ha_mcp_endpoint_appends_api_mcp_for_root_url() {
        let endpoint = derive_ha_mcp_endpoint("http://homeassistant.local:8123").unwrap();
        assert!(endpoint.ends_with("/api/mcp"));
    }

    #[test]
    fn derive_ha_mcp_endpoint_keeps_existing_path() {
        let endpoint = derive_ha_mcp_endpoint("https://example.com/api/mcp").unwrap();
        assert_eq!(endpoint, "https://example.com/api/mcp");
    }

    #[test]
    fn capabilities_flag_history_support_only_when_explicit_tools_exist() {
        let capabilities = HaMcpCapabilities::from_parts(
            "http://example.com/api/mcp".to_string(),
            InitializeResult {
                protocol_version: Some(MCP_PROTOCOL_VERSION.to_string()),
                server_info: ServerInfo {
                    name: Some("home-assistant".to_string()),
                    version: Some("1.26.0".to_string()),
                },
            },
            vec![
                HaMcpTool {
                    name: "HassTurnOn".to_string(),
                    description: String::new(),
                },
                HaMcpTool {
                    name: "GetLiveContext".to_string(),
                    description: String::new(),
                },
            ],
            vec![HaMcpPrompt {
                name: "Assist".to_string(),
                description: String::new(),
            }],
        );
        assert!(capabilities.supports_live_context);
        assert!(capabilities.supports_control);
        assert!(!capabilities.supports_audit_history);
        assert!(!capabilities.missing_audit_capabilities.is_empty());
    }

    #[test]
    fn extract_live_context_text_prefers_nested_result_field() {
        let raw = r#"{"success":true,"result":"line1\nline2"}"#;
        assert_eq!(extract_live_context_text(raw), "line1\nline2");
    }

    #[test]
    fn build_live_context_report_counts_and_truncates() {
        let body = [
            "Live Context: overview",
            "- names: 书房灯带",
            "  domain: light",
            "  state: 'off'",
            "  areas: 书房",
            "- names: 客厅主灯",
            "  domain: light",
            "  state: unavailable",
            "  areas: 客厅",
            "- names: 客厅主灯",
            "  domain: switch",
            "  state: 'off'",
            "  areas: 客厅",
            "- names: children_room_target_temperature",
            "  domain: sensor",
            "  state: '21'",
            "- names: 主卧空调",
            "  domain: climate",
            "  state: unknown",
            "  areas: 主卧",
        ]
        .join("\n");
        let report = build_live_context_report("http://example.com/api/mcp", &body);
        assert_eq!(report.status, "ok");
        assert_eq!(report.entity_count, 5);
        assert_eq!(report.area_count, 3);
        assert_eq!(report.domain_counts.get("light"), Some(&2));
        assert_eq!(report.domain_counts.get("climate"), Some(&1));
        assert_eq!(report.domain_counts.get("switch"), Some(&1));
        assert_eq!(report.domain_counts.get("sensor"), Some(&1));
        assert_eq!(report.unavailable_count, 1);
        assert_eq!(report.unknown_count, 1);
        assert_eq!(report.attention_entities.len(), 2);
        assert_eq!(report.findings.exposed_duplicate_names.len(), 1);
        assert_eq!(report.findings.exposed_cross_domain_conflicts.len(), 1);
        assert_eq!(report.findings.missing_area_exposure.len(), 1);
        assert!(report.preview.contains("书房灯带"));
    }

    #[test]
    fn parse_live_context_entities_extracts_domain_state_and_areas() {
        let text = [
            "Live Context: overview",
            "- names: 书房灯带",
            "  domain: light",
            "  state: 'off'",
            "  areas: 书房, 工作区",
            "- names: 主卧空调",
            "  domain: climate",
            "  state: unavailable",
        ]
        .join("\n");
        let entities = parse_live_context_entities(&text);
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].name, "书房灯带");
        assert_eq!(entities[0].domain, "light");
        assert_eq!(entities[0].state, "off");
        assert_eq!(
            entities[0].areas,
            vec!["书房".to_string(), "工作区".to_string()]
        );
        assert_eq!(entities[1].state, "unavailable");
    }

    #[test]
    fn detect_bad_name_reasons_flags_technical_patterns() {
        let reasons = detect_bad_name_reasons("shu fang san kai 2");
        assert!(reasons.contains(&"starts_with_lowercase_ascii".to_string()));
        assert!(reasons.contains(&"ends_with_numeric_suffix".to_string()));
        assert!(reasons.contains(&"mostly_ascii_technical_name".to_string()));
    }

    #[test]
    fn detect_missing_area_reasons_flags_entities_without_area_or_room_hint() {
        let entity = ParsedLiveContextEntity {
            name: "target_temperature_sensor".to_string(),
            domain: "sensor".to_string(),
            state: "21".to_string(),
            areas: Vec::new(),
        };
        let reasons = detect_missing_area_reasons(&entity);
        assert!(reasons.contains(&"no_area_exposed".to_string()));
        assert!(reasons.contains(&"name_lacks_room_semantic".to_string()));
    }

    #[test]
    fn build_live_context_report_marks_newly_seen_candidates_with_time_signals() {
        let previous_body = [
            "Live Context: overview",
            "- names: 书房灯带",
            "  domain: light",
            "  state: 'off'",
            "  areas: 书房",
        ]
        .join("\n");
        let current_body = [
            "Live Context: overview",
            "- names: 客厅主灯 2",
            "  domain: light",
            "  state: unavailable",
            "  areas: 客厅",
            "- names: 客厅吊灯 新版",
            "  domain: light",
            "  state: 'off'",
            "  areas: 客厅",
        ]
        .join("\n");
        let previous_snapshot = build_live_context_report_with_previous(
            "http://example.com/api/mcp",
            &previous_body,
            None,
        );
        let report = build_live_context_report_with_previous(
            "http://example.com/api/mcp",
            &current_body,
            Some(&previous_snapshot),
        );
        let candidate = report
            .findings
            .possible_replacements
            .first()
            .expect("expected a replacement candidate");
        assert!(candidate
            .time_signals
            .contains(&"recently_seen_since_previous_snapshot".to_string()));
        assert!(candidate
            .time_signals
            .contains(&"recently_unavailable_since_previous_snapshot".to_string()));
    }

    #[test]
    fn build_live_context_report_suppresses_persistent_same_signature_dual_active_pairs() {
        let body = [
            "Live Context: overview",
            "- names: 客厅主灯",
            "  domain: light",
            "  state: unavailable",
            "  areas: 客厅",
            "- names: 客厅主灯",
            "  domain: light",
            "  state: 'off'",
            "  areas: 客厅",
        ]
        .join("\n");
        let previous_snapshot =
            build_live_context_report_with_previous("http://example.com/api/mcp", &body, None);
        let report = build_live_context_report_with_previous(
            "http://example.com/api/mcp",
            &body,
            Some(&previous_snapshot),
        );
        assert!(report.findings.possible_replacements.is_empty());
    }

    #[test]
    fn infer_possible_replacements_matches_same_base_name_and_area() {
        let unavailable = ParsedLiveContextEntity {
            name: "客厅主灯 2".to_string(),
            domain: "light".to_string(),
            state: "unavailable".to_string(),
            areas: vec!["客厅".to_string()],
        };
        let candidate = ParsedLiveContextEntity {
            name: "客厅主灯".to_string(),
            domain: "light".to_string(),
            state: "on".to_string(),
            areas: vec!["客厅".to_string()],
        };
        let empty_counts: BTreeMap<String, SignatureActivity> = BTreeMap::new();
        let findings = infer_possible_replacements(
            &[&unavailable],
            &[&candidate],
            &empty_counts,
            &empty_counts,
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].replacement_name, "客厅主灯");
        assert!(findings[0].score >= 5);
    }

    #[test]
    fn refine_replacements_adds_typed_state_shape_and_time_signals() {
        let body = [
            "Live Context: overview",
            "- names: 客厅主灯 2",
            "  domain: light",
            "  state: unavailable",
            "  areas: 客厅",
            "- names: 客厅主灯",
            "  domain: light",
            "  state: 'off'",
            "  areas: 客厅",
        ]
        .join("\n");
        let mut report = build_live_context_report("http://example.com/api/mcp", &body);
        let states = vec![
            HaState {
                entity_id: "light.living_main_2".to_string(),
                state: Some("unavailable".to_string()),
                last_changed: Some("2026-04-15T00:00:00Z".to_string()),
                last_updated: None,
                attributes: crate::HaStateAttributes {
                    friendly_name: Some("客厅主灯 2".to_string()),
                    source: Some("homekit".to_string()),
                    ..crate::HaStateAttributes::default()
                },
            },
            HaState {
                entity_id: "light.living_main".to_string(),
                state: Some("off".to_string()),
                last_changed: Some("2026-04-15T01:00:00Z".to_string()),
                last_updated: None,
                attributes: crate::HaStateAttributes {
                    friendly_name: Some("客厅主灯".to_string()),
                    source: Some("homekit".to_string()),
                    ..crate::HaStateAttributes::default()
                },
            },
        ];

        refine_live_context_report_with_typed_states(&mut report, &states);

        let candidate = report
            .findings
            .possible_replacements
            .first()
            .expect("expected refined replacement candidate");
        assert!(candidate
            .reasons
            .contains(&"typed_state_linked".to_string()));
        assert!(candidate
            .reasons
            .contains(&"typed_state_shape_match".to_string()));
        assert!(candidate
            .time_signals
            .contains(&"typed_state_change_window_match".to_string()));
    }

    #[test]
    fn refine_replacements_suppresses_stale_same_source_duplicate_shape() {
        let body = [
            "Live Context: overview",
            "- names: 客厅主灯",
            "  domain: light",
            "  state: unavailable",
            "  areas: 客厅",
            "- names: 客厅主灯",
            "  domain: light",
            "  state: 'off'",
            "  areas: 客厅",
        ]
        .join("\n");
        let mut report = build_live_context_report("http://example.com/api/mcp", &body);
        assert!(!report.findings.possible_replacements.is_empty());

        let states = vec![
            HaState {
                entity_id: "light.living_main_2".to_string(),
                state: Some("unavailable".to_string()),
                last_changed: Some("2026-01-01T00:00:00Z".to_string()),
                last_updated: None,
                attributes: crate::HaStateAttributes {
                    friendly_name: Some("客厅主灯".to_string()),
                    source: Some("homekit".to_string()),
                    ..crate::HaStateAttributes::default()
                },
            },
            HaState {
                entity_id: "light.living_main".to_string(),
                state: Some("off".to_string()),
                last_changed: Some("2026-04-15T00:00:00Z".to_string()),
                last_updated: None,
                attributes: crate::HaStateAttributes {
                    friendly_name: Some("客厅主灯".to_string()),
                    source: Some("homekit".to_string()),
                    ..crate::HaStateAttributes::default()
                },
            },
        ];

        refine_live_context_report_with_typed_states(&mut report, &states);

        assert!(report.findings.possible_replacements.is_empty());
    }
}
