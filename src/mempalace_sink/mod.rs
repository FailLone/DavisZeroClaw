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
pub(crate) use emitter::MempalaceEmitter;
#[cfg(test)]
pub(crate) use emitter::SpySink;
#[cfg(test)]
pub(crate) use mcp_stdio::{InitializeParams, McpStdioClient};
pub(crate) use predicate::{Predicate, TripleId};
pub(crate) use worker_health::{SampleDebouncer, TimeDebouncer};
