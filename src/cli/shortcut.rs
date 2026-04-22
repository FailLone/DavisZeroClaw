use super::*;
use crate::RuntimePaths;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn first_non_empty_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

pub(super) fn build_shortcut(
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

pub(super) fn install_shortcut(
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

pub(super) fn open_shortcut_import(shortcut_path: &Path) -> Result<()> {
    ensure_macos("Shortcut import")?;
    let open = require_command("open").context("open is required to launch Shortcut import")?;
    run_status(
        Command::new(open)
            .arg(shortcut_path)
            .env("PATH", tool_path_env()),
        "open shortcut import",
    )
}

pub(super) fn resolve_shortcut_secret(
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

pub(super) fn toml_string_value(path: &Path, section: &str, key: &str) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let parsed: toml::Value = raw.parse().ok()?;
    let mut value = &parsed;
    for part in section.split('.') {
        value = value.get(part)?;
    }
    value.get(key)?.as_str().map(ToString::to_string)
}

pub(super) fn toml_string_array_value(path: &Path, section: &str, key: &str) -> Option<Vec<String>> {
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

pub(super) fn detect_host_ip() -> Option<String> {
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

pub(super) fn check_imessage_permissions() -> Result<()> {
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

pub(super) fn inspect_imessage(paths: &RuntimePaths) -> Result<()> {
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

pub(super) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME environment variable is not set"))
}

pub(super) fn ensure_sqlite_readable(sqlite3: &Path, db: &Path, label: &str) -> Result<()> {
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

pub(super) fn imessage_apple_accounts(sqlite3: &Path, accounts_db: &Path) -> Result<Vec<String>> {
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

pub(super) fn imessage_allowed_contact_candidates(
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

pub(super) fn sqlite_rows(sqlite3: &Path, db: &Path, query: &str) -> Result<Vec<Vec<String>>> {
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

