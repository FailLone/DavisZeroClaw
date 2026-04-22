use crate::{
    build_article_strategy_review_input, check_local_config, ArticleMemoryRecordStatus,
    RuntimePaths,
};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

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
    /// Manage runtime standard operating procedures (SOPs).
    Sops {
        #[command(subcommand)]
        command: SopsCommand,
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
    /// Manage Crawl4AI-backed crawl profiles and tasks.
    Crawl {
        #[command(subcommand)]
        command: CrawlCommand,
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
enum SopsCommand {
    /// Synchronize project SOPs into the runtime workspace.
    Sync,
    /// Check runtime SOP presence and validate loaded definitions.
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
enum CrawlCommand {
    /// Install Crawl4AI into the Davis runtime Python environment.
    Install,
    /// Check Crawl4AI runtime, adapter, and Python health.
    Check,
    /// List built-in crawl sources.
    Source {
        #[command(subcommand)]
        command: CrawlSourceCommand,
    },
    /// Run a built-in crawl source and print the result as JSON.
    Run {
        source: String,
        #[arg(long)]
        refresh: bool,
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        compact: bool,
    },
    /// Manage persistent Crawl4AI browser profiles.
    Profile {
        #[command(subcommand)]
        command: CrawlProfileCommand,
    },
}

#[derive(Debug, Subcommand)]
enum CrawlSourceCommand {
    /// List built-in crawl sources.
    List,
}

#[derive(Debug, Subcommand)]
enum CrawlProfileCommand {
    /// Open a login page inside a managed Crawl4AI profile.
    Login {
        #[arg(value_enum)]
        profile: CrawlProfile,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CrawlProfile {
    #[value(name = "express-ali", alias = "ali")]
    ExpressAli,
    #[value(name = "express-jd", alias = "jd")]
    ExpressJd,
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
        Commands::Sops { command } => match command {
            SopsCommand::Sync => sync_runtime_sops(&paths),
            SopsCommand::Check => check_sops(&paths),
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
        Commands::Crawl { command } => match command {
            CrawlCommand::Install => install_crawl4ai(&paths),
            CrawlCommand::Check => check_crawl4ai(&paths),
            CrawlCommand::Source { command } => match command {
                CrawlSourceCommand::List => list_crawl_sources(),
            },
            CrawlCommand::Run {
                source,
                refresh,
                query,
                compact,
            } => run_crawl_source(&paths, &source, query, refresh, compact).await,
            CrawlCommand::Profile { command } => match command {
                CrawlProfileCommand::Login { profile } => {
                    crawl_profile_login(&paths, profile).await
                }
            },
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
        },
    }
}

mod articles;
use articles::*;

mod service;
use service::*;

mod mempalace;
use mempalace::*;

mod skills;
use skills::*;
pub use skills::{
    sanitize_markdown_script_links, sync_runtime_skills, sync_runtime_sops, sync_workspace_files,
};

mod shortcut;
pub use shortcut::customize_shortcut_json;
use shortcut::*;

mod crawl;
use crawl::*;

mod process;
pub(crate) use process::tool_path_env;
use process::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::collections::BTreeSet;
    use std::fs;

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
    fn upsert_mcp_server_entry_appends_when_absent() {
        let raw = "[webhook]\nsecret = \"x\"\n";
        let entry =
            "[[mcp.servers]]\nname = \"mempalace\"\ntransport = \"stdio\"\ncommand = \"/p/py\"\n";
        let updated = super::mempalace::upsert_mcp_server_entry(raw, "mempalace", entry);

        assert!(updated.contains("[webhook]\nsecret = \"x\""));
        assert!(updated.contains("name = \"mempalace\""));
        assert!(updated.contains("command = \"/p/py\""));
    }

    #[test]
    fn upsert_mcp_server_entry_replaces_only_matching_block() {
        let raw = "\
[[mcp.servers]]
name = \"filesystem\"
transport = \"stdio\"
command = \"/fs/bin\"

[[mcp.servers]]
name = \"mempalace\"
transport = \"stdio\"
command = \"/old/py\"

[crawl4ai]
enabled = true
";
        let entry =
            "[[mcp.servers]]\nname = \"mempalace\"\ntransport = \"stdio\"\ncommand = \"/new/py\"\n";
        let updated = super::mempalace::upsert_mcp_server_entry(raw, "mempalace", entry);

        // The filesystem block is untouched.
        assert!(updated.contains("name = \"filesystem\""));
        assert!(updated.contains("command = \"/fs/bin\""));
        // The mempalace block is replaced.
        assert!(updated.contains("command = \"/new/py\""));
        assert!(!updated.contains("command = \"/old/py\""));
        // The following section survives.
        assert!(updated.contains("[crawl4ai]\nenabled = true"));
    }

    #[test]
    fn upsert_mcp_server_entry_does_not_touch_other_servers_when_missing() {
        let raw = "\
[[mcp.servers]]
name = \"filesystem\"
transport = \"stdio\"
command = \"/fs/bin\"
";
        let entry =
            "[[mcp.servers]]\nname = \"mempalace\"\ntransport = \"stdio\"\ncommand = \"/p/py\"\n";
        let updated = super::mempalace::upsert_mcp_server_entry(raw, "mempalace", entry);

        assert!(updated.contains("name = \"filesystem\""));
        assert!(updated.contains("command = \"/fs/bin\""));
        assert!(updated.contains("name = \"mempalace\""));
        assert!(updated.contains("command = \"/p/py\""));
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
    fn sync_runtime_sops_copies_and_marks_sources() {
        let root = unique_test_dir("sync_runtime_sops_copies");
        let paths = RuntimePaths {
            repo_root: root.join("repo"),
            runtime_dir: root.join("runtime"),
        };
        let project = root.join("project-sops");
        fs::create_dir_all(project.join("my-parcels")).unwrap();
        fs::write(
            project.join("my-parcels").join("SOP.toml"),
            "[sop]\nname = \"my-parcels\"\ndescription = \"test\"\n\n[[triggers]]\ntype = \"manual\"\n",
        )
        .unwrap();
        fs::write(
            project.join("my-parcels").join("SOP.md"),
            "## Steps\n\n1. **Fetch** — Use [doc](references/api.md).\n",
        )
        .unwrap();

        sync_runtime_sops_with_sources(&paths, &project).unwrap();

        let runtime_sops = paths.workspace_sops_dir();
        assert!(runtime_sops.join("my-parcels").join("SOP.toml").is_file());
        assert!(runtime_sops.join("my-parcels").join("SOP.md").is_file());
        assert_eq!(
            fs::read_to_string(runtime_sops.join("my-parcels").join(".davis-sop-source")).unwrap(),
            "project-sops\n"
        );

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
    fn runtime_sop_status_reports_synced_and_stale_states() {
        let synced = BTreeSet::from(["my-parcels".to_string()]);
        let stale = BTreeSet::from(["old".to_string()]);
        let empty = BTreeSet::new();

        assert_eq!(runtime_sop_status(&synced, &synced), "synced (1 SOP)");
        assert_eq!(
            runtime_sop_status(&synced, &stale),
            "WARN stale (missing: my-parcels; extra: old)"
        );
        assert_eq!(runtime_sop_status(&empty, &empty), "ok (empty)");
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
