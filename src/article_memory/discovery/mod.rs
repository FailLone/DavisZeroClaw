//! Topic-driven URL discovery. Feeds survivors into the existing IngestQueue.
//! See docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md Phase 1–2.

pub mod feed_ingestor;
pub mod search;
pub mod worker;

pub use worker::{DiscoveryWorker, DiscoveryWorkerDeps};
