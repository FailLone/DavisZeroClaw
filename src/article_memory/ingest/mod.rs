mod host_profile;
mod types;

// Consumed starting Task 6 (queue.rs); remove allow once consumers land.
#[allow(unused_imports)]
pub use host_profile::{
    normalize_url, resolve_profile, validate_url_for_ingest, NormalizeUrlError, ResolvedProfile,
    UrlValidationError,
};
// Consumed starting Task 6 (queue.rs); remove allow once consumers land.
#[allow(unused_imports)]
pub use types::{
    IngestJob, IngestJobError, IngestJobStatus, IngestOutcome, IngestOutcomeSummary, IngestRequest,
    IngestResponse, IngestSubmitError, ListFilter,
};
