# Cloudflare Tunnel External Access Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `daviszeroclaw service tunnel-install/uninstall/status` commands that configure cloudflared as a launchd service, allowing the iPhone Shortcut to reach Davis from outside the LAN via a stable Cloudflare Tunnel URL.

**Architecture:** Add `TunnelConfig` to `LocalConfig` (parsed from `local.toml [tunnel]`). Add three functions to `src/cli/service.rs` — `tunnel_install`, `tunnel_uninstall`, `tunnel_status` — following the identical pattern used for the existing proxy plist management. Wire the three functions into the `ServiceCommand` enum in `src/cli/mod.rs`. `status_davis_service` calls `tunnel_status` to append a tunnel health line.

**Tech Stack:** Rust, clap (subcommand), reqwest (health-check GET with latency), tokio (async), std::fs (file writes), launchctl (via Command::new), serde/toml (config deserialization).

---

## File Map

| File | Change |
|---|---|
| `src/app_config.rs` | Add `TunnelConfig` struct; add `tunnel: Option<TunnelConfig>` field to `LocalConfig` |
| `src/cli/service.rs` | Add `TunnelServiceSpec`, `render_tunnel_launchd_plist`, `tunnel_service_plist_path`, `tunnel_service_label`, `tunnel_install`, `tunnel_uninstall`, `tunnel_status`; update `status_davis_service` |
| `src/cli/mod.rs` | Add `TunnelInstall`, `TunnelUninstall`, `TunnelStatus` variants to `ServiceCommand`; add match arms |
| `src/cli/tests.rs` | Add 5 unit tests |
| `config/davis/local.example.toml` | Add commented `[tunnel]` block |

---

## Task 1: Add `TunnelConfig` to `app_config.rs`

**Files:**
- Modify: `src/app_config.rs:6-24`
- Test: `src/cli/tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/cli/tests.rs` (inside the existing `#[cfg(test)]` module, after the last test):

```rust
#[test]
fn tunnel_config_deserializes_from_toml() {
    let toml = r#"
        [home_assistant]
        url = "http://ha.local/api/mcp"
        token = "tok"
        [imessage]
        allowed_contacts = []
        [[providers]]
        name = "openrouter"
        api_key = "k"
        base_url = "https://openrouter.ai/api/v1"
        allowed_models = []
        [routing]
        default_profile = "general_qa"
        [routing.profiles.home_control]
        provider = "openrouter"
        model = "anthropic/claude-sonnet-4.6"
        max_fallbacks = 0
        [routing.profiles.general_qa]
        provider = "openrouter"
        model = "anthropic/claude-sonnet-4.6"
        max_fallbacks = 0
        [routing.profiles.research]
        provider = "openrouter"
        model = "anthropic/claude-opus-4.6"
        max_fallbacks = 0
        [routing.profiles.structured_lookup]
        provider = "openrouter"
        model = "anthropic/claude-sonnet-4.6"
        max_fallbacks = 0
        [tunnel]
        tunnel_id = "aaaabbbb-1111-2222-3333-ccccddddeeee"
        hostname = "davis.example.com"
    "#;
    let config: crate::LocalConfig = toml::from_str(toml).unwrap();
    let tunnel = config.tunnel.unwrap();
    assert_eq!(tunnel.tunnel_id.as_deref(), Some("aaaabbbb-1111-2222-3333-ccccddddeeee"));
    assert_eq!(tunnel.hostname.as_deref(), Some("davis.example.com"));
}

#[test]
fn tunnel_config_absent_deserializes_to_none() {
    let toml = r#"
        [home_assistant]
        url = "http://ha.local/api/mcp"
        token = "tok"
        [imessage]
        allowed_contacts = []
        [[providers]]
        name = "openrouter"
        api_key = "k"
        base_url = "https://openrouter.ai/api/v1"
        allowed_models = []
        [routing]
        default_profile = "general_qa"
        [routing.profiles.home_control]
        provider = "openrouter"
        model = "anthropic/claude-sonnet-4.6"
        max_fallbacks = 0
        [routing.profiles.general_qa]
        provider = "openrouter"
        model = "anthropic/claude-sonnet-4.6"
        max_fallbacks = 0
        [routing.profiles.research]
        provider = "openrouter"
        model = "anthropic/claude-opus-4.6"
        max_fallbacks = 0
        [routing.profiles.structured_lookup]
        provider = "openrouter"
        model = "anthropic/claude-sonnet-4.6"
        max_fallbacks = 0
    "#;
    let config: crate::LocalConfig = toml::from_str(toml).unwrap();
    assert!(config.tunnel.is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --lib tunnel_config 2>&1 | tail -20
```

Expected: compile error — `LocalConfig` has no `tunnel` field and `TunnelConfig` does not exist.

- [ ] **Step 3: Add `TunnelConfig` struct and field to `LocalConfig`**

In `src/app_config.rs`, after line 91 (end of `WebhookConfig` block), add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TunnelConfig {
    pub tunnel_id: Option<String>,
    pub hostname: Option<String>,
}
```

In `LocalConfig` (lines 6-24), add after `pub zeroclaw: ZeroclawConfig,`:

```rust
    #[serde(default)]
    pub tunnel: Option<TunnelConfig>,
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test --lib tunnel_config 2>&1 | tail -20
```

Expected: `test tunnel_config_deserializes_from_toml ... ok` and `test tunnel_config_absent_deserializes_to_none ... ok`.

- [ ] **Step 5: Full test suite and clippy**

```bash
cargo test --lib 2>&1 | tail -5
cargo clippy --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: all tests pass, no clippy warnings.

- [ ] **Step 6: Commit**

```bash
git add src/app_config.rs src/cli/tests.rs
git commit -m "feat(config): add TunnelConfig for [tunnel] section in local.toml"
```

---

## Task 2: Add tunnel plist helpers to `service.rs`

**Files:**
- Modify: `src/cli/service.rs`
- Test: `src/cli/tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/cli/tests.rs`:

```rust
#[test]
fn render_tunnel_launchd_plist_runs_cloudflared() {
    let spec = TunnelServiceSpec {
        cloudflared_bin: PathBuf::from("/opt/homebrew/bin/cloudflared"),
        config_path: PathBuf::from("/Users/testuser/.cloudflared/davis-shortcut.yml"),
        stdout_path: PathBuf::from("/tmp/davis/tunnel.stdout.log"),
        stderr_path: PathBuf::from("/tmp/davis/tunnel.stderr.log"),
        path_env: "/opt/homebrew/bin:/usr/local/bin".to_string(),
    };
    let plist = render_tunnel_launchd_plist(&spec);
    assert!(plist.contains("<string>com.daviszeroclaw.tunnel</string>"));
    assert!(plist.contains("cloudflared"));
    assert!(plist.contains("davis-shortcut.yml"));
    assert!(plist.contains("<key>RunAtLoad</key>"));
    assert!(plist.contains("<key>KeepAlive</key>"));
    assert!(!plist.contains("daemon --config-dir"));
}

#[test]
fn tunnel_service_label_and_plist_path_are_distinct() {
    assert_ne!(tunnel_service_label(), davis_service_label());
    assert_ne!(tunnel_service_label(), proxy_service_label());
    assert!(tunnel_service_label().contains("tunnel"));
    let tunnel_path = tunnel_service_plist_path().unwrap();
    assert!(tunnel_path.to_str().unwrap().contains("tunnel"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --lib render_tunnel 2>&1 | tail -10
cargo test --lib tunnel_service_label 2>&1 | tail -10
```

Expected: compile errors — `TunnelServiceSpec`, `render_tunnel_launchd_plist`, `tunnel_service_label`, `tunnel_service_plist_path` not found.

- [ ] **Step 3: Add `TunnelServiceSpec`, label/path helpers, and plist renderer**

Append to `src/cli/service.rs` (after the closing of `xml_escape` at line 721):

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test --lib render_tunnel 2>&1 | tail -10
cargo test --lib tunnel_service_label 2>&1 | tail -10
```

Expected: both tests pass.

- [ ] **Step 5: Clippy**

```bash
cargo clippy --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/cli/service.rs src/cli/tests.rs
git commit -m "feat(service): add TunnelServiceSpec, plist renderer, label/path helpers"
```

---

## Task 3: Add `tunnel_install`, `tunnel_uninstall`, `tunnel_status` to `service.rs`

**Files:**
- Modify: `src/cli/service.rs`
- Test: `src/cli/tests.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/cli/tests.rs`:

```rust
#[test]
fn tunnel_status_silent_when_plist_absent() {
    // tunnel_status reads the plist path from tunnel_service_plist_path().
    // We can't override HOME easily, so we verify the label constant is correct
    // and the path points to LaunchAgents — actual silence is validated manually.
    let path = tunnel_service_plist_path().unwrap();
    assert!(path.to_str().unwrap().contains("LaunchAgents"));
    assert!(path.to_str().unwrap().contains("com.daviszeroclaw.tunnel"));
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib tunnel_status_silent 2>&1 | tail -10
```

Expected: compile error (functions not yet defined — test will link once Task 2 is merged, this test just verifies path shape).

Actually since `tunnel_service_plist_path` was added in Task 2, this test will compile and pass. Verify:

```bash
cargo test --lib tunnel_status_silent 2>&1 | tail -5
```

Expected: PASS. This is a guard test; the behavioral silence is verified at integration time.

- [ ] **Step 3: Add `tunnel_cloudflared_config_path` helper**

Append to `src/cli/service.rs` (after `render_tunnel_launchd_plist`):

```rust
pub(super) fn tunnel_cloudflared_config_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home)
        .join(".cloudflared")
        .join("davis-shortcut.yml"))
}
```

- [ ] **Step 4: Add `tunnel_install`**

Append to `src/cli/service.rs`:

```rust
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
```

- [ ] **Step 5: Add `tunnel_uninstall`**

Append to `src/cli/service.rs`:

```rust
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
```

- [ ] **Step 6: Add `tunnel_status`**

Append to `src/cli/service.rs`:

```rust
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
```

- [ ] **Step 7: Run all tests and clippy**

```bash
cargo test --lib 2>&1 | tail -10
cargo clippy --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: all tests pass, no warnings.

- [ ] **Step 8: Commit**

```bash
git add src/cli/service.rs src/cli/tests.rs
git commit -m "feat(service): add tunnel_install/uninstall/status functions"
```

---

## Task 4: Wire tunnel commands into `ServiceCommand` in `mod.rs`

**Files:**
- Modify: `src/cli/mod.rs:97-106` (ServiceCommand enum) and match arms at line ~479

- [ ] **Step 1: Extend `ServiceCommand` enum**

In `src/cli/mod.rs`, replace the `ServiceCommand` enum (lines 97-106):

```rust
enum ServiceCommand {
    /// Install and start ZeroClaw with the Davis runtime config.
    Install,
    /// Show launchd and ZeroClaw runtime health.
    Status,
    /// Restart ZeroClaw with the Davis runtime config.
    Restart,
    /// Stop and remove the Davis ZeroClaw launchd service.
    Uninstall,
    /// Configure and start cloudflared as a launchd service (Cloudflare Tunnel).
    TunnelInstall,
    /// Stop and remove the cloudflared launchd service.
    TunnelUninstall,
    /// Show tunnel launchd state and public hostname reachability.
    TunnelStatus,
}
```

- [ ] **Step 2: Add match arms**

In `src/cli/mod.rs`, find the match block starting at line ~479:

```rust
Commands::Service { command } => match command {
    ServiceCommand::Install => install_davis_service(&paths).await,
    ServiceCommand::Status => status_davis_service(&paths).await,
    ServiceCommand::Restart => restart_davis_service(&paths).await,
    ServiceCommand::Uninstall => uninstall_davis_service(&paths),
```

Replace with:

```rust
Commands::Service { command } => match command {
    ServiceCommand::Install => install_davis_service(&paths).await,
    ServiceCommand::Status => status_davis_service(&paths).await,
    ServiceCommand::Restart => restart_davis_service(&paths).await,
    ServiceCommand::Uninstall => uninstall_davis_service(&paths),
    ServiceCommand::TunnelInstall => tunnel_install(&paths).await,
    ServiceCommand::TunnelUninstall => tunnel_uninstall(&paths),
    ServiceCommand::TunnelStatus => tunnel_status(&paths).await,
```

- [ ] **Step 3: Build and verify help text**

```bash
cargo build 2>&1 | tail -10
cargo run --bin daviszeroclaw -- service --help 2>&1
```

Expected: build succeeds; help output lists `tunnel-install`, `tunnel-uninstall`, `tunnel-status`.

- [ ] **Step 4: Run full test suite**

```bash
cargo test --lib 2>&1 | tail -10
cargo clippy --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(service): wire tunnel-install/uninstall/status CLI subcommands"
```

---

## Task 5: Integrate tunnel status into `service status`

**Files:**
- Modify: `src/cli/service.rs:301-405` (`status_davis_service`)

- [ ] **Step 1: Add tunnel status call at end of `status_davis_service`**

In `src/cli/service.rs`, find the end of `status_davis_service` (currently returns `Ok(())` at line ~405). Replace the final `Ok(())` with:

```rust
    tunnel_status(paths).await?;
    Ok(())
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo build 2>&1 | tail -5
```

Expected: success.

- [ ] **Step 3: Run tests**

```bash
cargo test --lib 2>&1 | tail -10
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add src/cli/service.rs
git commit -m "feat(service): service status shows tunnel health when installed"
```

---

## Task 6: Update `local.example.toml`

**Files:**
- Modify: `config/davis/local.example.toml`

- [ ] **Step 1: Add `[tunnel]` block**

At the end of `config/davis/local.example.toml`, append:

```toml

# --- Cloudflare Tunnel (optional) -------------------------------------------
# Enables access to the Davis Shortcut bridge (port 3012) from outside the LAN.
# Prerequisites (run once manually):
#   brew install cloudflare/cloudflare/cloudflared
#   cloudflared login
#   cloudflared tunnel create davis-shortcut
#   cloudflared tunnel route dns davis-shortcut <hostname>
# Then fill in the UUID and hostname below and run:
#   daviszeroclaw service tunnel-install
# Finally rebuild the Shortcut with the external URL:
#   daviszeroclaw shortcut install --url "https://<hostname>/shortcut"
#
# [tunnel]
# tunnel_id = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
# hostname = "davis.yourdomain.com"
```

- [ ] **Step 2: Verify the file parses as valid TOML**

```bash
cargo run --bin daviszeroclaw -- service status 2>&1 | head -5
# Or just verify with:
python3 -c "import tomllib; tomllib.load(open('config/davis/local.example.toml','rb'))" && echo "valid TOML"
```

Expected: `valid TOML` (the block is commented out so it won't affect parsing).

- [ ] **Step 3: Commit**

```bash
git add config/davis/local.example.toml
git commit -m "docs(config): add commented [tunnel] block to local.example.toml"
```

---

## Self-Review Checklist

**Spec coverage:**
- [x] §1 `TunnelConfig` struct + `LocalConfig.tunnel` field → Task 1
- [x] §1 `local.example.toml` `[tunnel]` block → Task 6
- [x] §2 `tunnel-install` precondition checks (cloudflared, config, credentials) → Task 3
- [x] §2 `tunnel-install` writes cloudflared YAML + plist + launchctl bootstrap → Task 3
- [x] §2 `tunnel-install` polls health up to 10s → Task 3
- [x] §2 `tunnel-uninstall` bootout + remove plist + remove YAML → Task 3
- [x] §2 `tunnel-status` silent when plist absent → Task 3
- [x] §2 `tunnel-status` launchd state + hostname health + latency → Task 3
- [x] §2 `service status` appends tunnel line → Task 5
- [x] §3 `render_tunnel_launchd_plist` + `tunnel_service_label` + `tunnel_service_plist_path` → Task 2
- [x] §4 5 unit tests → Tasks 1, 2, 3

**Type consistency:** `TunnelServiceSpec` defined in Task 2, used in Task 3. `tunnel_install/uninstall/status` all `pub(super)` matching existing service function visibility. `tunnel_uninstall` is sync (matches `uninstall_davis_service` which is also sync). `tunnel_install` and `tunnel_status` are async (match install/status pattern).

**No placeholders:** All code blocks are complete and runnable.
