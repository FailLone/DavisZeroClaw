//! Integration test: spawn a Python stub via the same mechanism
//! `PythonRouterChecker` uses, parse its stdout, and assert the resulting
//! `RouterCheckOutcome`.
//!
//! This validates the "spawn → stdout capture → last-line parse → outcome"
//! path end-to-end without depending on Playwright, Chromium, or a real
//! router. It does require `python3` on PATH (the same prerequisite as
//! `tests/rust/topic_crawl_*.rs`).

use davis_zero_claw::router_supervisor::{parse_outcome, RouterAction, RouterCheckOutcome};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

#[tokio::test]
async fn python_stub_emits_parseable_ok_none() {
    let python = which_python3().expect("python3 must be on PATH");
    let repo_root: PathBuf = env!("CARGO_MANIFEST_DIR").into();

    let mut cmd = Command::new(&python);
    cmd.arg("-m")
        .arg("tests.fixtures.router_stub")
        .env("PYTHONPATH", repo_root.display().to_string())
        .current_dir(&repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = cmd.spawn().expect("spawn stub");
    let output = timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .expect("stub should not timeout")
        .expect("stub run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let outcome = parse_outcome(&stdout, output.status.code(), &stderr);

    match outcome {
        RouterCheckOutcome::Ok {
            action,
            dhcp_was_enabled,
            duration_ms,
        } => {
            assert_eq!(action, RouterAction::None);
            assert!(!dhcp_was_enabled);
            assert_eq!(duration_ms, 42);
        }
        other => panic!("expected Ok, got {other:?}\nstdout: {stdout}\nstderr: {stderr}"),
    }
}

fn which_python3() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("python3");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
