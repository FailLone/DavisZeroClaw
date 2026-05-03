use super::*;
use crate::{check_local_config, zeroclaw_env_vars, RuntimePaths};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub(super) async fn start(paths: &RuntimePaths) -> Result<()> {
    println!("DavisZeroClaw startup");
    println!("Repository: {}", paths.repo_root.display());
    println!("Runtime: {}", paths.runtime_dir.display());

    let proxy_plist = proxy_service_plist_path()?;
    let zeroclaw_plist = davis_service_plist_path()?;
    if either_plist_exists(&proxy_plist, &zeroclaw_plist) {
        bail!(
            "Davis launchd service is already installed ({}).\n\
             Run `daviszeroclaw service uninstall` first, or use \
             `daviszeroclaw service restart` to reload.",
            proxy_service_label()
        );
    }

    print_start_step(1, "Preflight");
    let zeroclaw = require_command("zeroclaw")
        .context("zeroclaw was not found. Install it first: brew install zeroclaw")?;
    fs::create_dir_all(&paths.runtime_dir)?;

    if !paths.config_template_path().is_file() {
        bail!(
            "config template was not found: {}",
            paths.config_template_path().display()
        );
    }
    if !paths.local_config_path().is_file() {
        bail!(
            "local config was not found: {}\nCreate it first: cp {} {}",
            paths.local_config_path().display(),
            paths.local_config_example_path().display(),
            paths.local_config_path().display()
        );
    }

    check_imessage_permissions()?;
    println!("Preflight OK.");

    let cargo =
        require_command("cargo").context("cargo was not found; cannot build Davis services")?;
    print_start_step(2, "Build local Rust service");
    run_status(
        Command::new(cargo)
            .arg("build")
            .arg("--release")
            .arg("--bin")
            .arg("davis-local-proxy")
            .arg("--manifest-path")
            .arg(paths.repo_root.join("Cargo.toml"))
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "cargo build --release --bin davis-local-proxy",
    )?;
    println!("Build OK.");

    print_start_step(3, "Load config and provider credentials");
    let config = check_local_config(paths)?;
    let provider_env = zeroclaw_env_vars(&config);
    println!("Config OK: {}", paths.local_config_path().display());
    println!("Provider environment prepared for ZeroClaw.");

    print_start_step(4, "Prepare runtime skills, SOPs, and workspace files");
    report_agent_browser_status();
    report_skill_inventory(paths);
    sync_runtime_skills(paths)?;
    sync_runtime_sops(paths)?;
    sync_workspace_files(paths)?;

    let proxy_bin = release_bin_path(paths, "davis-local-proxy");
    let local_proxy_log = paths.local_proxy_log_path();
    let local_proxy_pid = paths.local_proxy_pid_path();
    let mut proxy_cmd = Command::new(&proxy_bin);
    proxy_cmd
        .env("DAVIS_REPO_ROOT", &paths.repo_root)
        .env("DAVIS_RUNTIME_DIR", &paths.runtime_dir)
        .env("PATH", tool_path_env())
        .current_dir(&paths.repo_root);
    for (key, value) in &provider_env {
        proxy_cmd.env(key, value);
    }

    print_start_step(5, "Start Davis local proxy");
    start_process(
        "Davis Local Proxy",
        &local_proxy_pid,
        &local_proxy_log,
        Probe::Http("http://127.0.0.1:3010/health".to_string()),
        proxy_cmd,
    )
    .await?;

    if !paths.config_report_cache_path().is_file() {
        println!("Generating the initial Home Assistant advisor report...");
        let _ = http_get_text("http://127.0.0.1:3010/advisor/config-report").await;
    }

    print_start_step(6, "Start ZeroClaw daemon");
    start_runtime_daemon(paths, &zeroclaw, &provider_env).await?;

    println!();
    println!("Startup complete.");
    println!("Local status:");
    println!("- Davis local proxy: http://127.0.0.1:3010/health");
    println!("- Runtime traces: http://127.0.0.1:3010/zeroclaw/runtime-traces");
    println!("- HA advisor report: http://127.0.0.1:3010/advisor/config-report");
    println!();
    println!("Network endpoints:");
    println!("- Gateway health: http://<mac-ip>:3000/health");
    println!("- Shortcut bridge: http://<mac-ip>:3012/shortcut");
    println!();
    println!("Channels:");
    println!("- iMessage: served through this Mac's Messages.app");
    println!("- Stop services: daviszeroclaw stop");

    Ok(())
}

pub(super) fn print_start_step(index: usize, title: &str) {
    println!();
    println!("[{index}/6] {title}");
}

pub(super) fn stop(paths: &RuntimePaths) -> Result<()> {
    println!("======================================");
    println!("    Stop DavisZeroClaw");
    println!("======================================");

    stop_process("Davis Local Proxy", &paths.local_proxy_pid_path())?;
    stop_process(
        "Legacy Davis Local Proxy",
        &paths.legacy_local_proxy_pid_path(),
    )?;
    stop_process("ZeroClaw Daemon", &paths.daemon_pid_path())?;
    stop_process("Channel Server", &paths.runtime_dir.join("channel.pid"))?;
    stop_process("Gateway", &paths.runtime_dir.join("gateway.pid"))?;
    Ok(())
}

pub(super) async fn install_davis_service(paths: &RuntimePaths) -> Result<()> {
    ensure_macos("Davis service management")?;
    fs::create_dir_all(&paths.runtime_dir)?;
    if pid_file_is_alive(&paths.local_proxy_pid_path())
        || pid_file_is_alive(&paths.daemon_pid_path())
    {
        bail!(
            "Davis foreground processes are running (started via `daviszeroclaw start`).\n\
             Run `daviszeroclaw stop` first, then retry `service install`."
        );
    }
    render_current_runtime_config(paths)?;
    sync_runtime_skills(paths)?;
    sync_runtime_sops(paths)?;
    sync_workspace_files(paths)?;
    let proxy_bin = ensure_release_binary(paths, "davis-local-proxy")?;
    let zeroclaw = require_command("zeroclaw")
        .context("zeroclaw was not found. Install it first: brew install zeroclaw")?;
    let plist_path = davis_service_plist_path()?;
    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let stdout_path = paths.runtime_dir.join("daemon.launchd.stdout.log");
    let stderr_path = paths.runtime_dir.join("daemon.launchd.stderr.log");
    let spec = DavisServiceSpec {
        label: davis_service_label().to_string(),
        repo_root: paths.repo_root.clone(),
        runtime_dir: paths.runtime_dir.clone(),
        zeroclaw_bin: zeroclaw.clone(),
        proxy_bin: proxy_bin.clone(),
        stdout_path,
        stderr_path,
        path_env: tool_path_env().to_string_lossy().to_string(),
    };
    let plist = render_davis_launchd_plist(&spec);
    fs::write(&plist_path, plist)
        .with_context(|| format!("failed to write {}", plist_path.display()))?;

    if let Some(plutil) = command_path("plutil") {
        run_status(
            Command::new(plutil)
                .arg("-lint")
                .arg(&plist_path)
                .env("PATH", tool_path_env()),
            "plutil -lint Davis service plist",
        )?;
    }

    let user_target = launchd_user_target()?;
    bootout_davis_service(&user_target, &plist_path);
    run_status(
        Command::new("launchctl")
            .arg("bootstrap")
            .arg(&user_target)
            .arg(&plist_path)
            .env("PATH", tool_path_env()),
        "launchctl bootstrap Davis service",
    )?;
    run_status(
        Command::new("launchctl")
            .arg("enable")
            .arg(launchd_service_target(&user_target))
            .env("PATH", tool_path_env()),
        "launchctl enable Davis service",
    )?;
    run_status(
        Command::new("launchctl")
            .arg("kickstart")
            .arg("-k")
            .arg(launchd_service_target(&user_target))
            .env("PATH", tool_path_env()),
        "launchctl kickstart Davis service",
    )?;

    let _ = wait_for_probe(
        &Probe::Http("http://127.0.0.1:3000/health".to_string()),
        20,
        Duration::from_millis(500),
    )
    .await;

    // Provision the davis-local-proxy launchd service.
    let proxy_plist_path = proxy_service_plist_path()?;
    if let Some(parent) = proxy_plist_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let proxy_stdout_path = paths.runtime_dir.join("proxy.launchd.stdout.log");
    let proxy_stderr_path = paths.runtime_dir.join("proxy.launchd.stderr.log");
    let proxy_spec = DavisServiceSpec {
        label: proxy_service_label().to_string(),
        repo_root: paths.repo_root.clone(),
        runtime_dir: paths.runtime_dir.clone(),
        zeroclaw_bin: zeroclaw.clone(),
        proxy_bin: proxy_bin.clone(),
        stdout_path: proxy_stdout_path,
        stderr_path: proxy_stderr_path,
        path_env: tool_path_env().to_string_lossy().to_string(),
    };
    let proxy_plist = render_proxy_launchd_plist(&proxy_spec);
    fs::write(&proxy_plist_path, proxy_plist)
        .with_context(|| format!("failed to write {}", proxy_plist_path.display()))?;

    if let Some(plutil) = command_path("plutil") {
        run_status(
            Command::new(plutil)
                .arg("-lint")
                .arg(&proxy_plist_path)
                .env("PATH", tool_path_env()),
            "plutil -lint proxy service plist",
        )?;
    }

    bootout_davis_service(&user_target, &proxy_plist_path);
    run_status(
        Command::new("launchctl")
            .arg("bootstrap")
            .arg(&user_target)
            .arg(&proxy_plist_path)
            .env("PATH", tool_path_env()),
        "launchctl bootstrap proxy service",
    )?;
    run_status(
        Command::new("launchctl")
            .arg("enable")
            .arg(format!("{user_target}/{}", proxy_service_label()))
            .env("PATH", tool_path_env()),
        "launchctl enable proxy service",
    )?;
    run_status(
        Command::new("launchctl")
            .arg("kickstart")
            .arg("-k")
            .arg(format!("{user_target}/{}", proxy_service_label()))
            .env("PATH", tool_path_env()),
        "launchctl kickstart proxy service",
    )?;

    let _ = wait_for_probe(
        &Probe::Http("http://127.0.0.1:3010/health".to_string()),
        20,
        Duration::from_millis(500),
    )
    .await;

    println!("Davis services installed.");
    println!("- zeroclaw plist: {}", plist_path.display());
    println!("- proxy plist:    {}", proxy_plist_path.display());
    println!("- config: {}", paths.runtime_config_path().display());
    status_davis_service(paths).await
}

pub(super) async fn status_davis_service(paths: &RuntimePaths) -> Result<()> {
    ensure_macos("Davis service management")?;
    let plist_path = davis_service_plist_path()?;
    let user_target = launchd_user_target()?;
    let mut print_cmd = Command::new("launchctl");
    print_cmd
        .arg("print")
        .arg(launchd_service_target(&user_target))
        .env("PATH", tool_path_env());
    let output = command_output(&mut print_cmd).unwrap_or(CommandOutput {
        status_success: false,
        stdout: String::new(),
        stderr: String::new(),
    });

    println!("Davis ZeroClaw service");
    println!("- label: {}", davis_service_label());
    println!("- plist: {}", plist_path.display());
    println!("- config: {}", paths.runtime_config_path().display());
    if output.status_success {
        let state = launchd_state_label(&output.stdout);
        println!("- launchd: loaded ({state})");
    } else if plist_path.is_file() {
        println!("- launchd: not loaded");
    } else {
        println!("- launchd: not installed");
    }

    match http_get_text("http://127.0.0.1:3000/health").await {
        Ok(payload) => {
            let health = serde_json::from_str::<Value>(&payload).ok();
            let top_status = health
                .as_ref()
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("ok");
            println!("- zeroclaw: {top_status} (http://127.0.0.1:3000/health)");
            if let Some(health) = &health {
                if let Some(pid) = health.pointer("/runtime/pid").and_then(Value::as_u64) {
                    println!("- pid: {pid}");
                }
                print_health_component(health, "gateway", "gateway");
                print_health_component(health, "scheduler", "scheduler");
                print_health_component(health, "channel:webhook", "webhook");
                print_health_component(health, "channel:imessage", "iMessage");
            }
        }
        Err(err) => println!("- zeroclaw: unavailable ({err})"),
    }

    println!(
        "- stdout: {}",
        paths
            .runtime_dir
            .join("daemon.launchd.stdout.log")
            .display()
    );
    println!(
        "- stderr: {}",
        paths
            .runtime_dir
            .join("daemon.launchd.stderr.log")
            .display()
    );

    println!();
    println!("Davis proxy service");
    println!("- label: {}", proxy_service_label());
    let proxy_plist_path = proxy_service_plist_path()?;
    println!("- plist: {}", proxy_plist_path.display());

    let mut proxy_print_cmd = Command::new("launchctl");
    proxy_print_cmd
        .arg("print")
        .arg(format!("{user_target}/{}", proxy_service_label()))
        .env("PATH", tool_path_env());
    let proxy_output = command_output(&mut proxy_print_cmd).unwrap_or(CommandOutput {
        status_success: false,
        stdout: String::new(),
        stderr: String::new(),
    });

    if proxy_output.status_success {
        let state = launchd_state_label(&proxy_output.stdout);
        println!("- launchd: loaded ({state})");
    } else if proxy_plist_path.is_file() {
        println!("- launchd: not loaded");
    } else {
        println!("- launchd: not installed");
    }

    match http_get_text("http://127.0.0.1:3010/health").await {
        Ok(_) => println!("- proxy: ok (http://127.0.0.1:3010/health)"),
        Err(err) => println!("- proxy: unavailable ({err})"),
    }

    println!(
        "- stdout: {}",
        paths.runtime_dir.join("proxy.launchd.stdout.log").display()
    );
    println!(
        "- stderr: {}",
        paths.runtime_dir.join("proxy.launchd.stderr.log").display()
    );
    tunnel_status(paths).await?;
    Ok(())
}

pub(super) async fn restart_davis_service(paths: &RuntimePaths) -> Result<()> {
    ensure_macos("Davis service management")?;
    let plist_path = davis_service_plist_path()?;
    if !plist_path.is_file() {
        println!("Davis service is not installed; installing it now.");
        return install_davis_service(paths).await;
    }

    render_current_runtime_config(paths)?;
    let user_target = launchd_user_target()?;

    run_status(
        Command::new("launchctl")
            .arg("kickstart")
            .arg("-k")
            .arg(launchd_service_target(&user_target))
            .env("PATH", tool_path_env()),
        "launchctl kickstart zeroclaw service",
    )?;

    let proxy_plist = proxy_service_plist_path()?;
    if proxy_plist.is_file() {
        run_status(
            Command::new("launchctl")
                .arg("kickstart")
                .arg("-k")
                .arg(format!("{user_target}/{}", proxy_service_label()))
                .env("PATH", tool_path_env()),
            "launchctl kickstart proxy service",
        )?;
        let _ = wait_for_probe(
            &Probe::Http("http://127.0.0.1:3010/health".to_string()),
            20,
            Duration::from_millis(500),
        )
        .await;
    }

    let _ = wait_for_probe(
        &Probe::Http("http://127.0.0.1:3000/health".to_string()),
        20,
        Duration::from_millis(500),
    )
    .await;
    println!("Davis services restarted.");
    status_davis_service(paths).await
}

pub(super) fn uninstall_davis_service(_paths: &RuntimePaths) -> Result<()> {
    ensure_macos("Davis service management")?;
    let user_target = launchd_user_target()?;

    let zeroclaw_plist = davis_service_plist_path()?;
    bootout_davis_service(&user_target, &zeroclaw_plist);
    if zeroclaw_plist.is_file() {
        fs::remove_file(&zeroclaw_plist)
            .with_context(|| format!("failed to remove {}", zeroclaw_plist.display()))?;
        println!("- removed: {}", zeroclaw_plist.display());
    } else {
        println!("- zeroclaw plist not found (already uninstalled?)");
    }

    let proxy_plist = proxy_service_plist_path()?;
    bootout_davis_service(&user_target, &proxy_plist);
    if proxy_plist.is_file() {
        fs::remove_file(&proxy_plist)
            .with_context(|| format!("failed to remove {}", proxy_plist.display()))?;
        println!("- removed: {}", proxy_plist.display());
    } else {
        println!("- proxy plist not found (already uninstalled?)");
    }

    println!("Davis services uninstalled.");
    Ok(())
}

#[derive(Debug)]
pub(super) struct DavisServiceSpec {
    pub(super) label: String,
    pub(super) repo_root: PathBuf,
    pub(super) runtime_dir: PathBuf,
    pub(super) zeroclaw_bin: PathBuf,
    pub(super) proxy_bin: PathBuf,
    pub(super) stdout_path: PathBuf,
    pub(super) stderr_path: PathBuf,
    pub(super) path_env: String,
}

pub(super) fn render_current_runtime_config(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    crate::render_runtime_config(paths, &config)?;
    Ok(())
}

pub(super) fn ensure_release_binary(paths: &RuntimePaths, name: &str) -> Result<PathBuf> {
    let bin = release_bin_path(paths, name);
    if bin.is_file() {
        return Ok(bin);
    }

    let cargo =
        require_command("cargo").context("cargo was not found; cannot build Davis binary")?;
    run_status(
        Command::new(cargo)
            .arg("build")
            .arg("--release")
            .arg("--bin")
            .arg(name)
            .arg("--manifest-path")
            .arg(paths.repo_root.join("Cargo.toml"))
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        &format!("cargo build --release --bin {name}"),
    )?;
    Ok(bin)
}

pub(super) fn render_davis_launchd_plist(spec: &DavisServiceSpec) -> String {
    let command = format!(
        "cd {} && eval \"$({} print-zeroclaw-env)\" && exec {} daemon --config-dir {}",
        shell_quote(&spec.repo_root.display().to_string()),
        shell_quote(&spec.proxy_bin.display().to_string()),
        shell_quote(&spec.zeroclaw_bin.display().to_string()),
        shell_quote(&spec.runtime_dir.display().to_string())
    );
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/zsh</string>
    <string>-lc</string>
    <string>{}</string>
  </array>
  <key>WorkingDirectory</key>
  <string>{}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>DAVIS_REPO_ROOT</key>
    <string>{}</string>
    <key>DAVIS_RUNTIME_DIR</key>
    <string>{}</string>
    <key>ZEROCLAW_CONFIG_DIR</key>
    <string>{}</string>
    <key>PATH</key>
    <string>{}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{}</string>
  <key>StandardErrorPath</key>
  <string>{}</string>
</dict>
</plist>
"#,
        xml_escape(&spec.label),
        xml_escape(&command),
        xml_escape(&spec.repo_root.display().to_string()),
        xml_escape(&spec.repo_root.display().to_string()),
        xml_escape(&spec.runtime_dir.display().to_string()),
        xml_escape(&spec.runtime_dir.display().to_string()),
        xml_escape(&spec.path_env),
        xml_escape(&spec.stdout_path.display().to_string()),
        xml_escape(&spec.stderr_path.display().to_string())
    )
}

pub(super) fn render_proxy_launchd_plist(spec: &DavisServiceSpec) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
  </array>
  <key>WorkingDirectory</key>
  <string>{}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>DAVIS_REPO_ROOT</key>
    <string>{}</string>
    <key>DAVIS_RUNTIME_DIR</key>
    <string>{}</string>
    <key>PATH</key>
    <string>{}</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{}</string>
  <key>StandardErrorPath</key>
  <string>{}</string>
</dict>
</plist>
"#,
        xml_escape(&spec.label),
        xml_escape(&spec.proxy_bin.display().to_string()),
        xml_escape(&spec.repo_root.display().to_string()),
        xml_escape(&spec.repo_root.display().to_string()),
        xml_escape(&spec.runtime_dir.display().to_string()),
        xml_escape(&spec.path_env),
        xml_escape(&spec.stdout_path.display().to_string()),
        xml_escape(&spec.stderr_path.display().to_string()),
    )
}

pub(super) fn davis_service_label() -> &'static str {
    "com.daviszeroclaw.zeroclaw"
}

pub(super) fn proxy_service_label() -> &'static str {
    "com.daviszeroclaw.proxy"
}

pub(super) fn proxy_service_plist_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", proxy_service_label())))
}

pub(super) fn davis_service_plist_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", davis_service_label())))
}

pub(super) fn launchd_user_target() -> Result<String> {
    let uid = command_text(Command::new("id").arg("-u").env("PATH", tool_path_env()))?;
    Ok(format!("gui/{}", uid.trim()))
}

pub(super) fn launchd_service_target(user_target: &str) -> String {
    format!("{user_target}/{}", davis_service_label())
}

pub(super) fn bootout_davis_service(user_target: &str, plist_path: &Path) {
    let _ = Command::new("launchctl")
        .arg("bootout")
        .arg(user_target)
        .arg(plist_path)
        .env("PATH", tool_path_env())
        .status();
}

pub(super) fn launchd_state_label(output: &str) -> String {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("state = "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("loaded")
        .to_string()
}

pub(super) fn print_health_component(health: &Value, component: &str, label: &str) {
    let Some(component) = health
        .get("runtime")
        .and_then(|runtime| runtime.get("components"))
        .and_then(|components| components.get(component))
    else {
        return;
    };
    let status = component
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let last_error = component.get("last_error").and_then(Value::as_str);
    if let Some(error) = last_error.filter(|error| !error.is_empty()) {
        println!("- {label}: {status} ({error})");
    } else {
        println!("- {label}: {status}");
    }
}

pub(super) fn either_plist_exists(proxy_plist: &Path, zeroclaw_plist: &Path) -> bool {
    proxy_plist.is_file() || zeroclaw_plist.is_file()
}

pub(super) fn pid_file_is_alive(pid_file: &Path) -> bool {
    read_pid(pid_file).is_some_and(pid_is_alive)
}

pub(super) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
