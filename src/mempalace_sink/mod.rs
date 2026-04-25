//! Davis → MemPalace fire-and-forget projection sink.
//!
//! See `CLAUDE.md` §MemPalace integration plan and
//! `docs/superpowers/plans/2026-04-25-mempalace-integration.md` for the
//! phased design and task list.

mod driver;
mod emitter;
mod mcp_stdio;
mod predicate;
mod worker_health;

pub(crate) use driver::MemPalaceSink;
#[cfg(test)]
pub(crate) use driver::SinkMetrics;
// `MempalaceEmitter` is exposed publicly so integration tests outside the
// crate can satisfy `TranslateWorkerDeps::mempalace_sink` (an
// `Arc<dyn MempalaceEmitter>`). All production call sites still go through
// the re-export path above — nothing new leaks into Davis's runtime surface.
pub use emitter::MempalaceEmitter;
#[cfg(test)]
pub(crate) use emitter::SpySink;
#[cfg(test)]
pub(crate) use mcp_stdio::{InitializeParams, McpStdioClient};
pub(crate) use predicate::{Predicate, TripleId};
pub(crate) use worker_health::{SampleDebouncer, TimeDebouncer};

/// Test-only helpers exposed on the public surface so integration tests
/// (`tests/rust/topic_crawl_translate.rs`) can hand a benign sink to
/// `TranslateWorkerDeps` without dragging in the real MCP child process.
///
/// `NoopSink` is intentionally trivial: all four projection methods are
/// no-ops. Production code uses `MemPalaceSink` instead; this type only
/// exists so external tests can satisfy the trait object.
pub mod testing {
    use super::emitter::MempalaceEmitter;
    use super::predicate::{Predicate, TripleId};

    #[derive(Debug, Default)]
    pub struct NoopSink;

    impl MempalaceEmitter for NoopSink {
        fn add_drawer(&self, _wing: &str, _room: &str, _content: &str) {}
        fn kg_add(&self, _subject: TripleId, _predicate: Predicate, _object: TripleId) {}
        fn kg_invalidate(&self, _subject: TripleId, _predicate: Predicate, _object: TripleId) {}
        fn diary_write(&self, _wing: &str, _entry: &str) {}
    }
}
