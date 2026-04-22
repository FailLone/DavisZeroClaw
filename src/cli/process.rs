use super::*;
use crate::{HaClient, HaMcpClient, HaState, RuntimePaths};
use anyhow::{anyhow, Context, Result};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::File;
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(super) async fn check_ha(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let client =
        HaClient::from_credentials(&config.home_assistant.url, &config.home_assistant.token)
            .map_err(|err| anyhow!("{err:?}"))?;
    let mcp_client =
        HaMcpClient::from_credentials(&config.home_assistant.url, &config.home_assistant.token)
            .map_err(|err| anyhow!("{err:?}"))?;

    println!("HA config loaded");
    match client.get_value("/api/").await {
        Ok(value) => println!("/api/ ok: {}", value),
        Err(err) => println!("/api/ err: {:?}", err),
    }
    match client.get_value("/api/states").await {
        Ok(value) => {
            let count = value
                .as_array()
                .map(|items| items.len())
                .unwrap_or_default();
            println!("/api/states ok: {} entries", count);
        }
        Err(err) => println!("/api/states err: {:?}", err),
    }
    match client.get_json::<Vec<HaState>>("/api/states").await {
        Ok(states) => println!("/api/states typed ok: {} entries", states.len()),
        Err(err) => println!("/api/states typed err: {:?}", err),
    }
    match mcp_client.capabilities().await {
        Ok(capabilities) => {
            println!(
                "/api/mcp ok: tools={}, prompts={}, live_context={}, audit_history={}",
                capabilities.tools.len(),
                capabilities.prompts.len(),
                capabilities.supports_live_context,
                capabilities.supports_audit_history
            );
        }
        Err(err) => println!("/api/mcp err: {:?}", err),
    }
    match mcp_client.live_context_report().await {
        Ok(report) => println!(
            "/api/mcp live-context ok: lines={}, chars={}, truncated={}",
            report.line_count, report.characters, report.truncated
        ),
        Err(err) => println!("/api/mcp live-context err: {:?}", err),
    }
    Ok(())
}

pub(super) async fn start_process(
    name: &str,
    pid_file: &Path,
    log_file: &Path,
    probe: Probe,
    mut command: Command,
) -> Result<()> {
    if let Some(existing_pid) = read_pid(pid_file) {
        if pid_is_alive(existing_pid) && wait_for_probe(&probe, 2, Duration::from_secs(1)).await {
            println!("{name} is already running. PID: {existing_pid}");
            println!("Log: {}", log_file.display());
            return Ok(());
        }
        let _ = fs::remove_file(pid_file);
    }

    if let Some(parent) = log_file.parent() {
        fs::create_dir_all(parent)?;
    }
    let stdout = File::create(log_file)?;
    let stderr = stdout.try_clone()?;
    let child = command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start {name}"))?;
    let pid = child.id();
    fs::write(pid_file, pid.to_string())?;

    if wait_for_probe(&probe, 15, Duration::from_secs(1)).await {
        println!("{name} started. PID: {pid}");
        println!("Log: {}", log_file.display());
        return Ok(());
    }

    println!("{name} failed to start. Recent log:");
    print_tail(log_file, 120);
    bail!("{name} failed to start");
}

pub(super) fn stop_process(name: &str, pid_file: &Path) -> Result<()> {
    if !pid_file.is_file() {
        println!("{name} is not running.");
        return Ok(());
    }

    let Some(pid) = read_pid(pid_file) else {
        println!("{name} has an invalid PID file; removing it.");
        fs::remove_file(pid_file)?;
        return Ok(());
    };

    if pid_is_alive(pid) {
        terminate_pid(pid)?;
        println!("Stopped {name}. PID: {pid}");
    } else {
        println!("{name} PID file is stale; removing it.");
    }
    fs::remove_file(pid_file)?;
    Ok(())
}

pub(super) async fn wait_for_probe(probe: &Probe, attempts: usize, delay: Duration) -> bool {
    for _ in 0..attempts {
        let ready = match probe {
            Probe::Http(url) => http_get_text(url).await.is_ok(),
            Probe::HttpAndPort(url, port) => http_get_text(url).await.is_ok() && port_ready(*port),
        };
        if ready {
            return true;
        }
        tokio::time::sleep(delay).await;
    }
    false
}

pub(super) async fn http_get_text(url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        bail!("GET {url} returned {}", response.status());
    }
    Ok(response.text().await?)
}

pub(super) fn port_ready(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok()
}

pub(super) fn read_pid(pid_file: &Path) -> Option<u32> {
    fs::read_to_string(pid_file).ok()?.trim().parse().ok()
}

pub(super) fn pid_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .is_ok_and(|status| status.success())
}

pub(super) fn terminate_pid(pid: u32) -> Result<()> {
    run_status(
        Command::new("kill").arg(pid.to_string()),
        &format!("kill {pid}"),
    )?;
    Ok(())
}

pub(super) fn run_status(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to run {description}"))?;
    if !status.success() {
        bail!("{description} failed with status {status}");
    }
    Ok(())
}

pub(super) fn run_status_filtering_shortcuts_warnings(command: &mut Command, description: &str) -> Result<()> {
    let output = command_output(command).with_context(|| format!("failed to run {description}"))?;
    print_command_streams(
        &output.stdout,
        &filter_known_shortcuts_warnings(&output.stderr),
    );
    if !output.status_success {
        bail!("{description} failed");
    }
    Ok(())
}

pub(super) fn print_command_streams(stdout: &str, stderr: &str) {
    if !stdout.is_empty() {
        print!("{stdout}");
        if !stdout.ends_with('\n') {
            println!();
        }
    }
    if !stderr.is_empty() {
        eprint!("{stderr}");
        if !stderr.ends_with('\n') {
            eprintln!();
        }
    }
}

pub(super) fn filter_known_shortcuts_warnings(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| {
            !(line.contains("Unrecognized attribute string flag '?'")
                && line.contains("property debugDescription"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug)]
pub(super) struct CommandOutput {
    pub(super) status_success: bool,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

pub(super) fn command_output(command: &mut Command) -> Result<CommandOutput> {
    let output = command.output()?;
    Ok(CommandOutput {
        status_success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

pub(super) fn command_text(command: &mut Command) -> Result<String> {
    let output = command_output(command)?;
    if !output.status_success {
        bail!("{}", output.stderr);
    }
    Ok(output.stdout)
}

pub(super) fn print_tail(path: &Path, max_lines: usize) {
    if let Ok(raw) = fs::read_to_string(path) {
        let lines = raw.lines().collect::<Vec<_>>();
        let start = lines.len().saturating_sub(max_lines);
        for line in &lines[start..] {
            println!("{line}");
        }
    }
}

pub(super) fn report_agent_browser_status() {
    let brew = command_path("brew");
    if let Some(brew) = brew {
        if Command::new(brew)
            .arg("list")
            .arg("--versions")
            .arg("agent-browser")
            .env("PATH", tool_path_env())
            .status()
            .is_ok_and(|status| status.success())
        {
            println!("agent-browser: installed through Homebrew.");
            return;
        }
    }

    if command_path("agent-browser").is_some() {
        println!("agent-browser: found on PATH. Homebrew install is preferred: brew install agent-browser");
    } else {
        println!("agent-browser: not found. Optional install: brew install agent-browser");
        println!("If this Mac has no reusable Chrome or Chromium, also run: agent-browser install");
    }
}

pub(super) fn report_skill_inventory(paths: &RuntimePaths) {
    let project_skills_dir = paths.repo_root.join("project-skills");
    let vendor_skills_dir = paths.repo_root.join("skills");
    let project_skill_count = count_skills(&project_skills_dir);
    let vendor_skill_names = skill_names(&vendor_skills_dir);

    println!(
        "Project skills: {} ({})",
        project_skill_count,
        project_skills_dir.display()
    );
    if !vendor_skill_names.is_empty() {
        println!("Vendor skills: {}", vendor_skill_names.join(", "));
    }
}

pub(super) fn count_skills(root: &Path) -> usize {
    skill_names(root).len()
}

pub(super) fn skill_names(root: &Path) -> Vec<String> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut names = fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| {
            entry.path().join("SKILL.md").is_file() || entry.path().join("SKILL.toml").is_file()
        })
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    names.sort();
    names
}

pub(super) fn skill_name_set(root: &Path) -> BTreeSet<String> {
    skill_names(root).into_iter().collect()
}

pub(super) fn sop_names(root: &Path) -> Vec<String> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut names = fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| entry.path().join("SOP.toml").is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    names.sort();
    names
}

pub(super) fn sop_name_set(root: &Path) -> BTreeSet<String> {
    sop_names(root).into_iter().collect()
}

pub(super) fn release_bin_path(paths: &RuntimePaths, name: &str) -> PathBuf {
    let mut bin = paths.repo_root.join("target").join("release").join(name);
    if cfg!(windows) {
        bin.set_extension("exe");
    }
    bin
}

pub(super) fn ensure_macos(feature: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        bail!("{feature} is only supported on macOS")
    }
}

pub(super) fn require_command(name: &str) -> Result<PathBuf> {
    command_path(name).ok_or_else(|| anyhow!("command not found: {name}"))
}

pub(super) fn command_path(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 && candidate.is_file() {
        return Some(candidate.to_path_buf());
    }

    for dir in std::env::split_paths(&tool_path_env()) {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

pub(crate) fn tool_path_env() -> OsString {
    let mut paths = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    if let Some(current) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&current) {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    std::env::join_paths(paths)
        .unwrap_or_else(|_| OsString::from("/opt/homebrew/bin:/usr/local/bin"))
}

pub(super) fn pseudo_uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:08X}-{:04X}-4{:03X}-A{:03X}-{:012X}",
        (nanos & 0xffff_ffff) as u32,
        ((nanos >> 32) & 0xffff) as u16,
        ((nanos >> 48) & 0x0fff) as u16,
        ((nanos >> 60) & 0x0fff) as u16,
        ((nanos >> 72) & 0xffff_ffff_ffff) as u64
    )
}

pub(super) fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}.{}", std::process::id(), nanos)
}

pub(super) struct CleanupFiles(pub(super) Vec<PathBuf>);

impl Drop for CleanupFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

