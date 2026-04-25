mod cleaning_fix;
mod content_signals;
mod engines;
mod host_profile;
mod llm_extract;
mod quality_gate;
mod queue;
pub(super) mod reply_text;
pub(super) mod report_context;
mod rule_types;
mod types;
mod value_signals;
mod worker;

pub use host_profile::{
    normalize_url, resolve_profile, validate_url_for_ingest, NormalizeUrlError, ResolvedProfile,
    UrlValidationError,
};
pub use queue::{IngestQueue, IngestQueueState, PersistHealth};
pub use types::{
    IngestJob, IngestJobError, IngestJobStatus, IngestOutcome, IngestOutcomeSummary, IngestRequest,
    IngestResponse, IngestSubmitError, ListFilter,
};
pub use worker::{IngestWorkerDeps, IngestWorkerPool};
// Consumed by T3 (cleaning_internals::normalize_article_text); the two
// structure-preserving helpers are now on the hot path for every normalized
// article. `normalize_markdown_preserving_structure` is held for T6 / Phase 2,
// hence the targeted `unused_imports` allow.
#[allow(unused_imports)]
pub use cleaning_fix::{
    normalize_line_preserving, normalize_markdown_preserving_structure, SlidingDedup,
};
#[allow(unused_imports)]
pub use content_signals::{compute_signals, ContentSignals};
#[allow(unused_imports)]
pub use engines::{next_engine, pick_engine, EngineChoice, ExtractEngineConfig};
#[allow(unused_imports)]
pub use llm_extract::llm_html_to_markdown;
#[allow(unused_imports)]
pub use quality_gate::{assess as assess_quality, GateResult, QualityGateConfig};
#[allow(unused_imports)]
pub use rule_types::{LearnedRule, RuleSample, RuleStats};
#[allow(unused_imports)]
pub use value_signals::{deterministic_score, gopher_reject};
