mod host_profile;
// Consumed starting Task 4; remove allow once consumers land.
#[allow(unused_imports)]
pub use host_profile::{
    normalize_url, resolve_profile, validate_url_for_ingest, NormalizeUrlError, ResolvedProfile,
    UrlValidationError,
};
