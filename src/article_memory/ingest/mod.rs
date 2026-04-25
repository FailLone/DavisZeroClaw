mod content_signals;
mod engines;
mod host_profile;
mod quality_gate;
mod queue;
pub(super) mod reply_text;
mod types;
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
// Consumed by T6 (quality gate) and Phase 2 scoring; currently only used in
// this module's unit tests.
#[allow(unused_imports)]
pub use content_signals::{compute_signals, ContentSignals};
#[allow(unused_imports)]
pub use engines::{next_engine, pick_engine, EngineChoice, ExtractEngineConfig};
#[allow(unused_imports)]
pub use quality_gate::{assess as assess_quality, GateResult, QualityGateConfig};
