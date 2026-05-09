//! `daviszeroclaw router-dhcp …` subcommand handlers.

use super::*;
use crate::{check_local_config, PythonRouterChecker, RouterChecker, RuntimePaths};
use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::process::Command;

pub(super) fn install_router_adapter(paths: &RuntimePaths) -> Result<()> {
    let python3 = resolve_host_python3()
        .context("need python3.10+ on PATH (e.g. brew install python@3.13)")?;
    let venv_dir = paths.router_adapter_venv_dir();
    let python = paths.router_adapter_python_path();
    let adapter_dir = paths.router_adapter_dir();
    let browsers_path = paths.playwright_browsers_path();

    if !adapter_dir.join("__main__.py").is_file() {
        bail!(
            "router_adapter package not found at {} — repo checkout incomplete",
            adapter_dir.display()
        );
    }

    fs::create_dir_all(&paths.runtime_dir)?;
    fs::create_dir_all(&browsers_path)?;

    if !python.is_file() {
        println!("Creating router-adapter venv: {}", venv_dir.display());
        run_status(
            Command::new(&python3)
                .arg("-m")
                .arg("venv")
                .arg(&venv_dir)
                .env("PATH", tool_path_env())
                .current_dir(&paths.repo_root),
            "python3 -m venv .runtime/davis/router-adapter-venv",
        )?;
    } else {
        println!("router-adapter venv already exists: {}", venv_dir.display());
    }

    println!("Upgrading pip.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("pip")
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "router-adapter pip upgrade",
    )?;

    println!("Installing router_adapter package + playwright + python-dotenv.");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg("-e")
            .arg(&adapter_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "pip install -e router_adapter",
    )?;

    println!(
        "Installing Playwright Chromium into shared cache: {}",
        browsers_path.display()
    );
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("playwright")
            .arg("install")
            .arg("chromium")
            .env("PLAYWRIGHT_BROWSERS_PATH", &browsers_path)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "python -m playwright install chromium",
    )?;

    println!("router-adapter installed.");
    println!("Python: {}", python.display());
    println!(
        "Browsers: {}  (shared with crawl4ai_adapter; do not double-install)",
        browsers_path.display()
    );
    println!("Next: daviszeroclaw router-dhcp run-once");
    Ok(())
}

pub(super) async fn run_once_router_check(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let cfg = config.router_dhcp.clone();
    if !cfg.enabled {
        println!("[router_dhcp].enabled is false in local.toml — running ad-hoc anyway.");
    }
    let checker = PythonRouterChecker::from_env(paths.clone(), cfg)
        .ok_or_else(|| anyhow!("ROUTER_USERNAME / ROUTER_PASSWORD env vars are not set"))?;
    let outcome = checker.check_once().await;
    println!("{outcome:#?}");
    Ok(())
}
