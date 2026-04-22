use super::*;
use crate::{check_local_config, RuntimePaths};
use anyhow::{anyhow, Context, Result};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub(super) async fn start_runtime_daemon(
    paths: &RuntimePaths,
    zeroclaw: &Path,
    provider_env: &[(String, String)],
) -> Result<()> {
    if let Some(existing_pid) = read_pid(&paths.daemon_pid_path()) {
        if pid_is_alive(existing_pid)
            && wait_for_probe(
                &Probe::HttpAndPort("http://127.0.0.1:3000/health".to_string(), 3001),
                2,
                Duration::from_secs(1),
            )
            .await
        {
            println!("ZeroClaw Daemon is already running. PID: {existing_pid}");
            println!("Log: {}", paths.daemon_log_path().display());
            return Ok(());
        }
        let _ = fs::remove_file(paths.daemon_pid_path());
    }

    let mut cmd = Command::new(zeroclaw);
    cmd.arg("daemon")
        .arg("--config-dir")
        .arg(&paths.runtime_dir)
        .env("PATH", tool_path_env())
        .current_dir(&paths.repo_root);
    for (key, value) in provider_env {
        cmd.env(key, value);
    }

    start_process(
        "ZeroClaw Daemon",
        &paths.daemon_pid_path(),
        &paths.daemon_log_path(),
        Probe::HttpAndPort("http://127.0.0.1:3000/health".to_string(), 3001),
        cmd,
    )
    .await
}

pub fn sync_runtime_skills(paths: &RuntimePaths) -> Result<()> {
    let project_skills_dir = std::env::var_os("DAVIS_PROJECT_SKILLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.repo_root.join("project-skills"));
    let vendor_skills_dir = std::env::var_os("DAVIS_VENDOR_SKILLS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.repo_root.join("skills"));
    sync_runtime_skills_with_sources(paths, &project_skills_dir, &vendor_skills_dir)
}

pub fn sync_runtime_sops(paths: &RuntimePaths) -> Result<()> {
    let project_sops_dir = std::env::var_os("DAVIS_PROJECT_SOPS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.repo_root.join("project-sops"));
    sync_runtime_sops_with_sources(paths, &project_sops_dir)
}

/// Copy personality files (TOOLS.md, SOUL.md, etc.) from `project-workspace/`
/// into the runtime workspace root so ZeroClaw loads them at startup.
pub fn sync_workspace_files(paths: &RuntimePaths) -> Result<()> {
    let project_workspace_dir = std::env::var_os("DAVIS_PROJECT_WORKSPACE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.repo_root.join("project-workspace"));
    if !project_workspace_dir.is_dir() {
        return Ok(());
    }
    let workspace_dir = paths.workspace_dir();
    fs::create_dir_all(&workspace_dir)?;
    let mut count = 0u32;
    for entry in fs::read_dir(&project_workspace_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let dest = workspace_dir.join(entry.file_name());
            fs::copy(&path, &dest)?;
            count += 1;
        }
    }
    if count > 0 {
        println!(
            "Workspace files synced: {} file(s) from {}",
            count,
            project_workspace_dir.display()
        );
    }
    Ok(())
}

pub(super) fn sync_runtime_skills_with_sources(
    paths: &RuntimePaths,
    project_skills_dir: &Path,
    vendor_skills_dir: &Path,
) -> Result<()> {
    let workspace_dir = paths.workspace_dir();
    let runtime_skills_dir = paths.workspace_skills_dir();
    let staging_dir = workspace_dir.join("skills.staging");

    fs::create_dir_all(&workspace_dir)?;
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir)?;
    }
    fs::create_dir_all(&staging_dir)?;

    copy_skill_tree(project_skills_dir, "project-skills", &staging_dir)?;
    copy_skill_tree(vendor_skills_dir, "skills", &staging_dir)?;

    if runtime_skills_dir.exists() {
        fs::remove_dir_all(&runtime_skills_dir)?;
    }
    fs::rename(&staging_dir, &runtime_skills_dir)?;

    println!("Runtime skills synced: {}", runtime_skills_dir.display());
    Ok(())
}

pub(super) fn sync_runtime_sops_with_sources(
    paths: &RuntimePaths,
    project_sops_dir: &Path,
) -> Result<()> {
    let workspace_dir = paths.workspace_dir();
    let runtime_sops_dir = paths.workspace_sops_dir();
    let staging_dir = workspace_dir.join("sops.staging");

    fs::create_dir_all(&workspace_dir)?;
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir)?;
    }
    fs::create_dir_all(&staging_dir)?;

    copy_sop_tree(project_sops_dir, "project-sops", &staging_dir)?;

    if runtime_sops_dir.exists() {
        fs::remove_dir_all(&runtime_sops_dir)?;
    }
    fs::rename(&staging_dir, &runtime_sops_dir)?;

    let synced_count = sop_name_set(&runtime_sops_dir).len();
    if synced_count == 0 {
        println!("Runtime SOPs: 0 synced to {}", runtime_sops_dir.display());
        println!(
            "  hint: add a SOP directory under {} containing a SOP.toml to activate it",
            project_sops_dir.display()
        );
    } else {
        println!(
            "Runtime SOPs synced: {} ({} {})",
            runtime_sops_dir.display(),
            synced_count,
            if synced_count == 1 { "SOP" } else { "SOPs" }
        );
    }
    Ok(())
}

pub(super) fn copy_skill_tree(
    source_root: &Path,
    source_label: &str,
    staging_dir: &Path,
) -> Result<()> {
    if !source_root.is_dir() {
        return Ok(());
    }

    let mut entries = fs::read_dir(source_root)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let skill_path = entry.path();
        if !skill_path.is_dir() {
            continue;
        }
        if !skill_path.join("SKILL.md").is_file() && !skill_path.join("SKILL.toml").is_file() {
            continue;
        }
        let skill_name = skill_path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| anyhow!("invalid skill path: {}", skill_path.display()))?;
        let dest_path = staging_dir.join(skill_name);
        if dest_path.exists() {
            bail!(
                "duplicate skill name detected: {skill_name}\n   source directory: {}\n   existing destination: {}\n   Keep project skills and skills.sh vendor skills under distinct names.",
                source_root.display(),
                dest_path.display()
            );
        }

        copy_dir_recursive(&skill_path, &dest_path)?;
        sanitize_markdown_links_in_dir(&dest_path)?;
        fs::write(
            dest_path.join(".davis-skill-source"),
            format!("{source_label}\n"),
        )?;
    }

    Ok(())
}

pub(super) fn copy_sop_tree(
    source_root: &Path,
    source_label: &str,
    staging_dir: &Path,
) -> Result<()> {
    if !source_root.is_dir() {
        return Ok(());
    }

    let mut entries = fs::read_dir(source_root)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let sop_path = entry.path();
        if !sop_path.is_dir() {
            continue;
        }
        if !sop_path.join("SOP.toml").is_file() {
            continue;
        }

        let sop_name = sop_path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| anyhow!("invalid SOP path: {}", sop_path.display()))?;
        let dest_path = staging_dir.join(sop_name);
        if dest_path.exists() {
            bail!(
                "duplicate SOP name detected: {sop_name}\n   source directory: {}\n   existing destination: {}",
                source_root.display(),
                dest_path.display()
            );
        }

        copy_dir_recursive(&sop_path, &dest_path)?;
        sanitize_markdown_links_in_dir(&dest_path)?;
        fs::write(
            dest_path.join(".davis-sop-source"),
            format!("{source_label}\n"),
        )?;
    }

    Ok(())
}

pub(super) fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&source_path, &dest_path)?;
        } else {
            fs::copy(&source_path, &dest_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source_path.display(),
                    dest_path.display()
                )
            })?;
        }
    }
    Ok(())
}

pub(super) fn sanitize_markdown_links_in_dir(root: &Path) -> Result<()> {
    for path in collect_files(root)? {
        if path.extension().and_then(OsStr::to_str) != Some("md") {
            continue;
        }
        let raw = fs::read_to_string(&path)?;
        let sanitized = sanitize_markdown_script_links(&raw);
        if sanitized != raw {
            fs::write(&path, sanitized)?;
        }
    }
    Ok(())
}

pub(super) fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.exists() {
        return Ok(files);
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            files.extend(collect_files(&path)?);
        } else {
            files.push(path);
        }
    }
    Ok(files)
}

pub fn sanitize_markdown_script_links(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;

    while let Some(open_rel) = input[cursor..].find('[') {
        let open = cursor + open_rel;
        let Some(close_text_rel) = input[open + 1..].find("](") else {
            break;
        };
        let text_end = open + 1 + close_text_rel;
        let url_start = text_end + 2;
        let Some(url_end_rel) = input[url_start..].find(')') else {
            break;
        };
        let url_end = url_start + url_end_rel;
        let label = &input[open + 1..text_end];
        let url = &input[url_start..url_end];

        output.push_str(&input[cursor..open]);
        if is_script_link(url) {
            output.push_str(label);
        } else {
            output.push_str(&input[open..=url_end]);
        }
        cursor = url_end + 1;
    }

    output.push_str(&input[cursor..]);
    output
}

pub(super) fn is_script_link(url: &str) -> bool {
    let target = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim()
        .to_ascii_lowercase();
    [".sh", ".bash", ".zsh", ".ps1"]
        .iter()
        .any(|suffix| target.ends_with(suffix))
}

pub(super) fn install_skills(paths: &RuntimePaths) -> Result<()> {
    let installed = install_mempalace_vendor_skill(paths)?;

    println!("Installed vendor skills:");
    println!("- mempalace ({})", installed.display());
    println!("Next: daviszeroclaw skills sync");
    Ok(())
}

pub(super) fn install_mempalace_vendor_skill(paths: &RuntimePaths) -> Result<PathBuf> {
    let skill_dir = paths.repo_root.join("skills").join("mempalace");
    fs::create_dir_all(&skill_dir)
        .with_context(|| format!("failed to create {}", skill_dir.display()))?;
    fs::write(
        skill_dir.join("SKILL.md"),
        render_mempalace_vendor_skill_adapter(paths),
    )
    .with_context(|| format!("failed to write {}", skill_dir.join("SKILL.md").display()))?;
    Ok(skill_dir)
}

pub(super) fn render_mempalace_vendor_skill_adapter(paths: &RuntimePaths) -> String {
    let python = paths.mempalace_python_path();
    format!(
        r#"---
name: mempalace
description: "MemPalace maintenance skill for DavisZeroClaw. Use only when the user explicitly asks to operate MemPalace itself: setup, init, mine external data, direct manual search, status, repair, MCP setup, or CLI help. For everyday Davis long-term personal memory and cross-session recall, use the project skill mempalace-memory and MemPalace MCP tools instead."
---

# MemPalace

This is a thin Davis/ZeroClaw adapter for MemPalace's official dynamic instructions. Use it only for MemPalace maintenance operations. It is not the daily memory path; that belongs to the `mempalace-memory` project skill plus MemPalace MCP tools.

## Official Instructions

MemPalace provides dynamic official instructions through its CLI. From this DavisZeroClaw repo, prefer the Davis-managed Python:

```bash
{} -m mempalace instructions <command>
```

Use one of these commands:

- `help`: available MemPalace commands and capabilities.
- `init`: initialize a palace.
- `mine`: mine projects or conversation exports into the palace.
- `search`: directly search the user's palace.
- `status`: show palace health, counts, and stats.

Read the returned instructions and follow them step by step.

Do not initialize or mine the DavisZeroClaw repository by default. DavisZeroClaw is the user's agent runtime, not the primary memory corpus. Only run `init` or `mine` when the user explicitly provides a source directory or asks to ingest data.

## MCP First

When MemPalace MCP tools are available, prefer them over shell commands for day-to-day recall, status, knowledge graph, and diary operations. Use CLI instructions only for maintenance workflows or explicit direct search.

## Boundaries

- If the user asks Davis to remember, recall, correct, forget, or preserve long-term facts, use the `mempalace-memory` project skill.
- If the user asks how to operate MemPalace itself, use this vendor skill.
- Do not install the generic upstream `search`, `status`, or `help` skills as top-level skills; this single `mempalace` entry is the vendor boundary.
"#,
        python.display()
    )
}

pub(super) fn check_skills(paths: &RuntimePaths) -> Result<()> {
    let project_skills_dir = paths.repo_root.join("project-skills");
    let vendor_skills_dir = paths.repo_root.join("skills");
    let runtime_skills_dir = paths.workspace_skills_dir();

    let project_names = skill_name_set(&project_skills_dir);
    let vendor_names = skill_name_set(&vendor_skills_dir);
    let runtime_names = skill_name_set(&runtime_skills_dir);
    let duplicates = project_names
        .intersection(&vendor_names)
        .cloned()
        .collect::<Vec<_>>();

    println!(
        "Project skills: {} ({})",
        format_skill_count(project_names.len()),
        project_skills_dir.display()
    );
    println!(
        "Vendor skills: {} ({})",
        format_skill_count(vendor_names.len()),
        vendor_skills_dir.display()
    );
    println!(
        "Runtime skills: {}",
        runtime_skill_status(&project_names, &vendor_names, &runtime_names)
    );

    if duplicates.is_empty() {
        println!("Duplicate names: none");
    } else {
        println!("Duplicate names: WARN ({})", duplicates.join(", "));
    }

    report_mempalace_vendor_skill_status(&vendor_skills_dir);
    report_mempalace_policy_skill_status(&project_skills_dir);
    report_mempalace_mcp_status(paths);

    Ok(())
}

pub(super) fn check_sops(paths: &RuntimePaths) -> Result<()> {
    let project_sops_dir = paths.repo_root.join("project-sops");
    let runtime_sops_dir = paths.workspace_sops_dir();
    let project_names = sop_name_set(&project_sops_dir);
    let runtime_names = sop_name_set(&runtime_sops_dir);

    println!(
        "Project SOPs: {} ({})",
        format_sop_count(project_names.len()),
        project_sops_dir.display()
    );
    println!(
        "Runtime SOPs: {}",
        runtime_sop_status(&project_names, &runtime_names)
    );

    if project_names.is_empty() && runtime_names.is_empty() {
        println!(
            "  hint: project-sops/ is empty. ZeroClaw will run without any runbooks — \
that's fine for most setups. To add one, create project-sops/<name>/SOP.toml \
and run `daviszeroclaw sops sync`."
        );
        return Ok(());
    }

    let zeroclaw = require_command("zeroclaw")
        .context("zeroclaw was not found. Install it first: brew install zeroclaw")?;
    run_status(
        Command::new(zeroclaw)
            .arg("sop")
            .arg("validate")
            .arg("--config-dir")
            .arg(&paths.runtime_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "zeroclaw sop validate",
    )?;

    Ok(())
}

pub(super) fn format_skill_count(count: usize) -> String {
    if count == 1 {
        "ok (1 skill)".to_string()
    } else {
        format!("ok ({count} skills)")
    }
}

pub(super) fn format_sop_count(count: usize) -> String {
    if count == 1 {
        "ok (1 SOP)".to_string()
    } else {
        format!("ok ({count} SOPs)")
    }
}

pub(super) fn runtime_skill_status(
    project_names: &BTreeSet<String>,
    vendor_names: &BTreeSet<String>,
    runtime_names: &BTreeSet<String>,
) -> String {
    let expected = project_names
        .union(vendor_names)
        .cloned()
        .collect::<BTreeSet<_>>();
    if expected.is_empty() && runtime_names.is_empty() {
        return "ok (empty)".to_string();
    }
    let missing = expected
        .difference(runtime_names)
        .cloned()
        .collect::<Vec<_>>();
    let extra = runtime_names
        .difference(&expected)
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() && extra.is_empty() {
        return format!("synced ({} skills)", runtime_names.len());
    }

    let mut parts = Vec::new();
    if !missing.is_empty() {
        parts.push(format!("missing: {}", missing.join(", ")));
    }
    if !extra.is_empty() {
        parts.push(format!("extra: {}", extra.join(", ")));
    }
    format!("WARN stale ({})", parts.join("; "))
}

pub(super) fn runtime_sop_status(
    project_names: &BTreeSet<String>,
    runtime_names: &BTreeSet<String>,
) -> String {
    if project_names.is_empty() && runtime_names.is_empty() {
        return "ok (empty)".to_string();
    }
    let missing = project_names
        .difference(runtime_names)
        .cloned()
        .collect::<Vec<_>>();
    let extra = runtime_names
        .difference(project_names)
        .cloned()
        .collect::<Vec<_>>();
    if missing.is_empty() && extra.is_empty() {
        return if runtime_names.len() == 1 {
            "synced (1 SOP)".to_string()
        } else {
            format!("synced ({} SOPs)", runtime_names.len())
        };
    }

    let mut parts = Vec::new();
    if !missing.is_empty() {
        parts.push(format!("missing: {}", missing.join(", ")));
    }
    if !extra.is_empty() {
        parts.push(format!("extra: {}", extra.join(", ")));
    }
    format!("WARN stale ({})", parts.join("; "))
}

pub(super) fn report_mempalace_vendor_skill_status(vendor_skills_dir: &Path) {
    let skill_path = vendor_skills_dir.join("mempalace").join("SKILL.md");
    if !skill_path.is_file() {
        println!("MemPalace vendor skill: WARN missing (run: daviszeroclaw skills install)");
        return;
    }

    match fs::read_to_string(&skill_path) {
        Ok(raw)
            if raw.contains("mempalace instructions <command>")
                && raw.contains("project skill mempalace-memory") =>
        {
            println!("MemPalace vendor skill: ok");
        }
        Ok(_) => {
            println!(
                "MemPalace vendor skill: WARN installed but does not look like the Davis vendor wrapper"
            );
        }
        Err(error) => {
            println!("MemPalace vendor skill: WARN failed to read ({error})");
        }
    }
}

pub(super) fn report_mempalace_policy_skill_status(project_skills_dir: &Path) {
    let skill_path = project_skills_dir.join("mempalace-memory").join("SKILL.md");
    if !skill_path.is_file() {
        println!("MemPalace memory policy skill: WARN missing");
        return;
    }

    let required_markers = [
        "Use MemPalace When",
        "Do Not Use MemPalace When",
        "Runtime Protocol",
        "Load the protocol",
        "Read before answering",
        "Write deliberately",
        "Placement",
        "Boundary With ZeroClaw Memory",
        "vendor `mempalace` skill",
    ];
    match fs::read_to_string(&skill_path) {
        Ok(raw) => {
            let missing = required_markers
                .iter()
                .filter(|marker| !raw.contains(**marker))
                .copied()
                .collect::<Vec<_>>();
            if missing.is_empty() {
                println!("MemPalace memory policy skill: ok");
            } else {
                println!(
                    "MemPalace memory policy skill: WARN too vague (missing: {})",
                    missing.join(", ")
                );
            }
        }
        Err(error) => {
            println!("MemPalace memory policy skill: WARN failed to read ({error})");
        }
    }
}

pub(super) fn report_mempalace_mcp_status(paths: &RuntimePaths) {
    let config = match check_local_config(paths) {
        Ok(config) => config,
        Err(error) => {
            println!("MemPalace MCP: WARN config invalid ({error})");
            return;
        }
    };
    let Some(server) = super::mempalace::find_mempalace_server(&config.mcp.servers) else {
        println!("MemPalace MCP: WARN not configured (run: daviszeroclaw memory mempalace enable)");
        return;
    };
    let python = PathBuf::from(&server.command);
    if !python.is_file() {
        println!(
            "MemPalace MCP: WARN Python missing at {} (run: daviszeroclaw memory mempalace install)",
            python.display()
        );
        return;
    }

    let import_check = command_output(
        Command::new(&python)
            .arg("-c")
            .arg("import mempalace")
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
    );
    match import_check {
        Ok(output) if output.status_success => {}
        Ok(output) => {
            let detail = first_non_empty_line(&output.stderr).unwrap_or("import failed");
            println!("MemPalace MCP: WARN package import failed ({detail})");
            return;
        }
        Err(error) => {
            println!("MemPalace MCP: WARN import check failed ({error})");
            return;
        }
    }

    let help_check = command_output(
        Command::new(&python)
            .arg("-m")
            .arg("mempalace.mcp_server")
            .arg("--help")
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
    );
    match help_check {
        Ok(output) if output.status_success => println!("MemPalace MCP: ok"),
        Ok(output) => {
            let detail = first_non_empty_line(&output.stderr).unwrap_or("mcp server --help failed");
            println!("MemPalace MCP: WARN unavailable ({detail})");
        }
        Err(error) => println!("MemPalace MCP: WARN check failed ({error})"),
    }
}
