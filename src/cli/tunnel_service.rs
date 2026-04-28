use super::*;
use crate::{check_local_config, RuntimePaths};
use anyhow::{anyhow, bail, Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

// ── Tunnel service helpers ────────────────────────────────────────────────────

#[derive(Debug)]
pub(super) struct TunnelServiceSpec {
    pub(super) cloudflared_bin: PathBuf,
    pub(super) config_path: PathBuf,
    pub(super) stdout_path: PathBuf,
    pub(super) stderr_path: PathBuf,
    pub(super) path_env: String,
}

pub(super) fn tunnel_service_label() -> &'static str {
    "com.daviszeroclaw.tunnel"
}

pub(super) fn tunnel_service_plist_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", tunnel_service_label())))
}

pub(super) fn tunnel_cloudflared_config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join(".cloudflared")
        .join("davis-shortcut.yml"))
}

pub(super) fn render_tunnel_launchd_plist(spec: &TunnelServiceSpec) -> String {
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
    <string>tunnel</string>
    <string>--config</string>
    <string>{}</string>
    <string>run</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
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
        xml_escape(tunnel_service_label()),
        xml_escape(&spec.cloudflared_bin.display().to_string()),
        xml_escape(&spec.config_path.display().to_string()),
        xml_escape(&spec.path_env),
        xml_escape(&spec.stdout_path.display().to_string()),
        xml_escape(&spec.stderr_path.display().to_string()),
    )
}

pub(super) async fn tunnel_install(paths: &RuntimePaths) -> Result<()> {
    ensure_macos("Davis tunnel management")?;

    let cloudflared = require_command("cloudflared").context(
        "cloudflared not found. Install it first: brew install cloudflare/cloudflare/cloudflared",
    )?;

    let config = check_local_config(paths)?;
    let tunnel_cfg = config
        .tunnel
        .as_ref()
        .filter(|t| t.tunnel_id.is_some() && t.hostname.is_some())
        .ok_or_else(|| {
            anyhow!(
                "[tunnel] tunnel_id and hostname are required in local.toml. \
                 See local.example.toml for an example."
            )
        })?;
    let tunnel_id = tunnel_cfg.tunnel_id.as_deref().unwrap();
    let hostname = tunnel_cfg.hostname.as_deref().unwrap();

    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    let credentials_path = PathBuf::from(&home)
        .join(".cloudflared")
        .join(format!("{tunnel_id}.json"));
    if !credentials_path.is_file() {
        bail!(
            "Tunnel credentials not found at {}.\n\
             Run: cloudflared tunnel create <name>",
            credentials_path.display()
        );
    }

    let cf_config_path = tunnel_cloudflared_config_path()?;
    if let Some(parent) = cf_config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let credentials_str = credentials_path.display().to_string();
    let cf_config = format!(
        "tunnel: {tunnel_id}\ncredentials-file: {credentials_str}\n\ningress:\n  - hostname: {hostname}\n    service: http://127.0.0.1:3012\n  - service: http_status:404\n"
    );
    fs::write(&cf_config_path, &cf_config)
        .with_context(|| format!("failed to write {}", cf_config_path.display()))?;
    println!("Written: {}", cf_config_path.display());

    let plist_path = tunnel_service_plist_path()?;
    if let Some(parent) = plist_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let stdout_path = paths.runtime_dir.join("tunnel.launchd.stdout.log");
    let stderr_path = paths.runtime_dir.join("tunnel.launchd.stderr.log");
    let spec = TunnelServiceSpec {
        cloudflared_bin: cloudflared,
        config_path: cf_config_path,
        stdout_path,
        stderr_path,
        path_env: tool_path_env().to_string_lossy().to_string(),
    };
    let plist = render_tunnel_launchd_plist(&spec);
    fs::write(&plist_path, plist)
        .with_context(|| format!("failed to write {}", plist_path.display()))?;

    let user_target = launchd_user_target()?;
    bootout_davis_service(&user_target, &plist_path);
    run_status(
        Command::new("launchctl")
            .arg("bootstrap")
            .arg(&user_target)
            .arg(&plist_path)
            .env("PATH", tool_path_env()),
        "launchctl bootstrap tunnel service",
    )?;
    run_status(
        Command::new("launchctl")
            .arg("enable")
            .arg(format!("{user_target}/{}", tunnel_service_label()))
            .env("PATH", tool_path_env()),
        "launchctl enable tunnel service",
    )?;
    run_status(
        Command::new("launchctl")
            .arg("kickstart")
            .arg("-k")
            .arg(format!("{user_target}/{}", tunnel_service_label()))
            .env("PATH", tool_path_env()),
        "launchctl kickstart tunnel service",
    )?;

    println!("Tunnel service installed: {}", plist_path.display());
    println!("Waiting for tunnel to come online (up to 10s)...");
    let health_url = format!("https://{hostname}/health");
    let online = wait_for_probe(
        &Probe::Http(health_url.clone()),
        10,
        Duration::from_secs(1),
    )
    .await;
    if online {
        println!("Tunnel online: {hostname}");
    } else {
        println!("Tunnel started (health check timed out — may need a few seconds to propagate)");
        println!("Verify manually: curl {health_url}");
    }
    Ok(())
}

pub(super) fn tunnel_uninstall(_paths: &RuntimePaths) -> Result<()> {
    ensure_macos("Davis tunnel management")?;
    let user_target = launchd_user_target()?;
    let plist_path = tunnel_service_plist_path()?;

    bootout_davis_service(&user_target, &plist_path);

    if plist_path.is_file() {
        fs::remove_file(&plist_path)
            .with_context(|| format!("failed to remove {}", plist_path.display()))?;
        println!("- removed: {}", plist_path.display());
    } else {
        println!("- tunnel plist not found (already uninstalled?)");
    }

    let cf_config_path = tunnel_cloudflared_config_path()?;
    if cf_config_path.is_file() {
        fs::remove_file(&cf_config_path)
            .with_context(|| format!("failed to remove {}", cf_config_path.display()))?;
        println!("- removed: {}", cf_config_path.display());
    } else {
        println!("- cloudflared config not found (already removed?)");
    }

    println!("Tunnel service uninstalled.");
    Ok(())
}

pub(super) async fn tunnel_status(paths: &RuntimePaths) -> Result<()> {
    let plist_path = tunnel_service_plist_path()?;
    if !plist_path.is_file() {
        return Ok(());
    }

    let config = check_local_config(paths).ok();
    let hostname = config
        .as_ref()
        .and_then(|c| c.tunnel.as_ref())
        .and_then(|t| t.hostname.as_deref());

    let user_target = launchd_user_target()?;
    let mut print_cmd = Command::new("launchctl");
    print_cmd
        .arg("print")
        .arg(format!("{user_target}/{}", tunnel_service_label()))
        .env("PATH", tool_path_env());
    let output = command_output(&mut print_cmd).unwrap_or(CommandOutput {
        status_success: false,
        stdout: String::new(),
        stderr: String::new(),
    });

    println!();
    println!("Davis tunnel service");
    println!("- label: {}", tunnel_service_label());
    println!("- plist: {}", plist_path.display());

    if output.status_success {
        let state = launchd_state_label(&output.stdout);
        println!("- launchd: loaded ({state})");
    } else {
        println!("- launchd: stopped");
        return Ok(());
    }

    if let Some(host) = hostname {
        let health_url = format!("https://{host}/health");
        let start = std::time::Instant::now();
        match http_get_text(&health_url).await {
            Ok(_) => {
                let ms = start.elapsed().as_millis();
                println!("- tunnel: running → {host} reachable (latency: {ms}ms)");
            }
            Err(_) => {
                println!("- tunnel: running → {host} unreachable (timeout)");
            }
        }
    } else {
        println!("- tunnel: running (hostname not configured in local.toml)");
    }
    Ok(())
}
