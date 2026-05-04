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
pub(crate) mod ha_mcp_projection;
pub mod imessage_send;
mod local_proxy;
// Publicly reachable so integration tests can use `mempalace_sink::testing::NoopSink`
// to satisfy `TranslateWorkerDeps::mempalace_sink`. Only the items re-exported
// from `mempalace_sink::mod.rs` are visible — internal structs like
// `MemPalaceSink` stay `pub(crate)`.
pub mod mempalace_sink;
mod model_routing;
mod models;
mod observability;
mod runtime_paths;
mod server;
pub mod server_digest;
mod shortcut_reply;
mod support;

pub use advisor::{
    build_replacement_candidates_report, generate_config_report, generate_config_report_with_states,
};
pub use app_config::{
    ArticleMemoryConfig, ArticleMemoryEmbeddingConfig, ArticleMemoryExtractConfig,
    ArticleMemoryHostProfile, ArticleMemoryIngestConfig, ArticleMemoryNormalizeConfig,
    ArticleMemoryValueConfig, Crawl4aiConfig, HomeAssistantConfig, ImessageConfig, LocalConfig,
    McpConfig, McpServerConfig, McpTransport, ModelProviderConfig, OpenRouterLlmEngineConfig,
    QualityGateToml, RoutingConfig, RoutingProfileConfig, RoutingProfilesConfig,
    RuleLearningConfig, TranslateConfig, WebhookConfig,
};
pub use article_memory::discovery::{
    BraveSearch, DiscoveryWorker, DiscoveryWorkerDeps, SearchError, SearchHit, SearchProvider,
};
pub use article_memory::refresh::{RefreshWorker, RefreshWorkerDeps};
pub use article_memory::translate::{run_one_cycle, TranslateWorker, TranslateWorkerDeps};
pub use article_memory::{
    add_article_memory, add_article_memory_override, article_memory_status,
    build_article_strategy_review_input, check_article_cleaning, check_article_memory,
    find_article_by_normalized_url, hybrid_search_article_memory, init_article_memory,
    judge_all_article_value_memory, judge_article_value_memory, list_article_clean_reports,
    list_article_memory, list_article_value_reports, load_article_index,
    normalize_all_article_memory, normalize_article_memory, normalize_url,
    rebuild_article_memory_embeddings, replay_article_cleaning, resolve_article_embedding_config,
    resolve_article_normalize_config, resolve_article_value_config, resolve_profile,
    save_article_index, search_article_memory, upsert_article_memory_embedding,
    validate_url_for_ingest, ArticleCleanAuditResponse, ArticleCleanReport,
    ArticleCleaningCheckResponse, ArticleCleaningConfig, ArticleCleaningDefaults,
    ArticleCleaningReplayResponse, ArticleMemoryAddRequest, ArticleMemoryEmbeddingIndex,
    ArticleMemoryEmbeddingRebuildResponse, ArticleMemoryEmbeddingRecord, ArticleMemoryIndex,
    ArticleMemoryListResponse, ArticleMemoryNormalizeResponse, ArticleMemoryRecord,
    ArticleMemoryRecordStatus, ArticleMemorySearchHit, ArticleMemorySearchResponse,
    ArticleMemoryStatusResponse, ArticleStrategyReviewInputResponse, ArticleValueAuditResponse,
    ArticleValueConfig, ArticleValueReport, IngestJob, IngestJobError, IngestJobStatus,
    IngestOutcome, IngestOutcomeSummary, IngestQueue, IngestQueueState, IngestRequest,
    IngestResponse, IngestSubmitError, IngestWorkerDeps, IngestWorkerPool, ListFilter,
    NormalizeUrlError, PersistHealth, ResolvedArticleEmbeddingConfig,
    ResolvedArticleNormalizeConfig, ResolvedArticleValueConfig, ResolvedProfile,
    UrlValidationError,
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
