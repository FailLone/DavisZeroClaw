use super::*;
use crate::RuntimePaths;
use anyhow::{Context, Result};
use chrono::Local;
use std::fs;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

const LOG_ROTATE_MAX_BYTES: u64 = 10 * 1024 * 1024;
const LOG_ROTATE_KEEP_DAYS: u64 = 3;

#[derive(Debug, Clone)]
pub(super) struct LogFile {
    component: LogComponent,
    label: &'static str,
    path: PathBuf,
    default_view: bool,
}

pub(super) fn runtime_log_files(paths: &RuntimePaths) -> Vec<LogFile> {
    vec![
        LogFile {
            component: LogComponent::Proxy,
            label: "Davis proxy stderr",
            path: paths.runtime_dir.join("proxy.launchd.stderr.log"),
            default_view: true,
        },
        LogFile {
            component: LogComponent::Proxy,
            label: "Davis proxy stdout",
            path: paths.runtime_dir.join("proxy.launchd.stdout.log"),
            default_view: false,
        },
        LogFile {
            component: LogComponent::Crawl4ai,
            label: "Crawl4AI adapter",
            path: paths.crawl4ai_log_path(),
            default_view: true,
        },
        LogFile {
            component: LogComponent::Zeroclaw,
            label: "ZeroClaw stderr",
            path: paths.runtime_dir.join("daemon.launchd.stderr.log"),
            default_view: true,
        },
        LogFile {
            component: LogComponent::Zeroclaw,
            label: "ZeroClaw stdout",
            path: paths.runtime_dir.join("daemon.launchd.stdout.log"),
            default_view: false,
        },
        LogFile {
            component: LogComponent::Proxy,
            label: "Foreground proxy",
            path: paths.local_proxy_log_path(),
            default_view: false,
        },
        LogFile {
            component: LogComponent::Zeroclaw,
            label: "Foreground ZeroClaw",
            path: paths.daemon_log_path(),
            default_view: false,
        },
        LogFile {
            component: LogComponent::Tunnel,
            label: "Tunnel stderr",
            path: paths.runtime_dir.join("tunnel.launchd.stderr.log"),
            default_view: false,
        },
        LogFile {
            component: LogComponent::Tunnel,
            label: "Tunnel stdout",
            path: paths.runtime_dir.join("tunnel.launchd.stdout.log"),
            default_view: false,
        },
    ]
}

pub(super) fn rotate_runtime_logs(paths: &RuntimePaths) -> Result<()> {
    for log in runtime_log_files(paths) {
        rotate_one_log(&log.path, LOG_ROTATE_MAX_BYTES, LOG_ROTATE_KEEP_DAYS)?;
    }
    Ok(())
}

fn rotate_one_log(path: &Path, max_bytes: u64, keep_days: u64) -> Result<()> {
    prune_archived_logs(path, keep_days)?;
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() <= max_bytes {
        return Ok(());
    }

    let rotated = rotated_path(path);
    fs::rename(path, &rotated)
        .with_context(|| format!("rotate log {} -> {}", path.display(), rotated.display()))?;
    prune_archived_logs(path, keep_days)?;
    Ok(())
}

fn rotated_path(path: &Path) -> PathBuf {
    let day = Local::now().format("%Y-%m-%d").to_string();
    let base = PathBuf::from(format!("{}.{}", path.display(), day));
    if !base.exists() {
        return base;
    }
    let timestamp = Local::now().format("%Y-%m-%d-%H%M%S").to_string();
    PathBuf::from(format!("{}.{}", path.display(), timestamp))
}

fn prune_archived_logs(path: &Path, keep_days: u64) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let prefix = format!("{file_name}.");
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(keep_days * 24 * 60 * 60))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::now());
        if modified < cutoff {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

pub(super) fn show_logs(
    paths: &RuntimePaths,
    component: LogComponent,
    tail: usize,
    follow: bool,
    paths_only: bool,
    clear: bool,
) -> Result<()> {
    let files = selected_logs(paths, component);
    print_log_guide(paths, component, &files);
    if clear {
        clear_logs(&files)?;
        return Ok(());
    }
    if paths_only {
        return Ok(());
    }
    let filter = line_filter(component);
    if follow {
        follow_logs(&files, tail, filter)
    } else {
        print_logs(&files, tail, filter);
        Ok(())
    }
}

fn clear_logs(files: &[LogFile]) -> Result<()> {
    for file in files {
        if let Some(parent) = file.path.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&file.path)
            .with_context(|| format!("truncate {}", file.path.display()))?;
        let removed = remove_archived_logs(&file.path)?;
        println!(
            "cleared {} (removed {removed} archived file{})",
            file.path.display(),
            if removed == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

fn remove_archived_logs(path: &Path) -> Result<usize> {
    let Some(parent) = path.parent() else {
        return Ok(0);
    };
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(0);
    };
    let prefix = format!("{file_name}.");
    let mut removed = 0;
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(&prefix) {
            if fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

fn selected_logs(paths: &RuntimePaths, component: LogComponent) -> Vec<LogFile> {
    let files = runtime_log_files(paths);
    match component {
        LogComponent::All => files
            .into_iter()
            .filter(|file| file.default_view)
            .collect::<Vec<_>>(),
        LogComponent::RouterDhcp => files
            .into_iter()
            .filter(|file| file.component == LogComponent::Proxy && file.label.contains("stderr"))
            .collect::<Vec<_>>(),
        other => files
            .into_iter()
            .filter(|file| file.component == other)
            .collect::<Vec<_>>(),
    }
}

fn line_filter(component: LogComponent) -> Option<&'static str> {
    match component {
        LogComponent::RouterDhcp => Some("router-dhcp"),
        LogComponent::Crawl4ai => Some("crawl4ai"),
        _ => None,
    }
}

fn print_log_guide(paths: &RuntimePaths, component: LogComponent, files: &[LogFile]) {
    println!("Davis logs");
    println!("- runtime: {}", paths.runtime_dir.display());
    println!("- status:  daviszeroclaw service status");
    println!("- follow:  daviszeroclaw logs --follow");
    println!();
    println!("Selected logs ({component:?}):");
    for file in files {
        let size = fs::metadata(&file.path)
            .map(|meta| human_bytes(meta.len()))
            .unwrap_or_else(|_| "missing".to_string());
        println!("- {}: {} ({size})", file.label, file.path.display());
    }
    println!();
}

fn print_logs(files: &[LogFile], tail: usize, filter: Option<&str>) {
    for file in files {
        println!("==> {} <==", file.path.display());
        match tail_lines(&file.path, tail, filter) {
            Ok(lines) if lines.is_empty() => println!("(no matching lines)"),
            Ok(lines) => {
                for line in lines {
                    println!("{line}");
                }
            }
            Err(err) => println!("(unavailable: {err})"),
        }
        println!();
    }
}

fn tail_lines(path: &Path, max_lines: usize, filter: Option<&str>) -> Result<Vec<String>> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut lines = raw
        .lines()
        .filter(|line| filter.is_none_or(|needle| line.contains(needle)))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let start = lines.len().saturating_sub(max_lines);
    Ok(lines.split_off(start))
}

fn follow_logs(files: &[LogFile], tail: usize, filter: Option<&str>) -> Result<()> {
    let tail_bin = require_command("tail").context("tail command not found")?;
    let mut command = Command::new(tail_bin);
    command.arg("-n").arg(tail.to_string()).arg("-f");
    for file in files {
        command.arg(&file.path);
    }
    let mut child = command
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to start tail -f")?;
    let Some(stdout) = child.stdout.take() else {
        return Ok(());
    };
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = line?;
        if filter.is_none_or(|needle| line.contains(needle)) {
            println!("{line}");
        }
    }
    Ok(())
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
