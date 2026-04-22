use crate::{Crawl4aiConfig, Crawl4aiPageRequest, Crawl4aiTransport, RuntimePaths};

fn fake_paths(tmp: &std::path::Path) -> RuntimePaths {
    RuntimePaths {
        repo_root: tmp.to_path_buf(),
        runtime_dir: tmp.join(".runtime").join("davis"),
    }
}

// Multi-thread flavor so timeout() + child reaper run concurrently; single-threaded can deadlock waiting for subprocess output while the timeout task sleeps.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn python_transport_honors_wall_clock_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = fake_paths(tmp.path());
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();

    // Point config.python at a script that sleeps forever.
    let sleeper = tmp.path().join("sleeper.sh");
    std::fs::write(&sleeper, "#!/bin/sh\nsleep 3600\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&sleeper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let config = Crawl4aiConfig {
        enabled: true,
        transport: Crawl4aiTransport::Python,
        python: sleeper.display().to_string(),
        timeout_secs: 1,
        ..Crawl4aiConfig::default()
    };

    // Call the internal helper directly with a 2s guard (total budget = 3s)
    // so `cargo test` doesn't pay the full 30s production guard on every run.
    let start = std::time::Instant::now();
    let result = crate::crawl4ai::crawl_via_python_with_guard(
        &paths,
        &config,
        Crawl4aiPageRequest {
            profile_name: "test".to_string(),
            url: "https://example.com".to_string(),
            wait_for: None,
            js_code: None,
        },
        2,
    )
    .await;
    let elapsed = start.elapsed();

    assert!(result.is_err(), "expected timeout error, got {result:?}");
    let err = result.unwrap_err();
    assert!(
        err.contains("timed out"),
        "error should mention timeout, got: {err}"
    );
    // Budget is timeout_secs(1) + guard(2) = 3s; 10s gives generous CI slack.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "did not kill child promptly: {elapsed:?}"
    );
}
