//! Tracing setup for daemon binaries.
//!
//! Only `davis-local-proxy` and `davis-ha-proxy` (running in server mode) call
//! `init_tracing`. CLI subcommands (`check-config`, `print-zeroclaw-env`,
//! `check-ha`, and the whole `daviszeroclaw` tool) must keep stdout/stderr
//! clean for human consumption and therefore leave tracing uninitialized —
//! `tracing::info!`/`warn!` calls become no-ops there, which is fine for
//! daemon-focused instrumentation.
//!
//! Format: pretty text to stderr (matches the launchd stderr log capture).
//! Filter: `RUST_LOG` env var; defaults to `info,davis_zero_claw=debug` so
//! Davis's own modules are verbose without pulling in axum/hyper internals.

use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

const DEFAULT_FILTER: &str = "info,davis_zero_claw=debug";

/// Initialize the global tracing subscriber. Safe to call more than once —
/// subsequent calls return without reinstalling. Call exactly at daemon
/// startup, not from library code.
pub fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    let layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_ansi(false)
        .with_level(true);

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();
}
