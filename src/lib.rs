mod advisor;
mod app_config;
mod article_memory;
mod audit;
pub mod cli;
mod constants;
mod control;
mod crawl4ai;
pub mod crawl4ai_error;
pub mod crawl4ai_supervisor;
mod crawl_sources;
mod entity;
mod express;
mod ha_client;
mod ha_mcp;
mod local_proxy;
mod model_routing;
mod models;
mod observability;
mod runtime_paths;
mod server;
mod support;

pub use advisor::{
    build_replacement_candidates_report, generate_config_report, generate_config_report_with_states,
};
pub use app_config::{
    ArticleMemoryConfig, ArticleMemoryEmbeddingConfig, ArticleMemoryNormalizeConfig,
    Crawl4aiConfig, Crawl4aiTransport, HomeAssistantConfig, ImessageConfig, LocalConfig, McpConfig,
    McpServerConfig, McpTransport, ModelProviderConfig, RoutingConfig, RoutingProfileConfig,
    RoutingProfilesConfig, WebhookConfig,
};
pub use article_memory::{
    add_article_memory, article_cleaning_preferred_selectors, article_memory_status,
    build_article_strategy_review_input, check_article_cleaning, check_article_memory,
    hybrid_search_article_memory, init_article_memory, judge_all_article_value_memory,
    judge_article_value_memory, list_article_clean_reports, list_article_memory,
    list_article_value_reports, normalize_all_article_memory, normalize_article_memory,
    rebuild_article_memory_embeddings, replay_article_cleaning, resolve_article_embedding_config,
    resolve_article_normalize_config, resolve_article_value_config, search_article_memory,
    upsert_article_memory_embedding, ArticleCleanAuditResponse, ArticleCleanReport,
    ArticleCleaningCheckResponse, ArticleCleaningConfig, ArticleCleaningDefaults,
    ArticleCleaningReplayResponse, ArticleCleaningSiteStrategy, ArticleMemoryAddRequest,
    ArticleMemoryEmbeddingIndex, ArticleMemoryEmbeddingRebuildResponse,
    ArticleMemoryEmbeddingRecord, ArticleMemoryListResponse, ArticleMemoryNormalizeResponse,
    ArticleMemoryRecord, ArticleMemoryRecordStatus, ArticleMemorySearchHit,
    ArticleMemorySearchResponse, ArticleMemoryStatusResponse, ArticleStrategyReviewInputResponse,
    ArticleValueAuditResponse, ArticleValueConfig, ArticleValueReport,
    ResolvedArticleEmbeddingConfig, ResolvedArticleNormalizeConfig, ResolvedArticleValueConfig,
};
pub use audit::{audit_entity, parse_window};
pub use constants::{
    CONTROL_FAILURE_THRESHOLD, CONTROL_FAILURE_WINDOW_HOURS, DEFAULT_WINDOW_MINUTES, USER_AGENT,
};
pub use control::{
    build_failure_summary, build_failure_summary_payload, execute_control, load_control_config,
    load_failure_state, maybe_consume_advisor_suggestion, prune_failure_state,
    record_control_failure, resolve_control_target, resolve_control_target_with_states,
    save_failure_state,
};
pub use crawl4ai::{crawl4ai_crawl, Crawl4aiPageRequest, Crawl4aiPageResult};
pub use crawl4ai_error::Crawl4aiError;
pub use crawl4ai_supervisor::Crawl4aiSupervisor;
pub use crawl_sources::{
    builtin_crawl_sources, find_builtin_crawl_source, run_builtin_crawl_source,
    CrawlSourceDefinition,
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
pub use local_proxy::run_local_proxy;
pub use model_routing::{
    check_local_config, render_runtime_config, zeroclaw_env_vars, RoutingProfile,
};
pub use models::{
    AdvancedOpportunity, AdvisorSuggestion, AssistEntitySuggestion, AuditActor,
    AuditConfigIssueResult, AuditCounts, AuditEntityRow, AuditEvidenceResult, AuditFindings,
    AuditNoEvidenceResult, AuditSource, AuditSourceObservation, AuditTimelineEntry, Candidate,
    ConfigMigrationSuggestion, ConfigReport, ConfigReportCounts, ConfigReportFindings,
    ConfigReportSuggestions, ControlAction, ControlConfig, ControlResolution, ControlTargetState,
    CrossDomainConflictFinding, CustomSentenceSuggestion, DuplicateFriendlyNameFinding,
    EntityAliasSuggestion, EntityBasicResolution, ExecuteControlRequest, ExecuteControlResponse,
    ExecutionError, ExpressAuthStatusResponse, ExpressPackage, ExpressPackagesResponse,
    ExpressSourceSnapshot, ExpressSourceStatus, FailureDetails, FailureEvent, FailureReason,
    FailureState, FailureSummary, GroupConfig, GroupSuggestion, HaState, HaStateAttributes, Issue,
    MissingRoomSemanticFinding, ReplacementCandidateReview, ReplacementCandidatesReport,
    ResolveEntityPayload, ServiceExecution, TopFailedQuery,
};
pub use observability::init_tracing;
pub use runtime_paths::RuntimePaths;
pub use server::{build_app, build_shortcut_bridge_app, AppState, Crawl4aiProfileLocks};
pub use support::{build_issue, isoformat, normalize_text};

pub(crate) use constants::{CONTROL_DOMAINS, ROOM_LIGHT_KEYWORDS};
pub(crate) use support::{entity_domain, now_utc, parse_time};

#[cfg(test)]
#[path = "../tests/rust/mod.rs"]
mod tests;
