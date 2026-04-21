use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum FailureReason {
    MissingAction,
    MissingCredentials,
    HaAuthFailed,
    HaUnreachable,
    ResolutionAmbiguous,
    #[default]
    ResolutionFailed,
    ResolutionNotFound,
    GroupMembersMissing,
    ExecutionFailed,
    BadRequest,
}

impl FailureReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MissingAction => "missing_action",
            Self::MissingCredentials => "missing_credentials",
            Self::HaAuthFailed => "ha_auth_failed",
            Self::HaUnreachable => "ha_unreachable",
            Self::ResolutionAmbiguous => "resolution_ambiguous",
            Self::ResolutionFailed => "resolution_failed",
            Self::ResolutionNotFound => "resolution_not_found",
            Self::GroupMembersMissing => "group_members_missing",
            Self::ExecutionFailed => "execution_failed",
            Self::BadRequest => "bad_request",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    #[default]
    Unknown,
    TurnOn,
    TurnOff,
    Toggle,
    SetBrightness,
    QueryState,
}

impl ControlAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::TurnOn => "turn_on",
            Self::TurnOff => "turn_off",
            Self::Toggle => "toggle",
            Self::SetBrightness => "set_brightness",
            Self::QueryState => "query_state",
        }
    }

    pub fn from_query(value: &str) -> Self {
        match value.trim() {
            "turn_on" => Self::TurnOn,
            "turn_off" => Self::TurnOff,
            "toggle" => Self::Toggle,
            "set_brightness" => Self::SetBrightness,
            "query_state" => Self::QueryState,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Issue {
    pub issue_type: String,
    pub issue_category: String,
    pub query_entity: String,
    pub recommended_actions: Vec<String>,
    pub missing_requirements: Vec<String>,
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroupConfig {
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlConfig {
    #[serde(default)]
    pub entity_aliases: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub area_aliases: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub groups: BTreeMap<String, GroupConfig>,
    #[serde(default)]
    pub domain_preferences: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub room_tokens: Vec<String>,
    #[serde(default)]
    pub ignored_entities: Vec<String>,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            entity_aliases: BTreeMap::new(),
            area_aliases: BTreeMap::new(),
            groups: BTreeMap::new(),
            domain_preferences: BTreeMap::from([
                (
                    "light".into(),
                    vec![
                        "灯", "灯带", "灯光", "吊灯", "主灯", "射灯", "柜灯", "筒灯", "夜灯",
                    ]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                ),
                (
                    "switch".into(),
                    vec!["开关", "插座", "按钮"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                ),
                (
                    "cover".into(),
                    vec!["窗帘", "帘子"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                ),
                (
                    "climate".into(),
                    vec!["空调", "暖气", "地暖"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                ),
                (
                    "fan".into(),
                    vec!["风扇", "新风"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                ),
                (
                    "scene".into(),
                    vec!["场景", "模式"]
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                ),
                (
                    "script".into(),
                    vec!["脚本"].into_iter().map(str::to_string).collect(),
                ),
                (
                    "lock".into(),
                    vec!["门锁", "锁"].into_iter().map(str::to_string).collect(),
                ),
            ]),
            room_tokens: vec![
                "书房",
                "主卧",
                "次卧",
                "父母间",
                "客厅",
                "餐厅",
                "厨房",
                "阳台",
                "卫生间",
                "主卫",
                "公卫",
                "浴室",
                "玄关",
                "走廊",
                "过道",
                "儿童房",
                "衣帽间",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            ignored_entities: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FailureEvent {
    pub time: String,
    pub query_entity: String,
    pub action: String,
    pub reason: FailureReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<FailureDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FailureState {
    #[serde(default)]
    pub events: Vec<FailureEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_suggested_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopFailedQuery {
    pub query_entity: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureSummary {
    pub status: String,
    pub window_hours: i64,
    pub threshold: usize,
    pub failure_count: usize,
    pub counts_by_reason: BTreeMap<FailureReason, usize>,
    pub top_failed_queries: Vec<TopFailedQuery>,
    pub events: Vec<FailureEvent>,
    pub suggestion_due: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_suggested_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Candidate {
    pub entity_id: String,
    pub friendly_name: String,
    pub domain: String,
    pub score: i64,
    pub matched_by: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ControlResolution {
    pub status: String,
    pub query_entity: String,
    pub action: ControlAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<FailureReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_type: Option<String>,
    #[serde(default)]
    pub resolved_targets: Vec<String>,
    #[serde(default)]
    pub missing_targets: Vec<String>,
    #[serde(default)]
    pub friendly_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_guess_used: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub second_best_gap: Option<i64>,
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ControlTargetState {
    pub entity_id: String,
    pub friendly_name: String,
    pub domain: Option<String>,
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness_pct: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaStateAttributes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_u16"
    )]
    pub brightness: Option<u16>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_u16"
    )]
    pub brightness_pct: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HaState {
    pub entity_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_changed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
    #[serde(default)]
    pub attributes: HaStateAttributes,
}

impl HaState {
    pub fn friendly_name(&self) -> String {
        self.attributes.friendly_name.clone().unwrap_or_default()
    }

    pub fn current_state(&self) -> Option<String> {
        self.state.clone()
    }

    pub fn domain(&self) -> String {
        self.entity_id
            .split_once('.')
            .map(|pair| pair.0.to_string())
            .unwrap_or_default()
    }

    pub fn suffix(&self) -> String {
        self.entity_id
            .split_once('.')
            .map(|pair| pair.1.to_string())
            .unwrap_or_else(|| self.entity_id.clone())
    }

    pub fn brightness_pct(&self) -> Option<u8> {
        if let Some(percent) = self.attributes.brightness_pct {
            return Some(percent.min(100) as u8);
        }
        self.attributes.brightness.map(|raw| {
            let bounded = raw.min(255) as f32;
            ((bounded / 255.0) * 100.0).round() as u8
        })
    }
}

fn deserialize_optional_u16<'de, D>(deserializer: D) -> Result<Option<u16>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    let Some(value) = value else {
        return Ok(None);
    };

    let parsed = match value {
        Value::Number(number) => number.as_u64().map(|value| value as u16).or_else(|| {
            number
                .as_f64()
                .map(|value| value.round().clamp(0.0, u16::MAX as f64) as u16)
        }),
        Value::String(raw) => raw
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| value.round().clamp(0.0, u16::MAX as f64) as u16),
        _ => None,
    };

    Ok(parsed)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EntityBasicResolution {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<HaState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_by: Option<String>,
    #[serde(default)]
    pub suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResolveEntityPayload {
    pub status: String,
    pub query_entity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_state: Option<String>,
    #[serde(default)]
    pub related_entity_ids: Vec<String>,
    #[serde(default)]
    pub suggestions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<Issue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceExecution {
    pub domain: String,
    pub service: String,
    pub entity_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecutionError {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default)]
    pub entity_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FailureDetails {
    Resolution { resolution: Box<ControlResolution> },
    ExecutionErrors { errors: Vec<ExecutionError> },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AdvisorSuggestion {
    pub skill: String,
    pub message: String,
    pub reason: String,
    pub failure_count: usize,
    pub window_hours: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecuteControlRequest {
    #[serde(default)]
    pub raw_text: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub query_entity: String,
    #[serde(default)]
    pub action: ControlAction,
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub service_data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExecuteControlResponse {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<FailureReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<FailureReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<Issue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<ControlAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ControlResolution>,
    #[serde(default)]
    pub executed_services: Vec<ServiceExecution>,
    #[serde(default)]
    pub errors: Vec<ExecutionError>,
    #[serde(default)]
    pub targets: Vec<ControlTargetState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisor_suggestion: Option<AdvisorSuggestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateFriendlyNameFinding {
    pub friendly_name: String,
    pub entities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossDomainConflictFinding {
    pub friendly_name: String,
    pub entities: Vec<String>,
    pub domains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingRoomSemanticFinding {
    pub entity_id: String,
    pub friendly_name: String,
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityAliasSuggestion {
    pub entity_id: String,
    pub friendly_name: String,
    pub recommended_aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSuggestion {
    pub group_name: String,
    pub entities: Vec<String>,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistEntitySuggestion {
    pub entity_id: String,
    pub friendly_name: String,
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomSentenceSuggestion {
    pub group_name: String,
    pub sentences: Vec<String>,
    pub config_room_tokens: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigMigrationSuggestion {
    #[serde(rename = "type")]
    pub suggestion_type: String,
    pub reason: String,
    pub target: String,
    #[serde(default)]
    pub current: Vec<String>,
    #[serde(default)]
    pub recommended: Vec<String>,
    pub snippet: String,
    pub requires_confirmation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedOpportunity {
    #[serde(rename = "type")]
    pub opportunity_type: String,
    pub reason: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigReportCounts {
    pub controllable_entities: usize,
    pub duplicate_friendly_names: usize,
    pub cross_domain_conflicts: usize,
    pub missing_room_semantic: usize,
    pub recent_failures: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigReportFindings {
    pub duplicate_friendly_names: Vec<DuplicateFriendlyNameFinding>,
    pub cross_domain_conflicts: Vec<CrossDomainConflictFinding>,
    pub missing_room_semantic: Vec<MissingRoomSemanticFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigReportSuggestions {
    pub entity_aliases: Vec<EntityAliasSuggestion>,
    pub groups: Vec<GroupSuggestion>,
    pub assist_entities: Vec<AssistEntitySuggestion>,
    pub custom_sentences: Vec<CustomSentenceSuggestion>,
    #[serde(default)]
    pub migration_suggestions: Vec<ConfigMigrationSuggestion>,
    #[serde(default)]
    pub replacement_candidates: Vec<ReplacementCandidateReview>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigReport {
    pub status: String,
    pub generated_at: String,
    pub counts: ConfigReportCounts,
    pub recent_failures: FailureSummary,
    pub findings: ConfigReportFindings,
    pub suggestions: ConfigReportSuggestions,
    pub advanced_opportunities: Vec<AdvancedOpportunity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ha_mcp_live_context: Option<crate::HaMcpLiveContextReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplacementCandidatesReport {
    pub status: String,
    pub generated_at: String,
    pub candidate_count: usize,
    pub high_confidence_count: usize,
    pub needs_review_count: usize,
    pub candidates: Vec<ReplacementCandidateReview>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplacementCandidateReview {
    pub unavailable_name: String,
    pub replacement_name: String,
    pub domain: String,
    pub score: i32,
    pub confidence: String,
    #[serde(default)]
    pub reasons: Vec<String>,
    #[serde(default)]
    pub unavailable_areas: Vec<String>,
    #[serde(default)]
    pub replacement_areas: Vec<String>,
    #[serde(default)]
    pub suggested_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditActor {
    #[serde(rename = "type")]
    pub actor_type: String,
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditSourceObservation {
    #[serde(rename = "type")]
    pub observation_type: String,
    pub integration: String,
    pub time: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub id: Option<String>,
    pub observations: Vec<AuditSourceObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditTimelineEntry {
    pub time: Option<String>,
    pub entity_id: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friendly_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntityRow {
    pub entity_id: String,
    pub friendly_name: String,
    pub current_state: Option<String>,
    pub history_count: usize,
    pub logbook_count: usize,
    pub timeline: Vec<AuditTimelineEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditFindings {
    pub primary_transition_count: usize,
    pub actor_identified: bool,
    pub upstream_source_identified: bool,
    pub integration_observation_count: usize,
    pub related_entity_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditCounts {
    pub entities: usize,
    pub history: usize,
    pub logbook: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfigIssueResult {
    pub result_type: String,
    pub issue: Issue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditNoEvidenceResult {
    pub result_type: String,
    pub query_entity: String,
    pub resolved_entity_id: String,
    pub related_entity_ids: Vec<String>,
    pub window_start: String,
    pub window_end: String,
    pub current_state: Option<String>,
    pub queried_sources: Vec<String>,
    pub missing_evidence_types: Vec<String>,
    pub possible_reasons: Vec<String>,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvidenceResult {
    pub result_type: String,
    pub query_entity: String,
    pub resolved_entity_id: String,
    pub matched_by: Option<String>,
    pub related_entity_ids: Vec<String>,
    pub window_start: String,
    pub window_end: String,
    pub current_state: Option<String>,
    pub actor: AuditActor,
    pub source: AuditSource,
    pub confidence: String,
    pub findings: AuditFindings,
    pub counts: AuditCounts,
    pub entities: Vec<AuditEntityRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ExpressPackage {
    pub id: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merchant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shop_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_update: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carrier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_no_masked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pickup_code_masked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail_url: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub raw_source_meta: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ExpressSourceStatus {
    pub source: String,
    pub status: String,
    pub checked_at: String,
    pub logged_in: bool,
    #[serde(default)]
    pub package_count: usize,
    #[serde(default)]
    pub cached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<Issue>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ExpressSourceSnapshot {
    #[serde(flatten)]
    pub source_status: ExpressSourceStatus,
    #[serde(default)]
    pub packages: Vec<ExpressPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ExpressAuthStatusResponse {
    pub status: String,
    pub checked_at: String,
    #[serde(default)]
    pub sources: Vec<ExpressSourceStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ExpressPackagesResponse {
    pub status: String,
    pub checked_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default)]
    pub refreshed: bool,
    #[serde(default)]
    pub package_count: usize,
    #[serde(default)]
    pub packages: Vec<ExpressPackage>,
    #[serde(default)]
    pub sources: Vec<ExpressSourceStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speech: Option<String>,
}
