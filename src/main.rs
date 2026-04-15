use davis_zero_claw::{
    build_app, check_local_config, load_control_config, zeroclaw_env_vars, AppState, HaClient,
    HaMcpClient, HaState, ModelRoutingManager, RuntimePaths,
};
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
        Some("print-browser-worker-env") => {
            let config = check_local_config(&paths)?;
            let browser_config = &config.browser_bridge;
            println!(
                "export DAVIS_BROWSER_BRIDGE_ENABLED='{}'",
                if browser_config.enabled {
                    "true"
                } else {
                    "false"
                }
            );
            println!(
                "export DAVIS_BROWSER_BRIDGE_PORT='{}'",
                browser_config.worker_port
            );
            println!(
                "export DAVIS_BROWSER_DEFAULT_PROFILE='{}'",
                shell_single_quote(&browser_config.default_profile)
            );
            println!(
                "export DAVIS_BROWSER_PROFILES_JSON='{}'",
                shell_single_quote(&serde_json::to_string(&browser_config.profiles)?)
            );
            println!(
                "export DAVIS_BROWSER_REMOTE_DEBUGGING_URL='{}'",
                shell_single_quote(&browser_config.user_session.remote_debugging_url)
            );
            println!(
                "export DAVIS_BROWSER_ALLOW_APPLESCRIPT_FALLBACK='{}'",
                if browser_config.user_session.allow_applescript_fallback {
                    "true"
                } else {
                    "false"
                }
            );
            println!(
                "export DAVIS_BROWSER_SCREENSHOTS_DIR='{}'",
                shell_single_quote(&paths.browser_screenshots_dir().display().to_string())
            );
            println!(
                "export DAVIS_BROWSER_PROFILES_DIR='{}'",
                shell_single_quote(&paths.browser_profiles_root().display().to_string())
            );
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
    let routing = ModelRoutingManager::spawn(paths.clone(), local_config.clone())?;
    let state = AppState::new(
        client,
        mcp_client,
        paths,
        control_config,
        Arc::new(local_config.browser_bridge.clone()),
        routing,
    );
    let app = build_app(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 3010));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn shell_single_quote(value: &str) -> String {
    value.replace('\'', "'\"'\"'")
}
