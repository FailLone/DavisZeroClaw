//! End-to-end smoke test: Davis sink → real MemPalace MCP server.
//!
//! This test is `#[ignore]` by default because it needs a working MemPalace
//! venv. Run it against a real install with:
//!
//! ```bash
//! DAVIS_MEMPALACE_VENV=/path/to/.runtime/davis/mempalace-venv \
//!   cargo test --lib -- --ignored smoke
//! ```
//!
//! The venv must have `mempalace` importable with the MCP server module
//! (`python -m mempalace.mcp_server` must work). The smoke test writes a
//! marker drawer and verifies `sent >= 1`.

use crate::mempalace_sink::MemPalaceSink;
use crate::runtime_paths::RuntimePaths;
use std::path::PathBuf;
use std::time::Duration;

fn runtime_paths_from_env() -> Option<RuntimePaths> {
    let venv = std::env::var_os("DAVIS_MEMPALACE_VENV")?;
    let venv_dir = PathBuf::from(venv);
    // Parent of the venv is assumed to be the runtime dir ({runtime}/mempalace-venv).
    let runtime_dir = venv_dir.parent()?.to_path_buf();
    let repo_root = std::env::current_dir().ok()?;
    Some(RuntimePaths {
        repo_root,
        runtime_dir,
    })
}

#[tokio::test]
#[ignore]
async fn smoke_writes_a_marker_drawer_against_real_mempalace() {
    let Some(paths) = runtime_paths_from_env() else {
        eprintln!("DAVIS_MEMPALACE_VENV not set; skipping");
        return;
    };
    let sink = MemPalaceSink::spawn(&paths);
    let marker = format!(
        "davis smoke test @ {}",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
    );
    sink.add_drawer("davis:test", "smoke", &marker);

    // First MCP connect can take 1-2s (embedding model, Chroma init); poll.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let m = sink.metrics().await;
        if m.sent >= 1 {
            assert_eq!(m.failed, 0, "unexpected failure: {m:?}");
            assert_eq!(m.dropped, 0, "unexpected drop: {m:?}");
            println!("smoke test OK: {m:?}");
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("smoke test: sink never recorded sent event, last metrics: {m:?}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
