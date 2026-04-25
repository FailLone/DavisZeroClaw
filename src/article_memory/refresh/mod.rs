//! Evergreen refresh — re-judges articles whose updated_at > stale_after_days.
//! Does NOT re-crawl (content-drift refresh deferred to Phase 6+).

pub mod worker;

pub use worker::{RefreshWorker, RefreshWorkerDeps};
