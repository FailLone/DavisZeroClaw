mod cleaning_fix;
mod content_signals;
mod engines;
mod host_profile;
mod learned_rules;
mod llm_extract;
mod quality_gate;
mod queue;
pub(super) mod reply_text;
pub(super) mod report_context;
mod rule_learning;
mod rule_learning_worker;
pub(crate) mod rule_mempalace_projection;
mod rule_samples;
mod rule_types;
mod types;
mod value_signals;
mod worker;

pub use cleaning_fix::{normalize_line_preserving, SlidingDedup};
#[allow(unused_imports)]
pub use content_signals::{compute_signals, ContentSignals};
#[allow(unused_imports)]
pub use engines::{pick_engine, EngineChoice, ExtractEngineConfig};
pub use host_profile::{
    normalize_url, resolve_profile, validate_url_for_ingest, NormalizeUrlError, ResolvedProfile,
    UrlValidationError,
};
#[allow(unused_imports)]
pub use learned_rules::{LearnedRuleStore, RuleStatsStore};
#[allow(unused_imports)]
pub use llm_extract::llm_html_to_markdown;
#[allow(unused_imports)]
pub use quality_gate::{assess as assess_quality, GateResult, QualityGateConfig};
pub use queue::{IngestQueue, IngestQueueState, PersistHealth};
#[allow(unused_imports)]
pub use rule_learning::{
    build_learn_prompt, parse_learn_response, simplify_dom, validate_rule, ValidationResult,
    LEARN_SYSTEM_PROMPT,
};
#[allow(unused_imports)]
pub use rule_learning_worker::{RuleLearningDeps, RuleLearningWorker};
#[allow(unused_imports)]
pub use rule_samples::SampleStore;
#[allow(unused_imports)]
pub use rule_types::{LearnedRule, RuleSample, RuleStats};
pub use types::{
    IngestJob, IngestJobError, IngestJobStatus, IngestOutcome, IngestOutcomeSummary, IngestRequest,
    IngestResponse, IngestSubmitError, ListFilter,
};
#[allow(unused_imports)]
pub use value_signals::{deterministic_score, gopher_reject};
pub use worker::{IngestWorkerDeps, IngestWorkerPool};
