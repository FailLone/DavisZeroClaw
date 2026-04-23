use super::*;
use crate::{run_builtin_crawl_source, RuntimePaths};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(super) fn install_crawl4ai(paths: &RuntimePaths) -> Result<()> {
    let python3 = require_command("python3").context("python3 is required to install Crawl4AI")?;
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
            .env("CRAWL4_AI_BASE_DIRECTORY", &crawl4ai_base_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "pip install --upgrade crawl4ai fastapi uvicorn[standard] pydantic",
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

pub(super) fn check_crawl4ai(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let python = resolve_crawl4ai_python(paths, &config.crawl4ai)?;
    let adapter_dir = paths.crawl4ai_adapter_dir();
    let crawl4ai_base_dir = paths.runtime_dir.display().to_string();

    println!("Crawl4AI config:");
    println!("- enabled: {}", config.crawl4ai.enabled);
    println!("- transport: {:?}", config.crawl4ai.transport);
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

    let adapter_check = command_output(
        Command::new(&python)
            .arg("-m")
            .arg("crawl4ai_adapter")
            .arg("crawl")
            .arg("--help")
            .env("CRAWL4_AI_BASE_DIRECTORY", &crawl4ai_base_dir)
            .env("PYTHONPATH", paths.repo_root.display().to_string())
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
    )?;
    if !adapter_check.status_success {
        print_command_streams(&adapter_check.stdout, &adapter_check.stderr);
        bail!("crawl4ai_adapter did not respond to --help");
    }

    fs::create_dir_all(paths.crawl4ai_profiles_root())?;
    println!("Running adapter health crawl.");
    let health_result = run_crawl4ai_health_check(paths, &python)?;
    if !health_result {
        bail!("crawl4ai adapter health crawl failed");
    }

    println!("Crawl4AI runtime is ready.");
    println!("Next: daviszeroclaw crawl profile login express-ali");
    Ok(())
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

pub(super) fn run_crawl4ai_health_check(paths: &RuntimePaths, python: &Path) -> Result<bool> {
    let health_profile = paths.crawl4ai_profiles_root().join("_healthcheck");
    fs::create_dir_all(&health_profile)?;

    let payload = json!({
        "profile_path": health_profile.display().to_string(),
        "url": "https://example.com",
        "timeout_secs": 45,
        "headless": true,
        "magic": false,
        "simulate_user": false,
        "override_navigator": false,
        "remove_overlay_elements": true,
        "enable_stealth": true,
    });

    let mut child = Command::new(python)
        .arg("-m")
        .arg("crawl4ai_adapter")
        .arg("crawl")
        .arg("--runtime-dir")
        .arg(paths.runtime_dir.display().to_string())
        .env(
            "CRAWL4_AI_BASE_DIRECTORY",
            paths.runtime_dir.display().to_string(),
        )
        .env("PYTHONPATH", paths.repo_root.display().to_string())
        .env("PATH", tool_path_env())
        .current_dir(&paths.repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn crawl4ai health check")?;

    if let Some(mut stdin) = child.stdin.take() {
        let raw = serde_json::to_vec(&payload)?;
        stdin.write_all(&raw)?;
    }

    let output = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        println!("{stderr}");
    }
    if !output.status.success() {
        if !stdout.is_empty() {
            println!("{stdout}");
        }
        return Ok(false);
    }

    let body = parse_crawl4ai_adapter_json(&output.stdout)
        .context("parse crawl4ai health check response json")?;
    let success = body
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let current_url = body
        .get("url")
        .or_else(|| body.get("redirected_url"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let status_code = body
        .get("status_code")
        .and_then(Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("Health crawl url: {current_url}");
    println!("Health crawl status: {status_code}");
    if !success {
        if let Some(error_message) = body
            .get("error")
            .or_else(|| body.get("error_message"))
            .and_then(Value::as_str)
        {
            println!("Health crawl error: {error_message}");
        }
    }
    Ok(success)
}

pub(super) fn parse_crawl4ai_adapter_json(stdout: &[u8]) -> Result<Value> {
    let text = String::from_utf8_lossy(stdout);
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return Ok(value);
        }
    }
    bail!("no json payload found in adapter stdout: {text}")
}

pub(super) async fn crawl_profile_login(paths: &RuntimePaths, profile: CrawlProfile) -> Result<()> {
    let local_config = check_local_config(paths)?;
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
    let python = resolve_crawl4ai_python(paths, &local_config.crawl4ai)?;
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

pub(super) fn resolve_crawl4ai_python(
    paths: &RuntimePaths,
    config: &crate::Crawl4aiConfig,
) -> Result<PathBuf> {
    if !config.python.is_empty() {
        let configured = PathBuf::from(&config.python);
        if configured.components().count() > 1 || configured.is_absolute() {
            return Ok(configured);
        }
        return require_command(&config.python).with_context(|| {
            format!(
                "configured crawl4ai.python was not found: {}",
                config.python
            )
        });
    }
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
