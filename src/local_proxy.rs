use crate::{
    build_app, build_shortcut_bridge_app, check_local_config, load_control_config,
    render_runtime_config, zeroclaw_env_vars, AppState, Crawl4aiProfileLocks, Crawl4aiSupervisor,
    HaClient, HaMcpClient, HaState, IngestQueue, IngestWorkerDeps, IngestWorkerPool, RuntimePaths,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn run_local_proxy() -> anyhow::Result<()> {
    let paths = RuntimePaths::from_env();
    match std::env::args().nth(1).as_deref() {
        Some("check-config") => {
            check_local_config(&paths)?;
            println!("local.toml ok");
            return Ok(());
        }
        Some("print-zeroclaw-env") => {
            let config = check_local_config(&paths)?;
            for (key, value) in zeroclaw_env_vars(&config) {
                println!("export {}='{}'", key, shell_single_quote(&value));
            }
            return Ok(());
        }
        Some("check-ha") => {
            let config = check_local_config(&paths)?;
            let client = HaClient::from_credentials(
                &config.home_assistant.url,
                &config.home_assistant.token,
            )
            .map_err(|err| anyhow::anyhow!("{err:?}"))?;
            let mcp_client = HaMcpClient::from_credentials(
                &config.home_assistant.url,
                &config.home_assistant.token,
            )
            .map_err(|err| anyhow::anyhow!("{err:?}"))?;
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
            return Ok(());
        }
        _ => {}
    }

    // Daemon mode — bring up structured logging before we talk to HA or
    // bind any sockets. CLI subcommands above intentionally skipped this
    // to keep their stdout clean.
    crate::init_tracing();
    tracing::info!("davis-local-proxy starting");

    std::fs::create_dir_all(paths.state_dir())?;
    let local_config = check_local_config(&paths)?;
    let control_config = Arc::new(load_control_config(&paths)?);
    let client = HaClient::from_credentials(
        &local_config.home_assistant.url,
        &local_config.home_assistant.token,
    )
    .map_err(|err| anyhow::anyhow!("{err:?}"))?;
    let mcp_client = HaMcpClient::from_credentials(
        &local_config.home_assistant.url,
        &local_config.home_assistant.token,
    )
    .map_err(|err| anyhow::anyhow!("{err:?}"))?;
    render_runtime_config(&paths, &local_config)?;

    // Bring the crawl4ai adapter up alongside the daemon. A start failure
    // (broken venv, port conflict, missing Python) drops us into a disabled
    // stub so unrelated routes (HA, article memory, advisor) still serve;
    // any `/express/*` request will surface `Crawl4aiError::Disabled`.
    let crawl4ai_supervisor = if local_config.crawl4ai.enabled {
        match Crawl4aiSupervisor::start(paths.clone(), local_config.crawl4ai.clone()).await {
            Ok(sup) => {
                tracing::info!("crawl4ai supervisor ready");
                Arc::new(sup)
            }
            Err(err) => {
                tracing::error!(
                    error = %err,
                    "crawl4ai supervisor failed to start; continuing without crawl support",
                );
                Arc::new(Crawl4aiSupervisor::disabled(paths.clone()))
            }
        }
    } else {
        tracing::info!("crawl4ai disabled in local config");
        Arc::new(Crawl4aiSupervisor::disabled(paths.clone()))
    };

    // Build a single profile_locks map shared by AppState's /express/*
    // routes and the ingest worker pool. Task 10 created the queue;
    // Task 11 adds the workers that actually process submitted jobs.
    let profile_locks: Crawl4aiProfileLocks = Arc::new(Mutex::new(HashMap::new()));
    let ingest_config = Arc::new(local_config.article_memory.ingest.clone());
    let ingest_queue = Arc::new(IngestQueue::load_or_create(&paths, ingest_config.clone()));

    // Rule-learning arcs are hoisted out of the `ingest_config.enabled` guard
    // so `AppState` (and therefore the `articles rule-learn` HTTP endpoints)
    // always has a handle, even when ingest workers are disabled. The workers
    // and hourly learner clone from these same arcs below.
    let learned_rules = Arc::new(crate::article_memory::LearnedRuleStore::load(
        &paths,
        Some(
            &paths
                .repo_root
                .join("config/davis/article_memory_overrides.toml"),
        ),
    )?);
    let rule_stats = Arc::new(crate::article_memory::RuleStatsStore::load(&paths)?);
    let sample_store = Arc::new(crate::article_memory::SampleStore::new(&paths));

    // Spawn the MemPalace sink up-front so the ingest worker can project into
    // it. Clone the sink into AppState via `with_mempalace_sink` below.
    let mempalace_sink = crate::mempalace_sink::MemPalaceSink::spawn(&paths);
    let ingest_sink: Arc<dyn crate::mempalace_sink::MempalaceEmitter> =
        Arc::new(mempalace_sink.clone());

    if ingest_config.enabled {
        let providers_arc = Arc::new(local_config.providers.clone());

        IngestWorkerPool::spawn(
            ingest_queue.clone(),
            IngestWorkerDeps {
                paths: paths.clone(),
                crawl4ai_config: Arc::new(local_config.crawl4ai.clone()),
                supervisor: crawl4ai_supervisor.clone(),
                profile_locks: profile_locks.clone(),
                article_memory_config: Arc::new(local_config.article_memory.clone()),
                providers: providers_arc.clone(),
                ingest_config: ingest_config.clone(),
                imessage_config: Arc::new(local_config.imessage.clone()),
                extract_config: Arc::new(local_config.article_memory.extract.clone()),
                quality_gate_config: Arc::new(local_config.article_memory.quality_gate.clone()),
                learned_rules: learned_rules.clone(),
                rule_stats: rule_stats.clone(),
                sample_store: sample_store.clone(),
                mempalace_sink: ingest_sink.clone(),
            },
            ingest_config.max_concurrency,
        );
        tracing::info!(
            workers = ingest_config.max_concurrency,
            "article memory ingest workers started"
        );

        // Spawn the hourly rule-learning worker (noop if disabled in config).
        let gate_toml = &local_config.article_memory.quality_gate;
        crate::article_memory::RuleLearningWorker::spawn(crate::article_memory::RuleLearningDeps {
            paths: paths.clone(),
            learned_rules: learned_rules.clone(),
            rule_stats: rule_stats.clone(),
            sample_store: sample_store.clone(),
            providers: providers_arc,
            config: Arc::new(local_config.article_memory.rule_learning.clone()),
            quality_gate: Arc::new(crate::article_memory::QualityGateConfig {
                enabled: gate_toml.enabled,
                min_markdown_chars: gate_toml.min_markdown_chars,
                min_kept_ratio: gate_toml.min_kept_ratio,
                min_paragraphs: gate_toml.min_paragraphs,
                max_link_density: gate_toml.max_link_density,
                boilerplate_markers: gate_toml.boilerplate_markers.clone(),
            }),
            mempalace_sink: ingest_sink.clone(),
        });
    } else {
        tracing::info!("article memory ingest disabled by config");
    }

    let state = AppState::new(
        client,
        mcp_client,
        paths,
        control_config,
        Arc::new(local_config.crawl4ai.clone()),
        crawl4ai_supervisor,
        Arc::new(local_config.article_memory.clone()),
        Arc::new(local_config.providers.clone()),
        local_config.webhook.secret.clone(),
        profile_locks,
        ingest_queue,
        learned_rules,
        rule_stats,
        sample_store,
    )
    .with_mempalace_sink(mempalace_sink);
    let app = build_app(state.clone());
    let shortcut_bridge_app = build_shortcut_bridge_app(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3010));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let shortcut_bridge_addr = SocketAddr::from(([0, 0, 0, 0], 3012));
    let shortcut_bridge_listener = tokio::net::TcpListener::bind(shortcut_bridge_addr).await?;
    tokio::try_join!(
        axum::serve(listener, app),
        axum::serve(shortcut_bridge_listener, shortcut_bridge_app),
    )?;
    Ok(())
}

fn shell_single_quote(value: &str) -> String {
    value.replace('\'', "'\"'\"'")
}
