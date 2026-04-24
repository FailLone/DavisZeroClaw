mod host_profile;
mod queue;
mod types;
mod worker;

// Consumed starting Task 11 (local_proxy boot); remove allow once consumers land.
#[allow(unused_imports)]
pub use host_profile::{
    normalize_url, resolve_profile, validate_url_for_ingest, NormalizeUrlError, ResolvedProfile,
    UrlValidationError,
};
// Consumed starting Task 10 (server routes) / Task 11 (local_proxy boot).
#[allow(unused_imports)]
pub use queue::{IngestQueue, IngestQueueState};
// Consumed starting Task 10 (server routes) / Task 11 (local_proxy boot).
#[allow(unused_imports)]
pub use types::{
    IngestJob, IngestJobError, IngestJobStatus, IngestOutcome, IngestOutcomeSummary, IngestRequest,
    IngestResponse, IngestSubmitError, ListFilter,
};
// Consumed starting Task 11 (local_proxy boot).
#[allow(unused_imports)]
pub use worker::{IngestWorkerDeps, IngestWorkerPool};
