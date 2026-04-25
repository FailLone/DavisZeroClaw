//! End-to-end smoke test: Davis sink → real MemPalace MCP server.
//!
//! `#[ignore]` by default because it needs a working MemPalace venv. Run with:
//!
//! ```bash
//! DAVIS_MEMPALACE_VENV=/path/to/.runtime/davis/mempalace-venv \
//!   cargo test --lib -- --ignored smoke --nocapture
//! ```
//!
//! The venv must have `mempalace` importable and `python -m mempalace.mcp_server`
//! must launch cleanly.
//!
//! What the smoke test covers:
//! - All four driver tool mappings actually work (`mempalace_add_drawer`,
//!   `mempalace_kg_add`, `mempalace_kg_invalidate`, `mempalace_diary_write`).
//! - `success=false` business errors propagate into `failed` + `last_error`
//!   (regression test for the Phase 1 driver dispatch bug).
//! - A drawer written via the sink is retrievable via `mempalace_search`, so we
//!   know the data actually landed in the palace, not just that the JSON-RPC
//!   hop succeeded.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::mempalace_sink::{MemPalaceSink, Predicate, TripleId};
use crate::runtime_paths::RuntimePaths;

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

async fn wait_for_metric<F: Fn(&crate::mempalace_sink::SinkMetrics) -> bool>(
    sink: &MemPalaceSink,
    predicate: F,
    label: &str,
    timeout: Duration,
) -> crate::mempalace_sink::SinkMetrics {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let m = sink.metrics().await;
        if predicate(&m) {
            return m;
        }
        if std::time::Instant::now() > deadline {
            panic!("smoke: timed out waiting for {label}: {m:?}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Single smoke test exercising all four tool mappings. We intentionally do
/// NOT split this into multiple `#[tokio::test]` functions: tokio runs tests
/// in parallel and two sinks pointed at the same MemPalace palace dir will
/// race on Chroma's sqlite lock, producing flaky `stdio reader closed` errors.
#[tokio::test]
#[ignore]
async fn smoke_exercises_all_four_tool_mappings_and_verifies_drawer_in_palace() {
    let Some(paths) = runtime_paths_from_env() else {
        eprintln!("DAVIS_MEMPALACE_VENV not set; skipping");
        return;
    };
    let sink = MemPalaceSink::spawn(&paths);

    // Unique marker so `mempalace_search` finds this run's drawer specifically.
    // Keep it pure ASCII — MemPalace serializes search results with
    // `ensure_ascii=True`, so non-ASCII chars come back as `\uXXXX` escapes
    // which defeat a naive `contains` check.
    let tag = format!("davisSmokeTag{}", chrono::Utc::now().timestamp());
    let marker = format!("davis smoke {tag} cross-tool mapping check");

    sink.add_drawer("davis.test", "smoke", &marker);
    sink.diary_write("davis.agent.smoke", &format!("smoke diary {tag}"));
    sink.kg_add(
        TripleId::entity(&format!("smoke.entity.{tag}")),
        Predicate::EntityHasState,
        TripleId::entity("smoke.state.on"),
    );
    sink.kg_invalidate(
        TripleId::entity(&format!("smoke.entity.{tag}")),
        Predicate::EntityHasState,
        TripleId::entity("smoke.state.on"),
    );

    let m = wait_for_metric(
        &sink,
        |m| m.sent + m.failed >= 4,
        "4 tool calls",
        Duration::from_secs(45),
    )
    .await;
    assert_eq!(
        m.sent, 4,
        "expected all four tools to succeed: {m:?}. If one is failed, \
         inspect last_error — it likely flags a tool-name or schema drift.",
    );
    assert_eq!(m.failed, 0, "unexpected failures: {m:?}");

    // Now verify the drawer actually materialised in the palace by spinning up
    // a second short-lived MCP client and calling `mempalace_search`.
    verify_drawer_searchable(&paths, &tag, &marker).await;
    println!("cross-tool smoke OK: {m:?}");
}

async fn verify_drawer_searchable(paths: &RuntimePaths, tag: &str, marker: &str) {
    use crate::mempalace_sink::McpStdioClient;
    let (program, args) = paths.mempalace_mcp_server_cmd();
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&args);
    let client = McpStdioClient::spawn(cmd)
        .await
        .expect("spawn second MCP client for verification");
    client
        .initialize(&crate::mempalace_sink::InitializeParams {
            client_name: "davis-smoke-verifier".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
        })
        .await
        .expect("initialize second MCP client");

    // MemPalace needs a brief moment for Chroma to persist + reopen for reads.
    // We retry a few times before giving up.
    let mut last: Option<Value> = None;
    for _ in 0..5 {
        let value = client
            .call_tool(
                "mempalace_search",
                json!({"query": tag, "wing": "davis.test", "limit": 3}),
            )
            .await
            .expect("search call");
        last = Some(value.clone());
        if value
            .get("content")
            .and_then(Value::as_array)
            .and_then(|arr| arr.iter().find_map(|i| i.get("text")))
            .and_then(Value::as_str)
            .is_some_and(|t| t.contains(marker))
        {
            client.shutdown().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    client.shutdown().await;
    panic!(
        "smoke: drawer for tag {tag} not found via search; last response: {}",
        last.map(|v| v.to_string()).unwrap_or_default(),
    );
}
