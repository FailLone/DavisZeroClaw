use super::*;
use crate::{run_builtin_crawl_source, RuntimePaths};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn install_crawl4ai(paths: &RuntimePaths) -> Result<()> {
    let python3 = resolve_host_python3().context(
        "need python3.10+ on PATH to build a Crawl4AI venv (try: brew install python@3.13)",
    )?;
    let venv_dir = paths.crawl4ai_venv_dir();
    let python = paths.crawl4ai_python_path();
    let crawl4ai_base_dir = paths.runtime_dir.display().to_string();

    fs::create_dir_all(&paths.runtime_dir)?;
    fs::create_dir_all(paths.crawl4ai_home_dir())?;
    if !python.is_file() {
        println!("Creating Crawl4AI venv: {}", venv_dir.display());
        run_status(
            Command::new(&python3)
                .arg("-m")
                .arg("venv")
                .arg(&venv_dir)
                .env("PATH", tool_path_env())
                .current_dir(&paths.repo_root),
            "python3 -m venv .runtime/davis/crawl4ai-venv",
        )?;
    } else {
        println!("Crawl4AI venv already exists: {}", venv_dir.display());
    }

    println!("Upgrading pip.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("pip")
            .env("CRAWL4_AI_BASE_DIRECTORY", &crawl4ai_base_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "crawl4ai pip upgrade",
    )?;

    println!("Installing Crawl4AI and HTTP server deps.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("crawl4ai")
            .arg("fastapi")
            .arg("uvicorn[standard]")
            .arg("pydantic")
            // trafilatura powers the default extraction engine in
            // crawl4ai_adapter/engines.py; httpx stays installed so the
            // adapter venv can run pytest against the openrouter-era test
            // fixtures and any future HTTP-based engines.
            .arg("trafilatura")
            .arg("httpx")
            .arg("beautifulsoup4")
            .env("CRAWL4_AI_BASE_DIRECTORY", &crawl4ai_base_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "pip install --upgrade crawl4ai fastapi uvicorn[standard] pydantic trafilatura httpx beautifulsoup4",
    )?;

    println!("Installing Playwright Chromium.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("playwright")
            .arg("install")
            .arg("chromium")
            .env("CRAWL4_AI_BASE_DIRECTORY", &crawl4ai_base_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "python -m playwright install chromium",
    )?;

    println!("Installing Patchright Chromium.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("patchright")
            .arg("install")
            .arg("chromium")
            .env("CRAWL4_AI_BASE_DIRECTORY", &crawl4ai_base_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "python -m patchright install chromium",
    )?;

    println!("Crawl4AI + HTTP server deps installed.");
    println!("Python: {}", python.display());
    println!("Adapter: {}", paths.crawl4ai_adapter_dir().display());
    println!("Next: daviszeroclaw crawl check");
    Ok(())
}

pub(super) async fn check_crawl4ai(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let python = resolve_crawl4ai_python(paths)?;
    let adapter_dir = paths.crawl4ai_adapter_dir();
    let crawl4ai_base_dir = paths.runtime_dir.display().to_string();

    println!("Crawl4AI config:");
    println!("- enabled: {}", config.crawl4ai.enabled);
    println!("- python: {}", python.display());
    println!("- adapter_dir: {}", adapter_dir.display());
    println!(
        "- profiles_dir: {}",
        paths.crawl4ai_profiles_root().display()
    );
    println!("- timeout_secs: {}", config.crawl4ai.timeout_secs);

    if !config.crawl4ai.enabled {
        bail!("Crawl4AI is not enabled. Set [crawl4ai].enabled = true in config/davis/local.toml");
    }
    if !adapter_dir.join("__main__.py").is_file() {
        bail!("crawl4ai_adapter was not found: {}", adapter_dir.display());
    }
    if !python.is_file() {
        bail!(
            "Crawl4AI Python was not found: {}\nRun: daviszeroclaw crawl install",
            python.display()
        );
    }

    let import_check = command_output(
        Command::new(&python)
            .arg("-c")
            .arg("import crawl4ai, playwright; print('crawl4ai import ok'); print('playwright import ok')")
            .env("CRAWL4_AI_BASE_DIRECTORY", &crawl4ai_base_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
    )?;
    print_command_streams(&import_check.stdout, &import_check.stderr);
    if !import_check.status_success {
        bail!("Crawl4AI or Playwright import failed");
    }

    fs::create_dir_all(paths.crawl4ai_profiles_root())?;

    // Probe via HTTP. Two paths:
    //   1. No daemon running → spin a throwaway supervisor, probe /health,
    //      run one end-to-end crawl on example.com, then drop. kill_on_drop
    //      kills the spawned Python child when the local Arc goes away.
    //   2. Daemon running → our start() fails because the port is held.
    //      Fall through to probing the daemon's own /health on :3010,
    //      which is the source of truth for a live system.
    let outcome = probe_crawl4ai_runtime(paths, &config.crawl4ai).await;
    match outcome {
        Ok(ProbeOutcome::Throwaway { versions }) => {
            if !versions.is_empty() {
                println!("Adapter versions: {versions}");
            }
            println!(
                "Crawl4AI runtime is ready (throwaway adapter spawn + end-to-end crawl succeeded)."
            );
        }
        Ok(ProbeOutcome::DaemonRunning) => {
            println!(
                "A daemon is already running and holding the crawl4ai port — skipping throwaway spawn."
            );
            println!("Daemon /health reports crawl4ai OK. For a live end-to-end probe use:");
            println!("  curl -sS http://127.0.0.1:3010/health | jq");
        }
        Err(err) => {
            bail!("crawl4ai runtime check failed: {err}");
        }
    }
    println!("Next: daviszeroclaw crawl profile login express-ali");
    Ok(())
}

enum ProbeOutcome {
    Throwaway { versions: String },
    DaemonRunning,
}

async fn probe_crawl4ai_runtime(
    paths: &RuntimePaths,
    config: &crate::Crawl4aiConfig,
) -> Result<ProbeOutcome> {
    use crate::{crawl4ai_crawl, Crawl4aiError, Crawl4aiPageRequest, Crawl4aiSupervisor};

    match Crawl4aiSupervisor::start(paths.clone(), config.clone()).await {
        Ok(supervisor) => {
            let versions = read_adapter_versions(&supervisor).await.unwrap_or_default();
            let request = Crawl4aiPageRequest {
                profile_name: "_healthcheck".to_string(),
                url: "https://example.com".to_string(),
                wait_for: None,
                js_code: None,
                markdown: false,
                extract_engine: None,
                openrouter_config: None,
                learned_rule: None,
            };
            match crawl4ai_crawl(paths, config, &supervisor, request).await {
                Ok(page) => {
                    println!(
                        "Health crawl url: {}",
                        page.current_url.as_deref().unwrap_or("unknown")
                    );
                    if let Some(code) = page.status_code {
                        println!("Health crawl status: {code}");
                    }
                    drop(supervisor);
                    Ok(ProbeOutcome::Throwaway { versions })
                }
                Err(e) => {
                    drop(supervisor);
                    Err(anyhow::anyhow!("end-to-end crawl failed: {e}"))
                }
            }
        }
        Err(Crawl4aiError::ServerUnavailable { details })
            if details.contains("Address already in use")
                || details.contains("port")
                || details.contains("bind") =>
        {
            // Port busy — assume daemon is holding it. Probe the daemon
            // itself to make sure it's actually our daemon and not some
            // unrelated process squatting on 11235.
            if probe_daemon_health().await {
                Ok(ProbeOutcome::DaemonRunning)
            } else {
                Err(anyhow::anyhow!(
                    "crawl4ai port is busy but daemon /health does not respond: {details}"
                ))
            }
        }
        Err(e) => Err(anyhow::anyhow!("spawn supervisor: {e}")),
    }
}

async fn read_adapter_versions(supervisor: &crate::Crawl4aiSupervisor) -> Option<String> {
    let base = supervisor.base_url().await;
    let client = supervisor.http_client();
    let resp = client.get(format!("{base}/health")).send().await.ok()?;
    let body: Value = resp.json().await.ok()?;
    body.get("versions").map(|v| v.to_string())
}

async fn probe_daemon_health() -> bool {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client.get("http://127.0.0.1:3010/health").send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

pub(super) fn list_crawl_sources() -> Result<()> {
    println!("Built-in crawl sources:");
    for source in crate::builtin_crawl_sources() {
        println!("- {}", source.id);
        println!("  category: {}", source.category);
        println!("  description: {}", source.description);
        println!("  profiles: {}", source.login_profiles.join(", "));
        println!("  urls: {}", source.urls.join(", "));
    }
    Ok(())
}

pub(super) async fn run_crawl_source(
    paths: &RuntimePaths,
    source: &str,
    query: Option<String>,
    refresh: bool,
    compact: bool,
) -> Result<()> {
    let local_config = check_local_config(paths)?;
    let source_definition = crate::find_builtin_crawl_source(source).ok_or_else(|| {
        anyhow!(
            "unknown crawl source: {source}\nRun `daviszeroclaw crawl source list` to inspect available sources."
        )
    })?;
    let result = run_builtin_crawl_source(
        paths.clone(),
        local_config.crawl4ai.clone(),
        source_definition.id,
        query,
        refresh,
    )
    .await
    .map_err(|err| anyhow!(err))?;
    if compact {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
}

pub(super) async fn crawl_profile_login(paths: &RuntimePaths, profile: CrawlProfile) -> Result<()> {
    let _ = check_local_config(paths)?;
    let adapter_dir = paths.crawl4ai_adapter_dir();
    if !adapter_dir.join("__main__.py").is_file() {
        bail!("crawl4ai adapter was not found: {}", adapter_dir.display());
    }
    let (profile_name, url) = match profile {
        CrawlProfile::ExpressAli => ("express-ali", ALI_ORDER_URL),
        CrawlProfile::ExpressJd => ("express-jd", JD_ORDER_URL),
    };
    migrate_legacy_crawl4ai_profiles(paths)?;
    let profile_dir = paths.crawl4ai_profiles_root().join(profile_name);
    std::fs::create_dir_all(&profile_dir).with_context(|| {
        format!(
            "failed to create crawl4ai profile directory: {}",
            profile_dir.display()
        )
    })?;
    let python = resolve_crawl4ai_python(paths)?;
    println!("Opening Crawl4AI-compatible browser profile.");
    println!("- profile id: {profile_name}");
    println!("- profile dir: {}", profile_dir.display());
    println!("- page: {url}");
    println!("- finish by returning to this terminal and pressing Enter after login completes");
    run_status(
        Command::new(python)
            .arg("-m")
            .arg("crawl4ai_adapter")
            .arg("login")
            .arg("--runtime-dir")
            .arg(&paths.runtime_dir)
            .arg("--profile-name")
            .arg(profile_name)
            .arg("--profile-path")
            .arg(&profile_dir)
            .arg("--url")
            .arg(url)
            .env("PYTHONPATH", paths.repo_root.display().to_string())
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "open Crawl4AI-compatible browser login flow",
    )
}

/// Pick the newest host `python3` interpreter on PATH for building a Crawl4AI venv.
///
/// crawl4ai + recent fastapi/pydantic need Python >= 3.10; the macOS system
/// `/usr/bin/python3` is 3.9 and would silently succeed `venv` creation only
/// to fail later during `pip install`. We scan every `python3*` file on PATH,
/// ask each interpreter for its actual version (so we see through symlinks
/// like `python3 → python3.13`), and pick the highest satisfying (>= 3.10).
fn resolve_host_python3() -> Result<PathBuf> {
    const MIN_MINOR: u32 = 10;

    // Use the same augmented PATH as every other subprocess call in this
    // module (prepends /opt/homebrew/bin so brew-installed python3.NN is
    // visible even under launchd, which has a minimal default PATH).
    let path_env = tool_path_env();
    let mut best: Option<((u32, u32), PathBuf)> = None;

    for dir in std::env::split_paths(&path_env) {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            // Accept `python3`, `python3.10`, `python3.13`, ... but not
            // `python3-config`, `python3-dbg`, `python3.13t` (free-threaded).
            if !name.starts_with("python3") {
                continue;
            }
            let suffix = &name[7..];
            let suffix_ok = suffix.is_empty()
                || (suffix.starts_with('.') && suffix[1..].chars().all(|c| c.is_ascii_digit()));
            if !suffix_ok {
                continue;
            }
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(version) = probe_python_version(&path) else {
                continue;
            };
            if version.0 != 3 || version.1 < MIN_MINOR {
                continue;
            }
            if best
                .as_ref()
                .is_none_or(|(best_ver, _)| version > *best_ver)
            {
                best = Some((version, path));
            }
        }
    }

    if let Some((_, path)) = best {
        return Ok(path);
    }
    bail!(
        "no python3 >= 3.{MIN_MINOR} found on PATH — install a modern Python (e.g. `brew install python@3.13`) and retry"
    )
}

fn probe_python_version(path: &Path) -> Option<(u32, u32)> {
    let output = Command::new(path)
        .arg("-c")
        .arg("import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

pub(super) fn resolve_crawl4ai_python(paths: &RuntimePaths) -> Result<PathBuf> {
    let runtime_python = paths.crawl4ai_python_path();
    if runtime_python.is_file() {
        return Ok(runtime_python);
    }
    require_command("python3").context("python3 is required to run crawl4ai_adapter")
}

pub(super) fn migrate_legacy_crawl4ai_profiles(paths: &RuntimePaths) -> Result<()> {
    let legacy = paths.crawl4ai_legacy_profiles_root();
    let current = paths.crawl4ai_profiles_root();
    if current.exists() || !legacy.exists() {
        return Ok(());
    }
    if let Some(parent) = current.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(legacy, current)?;
    Ok(())
}

/// Operator-facing: inspect the supervised crawl4ai adapter's pid file,
/// process liveness, and `/health` response.
///
/// Reads `.runtime/davis/crawl4ai.pid` (written by `Crawl4aiSupervisor::
/// spawn_child`), sanity-checks the pid with `kill(pid, 0)` (standard
/// "does this process exist?" probe — returns `ESRCH` if not, `0` if
/// yes), then probes the adapter's own `/health`. Stale pid files
/// (pid written but process gone — e.g. after a `kill -9 daviszeroclaw`)
/// are surfaced loudly instead of showing a silent "not alive".
pub(super) async fn crawl_service_status(paths: &RuntimePaths) -> Result<()> {
    let pid_path = paths.crawl4ai_pid_path();
    let log_path = paths.crawl4ai_log_path();
    println!("pid file : {}", pid_path.display());
    println!("log file : {}", log_path.display());

    let pid_state = match fs::read_to_string(&pid_path) {
        Ok(raw) => {
            let trimmed = raw.trim();
            match trimmed.parse::<i32>() {
                Ok(pid) => {
                    println!("pid      : {pid}");
                    if is_process_alive(pid) {
                        PidState::Alive
                    } else {
                        // Pid recorded but the process is gone. Almost
                        // always means daviszeroclaw was force-killed and
                        // didn't get a chance to tear down its child.
                        println!("note     : pid file exists but process is gone (stale)");
                        PidState::Stale
                    }
                }
                Err(_) => {
                    println!("pid      : <malformed: {trimmed:?}>");
                    PidState::Malformed
                }
            }
        }
        Err(_) => {
            println!("pid      : <no pid file; daemon may not have started crawl4ai>");
            PidState::Missing
        }
    };

    // Health probe — best effort. A 200 means the adapter is answering
    // even if the pid file is stale (unlikely but possible if something
    // respawned it). A timeout / connection refused is the common case
    // when the daemon is not running.
    let config = check_local_config(paths)?;
    let url = format!("{}/health", config.crawl4ai.base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .context("build crawl4ai health probe client")?;
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let first_line = body.lines().next().unwrap_or("");
            // Slice by characters, not bytes. `str::len()` returns byte
            // count, so `&first_line[..200]` panics when byte 200 lands
            // mid-codepoint — trivially reproducible with a Chinese
            // /health error body like "服务暂时不可用...".
            let excerpt: String = first_line.chars().take(200).collect();
            println!("health   : {status} ({url})");
            if !excerpt.is_empty() {
                println!("           {excerpt}");
            }
        }
        Err(err) => println!("health   : unreachable ({err})"),
    }

    if matches!(pid_state, PidState::Stale) {
        println!();
        println!(
            "Stale pid file. Either daviszeroclaw was kill -9'd, or the daemon is not running."
        );
        println!(
            "Safe to remove: `trash {}` or `rm {}`",
            pid_path.display(),
            pid_path.display()
        );
    }
    Ok(())
}

/// Operator-facing: SIGTERM the adapter. The daemon's supervisor loop
/// will notice the child exited and respawn it on the next tick (see
/// `Crawl4aiSupervisor::spawn_restart_loop`). No-op if the pid file is
/// missing or points at a dead process.
pub(super) async fn crawl_service_restart(paths: &RuntimePaths) -> Result<()> {
    crawl_service_stop_inner(paths, /* remove_pid = */ false)?;
    println!("Daemon supervisor will respawn the adapter on its next restart loop tick.");
    Ok(())
}

/// Operator-facing: SIGTERM the adapter and clear the pid file so the
/// next `crawl service status` reports "no pid file" rather than stale
/// liveness. Use this when shutting down the daemon is inconvenient but
/// you want the adapter stopped.
pub(super) async fn crawl_service_stop(paths: &RuntimePaths) -> Result<()> {
    crawl_service_stop_inner(paths, /* remove_pid = */ true)
}

fn crawl_service_stop_inner(paths: &RuntimePaths, remove_pid: bool) -> Result<()> {
    let pid_path = paths.crawl4ai_pid_path();
    let raw = match fs::read_to_string(&pid_path) {
        Ok(raw) => raw,
        Err(_) => {
            println!("No pid file at {}; nothing to stop.", pid_path.display());
            return Ok(());
        }
    };
    let pid: i32 = raw.trim().parse().context("parse crawl4ai.pid")?;
    // Guard POSIX kill(2) special semantics before we reach libc::kill:
    //   pid ==  0 signals the whole calling process group
    //   pid == -1 signals every process the caller can signal
    //   pid <  -1 signals process group -pid
    // A corrupted pid file (disk-full truncation, concurrent editor
    // save, operator meddling) would otherwise turn `crawl service stop`
    // into a self-SIGTERM against the daemon and its siblings. `pid == 1`
    // is reserved for init / launchd; also refuse it.
    if pid <= 1 {
        bail!("refusing to signal pid {pid} from pid file (invalid or reserved)");
    }
    if pid == std::process::id() as i32 {
        bail!("refusing to signal self (pid file points at daviszeroclaw itself: {pid})");
    }
    // Detect stale pid before signaling — avoids racing another process
    // that happens to have been assigned the same pid after ours exited.
    if !is_process_alive(pid) {
        println!("pid {pid} is already gone (stale pid file).");
        if remove_pid {
            let _ = fs::remove_file(&pid_path);
            println!("Removed stale pid file.");
        }
        return Ok(());
    }
    // SAFETY: kill(2) is async-signal-safe and does not alter the caller's
    // process state. We're passing SIGTERM to a pid we wrote ourselves and
    // just verified is alive; worst case the process has already exited in
    // the last few microseconds and libc::kill returns ESRCH, which we
    // surface via std::io::Error below.
    unsafe {
        if libc::kill(pid, libc::SIGTERM) != 0 {
            let err = std::io::Error::last_os_error();
            bail!("kill({pid}, SIGTERM): {err}");
        }
    }
    println!("Sent SIGTERM to {pid}.");
    if remove_pid {
        let _ = fs::remove_file(&pid_path);
        println!("Removed pid file.");
    }
    Ok(())
}

enum PidState {
    Alive,
    Stale,
    Malformed,
    Missing,
}

/// `kill(pid, 0)` is the standard POSIX way to test process existence
/// without actually signaling. Returns 0 if the process exists (even as
/// a zombie or owned by another user), `ESRCH` otherwise. We deliberately
/// do not distinguish "no permission" (`EPERM`) from "exists" — either
/// way the process is still around.
///
/// We short-circuit on `pid <= 1` because `kill(0, 0)` returns 0 (process
/// group 0 exists and we can signal it), which would wrongly report the
/// adapter as "alive" when the pid file is actually corrupt. `pid == 1`
/// is reserved for init / launchd and never belongs to us.
fn is_process_alive(pid: i32) -> bool {
    if pid <= 1 {
        return false;
    }
    // SAFETY: kill(pid, 0) with signal 0 performs only the error check
    // and does not alter any process state.
    unsafe { libc::kill(pid, 0) == 0 }
}
