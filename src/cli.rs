use crate::{
    add_article_memory, build_article_strategy_review_input, check_article_cleaning,
    check_article_memory, check_local_config, hybrid_search_article_memory,
    ingest_article_from_browser, init_article_memory, judge_all_article_value_memory,
    judge_article_value_memory, list_article_clean_reports, list_article_memory,
    list_article_value_reports, normalize_all_article_memory, normalize_article_memory,
    rebuild_article_memory_embeddings, replay_article_cleaning, resolve_article_embedding_config,
    resolve_article_normalize_config, resolve_article_value_config, search_article_memory,
    upsert_article_memory_embedding, zeroclaw_env_vars, ArticleMemoryAddRequest,
    ArticleMemoryIngestRequest, ArticleMemoryRecordStatus, BrowserBridgeConfig, HaClient,
    HaMcpClient, HaState, ModelRoutingManager, RuntimePaths,
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
    /// Manage the ZeroClaw launchd service for the Davis profile.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
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
    /// Manage Davis memory integrations.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Manage Davis article memory.
    Articles {
        #[command(subcommand)]
        command: ArticlesCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SkillsCommand {
    /// Synchronize project and vendor skills into the runtime workspace.
    Sync,
    /// Install or refresh supported vendor skills.
    Install,
    /// Check project, vendor, runtime, and MemPalace skill status.
    Check,
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Install and start ZeroClaw with the Davis runtime config.
    Install,
    /// Show launchd and ZeroClaw runtime health.
    Status,
    /// Restart ZeroClaw with the Davis runtime config.
    Restart,
    /// Stop and remove the Davis ZeroClaw launchd service.
    Uninstall,
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

#[derive(Debug, Subcommand)]
enum MemoryCommand {
    /// Manage MemPalace integration.
    Mempalace {
        #[command(subcommand)]
        command: MempalaceCommand,
    },
}

#[derive(Debug, Subcommand)]
enum MempalaceCommand {
    /// Install MemPalace into Davis runtime venv.
    Install,
    /// Enable MemPalace MCP server in config/davis/local.toml.
    Enable,
    /// Check MemPalace package, palace directory, and local config.
    Check,
}

#[derive(Debug, Subcommand)]
enum ArticlesCommand {
    /// Initialize the local article memory store.
    Init,
    /// Check the local article memory store.
    Check,
    /// Add a local article from text files.
    Add {
        #[arg(long)]
        title: String,
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value = "manual")]
        source: String,
        #[arg(long)]
        language: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        content_file: PathBuf,
        #[arg(long)]
        summary_file: Option<PathBuf>,
        #[arg(long)]
        translation_file: Option<PathBuf>,
        #[arg(long)]
        score: Option<f32>,
        #[arg(long, value_enum, default_value_t = ArticleStatusArg::Saved)]
        status: ArticleStatusArg,
        #[arg(long)]
        notes: Option<String>,
    },
    /// List recent article memory records.
    List {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Search the local article memory store.
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        keyword_only: bool,
    },
    /// Rebuild semantic embedding index for saved articles.
    Index,
    /// Inspect article cleaning strategy and recent clean reports.
    Cleaning {
        #[command(subcommand)]
        command: ArticleCleaningCommand,
    },
    /// Judge article value before expensive polish/summary/indexing.
    Judging {
        #[command(subcommand)]
        command: ArticleJudgingCommand,
    },
    /// Prepare report context for article memory strategy review.
    Strategy {
        #[command(subcommand)]
        command: ArticleStrategyCommand,
    },
    /// Normalize article Markdown and optional LLM summary/polish.
    Normalize {
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        no_llm: bool,
    },
    /// Ingest an article from a browser page or URL.
    Ingest {
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        tab_id: Option<String>,
        #[arg(long)]
        new_tab: bool,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        language: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        score: Option<f32>,
        #[arg(long, value_enum, default_value_t = ArticleStatusArg::Candidate)]
        status: ArticleStatusArg,
        #[arg(long)]
        notes: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum ArticleCleaningCommand {
    /// Check article cleaning strategy config.
    Check,
    /// Show recent clean reports.
    Audit {
        #[arg(long, default_value_t = 20)]
        recent: usize,
    },
    /// Replay deterministic cleaning without LLM polish/summary.
    Replay {
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        all: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ArticleJudgingCommand {
    /// Run value judging through the normalize gate.
    Run {
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        no_llm: bool,
    },
    /// Show recent value reports.
    Audit {
        #[arg(long, default_value_t = 20)]
        recent: usize,
    },
}

#[derive(Debug, Subcommand)]
enum ArticleStrategyCommand {
    /// Generate a bounded review input for strategy-only agent edits.
    ReviewInput {
        #[arg(long, default_value_t = 20)]
        recent: usize,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ArticleStatusArg {
    Candidate,
    Saved,
    Rejected,
    Archived,
}

impl From<ArticleStatusArg> for ArticleMemoryRecordStatus {
    fn from(value: ArticleStatusArg) -> Self {
        match value {
            ArticleStatusArg::Candidate => Self::Candidate,
            ArticleStatusArg::Saved => Self::Saved,
            ArticleStatusArg::Rejected => Self::Rejected,
            ArticleStatusArg::Archived => Self::Archived,
        }
    }
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
        Commands::Service { command } => match command {
            ServiceCommand::Install => install_davis_service(&paths).await,
            ServiceCommand::Status => status_davis_service(&paths).await,
            ServiceCommand::Restart => restart_davis_service(&paths).await,
            ServiceCommand::Uninstall => uninstall_davis_service(&paths),
        },
        Commands::Skills { command } => match command {
            SkillsCommand::Sync => sync_runtime_skills(&paths),
            SkillsCommand::Install => install_skills(&paths),
            SkillsCommand::Check => check_skills(&paths),
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
            ImessageCommand::Inspect => inspect_imessage(&paths),
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
        Commands::Memory { command } => match command {
            MemoryCommand::Mempalace { command } => match command {
                MempalaceCommand::Install => install_mempalace(&paths),
                MempalaceCommand::Enable => enable_mempalace(&paths),
                MempalaceCommand::Check => check_mempalace(&paths),
            },
        },
        Commands::Articles { command } => match command {
            ArticlesCommand::Init => init_articles(&paths),
            ArticlesCommand::Check => check_articles(&paths),
            ArticlesCommand::Add {
                title,
                url,
                source,
                language,
                tags,
                content_file,
                summary_file,
                translation_file,
                score,
                status,
                notes,
            } => {
                add_article(
                    &paths,
                    ArticleCliAdd {
                        title,
                        url,
                        source,
                        language,
                        tags,
                        content_file,
                        summary_file,
                        translation_file,
                        score,
                        status,
                        notes,
                    },
                )
                .await
            }
            ArticlesCommand::List { limit } => list_articles(&paths, limit),
            ArticlesCommand::Search {
                query,
                limit,
                keyword_only,
            } => search_articles(&paths, &query, limit, keyword_only).await,
            ArticlesCommand::Index => index_articles(&paths).await,
            ArticlesCommand::Cleaning { command } => match command {
                ArticleCleaningCommand::Check => check_article_cleaning_cli(&paths),
                ArticleCleaningCommand::Audit { recent } => clean_audit_articles(&paths, recent),
                ArticleCleaningCommand::Replay { id, all } => {
                    replay_cleaning_articles(&paths, id, all).await
                }
            },
            ArticlesCommand::Judging { command } => match command {
                ArticleJudgingCommand::Run { id, all, no_llm } => {
                    judge_articles(&paths, id, all, no_llm).await
                }
                ArticleJudgingCommand::Audit { recent } => value_audit_articles(&paths, recent),
            },
            ArticlesCommand::Strategy { command } => match command {
                ArticleStrategyCommand::ReviewInput { recent } => {
                    review_article_strategy_input(&paths, recent)
                }
            },
            ArticlesCommand::Normalize { id, all, no_llm } => {
                normalize_articles(&paths, id, all, no_llm).await
            }
            ArticlesCommand::Ingest {
                url,
                profile,
                tab_id,
                new_tab,
                source,
                language,
                tags,
                score,
                status,
                notes,
            } => {
                ingest_article(
                    &paths,
                    ArticleMemoryIngestRequest {
                        url,
                        profile,
                        tab_id,
                        new_tab,
                        source,
                        language,
                        tags,
                        status: status.into(),
                        value_score: score,
                        notes,
                    },
                )
                .await
            }
        },
    }
}

#[derive(Debug)]
struct ArticleCliAdd {
    title: String,
    url: Option<String>,
    source: String,
    language: Option<String>,
    tags: Vec<String>,
    content_file: PathBuf,
    summary_file: Option<PathBuf>,
    translation_file: Option<PathBuf>,
    score: Option<f32>,
    status: ArticleStatusArg,
    notes: Option<String>,
}

fn init_articles(paths: &RuntimePaths) -> Result<()> {
    let status = init_article_memory(paths)?;
    println!("Article memory initialized.");
    print_article_status(&status);
    println!("Next: daviszeroclaw articles add --title <title> --content-file <file>");
    Ok(())
}

fn check_articles(paths: &RuntimePaths) -> Result<()> {
    let status = check_article_memory(paths)?;
    println!("Article memory ok.");
    print_article_status(&status);
    Ok(())
}

async fn add_article(paths: &RuntimePaths, input: ArticleCliAdd) -> Result<()> {
    let content = fs::read_to_string(&input.content_file)
        .with_context(|| format!("failed to read {}", input.content_file.display()))?;
    let summary = read_optional_text_file(input.summary_file.as_deref())?;
    let translation = read_optional_text_file(input.translation_file.as_deref())?;
    let record = add_article_memory(
        paths,
        ArticleMemoryAddRequest {
            title: input.title,
            url: input.url,
            source: input.source,
            language: input.language,
            tags: input.tags,
            content,
            summary,
            translation,
            status: input.status.into(),
            value_score: input.score,
            notes: input.notes,
        },
    )?;
    println!("Article stored.");
    println!("- id: {}", record.id);
    println!("- title: {}", record.title);
    println!("- status: {}", record.status);
    println!(
        "- content: {}",
        paths
            .article_memory_dir()
            .join(&record.content_path)
            .display()
    );
    let config = check_local_config(paths)?;
    let normalize_config =
        resolve_article_normalize_config(&config.article_memory.normalize, &config.providers)?;
    let value_config = resolve_article_value_config(paths, &config.providers)?;
    let normalize_response = normalize_article_memory(
        paths,
        normalize_config.as_ref(),
        value_config.as_ref(),
        &record.id,
    )
    .await?;
    println!(
        "- normalize: {} ({})",
        normalize_response.clean_status, normalize_response.clean_profile
    );
    match (
        normalize_response.value_decision.as_deref(),
        resolve_article_embedding_config(&config.article_memory.embedding, &config.providers)?,
    ) {
        (Some("reject"), _) => println!("- embedding: skipped (value rejected)"),
        (_, Some(embedding_config)) => {
            upsert_article_memory_embedding(paths, &embedding_config, &record).await?;
            println!("- embedding: indexed");
        }
        (_, None) => println!("- embedding: disabled"),
    }
    Ok(())
}

fn list_articles(paths: &RuntimePaths, limit: usize) -> Result<()> {
    let response = list_article_memory(paths, limit);
    if response.status != "ok" {
        bail!(
            "{}",
            response
                .message
                .unwrap_or_else(|| format!("article memory {}", response.status))
        );
    }
    println!(
        "Article memory records: {} of {}",
        response.returned, response.total_articles
    );
    for article in response.articles {
        println!(
            "- {} | {} | {} | {}",
            article.id, article.status, article.captured_at, article.title
        );
    }
    Ok(())
}

async fn search_articles(
    paths: &RuntimePaths,
    query: &str,
    limit: usize,
    keyword_only: bool,
) -> Result<()> {
    let config = if keyword_only {
        None
    } else {
        let config = check_local_config(paths)?;
        resolve_article_embedding_config(&config.article_memory.embedding, &config.providers)?
    };
    let response = if keyword_only {
        search_article_memory(paths, query, limit)
    } else {
        hybrid_search_article_memory(paths, config.as_ref(), query, limit).await
    };
    match response.status.as_str() {
        "ok" | "empty" => {}
        _ => bail!(
            "{}",
            response
                .message
                .unwrap_or_else(|| format!("article memory {}", response.status))
        ),
    }
    println!(
        "Article memory search: {} hit(s), showing {} ({})",
        response.total_hits, response.returned, response.search_mode
    );
    if let Some(semantic_status) = response.semantic_status {
        println!("Semantic index: {semantic_status}");
    }
    for hit in response.hits {
        println!(
            "- {} | keyword={} | semantic={} | {} | {}",
            hit.id,
            hit.score,
            hit.semantic_score
                .map(|score| format!("{score:.3}"))
                .unwrap_or_else(|| "n/a".to_string()),
            hit.status,
            hit.title
        );
        if let Some(url) = hit.url {
            println!("  url: {url}");
        }
        if let Some(snippet) = hit.snippet {
            println!("  snippet: {snippet}");
        }
    }
    Ok(())
}

async fn normalize_articles(
    paths: &RuntimePaths,
    id: Option<String>,
    all: bool,
    no_llm: bool,
) -> Result<()> {
    let config = check_local_config(paths)?;
    let normalize_config = if no_llm {
        None
    } else {
        resolve_article_normalize_config(&config.article_memory.normalize, &config.providers)?
    };
    let value_config = if no_llm {
        None
    } else {
        resolve_article_value_config(paths, &config.providers)?
    };
    let responses = if all {
        normalize_all_article_memory(paths, normalize_config.as_ref(), value_config.as_ref())
            .await?
    } else {
        let id = id.ok_or_else(|| anyhow!("provide --id <article-id> or --all"))?;
        vec![
            normalize_article_memory(paths, normalize_config.as_ref(), value_config.as_ref(), &id)
                .await?,
        ]
    };
    println!("Article normalization complete.");
    for response in responses {
        println!(
            "- {} | {} | profile={} | raw={} normalized={} final={} polished={} summary={}",
            response.article_id,
            response.clean_status,
            response.clean_profile,
            response.raw_chars,
            response.normalized_chars,
            response.final_chars,
            response.polished,
            response.summary_generated
        );
        if let Some(message) = response.message {
            println!("  note: {message}");
        }
        println!("  clean_report: {}", response.clean_report_path);
        if let Some(decision) = response.value_decision {
            println!(
                "  value: {} ({})",
                decision,
                response
                    .value_score
                    .map(|score| format!("{score:.2}"))
                    .unwrap_or_else(|| "n/a".to_string())
            );
        }
        if let Some(path) = response.value_report_path {
            println!("  value_report: {path}");
        }
    }
    Ok(())
}

fn check_article_cleaning_cli(paths: &RuntimePaths) -> Result<()> {
    let response = check_article_cleaning(paths)?;
    println!("Article cleaning strategy {}.", response.status);
    println!("- config: {}", response.config_path);
    println!("- sites: {}", response.sites.join(", "));
    for warning in response.warnings {
        println!("WARN: {warning}");
    }
    Ok(())
}

fn clean_audit_articles(paths: &RuntimePaths, recent: usize) -> Result<()> {
    let response = list_article_clean_reports(paths, recent)?;
    println!(
        "Article clean reports: {} report(s), status={}",
        response.returned, response.status
    );
    for report in response.reports {
        println!(
            "- {} | strategy={}@{} | clean={} | raw={} normalized={} final={} kept={:.2} | risks={}",
            report.article_id,
            report.strategy_name,
            report.strategy_version,
            report.clean_status,
            report.raw_chars,
            report.normalized_chars,
            report.final_chars,
            report.kept_ratio,
            if report.risk_flags.is_empty() {
                "none".to_string()
            } else {
                report.risk_flags.join(",")
            }
        );
        if let Some(url) = report.url {
            println!("  url: {url}");
        }
        if !report.leftover_noise_candidates.is_empty() {
            println!(
                "  leftover_noise: {}",
                report.leftover_noise_candidates.join(", ")
            );
        }
    }
    Ok(())
}

async fn judge_articles(
    paths: &RuntimePaths,
    id: Option<String>,
    all: bool,
    no_llm: bool,
) -> Result<()> {
    let config = check_local_config(paths)?;
    let value_config = if no_llm {
        let mut resolved = resolve_article_value_config(paths, &config.providers)?
            .ok_or_else(|| anyhow!("article value judging is disabled"))?;
        resolved.llm_judge = false;
        Some(resolved)
    } else {
        resolve_article_value_config(paths, &config.providers)?
    };
    let reports = if all {
        judge_all_article_value_memory(
            paths,
            value_config
                .as_ref()
                .ok_or_else(|| anyhow!("article value judging is disabled"))?,
        )
        .await?
    } else {
        let id = id.ok_or_else(|| anyhow!("provide --id <article-id> or --all"))?;
        vec![
            judge_article_value_memory(
                paths,
                value_config
                    .as_ref()
                    .ok_or_else(|| anyhow!("article value judging is disabled"))?,
                &id,
            )
            .await?,
        ]
    };
    println!("Article value judging complete.");
    for report in reports {
        println!(
            "- {} | value={} | score={:.2} | topics={} | risks={}",
            report.article_id,
            report.decision,
            report.value_score,
            if report.topic_tags.is_empty() {
                "none".to_string()
            } else {
                report.topic_tags.join(",")
            },
            if report.risk_flags.is_empty() {
                "none".to_string()
            } else {
                report.risk_flags.join(",")
            }
        );
        for reason in report.reasons.iter().take(3) {
            println!("  reason: {reason}");
        }
    }
    Ok(())
}

fn value_audit_articles(paths: &RuntimePaths, recent: usize) -> Result<()> {
    let response = list_article_value_reports(paths, recent)?;
    println!(
        "Article value reports: {} report(s), status={}",
        response.returned, response.status
    );
    for report in response.reports {
        println!(
            "- {} | decision={} | score={:.2} | topics={} | risks={}",
            report.article_id,
            report.decision,
            report.value_score,
            if report.topic_tags.is_empty() {
                "none".to_string()
            } else {
                report.topic_tags.join(",")
            },
            if report.risk_flags.is_empty() {
                "none".to_string()
            } else {
                report.risk_flags.join(",")
            }
        );
        for reason in report.reasons.iter().take(3) {
            println!("  reason: {reason}");
        }
    }
    Ok(())
}

fn review_article_strategy_input(paths: &RuntimePaths, recent: usize) -> Result<()> {
    let response = build_article_strategy_review_input(paths, recent)?;
    println!("Article strategy review input generated.");
    println!("- status: {}", response.status);
    println!("- report: {}", response.report_path);
    println!("- editable config: {}", response.config_path);
    println!(
        "- implementation requests: {}",
        response.implementation_requests_dir
    );
    println!();
    println!("{}", response.markdown);
    Ok(())
}

async fn replay_cleaning_articles(
    paths: &RuntimePaths,
    id: Option<String>,
    all: bool,
) -> Result<()> {
    if !all && id.is_none() {
        bail!("provide --id <article-id> or --all");
    }
    let response = if all {
        replay_article_cleaning(paths, None)?
    } else {
        replay_article_cleaning(paths, id.as_deref())?
    };
    println!("Article deterministic cleaning replay complete.");
    for report in response.reports {
        println!(
            "- {} | {} | strategy={}@{} | raw={} normalized={} kept={:.2} risks={}",
            report.article_id,
            report.clean_status,
            report.strategy_name,
            report.strategy_version,
            report.raw_chars,
            report.normalized_chars,
            report.kept_ratio,
            if report.risk_flags.is_empty() {
                "none".to_string()
            } else {
                report.risk_flags.join(",")
            }
        );
    }
    Ok(())
}

async fn index_articles(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let Some(embedding_config) =
        resolve_article_embedding_config(&config.article_memory.embedding, &config.providers)?
    else {
        bail!("article_memory.embedding is disabled. Enable it in config/davis/local.toml first");
    };
    let response = rebuild_article_memory_embeddings(paths, &embedding_config).await?;
    println!("Article memory semantic index rebuilt.");
    println!("- provider: {}", response.provider);
    println!("- model: {}", response.model);
    println!("- dimensions: {}", response.dimensions);
    println!("- indexed: {}", response.indexed);
    println!("- skipped: {}", response.skipped);
    println!("- index: {}", response.index_path);
    Ok(())
}

async fn ingest_article(paths: &RuntimePaths, request: ArticleMemoryIngestRequest) -> Result<()> {
    let config = check_local_config(paths)?;
    let response = ingest_article_from_browser(
        paths,
        &config.browser_bridge,
        &config.article_memory,
        &config.providers,
        request,
    )
    .await?;

    match response.status.as_str() {
        "ok" => {
            let article = response
                .article
                .ok_or_else(|| anyhow!("ingest response did not include article"))?;
            println!("Article ingested.");
            println!("- id: {}", article.id);
            println!("- title: {}", article.title);
            if let Some(url) = article.url {
                println!("- url: {url}");
            }
            println!("- status: {}", article.status);
            println!("- content_length: {}", response.extraction.content_length);
            println!("- embedding: {}", response.embedding_status);
        }
        "duplicate" => {
            println!("Article already exists.");
            println!("- title: {}", response.extraction.title);
            println!("- url: {}", response.extraction.url);
            println!("- duplicate_count: {}", response.duplicate_count);
        }
        other => bail!(
            "{}",
            response
                .message
                .unwrap_or_else(|| format!("article ingest failed: {other}"))
        ),
    }
    Ok(())
}

fn read_optional_text_file(path: Option<&Path>) -> Result<Option<String>> {
    path.map(|path| {
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
    })
    .transpose()
}

fn print_article_status(status: &crate::ArticleMemoryStatusResponse) {
    println!("- root: {}", status.root);
    println!("- index: {}", status.index_path);
    println!("- articles: {}", status.total_articles);
    println!("- saved: {}", status.saved_articles);
    println!("- candidates: {}", status.candidate_articles);
    println!("- rejected: {}", status.rejected_articles);
    println!("- archived: {}", status.archived_articles);
}

async fn start(paths: &RuntimePaths) -> Result<()> {
    println!("DavisZeroClaw startup");
    println!("Repository: {}", paths.repo_root.display());
    println!("Runtime: {}", paths.runtime_dir.display());

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

    print_start_step(4, "Prepare runtime skills");
    report_agent_browser_status();
    report_skill_inventory(paths);
    sync_runtime_skills(paths)?;

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

    print_start_step(6, "Start browser worker");
    start_browser_worker(paths, &config.browser_bridge).await?;

    print_start_step(7, "Wait for model routing");
    if !wait_for_model_routing_ready(60, Duration::from_secs(2)).await {
        println!("Model routing did not become ready. Recent proxy log:");
        print_tail(&local_proxy_log, 120);
        bail!("model routing did not become ready");
    }
    println!("Model routing OK.");

    if !paths.config_report_cache_path().is_file() {
        println!("Generating the initial Home Assistant advisor report...");
        let _ = http_get_text("http://127.0.0.1:3010/advisor/config-report").await;
    }

    print_start_step(8, "Start ZeroClaw daemon");
    start_runtime_daemon(paths, &zeroclaw, &provider_env).await?;

    println!();
    println!("Startup complete.");
    println!("Local status:");
    println!("- Davis local proxy: http://127.0.0.1:3010/health");
    println!("- Model routing: http://127.0.0.1:3010/model-routing/status");
    println!("- Runtime traces: http://127.0.0.1:3010/zeroclaw/runtime-traces");
    println!("- Browser bridge: http://127.0.0.1:3010/browser/status");
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

fn print_start_step(index: usize, title: &str) {
    println!();
    println!("[{index}/8] {title}");
}

fn stop(paths: &RuntimePaths) -> Result<()> {
    println!("======================================");
    println!("    Stop DavisZeroClaw");
    println!("======================================");

    stop_process("Davis Local Proxy", &paths.local_proxy_pid_path())?;
    stop_process(
        "Legacy Davis Local Proxy",
        &paths.legacy_local_proxy_pid_path(),
    )?;
    stop_process("Browser Worker", &paths.browser_worker_pid_path())?;
    stop_process("ZeroClaw Daemon", &paths.daemon_pid_path())?;
    stop_process("Channel Server", &paths.runtime_dir.join("channel.pid"))?;
    stop_process("Gateway", &paths.runtime_dir.join("gateway.pid"))?;
    Ok(())
}

async fn install_davis_service(paths: &RuntimePaths) -> Result<()> {
    ensure_macos("Davis service management")?;
    fs::create_dir_all(&paths.runtime_dir)?;
    render_current_runtime_config(paths)?;
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
        zeroclaw_bin: zeroclaw,
        proxy_bin,
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
    println!("Davis ZeroClaw service installed.");
    println!("- plist: {}", plist_path.display());
    println!("- config: {}", paths.runtime_config_path().display());
    status_davis_service(paths).await
}

async fn status_davis_service(paths: &RuntimePaths) -> Result<()> {
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
    Ok(())
}

async fn restart_davis_service(paths: &RuntimePaths) -> Result<()> {
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
        "launchctl kickstart Davis service",
    )?;
    let _ = wait_for_probe(
        &Probe::Http("http://127.0.0.1:3000/health".to_string()),
        20,
        Duration::from_millis(500),
    )
    .await;
    println!("Davis ZeroClaw service restarted.");
    status_davis_service(paths).await
}

fn uninstall_davis_service(_paths: &RuntimePaths) -> Result<()> {
    ensure_macos("Davis service management")?;
    let plist_path = davis_service_plist_path()?;
    let user_target = launchd_user_target()?;
    bootout_davis_service(&user_target, &plist_path);
    if plist_path.is_file() {
        fs::remove_file(&plist_path)
            .with_context(|| format!("failed to remove {}", plist_path.display()))?;
    }
    println!("Davis ZeroClaw service uninstalled.");
    println!("- removed: {}", plist_path.display());
    Ok(())
}

#[derive(Debug)]
struct DavisServiceSpec {
    label: String,
    repo_root: PathBuf,
    runtime_dir: PathBuf,
    zeroclaw_bin: PathBuf,
    proxy_bin: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    path_env: String,
}

fn render_current_runtime_config(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let _manager = ModelRoutingManager::spawn(paths.clone(), config)?;
    Ok(())
}

fn ensure_release_binary(paths: &RuntimePaths, name: &str) -> Result<PathBuf> {
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

fn render_davis_launchd_plist(spec: &DavisServiceSpec) -> String {
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

fn davis_service_label() -> &'static str {
    "com.daviszeroclaw.zeroclaw"
}

fn davis_service_plist_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", davis_service_label())))
}

fn launchd_user_target() -> Result<String> {
    let uid = command_text(Command::new("id").arg("-u").env("PATH", tool_path_env()))?;
    Ok(format!("gui/{}", uid.trim()))
}

fn launchd_service_target(user_target: &str) -> String {
    format!("{user_target}/{}", davis_service_label())
}

fn bootout_davis_service(user_target: &str, plist_path: &Path) {
    let _ = Command::new("launchctl")
        .arg("bootout")
        .arg(user_target)
        .arg(plist_path)
        .env("PATH", tool_path_env())
        .status();
}

fn launchd_state_label(output: &str) -> String {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("state = "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("loaded")
        .to_string()
}

fn print_health_component(health: &Value, component: &str, label: &str) {
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

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

async fn start_browser_worker(paths: &RuntimePaths, config: &BrowserBridgeConfig) -> Result<()> {
    if !config.enabled {
        println!("Browser bridge is disabled; skipping browser worker.");
        return Ok(());
    }

    let worker_entry = paths.browser_worker_script_path();
    if !worker_entry.is_file() {
        bail!(
            "browser worker entry was not found: {}",
            worker_entry.display()
        );
    }

    ensure_browser_worker_deps(paths)?;

    let runner = command_path("bun")
        .or_else(|| command_path("node"))
        .ok_or_else(|| anyhow!("browser worker requires bun or node"))?;
    let runner_name = runner
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("runner")
        .to_string();

    println!("Starting browser worker with {runner_name}.");
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
        println!("No browser-worker package.json found; skipping dependency install.");
        return Ok(());
    }
    if worker_dir.join("node_modules").join("playwright").is_dir() {
        println!("Browser worker dependencies already installed.");
        return Ok(());
    }

    if let Some(bun) = command_path("bun") {
        println!("Installing browser worker dependencies with Bun.");
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
        println!("Installing browser worker dependencies with npm.");
        run_status(
            Command::new(npm)
                .arg("install")
                .env("PATH", tool_path_env())
                .current_dir(&worker_dir),
            "npm install",
        )?;
        return Ok(());
    }

    bail!("browser worker requires bun or npm to install dependencies");
}

fn install_mempalace(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let package = config.memory_integrations.mempalace.package;
    let python3 = require_command("python3").context("python3 is required to install MemPalace")?;
    let venv_dir = paths.mempalace_venv_dir();
    let python = paths.mempalace_python_path();
    let palace_dir = paths.mempalace_palace_dir();

    fs::create_dir_all(&paths.runtime_dir)?;
    if !python.is_file() {
        println!("Creating MemPalace venv: {}", venv_dir.display());
        run_status(
            Command::new(&python3)
                .arg("-m")
                .arg("venv")
                .arg(&venv_dir)
                .env("PATH", tool_path_env())
                .current_dir(&paths.repo_root),
            "python3 -m venv .runtime/davis/mempalace-venv",
        )?;
    } else {
        println!("MemPalace venv already exists: {}", venv_dir.display());
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
        "mempalace pip upgrade",
    )?;

    println!("Installing MemPalace package: {package}");
    run_status(
        Command::new(&python)
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--upgrade")
            .arg(&package)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
        "pip install mempalace",
    )?;

    fs::create_dir_all(&palace_dir)?;
    println!("MemPalace installed.");
    println!("Python: {}", python.display());
    println!("Palace: {}", palace_dir.display());
    println!("Next: daviszeroclaw memory mempalace enable");
    Ok(())
}

fn enable_mempalace(paths: &RuntimePaths) -> Result<()> {
    let config_path = paths.local_config_path();
    let raw = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let section = r#"[memory_integrations.mempalace]
enabled = true
python = ""
palace_dir = ""
package = "mempalace"
tool_timeout_secs = 30
"#;
    let updated = replace_toml_section(&raw, "[memory_integrations.mempalace]", section);
    fs::write(&config_path, updated)
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    println!("MemPalace enabled in {}", config_path.display());
    println!("Next: daviszeroclaw memory mempalace check");
    Ok(())
}

fn check_mempalace(paths: &RuntimePaths) -> Result<()> {
    let config = check_local_config(paths)?;
    let mempalace = config.memory_integrations.mempalace;
    let python = if mempalace.python.trim().is_empty() {
        paths.mempalace_python_path()
    } else {
        PathBuf::from(mempalace.python.trim())
    };
    let palace_dir = if mempalace.palace_dir.trim().is_empty() {
        paths.mempalace_palace_dir()
    } else {
        PathBuf::from(mempalace.palace_dir.trim())
    };

    println!("MemPalace config:");
    println!("- enabled: {}", mempalace.enabled);
    println!("- python: {}", python.display());
    println!("- palace_dir: {}", palace_dir.display());
    println!("- package: {}", mempalace.package);
    println!("- tool_timeout_secs: {}", mempalace.tool_timeout_secs);

    if !mempalace.enabled {
        bail!("MemPalace is not enabled. Run: daviszeroclaw memory mempalace enable");
    }
    if !python.is_file() {
        bail!(
            "MemPalace Python was not found: {}\nRun: daviszeroclaw memory mempalace install",
            python.display()
        );
    }
    fs::create_dir_all(&palace_dir)?;

    let import_check = command_output(
        Command::new(&python)
            .arg("-c")
            .arg("import mempalace; print('mempalace import ok')")
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
    )?;
    print_command_streams(&import_check.stdout, &import_check.stderr);
    if !import_check.status_success {
        bail!("MemPalace package import failed");
    }

    let help_check = command_output(
        Command::new(&python)
            .arg("-m")
            .arg("mempalace.mcp_server")
            .arg("--help")
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
    )?;
    if !help_check.status_success {
        print_command_streams(&help_check.stdout, &help_check.stderr);
        bail!("MemPalace MCP server did not respond to --help");
    }

    println!("MemPalace MCP server is available.");
    println!("Running MemPalace MCP smoke test.");
    let smoke_check = command_output(
        Command::new(&python)
            .arg("-c")
            .arg(MEMPALACE_SMOKE_TEST_SCRIPT)
            .arg(&palace_dir)
            .env("PATH", tool_path_env())
            .current_dir(&paths.repo_root),
    )?;
    print_command_streams(&smoke_check.stdout, &smoke_check.stderr);
    if !smoke_check.status_success {
        bail!("MemPalace MCP smoke test failed");
    }

    println!("Restart Davis to render the MCP server into ZeroClaw config.");
    Ok(())
}

const MEMPALACE_SMOKE_TEST_SCRIPT: &str = r#"
import json
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

palace = Path(sys.argv[1])
marker = f"davis_mempalace_check_{int(time.time())}"
drawer_content = f"{marker}: temporary MemPalace MCP check drawer. Delete after verification."
diary_content = f"{marker}: temporary MemPalace MCP check diary entry. Delete after verification."
subject = f"{marker}_subject"

proc = subprocess.Popen(
    [sys.executable, "-m", "mempalace.mcp_server", "--palace", str(palace)],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
)
next_id = 1

def request(method, params=None):
    global next_id
    payload = {"jsonrpc": "2.0", "id": next_id, "method": method}
    if params is not None:
        payload["params"] = params
    next_id += 1
    proc.stdin.write(json.dumps(payload) + "\n")
    proc.stdin.flush()
    line = proc.stdout.readline()
    if not line:
        stderr = proc.stderr.read()
        raise RuntimeError(f"MemPalace MCP server returned no response: {stderr}")
    response = json.loads(line)
    if "error" in response:
        raise RuntimeError(response["error"])
    return response

def tool(name, arguments=None):
    response = request("tools/call", {"name": name, "arguments": arguments or {}})
    text = response["result"]["content"][0]["text"]
    return json.loads(text)

def ensure(condition, message):
    if not condition:
        raise RuntimeError(message)

def cleanup_kg():
    db = palace / "knowledge_graph.sqlite3"
    if not db.is_file():
        return
    with sqlite3.connect(db) as conn:
        conn.execute("delete from triples where subject = ? or object = ?", (subject, subject))
        conn.execute("delete from entities where id = ?", (subject,))
        conn.commit()

drawer_id = None
diary_id = None
try:
    request("initialize", {
        "protocolVersion": "2025-11-25",
        "capabilities": {},
        "clientInfo": {"name": "daviszeroclaw-mempalace-check", "version": "0"},
    })
    tools = request("tools/list")["result"]["tools"]
    tool_names = {tool["name"] for tool in tools}
    required = {
        "mempalace_status",
        "mempalace_search",
        "mempalace_add_drawer",
        "mempalace_delete_drawer",
        "mempalace_diary_write",
        "mempalace_diary_read",
        "mempalace_kg_add",
        "mempalace_kg_query",
        "mempalace_kg_invalidate",
    }
    missing = sorted(required - tool_names)
    ensure(not missing, f"missing required tools: {', '.join(missing)}")

    status_before = tool("mempalace_status")

    added = tool("mempalace_add_drawer", {
        "wing": "davis",
        "room": "smoke-test",
        "content": drawer_content,
    })
    ensure(added.get("success"), f"add_drawer failed: {added.get('error')}")
    drawer_id = added.get("drawer_id")
    ensure(drawer_id, "add_drawer did not return drawer_id")

    search = tool("mempalace_search", {"query": marker, "limit": 3})
    results_text = json.dumps(search, ensure_ascii=False)
    ensure(drawer_content in results_text, "search did not return the smoke-test drawer")

    deleted = tool("mempalace_delete_drawer", {"drawer_id": drawer_id})
    ensure(deleted.get("success"), f"delete_drawer failed: {deleted.get('error')}")
    drawer_id = None

    search_after_delete = tool("mempalace_search", {"query": marker, "limit": 3})
    remaining = search_after_delete.get("results") or []
    ensure(
        not any(drawer_content in json.dumps(item, ensure_ascii=False) for item in remaining),
        "deleted drawer still appears in search results",
    )

    diary = tool("mempalace_diary_write", {
        "agent_name": "davis",
        "topic": "smoke-test",
        "entry": diary_content,
    })
    ensure(diary.get("success"), f"diary_write failed: {diary.get('error')}")
    diary_id = diary.get("entry_id")
    ensure(diary_id, "diary_write did not return entry_id")

    diary_read = tool("mempalace_diary_read", {"agent_name": "davis", "last_n": 5})
    ensure(diary_content in json.dumps(diary_read, ensure_ascii=False), "diary_read did not return the smoke-test entry")

    diary_delete = tool("mempalace_delete_drawer", {"drawer_id": diary_id})
    ensure(diary_delete.get("success"), f"delete diary entry failed: {diary_delete.get('error')}")
    diary_id = None

    kg_add = tool("mempalace_kg_add", {
        "subject": subject,
        "predicate": "check_predicate",
        "object": "check_object",
        "valid_from": "2026-04-17",
    })
    ensure(kg_add.get("success"), f"kg_add failed: {kg_add.get('error')}")

    kg_query = tool("mempalace_kg_query", {"entity": subject, "direction": "both"})
    ensure(kg_query.get("count", 0) >= 1, "kg_query did not return the smoke-test fact")

    kg_invalidate = tool("mempalace_kg_invalidate", {
        "subject": subject,
        "predicate": "check_predicate",
        "object": "check_object",
        "ended": "2026-04-17",
    })
    ensure(kg_invalidate.get("success"), f"kg_invalidate failed: {kg_invalidate.get('error')}")
    cleanup_kg()

    status_after = tool("mempalace_status")
    ensure(status_after.get("protocol"), "mempalace_status did not return Memory Protocol after smoke test")

    before_drawers = status_before.get("total_drawers")
    after_drawers = status_after.get("total_drawers")
    print("MemPalace MCP smoke test ok.")
    print(f"- tools: {len(tool_names)} available")
    print(f"- drawer/search/delete: ok")
    print(f"- diary write/read/delete: ok")
    print(f"- KG add/query/invalidate: ok")
    print(f"- Memory Protocol: ok")
    if before_drawers is not None and after_drawers is not None:
        print(f"- total_drawers: {before_drawers} -> {after_drawers}")
except Exception as exc:
    print(f"MemPalace MCP smoke test failed: {exc}", file=sys.stderr)
    print("Hint: if the error mentions SSL, handshake, or ONNX, remove a corrupt Chroma model cache and retry:", file=sys.stderr)
    print("  rm -f ~/.cache/chroma/onnx_models/all-MiniLM-L6-v2/onnx.tar.gz", file=sys.stderr)
    raise
finally:
    try:
        if drawer_id:
            tool("mempalace_delete_drawer", {"drawer_id": drawer_id})
    except Exception:
        pass
    try:
        if diary_id:
            tool("mempalace_delete_drawer", {"drawer_id": diary_id})
    except Exception:
        pass
    try:
        cleanup_kg()
    except Exception:
        pass
    proc.terminate()
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        proc.kill()
"#;

fn replace_toml_section(raw: &str, header: &str, replacement: &str) -> String {
    let lines = raw.lines().collect::<Vec<_>>();
    let Some(start) = lines.iter().position(|line| line.trim() == header) else {
        let mut output = raw.trim_end().to_string();
        output.push_str("\n\n");
        output.push_str(replacement.trim_end());
        output.push('\n');
        return output;
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| {
            let trimmed = line.trim();
            (trimmed.starts_with('[') && trimmed.ends_with(']')).then_some(index)
        })
        .unwrap_or(lines.len());

    let mut output = String::new();
    for line in &lines[..start] {
        output.push_str(line);
        output.push('\n');
    }
    output.push_str(replacement.trim_end());
    output.push('\n');
    for line in &lines[end..] {
        output.push_str(line);
        output.push('\n');
    }
    output
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

    println!("Runtime skills synced: {}", runtime_skills_dir.display());
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

fn install_skills(paths: &RuntimePaths) -> Result<()> {
    let installed = install_mempalace_vendor_skill(paths)?;

    println!("Installed vendor skills:");
    println!("- mempalace ({})", installed.display());
    println!("Next: daviszeroclaw skills sync");
    Ok(())
}

fn install_mempalace_vendor_skill(paths: &RuntimePaths) -> Result<PathBuf> {
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

fn render_mempalace_vendor_skill_adapter(paths: &RuntimePaths) -> String {
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

fn check_skills(paths: &RuntimePaths) -> Result<()> {
    let project_skills_dir = paths.repo_root.join("project-skills");
    let vendor_skills_dir = paths.repo_root.join("skills");
    let runtime_skills_dir = paths.workspace_dir().join("skills");

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

fn format_skill_count(count: usize) -> String {
    if count == 1 {
        "ok (1 skill)".to_string()
    } else {
        format!("ok ({count} skills)")
    }
}

fn runtime_skill_status(
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

fn report_mempalace_vendor_skill_status(vendor_skills_dir: &Path) {
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

fn report_mempalace_policy_skill_status(project_skills_dir: &Path) {
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

fn report_mempalace_mcp_status(paths: &RuntimePaths) {
    let config = match check_local_config(paths) {
        Ok(config) => config,
        Err(error) => {
            println!("MemPalace MCP: WARN config invalid ({error})");
            return;
        }
    };
    let mempalace = config.memory_integrations.mempalace;
    let python = if mempalace.python.trim().is_empty() {
        paths.mempalace_python_path()
    } else {
        PathBuf::from(mempalace.python.trim())
    };

    if !mempalace.enabled {
        println!("MemPalace MCP: WARN disabled (run: daviszeroclaw memory mempalace enable)");
        return;
    }
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

fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

fn build_shortcut(
    paths: &RuntimePaths,
    url: Option<String>,
    secret: Option<String>,
    no_secret: bool,
) -> Result<ShortcutBuild> {
    ensure_macos("Shortcut build")?;
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
                std::env::var("DAVIS_SHORTCUT_WEBHOOK_PORT").unwrap_or_else(|_| "3012".to_string());
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
    run_status_filtering_shortcuts_warnings(
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
    ensure_macos("Shortcut import")?;
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
                toml_string_value(&paths.runtime_config_path(), "channels.webhook", "secret")
                    .filter(|value| !value.is_empty())
            })
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

fn toml_string_array_value(path: &Path, section: &str, key: &str) -> Option<Vec<String>> {
    let raw = fs::read_to_string(path).ok()?;
    let parsed: toml::Value = raw.parse().ok()?;
    let mut value = &parsed;
    for part in section.split('.') {
        value = value.get(part)?;
    }
    Some(
        value
            .get(key)?
            .as_array()?
            .iter()
            .filter_map(|item| item.as_str())
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToString::to_string)
            .collect(),
    )
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
    println!("Checking iMessage permissions.");

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME environment variable is not set"))?;
    let messages_db = home.join("Library").join("Messages").join("chat.db");
    let messages_app = Path::new("/System/Applications/Messages.app");

    if !messages_app.is_dir() {
        bail!(
            "Messages.app was not found at {}. This macOS installation does not appear to support Messages.app.",
            messages_app.display()
        );
    }
    if !messages_db.is_file() {
        bail!(
            "{} was not found. Open Messages.app, sign in to iMessage, and send or receive at least one message before retrying.",
            messages_db.display()
        );
    }

    let sqlite3 = require_command("sqlite3")
        .context("sqlite3 is required to verify Messages database access")?;
    let sqlite_output = command_output(
        Command::new(sqlite3)
            .arg(&messages_db)
            .arg("select count(*) from message limit 1;")
            .env("PATH", tool_path_env()),
    )?;
    if !sqlite_output.status_success {
        bail!(
            "The current host cannot read the Messages database.\n   Open System Settings -> Privacy & Security -> Full Disk Access.\n   Grant access to the app that runs daviszeroclaw start, such as Terminal, iTerm, or Codex.\n   sqlite3 error: {}",
            sqlite_output.stderr.replace('\n', " ")
        );
    }

    println!("Checking Messages automation permission. macOS may ask whether to allow control of Messages.");
    let osascript = require_command("osascript")
        .context("osascript is required to verify Automation permission")?;
    let ae_output = command_output(
        Command::new(osascript)
            .arg("-e")
            .arg("tell application \"Messages\" to get name")
            .env("PATH", tool_path_env()),
    )?;
    if !ae_output.status_success {
        bail!(
            "The current host cannot control Messages.app through Apple Events.\n   Open System Settings -> Privacy & Security -> Automation.\n   Allow the current host app to control Messages.\n   osascript error: {}",
            ae_output.stderr.replace('\n', " ")
        );
    }

    println!("iMessage permissions OK.");
    Ok(())
}

fn inspect_imessage(paths: &RuntimePaths) -> Result<()> {
    ensure_macos("iMessage inspect")?;
    println!("Inspecting local iMessage configuration...");

    let home = home_dir()?;
    let messages_db = home.join("Library").join("Messages").join("chat.db");
    let accounts_db = home
        .join("Library")
        .join("Accounts")
        .join("Accounts4.sqlite");
    let messages_app = Path::new("/System/Applications/Messages.app");

    if !messages_app.is_dir() {
        bail!(
            "Messages.app was not found at {}. This macOS installation does not appear to support Messages.app.",
            messages_app.display()
        );
    }
    if !messages_db.is_file() {
        bail!(
            "{} was not found. Open Messages.app, sign in to iMessage, and send or receive at least one message before retrying.",
            messages_db.display()
        );
    }

    let sqlite3 =
        require_command("sqlite3").context("sqlite3 is required to read iMessage diagnostics")?;
    ensure_sqlite_readable(&sqlite3, &messages_db, "Messages database")?;

    let apple_accounts = if accounts_db.is_file() {
        ensure_sqlite_readable(&sqlite3, &accounts_db, "Accounts database")?;
        imessage_apple_accounts(&sqlite3, &accounts_db)?
    } else {
        Vec::new()
    };
    let candidates = imessage_allowed_contact_candidates(&sqlite3, &messages_db)?;
    let configured_contacts =
        toml_string_array_value(&paths.local_config_path(), "imessage", "allowed_contacts")
            .unwrap_or_default();

    println!();
    println!("Messages Apple Account:");
    if apple_accounts.is_empty() {
        println!("- Not found in Accounts4.sqlite.");
    } else {
        for account in &apple_accounts {
            println!("- {account}");
        }
    }

    println!();
    println!("Davis config file:");
    println!("- {}", paths.local_config_path().display());

    println!();
    println!("Configured allowed_contacts:");
    if configured_contacts.is_empty() {
        println!("- No string values found in [imessage].allowed_contacts.");
    } else {
        for contact in &configured_contacts {
            println!("- {contact}");
        }
    }

    println!();
    println!("Configuration status:");
    if candidates.is_empty() {
        println!("- Unable to verify allowed_contacts from iMessage metadata.");
        println!(
            "- Send a test iMessage from your iPhone to this Mac, then run this command again."
        );
    } else {
        let best_candidate = &candidates[0];
        let config_contains_best = configured_contacts
            .iter()
            .any(|contact| contact == &best_candidate.identity);

        if config_contains_best {
            println!(
                "OK: [imessage].allowed_contacts already includes the best observed sender: {}.",
                best_candidate.identity
            );
        } else if configured_contacts.is_empty() {
            println!(
                "Update needed: [imessage].allowed_contacts is empty or missing the best observed sender: {}.",
                best_candidate.identity
            );
        } else {
            println!(
                "Review needed: [imessage].allowed_contacts does not include the best observed sender: {}.",
                best_candidate.identity
            );
        }

        println!();
        println!("Observed allowed_contacts candidates:");
        for (index, candidate) in candidates.iter().take(5).enumerate() {
            let suffix = if index == 0 { " (best match)" } else { "" };
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

        if !config_contains_best {
            println!();
            println!("Suggested config:");
            println!("[imessage]");
            println!("allowed_contacts = [\"{}\"]", best_candidate.identity);
        }
    }

    println!();
    println!("Note: inspect reads account, handle, direction, and timestamp metadata only. It does not read message bodies.");
    Ok(())
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME environment variable is not set"))
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
            "The current host cannot read the {label}: {}\n   Open System Settings -> Privacy & Security -> Full Disk Access.\n   Grant access to the app that runs daviszeroclaw, such as Terminal, iTerm, or Codex.\n   sqlite3 error: {}",
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
    ensure_macos("express login page")?;
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

fn stop_process(name: &str, pid_file: &Path) -> Result<()> {
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
            println!("Model routing initialization failed: {payload}");
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

fn run_status_filtering_shortcuts_warnings(command: &mut Command, description: &str) -> Result<()> {
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

fn print_command_streams(stdout: &str, stderr: &str) {
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

fn filter_known_shortcuts_warnings(stderr: &str) -> String {
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

fn report_skill_inventory(paths: &RuntimePaths) {
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

fn skill_name_set(root: &Path) -> BTreeSet<String> {
    skill_names(root).into_iter().collect()
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
        bail!("{feature} is only supported on macOS")
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
    fn replace_toml_section_appends_missing_section() {
        let raw = "[webhook]\nsecret = \"x\"\n";
        let updated = replace_toml_section(
            raw,
            "[memory_integrations.mempalace]",
            "[memory_integrations.mempalace]\nenabled = true\n",
        );

        assert!(updated.contains("[webhook]\nsecret = \"x\""));
        assert!(updated.contains("[memory_integrations.mempalace]\nenabled = true"));
    }

    #[test]
    fn replace_toml_section_replaces_existing_section() {
        let raw = "[webhook]\nsecret = \"x\"\n\n[memory_integrations.mempalace]\nenabled = false\npython = \"old\"\n\n[browser_bridge]\nenabled = true\n";
        let updated = replace_toml_section(
            raw,
            "[memory_integrations.mempalace]",
            "[memory_integrations.mempalace]\nenabled = true\n",
        );

        assert!(updated.contains("[memory_integrations.mempalace]\nenabled = true\n"));
        assert!(!updated.contains("python = \"old\""));
        assert!(updated.contains("[browser_bridge]\nenabled = true"));
    }

    #[test]
    fn toml_string_array_value_reads_imessage_allowed_contacts() {
        let root = unique_test_dir("toml-string-array");
        fs::create_dir_all(&root).unwrap();
        let config_path = root.join("local.toml");
        fs::write(
            &config_path,
            r#"
[imessage]
allowed_contacts = [" +8618672954807 ", "user@example.com"]
"#,
        )
        .unwrap();

        assert_eq!(
            toml_string_array_value(&config_path, "imessage", "allowed_contacts").unwrap(),
            vec!["+8618672954807".to_string(), "user@example.com".to_string()]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn filter_known_shortcuts_warnings_removes_debug_description_noise_only() {
        let raw = concat!(
            "ERROR: Unrecognized attribute string flag '?' in attribute string ",
            "\"T@\\\"NSString\\\",?,R,C\" for property debugDescription\n",
            "real error\n"
        );

        assert_eq!(filter_known_shortcuts_warnings(raw), "real error");
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
        assert!(error.contains("duplicate skill name detected"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn install_mempalace_vendor_skill_writes_thin_adapter_skill() {
        let root = unique_test_dir("install_mempalace_vendor_skill");
        let paths = RuntimePaths {
            repo_root: root.join("repo"),
            runtime_dir: root.join("runtime"),
        };
        fs::create_dir_all(&paths.repo_root).unwrap();

        let skill_dir = install_mempalace_vendor_skill(&paths).unwrap();
        let skill = fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();

        assert!(skill.contains("name: mempalace"));
        assert!(skill.contains("mempalace instructions <command>"));
        assert!(skill.contains("project skill mempalace-memory"));
        assert!(skill.contains("mempalace-venv/bin/python"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_skill_status_reports_synced_and_stale_states() {
        let project = BTreeSet::from(["mempalace-memory".to_string()]);
        let vendor = BTreeSet::from(["mempalace".to_string()]);
        let synced = BTreeSet::from(["mempalace-memory".to_string(), "mempalace".to_string()]);
        let stale = BTreeSet::from(["mempalace-memory".to_string(), "old".to_string()]);

        assert_eq!(
            runtime_skill_status(&project, &vendor, &synced),
            "synced (2 skills)"
        );
        assert_eq!(
            runtime_skill_status(&project, &vendor, &stale),
            "WARN stale (missing: mempalace; extra: old)"
        );
    }

    #[test]
    fn render_davis_launchd_plist_uses_davis_runtime_config() {
        let spec = DavisServiceSpec {
            label: davis_service_label().to_string(),
            repo_root: PathBuf::from("/tmp/Davis ZeroClaw"),
            runtime_dir: PathBuf::from("/tmp/Davis ZeroClaw/.runtime/davis"),
            zeroclaw_bin: PathBuf::from("/opt/homebrew/bin/zeroclaw"),
            proxy_bin: PathBuf::from("/tmp/Davis ZeroClaw/target/release/davis-local-proxy"),
            stdout_path: PathBuf::from("/tmp/Davis ZeroClaw/.runtime/davis/stdout.log"),
            stderr_path: PathBuf::from("/tmp/Davis ZeroClaw/.runtime/davis/stderr.log"),
            path_env: "/opt/homebrew/bin:/usr/local/bin".to_string(),
        };

        let plist = render_davis_launchd_plist(&spec);
        assert!(plist.contains("<string>com.daviszeroclaw.zeroclaw</string>"));
        assert!(
            plist.contains("daemon --config-dir &apos;/tmp/Davis ZeroClaw/.runtime/davis&apos;")
        );
        assert!(plist.contains("<key>ZEROCLAW_CONFIG_DIR</key>"));
        assert!(plist.contains("<key>DAVIS_REPO_ROOT</key>"));
        assert!(!plist.contains("/opt/homebrew/var/zeroclaw"));
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("daviszeroclaw-{name}-{}", unique_suffix()));
        if path.exists() {
            fs::remove_dir_all(&path).unwrap();
        }
        path
    }
}
