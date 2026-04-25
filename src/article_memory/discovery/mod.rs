//! Topic-driven URL discovery. Feeds survivors into the existing IngestQueue.
//! See docs/superpowers/plans/2026-04-25-topic-crawl-mvp.md Phase 1–2.

// Consumed starting Task 6 (worker.rs); remove allow once the worker lands.
#[allow(dead_code)]
pub mod feed_ingestor;
