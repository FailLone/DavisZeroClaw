//! Davis → MemPalace fire-and-forget projection sink.
//!
//! See `CLAUDE.md` §MemPalace integration plan and
//! `docs/superpowers/plans/2026-04-25-mempalace-integration.md` for the
//! phased design and task list.

mod driver;
mod mcp_stdio;
mod predicate;

pub(crate) use driver::MemPalaceSink;
#[cfg(test)]
pub(crate) use driver::SinkMetrics;
#[cfg(test)]
pub(crate) use mcp_stdio::{InitializeParams, McpStdioClient};
#[cfg(test)]
pub(crate) use predicate::{Predicate, TripleId};
