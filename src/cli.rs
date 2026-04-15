use crate::{
    check_local_config, zeroclaw_env_vars, BrowserBridgeConfig, HaClient, HaMcpClient, HaState,
    RuntimePaths,
};
use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ALI_ORDER_URL: &str = "https://buyertrade.taobao.com/trade/itemlist/list_bought_items.htm";
const JD_ORDER_URL: &str = "https://order.jd.com/center/list.action";

#[derive(Debug, Parser)]
#[command(name = "daviszeroclaw")]
#[command(about = "Unified DavisZeroClaw operations CLI")]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start Davis local services and the ZeroClaw daemon.
    Start,
    /// Stop Davis local services and known ZeroClaw child services.
    Stop,
    /// Manage runtime skills.
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
    /// Build and customize the iOS Shortcut.
    Shortcut {
        #[command(subcommand)]
        command: ShortcutCommand,
    },
    /// Check iMessage runtime permissions.
    Imessage {
        #[command(subcommand)]
        command: ImessageCommand,
    },
    /// Open express provider login pages.
    Express {
        #[command(subcommand)]
        command: ExpressCommand,
    },
    /// Inspect Davis configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Check Home Assistant connectivity.
    Ha {
        #[command(subcommand)]
        command: HaCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SkillsCommand {
    /// Synchronize project and vendor skills into the runtime workspace.
    Sync,
    /// Install the default vendor skills, then synchronize runtime skills.
    InstallVendor,
}

#[derive(Debug, Subcommand)]
enum ShortcutCommand {
    /// Build a signed Shortcut customized for the current Davis host.
    Build {
        #[arg(long)]
        url: Option<String>,
        #[arg(long, conflicts_with = "no_secret")]
        secret: Option<String>,
        #[arg(long)]
        no_secret: bool,
    },
    /// Build the Shortcut and open the macOS import flow.
    Install {
        #[arg(long)]
        url: Option<String>,
        #[arg(long, conflicts_with = "no_secret")]
        secret: Option<String>,
        #[arg(long)]
        no_secret: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ImessageCommand {
    /// Check Messages.app database and Apple Events permissions.
    CheckPermissions,
    /// Inspect local iMessage account and suggest allowed_contacts.
    Inspect,
}

#[derive(Debug, Subcommand)]
enum ExpressCommand {
    /// Open a provider login page in Google Chrome.
    Login {
        #[arg(value_enum)]
        source: ExpressLoginSource,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ExpressLoginSource {
    Ali,
    Jd,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Validate config/davis/local.toml.
    Check,
}

#[derive(Debug, Subcommand)]
enum HaCommand {
    /// Check Home Assistant REST and MCP connectivity.
    Check,
}

#[derive(Debug, Clone)]
enum Probe {
    Http(String),
    HttpAndPort(String, u16),
}

#[derive(Debug, Clone)]
struct ShortcutBuild {
    output_shortcut: PathBuf,
}

#[derive(Debug, Clone)]
struct ImessageAllowedContactCandidate {
    identity: String,
    messages: usize,
    incoming: usize,
    outgoing: usize,
    max_rowid: i64,
    last_seen_local: String,
    reason: String,
}

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    run_cli(cli).await
}

pub async fn run_cli(cli: Cli) -> Result<()> {
    let paths = RuntimePaths::from_env();
    match cli.command {
        Commands::Start => start(&paths).await,
        Commands::Stop => stop(&paths),
        Commands::Skills { command } => match command {
            SkillsCommand::Sync => sync_runtime_skills(&paths),
            SkillsCommand::InstallVendor => install_vendor_skills(&paths),
        },
        Commands::Shortcut { command } => match command {
            ShortcutCommand::Build {
                url,
                secret,
                no_secret,
            } => build_shortcut(&paths, url, secret, no_secret).map(|_| ()),
            ShortcutCommand::Install {
                url,
                secret,
                no_secret,
            } => install_shortcut(&paths, url, secret, no_secret),
        },
        Commands::Imessage { command } => match command {
            ImessageCommand::CheckPermissions => check_imessage_permissions(),
            ImessageCommand::Inspect => inspect_imessage(),
        },
        Commands::Express { command } => match command {
            ExpressCommand::Login { source } => express_login(source),
        },
        Commands::Config { command } => match command {
            ConfigCommand::Check => {
                check_local_config(&paths)?;
                println!("local.toml ok");
                Ok(())
            }
        },
        Commands::Ha { command } => match command {
            HaCommand::Check => check_ha(&paths).await,
        },
    }
}

async fn start(paths: &RuntimePaths) -> Result<()> {
    println!("======================================");
    println!("    启动 DavisZeroClaw 智能管家");
    println!("======================================");

    let zeroclaw = require_command("zeroclaw")
        .context("找不到底层引擎 zeroclaw。请先通过 Homebrew 安装: brew install zeroclaw")?;
    fs::create_dir_all(&paths.runtime_dir)?;

    if !paths.config_template_path().is_file() {
        bail!("找不到配置模板: {}", paths.config_template_path().display());
    }
    if !paths.local_config_path().is_file() {
        bail!(
            "找不到用户配置文件: {}。请先复制模板：cp {} {}",
            paths.local_config_path().display(),
            paths.local_config_example_path().display(),
            paths.local_config_path().display()
        );
    }

    check_imessage_permissions()?;

    let cargo =
        require_command("cargo").context("找不到 cargo，无法编译并启动本地 Davis Rust 服务")?;
    println!("🦀 正在编译 Davis Rust 服务...");
    run_status(
        Command::new(cargo)
            .arg("build")
            .arg("--release")
            .arg("--bin")
            .arg("davis-ha-proxy")
            .arg("--manifest-path")
            .arg(paths.repo_root.join("Cargo.toml"))
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "cargo build --release --bin davis-ha-proxy",
    )?;

    println!("🔎 正在校验 config/davis/local.toml ...");
    let config = check_local_config(paths)?;
    println!("local.toml ok");

    println!("🔑 正在为 ZeroClaw 准备 provider 凭证环境变量...");
    let provider_env = zeroclaw_env_vars(&config);
    println!("🌐 正在读取 browser worker 运行时配置...");

    report_agent_browser_status();
    report_skill_inventory(paths);

    println!("🧩 正在同步运行时 skills（project-skills + skills.sh 第三方 skills）...");
    sync_runtime_skills(paths)?;

    let proxy_bin = release_bin_path(paths, "davis-ha-proxy");
    let audit_proxy_log = paths.runtime_dir.join("ha_audit_proxy.log");
    let audit_proxy_pid = paths.runtime_dir.join("ha_audit_proxy.pid");
    let mut proxy_cmd = Command::new(&proxy_bin);
    proxy_cmd
        .env("DAVIS_REPO_ROOT", &paths.repo_root)
        .env("DAVIS_RUNTIME_DIR", &paths.runtime_dir)
        .env("PATH", tool_path_env())
        .current_dir(&paths.repo_root);
    for (key, value) in &provider_env {
        proxy_cmd.env(key, value);
    }

    println!("🚀 正在启动 Davis 本地 Rust 服务...");
    start_process(
        "Davis HA Proxy",
        &audit_proxy_pid,
        &audit_proxy_log,
        Probe::Http("http://127.0.0.1:3010/health".to_string()),
        proxy_cmd,
    )
    .await?;

    start_browser_worker(paths, &config.browser_bridge).await?;

    println!("🧠 等待初始模型路由与 ZeroClaw 运行时配置生成...");
    if !wait_for_model_routing_ready(60, Duration::from_secs(2)).await {
        println!("❌ 模型路由未能就绪。最近日志如下：");
        print_tail(&audit_proxy_log, 120);
        bail!("模型路由未能就绪");
    }

    if !paths.config_report_cache_path().is_file() {
        println!("🩺 首次启动后自动生成 HA 配置体检报告...");
        let _ = http_get_text("http://127.0.0.1:3010/advisor/config-report").await;
    }

    println!("🚀 正在启动 ZeroClaw Daemon...");
    start_runtime_daemon(paths, &zeroclaw, &provider_env).await?;

    println!("🔎 本地 Davis HA 代理: http://127.0.0.1:3010/health");
    println!("🧠 模型路由状态: http://127.0.0.1:3010/model-routing/status");
    println!("🧾 ZeroClaw Runtime Traces: http://127.0.0.1:3010/zeroclaw/runtime-traces");
    println!("🌍 Browser Bridge 状态: http://127.0.0.1:3010/browser/status");
    println!("🩺 HA 配置体检报告: http://127.0.0.1:3010/advisor/config-report");
    println!("🌐 Gateway 健康检查: http://<mac-ip>:3000/health");
    println!("🔗 Shortcut Webhook Channel: http://<mac-ip>:3001/shortcut");
    println!("💬 iMessage Channel: 由本机 Messages.app 常驻接入");
    println!("🛑 停止服务: daviszeroclaw stop");

    Ok(())
}

fn stop(paths: &RuntimePaths) -> Result<()> {
    println!("======================================");
    println!("    停止 DavisZeroClaw 智能管家");
    println!("======================================");

    stop_process(
        "HA Audit Proxy",
        &paths.runtime_dir.join("ha_audit_proxy.pid"),
    )?;
    stop_process("Browser Worker", &paths.browser_worker_pid_path())?;
    stop_process("ZeroClaw Daemon", &paths.daemon_pid_path())?;
    stop_process("Channel Server", &paths.runtime_dir.join("channel.pid"))?;
    stop_process("Gateway", &paths.runtime_dir.join("gateway.pid"))?;
    Ok(())
}

async fn start_browser_worker(paths: &RuntimePaths, config: &BrowserBridgeConfig) -> Result<()> {
    if !config.enabled {
        println!("ℹ️ browser bridge 已禁用，跳过 browser worker。");
        return Ok(());
    }

    let worker_entry = paths.browser_worker_script_path();
    if !worker_entry.is_file() {
        bail!("找不到 browser worker 入口：{}", worker_entry.display());
    }

    ensure_browser_worker_deps(paths)?;

    let runner = command_path("bun")
        .or_else(|| command_path("node"))
        .ok_or_else(|| anyhow!("browser worker 需要 bun 或 node"))?;
    let runner_name = runner
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("runner")
        .to_string();

    println!("🚀 正在启动 browser worker（{}）...", runner_name);
    let mut cmd = Command::new(runner);
    cmd.arg(&worker_entry)
        .env("DAVIS_BROWSER_BRIDGE_ENABLED", "true")
        .env("DAVIS_BROWSER_BRIDGE_PORT", config.worker_port.to_string())
        .env("DAVIS_BROWSER_DEFAULT_PROFILE", &config.default_profile)
        .env(
            "DAVIS_BROWSER_PROFILES_JSON",
            serde_json::to_string(&config.profiles)?,
        )
        .env(
            "DAVIS_BROWSER_REMOTE_DEBUGGING_URL",
            &config.user_session.remote_debugging_url,
        )
        .env(
            "DAVIS_BROWSER_ALLOW_APPLESCRIPT_FALLBACK",
            if config.user_session.allow_applescript_fallback {
                "true"
            } else {
                "false"
            },
        )
        .env(
            "DAVIS_BROWSER_SCREENSHOTS_DIR",
            paths.browser_screenshots_dir(),
        )
        .env("DAVIS_BROWSER_PROFILES_DIR", paths.browser_profiles_root())
        .env("PATH", tool_path_env())
        .current_dir(paths.repo_root.join("browser-worker"));

    start_process(
        "Browser Worker",
        &paths.browser_worker_pid_path(),
        &paths.browser_worker_log_path(),
        Probe::Http(format!("http://127.0.0.1:{}/status", config.worker_port)),
        cmd,
    )
    .await
}

fn ensure_browser_worker_deps(paths: &RuntimePaths) -> Result<()> {
    let worker_dir = paths.repo_root.join("browser-worker");
    if !worker_dir.join("package.json").is_file() {
        println!("ℹ️ 未检测到 browser worker package.json，跳过依赖安装。");
        return Ok(());
    }
    if worker_dir.join("node_modules").join("playwright").is_dir() {
        println!("ℹ️ browser worker 依赖已存在。");
        return Ok(());
    }

    if let Some(bun) = command_path("bun") {
        println!("📦 使用 Bun 安装 browser worker 依赖...");
        run_status(
            Command::new(bun)
                .arg("install")
                .env("PATH", tool_path_env())
                .current_dir(&worker_dir),
            "bun install",
        )?;
        return Ok(());
    }

    if let Some(npm) = command_path("npm") {
        println!("📦 使用 npm 安装 browser worker 依赖...");
        run_status(
            Command::new(npm)
                .arg("install")
                .env("PATH", tool_path_env())
                .current_dir(&worker_dir),
            "npm install",
        )?;
        return Ok(());
    }

    bail!("browser worker 需要 bun 或 npm 来安装依赖");
}

async fn start_runtime_daemon(
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
            println!("ℹ️ ZeroClaw Daemon 已在运行，PID: {existing_pid}");
            println!("   日志: {}", paths.daemon_log_path().display());
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

fn sync_runtime_skills_with_sources(
    paths: &RuntimePaths,
    project_skills_dir: &Path,
    vendor_skills_dir: &Path,
) -> Result<()> {
    let workspace_dir = paths.workspace_dir();
    let runtime_skills_dir = workspace_dir.join("skills");
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

    println!("🧩 已同步运行时 skills 到 {}", runtime_skills_dir.display());
    Ok(())
}

fn copy_skill_tree(source_root: &Path, source_label: &str, staging_dir: &Path) -> Result<()> {
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
                "检测到同名 skill 冲突: {skill_name}\n   来源目录: {}\n   已存在于: {}\n   请把项目自带 skill 与 skills.sh 第三方 skill 保持不同名字。",
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

fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
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

fn sanitize_markdown_links_in_dir(root: &Path) -> Result<()> {
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

fn collect_files(root: &Path) -> Result<Vec<PathBuf>> {
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

fn is_script_link(url: &str) -> bool {
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

fn install_vendor_skills(paths: &RuntimePaths) -> Result<()> {
    let npx = require_command("npx").context("未检测到 npx。请先安装 Node.js / npm")?;
    fs::create_dir_all(paths.repo_root.join("skills"))?;

    println!("🧩 正在安装第三方 skill: agent-browser");
    run_status(
        Command::new(npx)
            .arg("skills")
            .arg("add")
            .arg("https://github.com/vercel-labs/agent-browser")
            .arg("--skill")
            .arg("agent-browser")
            .arg("--agent")
            .arg("openclaw")
            .arg("-y")
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "npx skills add https://github.com/vercel-labs/agent-browser --skill agent-browser --agent openclaw -y",
    )?;

    println!("🧩 正在同步运行时 skills...");
    sync_runtime_skills(paths)?;
    println!("✅ 第三方 skills 安装完成");
    Ok(())
}

fn build_shortcut(
    paths: &RuntimePaths,
    url: Option<String>,
    secret: Option<String>,
    no_secret: bool,
) -> Result<ShortcutBuild> {
    ensure_macos("Shortcut 构建")?;
    let plutil =
        require_command("plutil").context("plutil is required to build the shortcut template")?;
    let shortcuts = require_command("shortcuts")
        .context("shortcuts CLI is required to sign the shortcut template")?;

    let shortcut_json = paths
        .repo_root
        .join("shortcuts")
        .join("叫下戴维斯.shortcut.json");
    let output_shortcut = paths
        .repo_root
        .join("shortcuts")
        .join("叫下戴维斯.shortcut");
    let webhook_url = match url
        .or_else(|| std::env::var("DAVIS_SHORTCUT_WEBHOOK_URL").ok())
        .filter(|value| !value.trim().is_empty())
    {
        Some(value) => value,
        None => {
            let host_ip = detect_host_ip().unwrap_or_else(|| {
                eprintln!(
                    "Warning: could not detect this Mac's LAN IP; leaving URL host as <mac-ip>."
                );
                "<mac-ip>".to_string()
            });
            let port =
                std::env::var("DAVIS_SHORTCUT_WEBHOOK_PORT").unwrap_or_else(|_| "3001".to_string());
            let path = std::env::var("DAVIS_SHORTCUT_WEBHOOK_PATH")
                .unwrap_or_else(|_| "/shortcut".to_string());
            format!("http://{host_ip}:{port}{path}")
        }
    };

    let webhook_secret = resolve_shortcut_secret(paths, secret, no_secret);
    let raw = fs::read_to_string(&shortcut_json)
        .with_context(|| format!("failed to read {}", shortcut_json.display()))?;
    let mut workflow: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid shortcut JSON: {}", shortcut_json.display()))?;
    customize_shortcut_json(&mut workflow, &webhook_url, webhook_secret.as_deref())?;

    let unique = unique_suffix();
    let tmp_json = paths
        .repo_root
        .join("shortcuts")
        .join(format!("叫下戴维斯.custom.{unique}.json"));
    let tmp_wflow = paths
        .repo_root
        .join("shortcuts")
        .join(format!("叫下戴维斯.custom.{unique}.wflow"));
    let cleanup = CleanupFiles(vec![tmp_json.clone(), tmp_wflow.clone()]);

    fs::write(&tmp_json, serde_json::to_string_pretty(&workflow)?)?;
    run_status(
        Command::new(plutil)
            .arg("-convert")
            .arg("binary1")
            .arg(&tmp_json)
            .arg("-o")
            .arg(&tmp_wflow)
            .env("PATH", tool_path_env()),
        "plutil -convert binary1",
    )?;
    run_status(
        Command::new(shortcuts)
            .arg("sign")
            .arg("-m")
            .arg("anyone")
            .arg("-i")
            .arg(&tmp_wflow)
            .arg("-o")
            .arg(&output_shortcut)
            .env("PATH", tool_path_env()),
        "shortcuts sign",
    )?;
    drop(cleanup);

    println!("Built {}", output_shortcut.display());
    println!("Webhook URL: {webhook_url}");
    let embedded_secret = webhook_secret.is_some();
    if embedded_secret {
        println!("Embedded header: X-Webhook-Secret");
    } else {
        println!("Embedded header: none (no webhook secret found)");
    }
    Ok(ShortcutBuild { output_shortcut })
}

fn install_shortcut(
    paths: &RuntimePaths,
    url: Option<String>,
    secret: Option<String>,
    no_secret: bool,
) -> Result<()> {
    let shortcut = build_shortcut(paths, url, secret, no_secret)?;
    open_shortcut_import(&shortcut.output_shortcut)?;
    println!(
        "Opened Shortcuts import flow for {}",
        shortcut.output_shortcut.display()
    );
    println!("Complete the confirmation in the Shortcuts app to finish installing.");
    Ok(())
}

fn open_shortcut_import(shortcut_path: &Path) -> Result<()> {
    ensure_macos("Shortcut 导入")?;
    let open = require_command("open").context("open is required to launch Shortcut import")?;
    run_status(
        Command::new(open)
            .arg(shortcut_path)
            .env("PATH", tool_path_env()),
        "open shortcut import",
    )
}

fn resolve_shortcut_secret(
    paths: &RuntimePaths,
    explicit_secret: Option<String>,
    no_secret: bool,
) -> Option<String> {
    let secret = if no_secret {
        None
    } else if let Some(secret) = explicit_secret {
        Some(secret)
    } else if let Some(secret) = std::env::var_os("DAVIS_SHORTCUT_WEBHOOK_SECRET") {
        Some(secret.to_string_lossy().to_string())
    } else {
        toml_string_value(&paths.local_config_path(), "webhook", "secret")
            .filter(|value| !value.is_empty())
            .or_else(|| {
                toml_string_value(
                    &paths.runtime_config_path(),
                    "channels_config.webhook",
                    "secret",
                )
                .filter(|value| !value.is_empty())
            })
    };

    secret.filter(|value| !value.is_empty())
}

fn toml_string_value(path: &Path, section: &str, key: &str) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let parsed: toml::Value = raw.parse().ok()?;
    let mut value = &parsed;
    for part in section.split('.') {
        value = value.get(part)?;
    }
    value.get(key)?.as_str().map(ToString::to_string)
}

pub fn customize_shortcut_json(
    workflow: &mut Value,
    webhook_url: &str,
    webhook_secret: Option<&str>,
) -> Result<()> {
    *workflow
        .pointer_mut("/WFWorkflowImportQuestions/0/DefaultValue")
        .ok_or_else(|| {
            anyhow!("shortcut template missing WFWorkflowImportQuestions.0.DefaultValue")
        })? = Value::String(webhook_url.to_string());
    *workflow
        .pointer_mut("/WFWorkflowActions/1/WFWorkflowActionParameters/WFURL")
        .ok_or_else(|| anyhow!("shortcut template missing WFWorkflowActions.1.WFURL"))? =
        Value::String(webhook_url.to_string());

    let params = workflow
        .pointer_mut("/WFWorkflowActions/1/WFWorkflowActionParameters")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("shortcut template missing download URL action parameters"))?;

    params.remove("WFHTTPHeaders");
    params.remove("ShowHeaders");

    if let Some(secret) = webhook_secret.filter(|value| !value.is_empty()) {
        params.insert(
            "WFHTTPHeaders".to_string(),
            json!({
                "Value": {
                    "WFDictionaryFieldValueItems": [
                        {
                            "UUID": pseudo_uuid(),
                            "WFItemType": 0,
                            "WFKey": "X-Webhook-Secret",
                            "WFValue": secret
                        }
                    ]
                },
                "WFSerializationType": "WFDictionaryFieldValue"
            }),
        );
        params.insert("ShowHeaders".to_string(), Value::Bool(true));
    }

    Ok(())
}

fn detect_host_ip() -> Option<String> {
    if let Ok(value) = std::env::var("DAVIS_SHORTCUT_HOST_IP") {
        if !value.trim().is_empty() {
            return Some(value);
        }
    }

    let default_interface = command_text(
        Command::new(command_path("route")?)
            .arg("get")
            .arg("default")
            .env("PATH", tool_path_env()),
    )
    .ok()
    .and_then(|output| {
        output.lines().find_map(|line| {
            let line = line.trim();
            line.strip_prefix("interface:")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
    });

    let mut candidates = Vec::new();
    if let Some(interface) = default_interface {
        candidates.push(interface);
    }
    candidates.push("en0".to_string());
    candidates.push("en1".to_string());

    let ipconfig = command_path("ipconfig")?;
    for interface in candidates {
        if let Ok(output) = command_text(
            Command::new(&ipconfig)
                .arg("getifaddr")
                .arg(&interface)
                .env("PATH", tool_path_env()),
        ) {
            let ip = output.trim();
            if !ip.is_empty() {
                return Some(ip.to_string());
            }
        }
    }
    None
}

fn check_imessage_permissions() -> Result<()> {
    ensure_macos("iMessage channel")?;
    println!("🔐 正在检查 iMessage 常驻能力所需权限...");

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("无法读取 HOME 环境变量"))?;
    let messages_db = home.join("Library").join("Messages").join("chat.db");
    let messages_app = Path::new("/System/Applications/Messages.app");

    if !messages_app.is_dir() {
        bail!(
            "找不到 {}。当前系统似乎不可用 Messages.app。",
            messages_app.display()
        );
    }
    if !messages_db.is_file() {
        bail!(
            "未找到 {}。请先打开 Messages.app，登录 iMessage，并至少收发一条消息。",
            messages_db.display()
        );
    }

    let sqlite3 =
        require_command("sqlite3").context("系统缺少 sqlite3，无法验证 Messages 数据库权限")?;
    let sqlite_output = command_output(
        Command::new(sqlite3)
            .arg(&messages_db)
            .arg("select count(*) from message limit 1;")
            .env("PATH", tool_path_env()),
    )?;
    if !sqlite_output.status_success {
        bail!(
            "当前宿主没有读取 Messages 数据库的权限。\n   请前往：系统设置 -> 隐私与安全性 -> 完全磁盘访问权限\n   然后给你实际运行 daviszeroclaw start 的宿主 App 授权。\n   常见宿主 App：Terminal、iTerm、Codex。\n   sqlite3 错误：{}",
            sqlite_output.stderr.replace('\n', " ")
        );
    }

    println!(
        "ℹ️ 即将验证 Automation 权限。首次运行时，macOS 可能会弹出“允许控制 Messages”的提示。"
    );
    let osascript =
        require_command("osascript").context("系统缺少 osascript，无法验证 Automation 权限")?;
    let ae_output = command_output(
        Command::new(osascript)
            .arg("-e")
            .arg("tell application \"Messages\" to get name")
            .env("PATH", tool_path_env()),
    )?;
    if !ae_output.status_success {
        bail!(
            "当前宿主还不能通过 Apple Events 控制 Messages.app。\n   请前往：系统设置 -> 隐私与安全性 -> 自动化\n   然后允许当前宿主 App 控制 Messages。\n   osascript 错误：{}",
            ae_output.stderr.replace('\n', " ")
        );
    }

    println!("✅ iMessage 权限检查通过。");
    Ok(())
}

fn inspect_imessage() -> Result<()> {
    ensure_macos("iMessage inspect")?;
    println!("🔎 正在检查本机 iMessage 配置...");

    let home = home_dir()?;
    let messages_db = home.join("Library").join("Messages").join("chat.db");
    let accounts_db = home
        .join("Library")
        .join("Accounts")
        .join("Accounts4.sqlite");
    let messages_app = Path::new("/System/Applications/Messages.app");

    if !messages_app.is_dir() {
        bail!(
            "找不到 {}。当前系统似乎不可用 Messages.app。",
            messages_app.display()
        );
    }
    if !messages_db.is_file() {
        bail!(
            "未找到 {}。请先打开 Messages.app，登录 iMessage，并至少收发一条消息。",
            messages_db.display()
        );
    }

    let sqlite3 =
        require_command("sqlite3").context("系统缺少 sqlite3，无法读取 iMessage 诊断信息")?;
    ensure_sqlite_readable(&sqlite3, &messages_db, "Messages 数据库")?;

    let apple_accounts = if accounts_db.is_file() {
        ensure_sqlite_readable(&sqlite3, &accounts_db, "Accounts 数据库")?;
        imessage_apple_accounts(&sqlite3, &accounts_db)?
    } else {
        Vec::new()
    };
    let candidates = imessage_allowed_contact_candidates(&sqlite3, &messages_db)?;

    println!();
    println!("Messages Apple Account:");
    if apple_accounts.is_empty() {
        println!("- 未能从 Accounts4.sqlite 确认。");
    } else {
        for account in &apple_accounts {
            println!("- {account}");
        }
    }

    println!();
    println!("Davis allowed_contacts candidates:");
    if candidates.is_empty() {
        println!("- 未能从历史 iMessage 元数据中确认。");
        println!("  请从 iPhone 给这台 Mac 的手机号或 Apple Account 发一条测试消息后重试。");
    } else {
        for (index, candidate) in candidates.iter().take(5).enumerate() {
            let suffix = if index == 0 { " (recommended)" } else { "" };
            println!(
                "{}. {}{} | {} messages, incoming={}, outgoing={}, last={}, rowid={}",
                index + 1,
                candidate.identity,
                suffix,
                candidate.messages,
                candidate.incoming,
                candidate.outgoing,
                candidate.last_seen_local,
                candidate.max_rowid
            );
            println!("   reason: {}", candidate.reason);
        }

        println!();
        println!("Suggested config:");
        println!("[imessage]");
        println!("allowed_contacts = [\"{}\"]", candidates[0].identity);
    }

    println!();
    println!("Note: inspect 只读取账号、句柄、方向和时间等元数据，不读取消息正文。");
    Ok(())
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("无法读取 HOME 环境变量"))
}

fn ensure_sqlite_readable(sqlite3: &Path, db: &Path, label: &str) -> Result<()> {
    let output = command_output(
        Command::new(sqlite3)
            .arg("-readonly")
            .arg(db)
            .arg("select 1;")
            .env("PATH", tool_path_env()),
    )?;
    if !output.status_success {
        bail!(
            "当前宿主无法读取{}: {}\n   请前往：系统设置 -> 隐私与安全性 -> 完全磁盘访问权限\n   然后给你实际运行 daviszeroclaw 的宿主 App 授权。\n   sqlite3 错误：{}",
            label,
            db.display(),
            output.stderr.replace('\n', " ")
        );
    }
    Ok(())
}

fn imessage_apple_accounts(sqlite3: &Path, accounts_db: &Path) -> Result<Vec<String>> {
    let rows = sqlite_rows(
        sqlite3,
        accounts_db,
        r#"
select distinct a.zusername
from zaccount a
join zaccounttype t on t.z_pk = a.zaccounttype
where t.zidentifier = 'com.apple.account.IdentityServices'
  and a.zactive = 1
  and a.zauthenticated = 1
  and a.zusername is not null
  and trim(a.zusername) != ''
order by a.z_pk;
"#,
    )?;

    let mut accounts = rows
        .into_iter()
        .filter_map(|row| row.first().cloned())
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();

    if accounts.is_empty() {
        accounts = sqlite_rows(
            sqlite3,
            accounts_db,
            r#"
select distinct a.zusername
from zaccount a
join zaccounttype t on t.z_pk = a.zaccounttype
where t.zidentifier in ('com.apple.account.AppleAccount', 'com.apple.account.AppleIDAuthentication')
  and a.zactive = 1
  and a.zauthenticated = 1
  and a.zusername is not null
  and trim(a.zusername) != ''
order by a.z_pk;
"#,
        )?
        .into_iter()
        .filter_map(|row| row.first().cloned())
        .filter(|value| !value.trim().is_empty())
        .collect();
    }

    let mut seen = BTreeSet::new();
    accounts.retain(|value| seen.insert(value.clone()));
    Ok(accounts)
}

fn imessage_allowed_contact_candidates(
    sqlite3: &Path,
    messages_db: &Path,
) -> Result<Vec<ImessageAllowedContactCandidate>> {
    let rows = sqlite_rows(
        sqlite3,
        messages_db,
        r#"
with per_identity as (
  select
    h.id as identity,
    count(*) as messages,
    sum(case when m.is_from_me = 0 then 1 else 0 end) as incoming,
    sum(case when m.is_from_me = 1 then 1 else 0 end) as outgoing,
    max(m.rowid) as max_rowid,
    datetime(max(case when m.date > 1000000000000 then m.date / 1000000000 else m.date end) + 978307200, 'unixepoch', 'localtime') as last_seen_local,
    max(case when m.destination_caller_id = h.id then 1 else 0 end) as destination_matches
  from message m
  join handle h on h.rowid = m.handle_id
  where m.service = 'iMessage'
    and h.service = 'iMessage'
    and h.id is not null
    and trim(h.id) != ''
  group by h.id
)
select
  identity,
  messages,
  incoming,
  outgoing,
  max_rowid,
  last_seen_local,
  destination_matches,
  case
    when incoming > 0 and outgoing > 0 and destination_matches = 1 then 'recent self iMessage loopback: sender handle matches destination caller id, with both incoming and outgoing rows'
    when incoming > 0 and destination_matches = 1 then 'incoming iMessage whose sender handle matches destination caller id'
    when incoming > 0 then 'incoming iMessage sender handle observed in Messages DB'
    else 'iMessage handle observed, but no incoming row was found'
  end as reason
from per_identity
where incoming > 0
order by
  case
    when incoming > 0 and outgoing > 0 and destination_matches = 1 then 0
    when incoming > 0 and destination_matches = 1 then 1
    else 2
  end,
  max_rowid desc
limit 10;
"#,
    )?;

    let mut candidates = Vec::new();
    for row in rows {
        if row.len() < 8 {
            continue;
        }
        candidates.push(ImessageAllowedContactCandidate {
            identity: row[0].clone(),
            messages: row[1].parse().unwrap_or_default(),
            incoming: row[2].parse().unwrap_or_default(),
            outgoing: row[3].parse().unwrap_or_default(),
            max_rowid: row[4].parse().unwrap_or_default(),
            last_seen_local: row[5].clone(),
            reason: row[7].clone(),
        });
    }

    Ok(candidates)
}

fn sqlite_rows(sqlite3: &Path, db: &Path, query: &str) -> Result<Vec<Vec<String>>> {
    let output = command_output(
        Command::new(sqlite3)
            .arg("-readonly")
            .arg(db)
            .arg("-separator")
            .arg("\t")
            .arg(query)
            .env("PATH", tool_path_env()),
    )?;
    if !output.status_success {
        bail!("{}", output.stderr.replace('\n', " "));
    }
    Ok(output
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split('\t').map(ToString::to_string).collect())
        .collect())
}

fn express_login(source: ExpressLoginSource) -> Result<()> {
    ensure_macos("快递登录页")?;
    let open = require_command("open").context("macOS open command is required")?;
    let url = match source {
        ExpressLoginSource::Ali => ALI_ORDER_URL,
        ExpressLoginSource::Jd => JD_ORDER_URL,
    };
    run_status(
        Command::new(open)
            .arg("-a")
            .arg("Google Chrome")
            .arg(url)
            .env("PATH", tool_path_env()),
        "open -a Google Chrome",
    )
}

async fn check_ha(paths: &RuntimePaths) -> Result<()> {
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

async fn start_process(
    name: &str,
    pid_file: &Path,
    log_file: &Path,
    probe: Probe,
    mut command: Command,
) -> Result<()> {
    if let Some(existing_pid) = read_pid(pid_file) {
        if pid_is_alive(existing_pid) && wait_for_probe(&probe, 2, Duration::from_secs(1)).await {
            println!("ℹ️ {name} 已在运行，PID: {existing_pid}");
            println!("   日志: {}", log_file.display());
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
        println!("✅ {name} 已启动，PID: {pid}");
        println!("   日志: {}", log_file.display());
        return Ok(());
    }

    println!("❌ {name} 启动失败。最近日志如下：");
    print_tail(log_file, 120);
    bail!("{name} 启动失败");
}

fn stop_process(name: &str, pid_file: &Path) -> Result<()> {
    if !pid_file.is_file() {
        println!("ℹ️ {name} 未运行。");
        return Ok(());
    }

    let Some(pid) = read_pid(pid_file) else {
        println!("ℹ️ {name} 的 PID 文件无效，已清理。");
        fs::remove_file(pid_file)?;
        return Ok(());
    };

    if pid_is_alive(pid) {
        terminate_pid(pid)?;
        println!("✅ 已停止 {name}，PID: {pid}");
    } else {
        println!("ℹ️ {name} 的 PID 文件已过期，已清理。");
    }
    fs::remove_file(pid_file)?;
    Ok(())
}

async fn wait_for_probe(probe: &Probe, attempts: usize, delay: Duration) -> bool {
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

async fn wait_for_model_routing_ready(attempts: usize, delay: Duration) -> bool {
    for _ in 0..attempts {
        let payload = http_get_text("http://127.0.0.1:3010/model-routing/status")
            .await
            .unwrap_or_default();
        if payload.contains("\"route_ready\":true") {
            return true;
        }
        if payload.contains("\"status\":\"error\"") {
            println!("❌ 模型路由初始化失败：{payload}");
            return false;
        }
        tokio::time::sleep(delay).await;
    }
    false
}

async fn http_get_text(url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        bail!("GET {url} returned {}", response.status());
    }
    Ok(response.text().await?)
}

fn port_ready(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok()
}

fn read_pid(pid_file: &Path) -> Option<u32> {
    fs::read_to_string(pid_file).ok()?.trim().parse().ok()
}

fn pid_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .is_ok_and(|status| status.success())
}

fn terminate_pid(pid: u32) -> Result<()> {
    run_status(
        Command::new("kill").arg(pid.to_string()),
        &format!("kill {pid}"),
    )?;
    Ok(())
}

fn run_status(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to run {description}"))?;
    if !status.success() {
        bail!("{description} failed with status {status}");
    }
    Ok(())
}

#[derive(Debug)]
struct CommandOutput {
    status_success: bool,
    stdout: String,
    stderr: String,
}

fn command_output(command: &mut Command) -> Result<CommandOutput> {
    let output = command.output()?;
    Ok(CommandOutput {
        status_success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn command_text(command: &mut Command) -> Result<String> {
    let output = command_output(command)?;
    if !output.status_success {
        bail!("{}", output.stderr);
    }
    Ok(output.stdout)
}

fn print_tail(path: &Path, max_lines: usize) {
    if let Ok(raw) = fs::read_to_string(path) {
        let lines = raw.lines().collect::<Vec<_>>();
        let start = lines.len().saturating_sub(max_lines);
        for line in &lines[start..] {
            println!("{line}");
        }
    }
}

fn report_agent_browser_status() {
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
            println!("ℹ️ 已检测到 Homebrew 安装的 agent-browser CLI。");
            return;
        }
    }

    if command_path("agent-browser").is_some() {
        println!("ℹ️ 已检测到 agent-browser CLI。项目推荐优先使用 Homebrew 安装：brew install agent-browser");
    } else {
        println!("ℹ️ 未检测到 agent-browser CLI。若要启用按需知乎研究，优先执行：brew install agent-browser");
        println!("   如机器上没有可复用的 Chrome / Chromium，再执行：agent-browser install");
    }
}

fn report_skill_inventory(paths: &RuntimePaths) {
    let project_skills_dir = paths.repo_root.join("project-skills");
    let vendor_skills_dir = paths.repo_root.join("skills");
    let project_skill_count = count_skills(&project_skills_dir);
    let vendor_skill_names = skill_names(&vendor_skills_dir);

    println!(
        "ℹ️ 已检测到 {} 个仓库自带 skill（目录：{}）。",
        project_skill_count,
        project_skills_dir.display()
    );
    if !vendor_skill_names.is_empty() {
        println!(
            "ℹ️ 已检测到 skills.sh 管理的第三方 skill：{}",
            vendor_skill_names.join(", ")
        );
    }
}

fn count_skills(root: &Path) -> usize {
    skill_names(root).len()
}

fn skill_names(root: &Path) -> Vec<String> {
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

fn release_bin_path(paths: &RuntimePaths, name: &str) -> PathBuf {
    let mut bin = paths.repo_root.join("target").join("release").join(name);
    if cfg!(windows) {
        bin.set_extension("exe");
    }
    bin
}

fn ensure_macos(feature: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        bail!("{feature} 仅支持 macOS")
    }
}

fn require_command(name: &str) -> Result<PathBuf> {
    command_path(name).ok_or_else(|| anyhow!("command not found: {name}"))
}

fn command_path(name: &str) -> Option<PathBuf> {
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

fn tool_path_env() -> OsString {
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

fn pseudo_uuid() -> String {
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

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}.{}", std::process::id(), nanos)
}

struct CleanupFiles(Vec<PathBuf>);

impl Drop for CleanupFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_markdown_script_links_removes_script_targets_only() {
        let raw = "Run [setup](scripts/setup.sh), keep [docs](docs/readme.md), and [ps](x/install.ps1#L1).";
        assert_eq!(
            sanitize_markdown_script_links(raw),
            "Run setup, keep [docs](docs/readme.md), and ps."
        );
    }

    #[test]
    fn customize_shortcut_json_sets_url_and_secret_header() {
        let mut workflow = json!({
            "WFWorkflowImportQuestions": [
                { "DefaultValue": "http://old" }
            ],
            "WFWorkflowActions": [
                {},
                {
                    "WFWorkflowActionParameters": {
                        "WFURL": "http://old"
                    }
                }
            ]
        });

        customize_shortcut_json(
            &mut workflow,
            "https://davis.example.com/shortcut",
            Some("secret"),
        )
        .unwrap();

        assert_eq!(
            workflow.pointer("/WFWorkflowImportQuestions/0/DefaultValue"),
            Some(&Value::String(
                "https://davis.example.com/shortcut".to_string()
            ))
        );
        assert_eq!(
            workflow.pointer("/WFWorkflowActions/1/WFWorkflowActionParameters/WFURL"),
            Some(&Value::String(
                "https://davis.example.com/shortcut".to_string()
            ))
        );
        assert_eq!(
            workflow.pointer(
                "/WFWorkflowActions/1/WFWorkflowActionParameters/WFHTTPHeaders/Value/WFDictionaryFieldValueItems/0/WFKey"
            ),
            Some(&Value::String("X-Webhook-Secret".to_string()))
        );
        assert_eq!(
            workflow.pointer(
                "/WFWorkflowActions/1/WFWorkflowActionParameters/WFHTTPHeaders/Value/WFDictionaryFieldValueItems/0/WFValue"
            ),
            Some(&Value::String("secret".to_string()))
        );
    }

    #[test]
    fn customize_shortcut_json_removes_secret_header_when_disabled() {
        let mut workflow = json!({
            "WFWorkflowImportQuestions": [
                { "DefaultValue": "http://old" }
            ],
            "WFWorkflowActions": [
                {},
                {
                    "WFWorkflowActionParameters": {
                        "WFURL": "http://old",
                        "ShowHeaders": true,
                        "WFHTTPHeaders": { "old": true }
                    }
                }
            ]
        });

        customize_shortcut_json(&mut workflow, "http://new", None).unwrap();

        assert!(workflow
            .pointer("/WFWorkflowActions/1/WFWorkflowActionParameters/WFHTTPHeaders")
            .is_none());
        assert!(workflow
            .pointer("/WFWorkflowActions/1/WFWorkflowActionParameters/ShowHeaders")
            .is_none());
    }

    #[test]
    fn sync_runtime_skills_copies_and_marks_sources() {
        let root = unique_test_dir("sync_runtime_skills_copies");
        let paths = RuntimePaths {
            repo_root: root.join("repo"),
            runtime_dir: root.join("runtime"),
        };
        let project = root.join("project-skills");
        let vendor = root.join("vendor-skills");
        fs::create_dir_all(project.join("ha-control")).unwrap();
        fs::create_dir_all(vendor.join("agent-browser")).unwrap();
        fs::write(
            project.join("ha-control").join("SKILL.md"),
            "Use [script](bin/setup.sh) and [doc](README.md).",
        )
        .unwrap();
        fs::write(vendor.join("agent-browser").join("SKILL.md"), "browser").unwrap();

        sync_runtime_skills_with_sources(&paths, &project, &vendor).unwrap();

        let runtime_skills = paths.workspace_dir().join("skills");
        assert!(runtime_skills.join("ha-control").join("SKILL.md").is_file());
        assert!(runtime_skills
            .join("agent-browser")
            .join("SKILL.md")
            .is_file());
        assert_eq!(
            fs::read_to_string(runtime_skills.join("ha-control").join("SKILL.md")).unwrap(),
            "Use script and [doc](README.md)."
        );
        assert_eq!(
            fs::read_to_string(
                runtime_skills
                    .join("ha-control")
                    .join(".davis-skill-source")
            )
            .unwrap(),
            "project-skills\n"
        );
        assert_eq!(
            fs::read_to_string(
                runtime_skills
                    .join("agent-browser")
                    .join(".davis-skill-source")
            )
            .unwrap(),
            "skills\n"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sync_runtime_skills_rejects_duplicate_names() {
        let root = unique_test_dir("sync_runtime_skills_duplicates");
        let paths = RuntimePaths {
            repo_root: root.join("repo"),
            runtime_dir: root.join("runtime"),
        };
        let project = root.join("project-skills");
        let vendor = root.join("vendor-skills");
        fs::create_dir_all(project.join("same")).unwrap();
        fs::create_dir_all(vendor.join("same")).unwrap();
        fs::write(project.join("same").join("SKILL.md"), "project").unwrap();
        fs::write(vendor.join("same").join("SKILL.md"), "vendor").unwrap();

        let error = sync_runtime_skills_with_sources(&paths, &project, &vendor)
            .unwrap_err()
            .to_string();
        assert!(error.contains("同名 skill 冲突"));

        let _ = fs::remove_dir_all(root);
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("daviszeroclaw-{name}-{}", unique_suffix()));
        if path.exists() {
            fs::remove_dir_all(&path).unwrap();
        }
        path
    }
}
