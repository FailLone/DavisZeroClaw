mod advisor;
mod app_config;
mod audit;
mod browser;
pub mod cli;
mod constants;
mod control;
mod entity;
mod express;
mod ha_client;
mod ha_mcp;
mod model_routing;
mod models;
mod runtime_paths;
mod server;
mod support;

pub use advisor::{
    build_replacement_candidates_report, generate_config_report, generate_config_report_with_states,
};
pub use app_config::{
    BrowserBridgeConfig, BrowserProfileConfig, BrowserUserSessionConfig, BrowserWritePolicyConfig,
    HomeAssistantConfig, ImessageConfig, LocalConfig, MetricWeights, ModelProviderConfig,
    ProfileMinimums, RoutingConfig, RoutingProfileConfig, RoutingProfilesConfig, WebhookConfig,
};
pub use audit::{audit_entity, parse_window};
pub use browser::{
    browser_action, browser_evaluate, browser_focus, browser_open, browser_profiles,
    browser_screenshot, browser_snapshot, browser_status, browser_tabs, browser_wait,
};
pub use constants::{
    CONTROL_FAILURE_THRESHOLD, CONTROL_FAILURE_WINDOW_HOURS, DEFAULT_WINDOW_MINUTES, USER_AGENT,
};
pub use control::{
    build_failure_summary, build_failure_summary_payload, execute_control, load_control_config,
    load_failure_state, maybe_consume_advisor_suggestion, prune_failure_state,
    record_control_failure, resolve_control_target, resolve_control_target_with_states,
    save_failure_state,
};
pub use entity::{related_entity_ids, resolve_entity_basic, resolve_entity_payload};
pub use express::{express_auth_status, express_packages};
pub use ha_client::{
    derive_ha_origin, fetch_all_states, fetch_all_states_typed, HaClient, ProxyError,
};
pub use ha_mcp::{
    derive_ha_mcp_endpoint, refine_live_context_report_with_typed_states, HaMcpCapabilities,
    HaMcpClient, HaMcpLiveContextReport, HaMcpPrompt, HaMcpTool,
};
pub use model_routing::{
    check_local_config, zeroclaw_env_vars, ModelCostObservation, ModelRoutePlan,
    ModelRoutingManager, ModelScoreEntry, PlannedModel, PlannedProfileRoute,
    ProfileRuntimeObservation, RoutingProfile, RoutingStatus, RuntimeObservations,
    RuntimeScoreSignals,
};
pub use models::{
    AdvancedOpportunity, AdvisorSuggestion, AssistEntitySuggestion, AuditActor,
    AuditConfigIssueResult, AuditCounts, AuditEntityRow, AuditEvidenceResult, AuditFindings,
    AuditNoEvidenceResult, AuditSource, AuditSourceObservation, AuditTimelineEntry,
    BrowserActionPreview, BrowserActionRequest, BrowserActionResponse, BrowserEvaluateRequest,
    BrowserFocusRequest, BrowserOpenRequest, BrowserProfileState, BrowserProfilesResponse,
    BrowserScreenshotRequest, BrowserSnapshotRequest, BrowserStatusResponse, BrowserTab,
    BrowserTabsResponse, BrowserTarget, BrowserWaitRequest, Candidate, ConfigMigrationSuggestion,
    ConfigReport, ConfigReportCounts, ConfigReportFindings, ConfigReportSuggestions, ControlAction,
    ControlConfig, ControlResolution, ControlTargetState, CrossDomainConflictFinding,
    CustomSentenceSuggestion, DuplicateFriendlyNameFinding, EntityAliasSuggestion,
    EntityBasicResolution, ExecuteControlRequest, ExecuteControlResponse, ExecutionError,
    ExpressAuthStatusResponse, ExpressPackage, ExpressPackagesResponse, ExpressSourceSnapshot,
    ExpressSourceStatus, FailureDetails, FailureEvent, FailureReason, FailureState, FailureSummary,
    GroupConfig, GroupSuggestion, HaState, HaStateAttributes, Issue, MissingRoomSemanticFinding,
    ReplacementCandidateReview, ReplacementCandidatesReport, ResolveEntityPayload,
    ServiceExecution, TopFailedQuery,
};
pub use runtime_paths::RuntimePaths;
pub use server::{build_app, AppState};
pub use support::{build_issue, isoformat, normalize_text};

pub(crate) use constants::{CONTROL_DOMAINS, ROOM_LIGHT_KEYWORDS};
pub(crate) use support::{entity_domain, now_utc, parse_time};

#[cfg(test)]
#[path = "../tests/rust/mod.rs"]
mod tests;
