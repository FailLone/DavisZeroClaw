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
    let route_config = resolve_shortcut_route_config(paths, url);

    let webhook_secret = resolve_shortcut_secret(paths, secret, no_secret);
    let raw = fs::read_to_string(&shortcut_json)
        .with_context(|| format!("failed to read {}", shortcut_json.display()))?;
    let mut workflow: Value = serde_json::from_str(&raw)
        .with_context(|| format!("invalid shortcut JSON: {}", shortcut_json.display()))?;
    let reply_phrases = load_reply_phrases(paths);
    customize_shortcut_json_with_reply(
        &mut workflow,
        &route_config.external_url,
        route_config.lan.as_ref(),
        webhook_secret.as_deref(),
        reply_phrases.as_ref(),
    )?;

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
    sign_shortcut(&shortcuts, &tmp_wflow, &output_shortcut)?;
    drop(cleanup);

    println!("Built {}", output_shortcut.display());
    println!("Webhook URL: {}", route_config.external_url);
    if let Some(lan) = &route_config.lan {
        println!("LAN URL: {}", lan.lan_url);
        println!("LAN SSIDs: {}", lan.lan_ssids.join(", "));
    }
    let embedded_secret = webhook_secret.is_some();
    if embedded_secret {
        println!("Embedded header: X-Webhook-Secret");
    } else {
        println!("Embedded header: none (no webhook secret found)");
    }
    Ok(ShortcutBuild { output_shortcut })
}

fn sign_shortcut(shortcuts: &Path, input: &Path, output: &Path) -> Result<()> {
    match sign_shortcut_with_mode(shortcuts, "anyone", input, output) {
        Ok(()) => Ok(()),
        Err(anyone_err) => {
            eprintln!("Warning: {anyone_err}. Retrying with people-who-know-me signing mode.");
            sign_shortcut_with_mode(shortcuts, "people-who-know-me", input, output)
                .context("shortcuts sign failed in both anyone and people-who-know-me modes")
        }
    }
}

fn sign_shortcut_with_mode(
    shortcuts: &Path,
    mode: &str,
    input: &Path,
    output: &Path,
) -> Result<()> {
    let command_output = command_output(
        Command::new(shortcuts)
            .arg("sign")
            .arg("-m")
            .arg(mode)
            .arg("-i")
            .arg(input)
            .arg("-o")
            .arg(output)
            .env("PATH", tool_path_env()),
    )
    .with_context(|| format!("failed to run shortcuts sign -m {mode}"))?;
    let stderr = filter_known_shortcuts_warnings(&command_output.stderr);
    if command_output.status_success {
        print_command_streams(&command_output.stdout, &stderr);
        return Ok(());
    }

    let detail = first_non_empty_line(&stderr)
        .or_else(|| first_non_empty_line(&command_output.stdout))
        .unwrap_or("no error details");
    bail!("shortcuts sign -m {mode} failed: {detail}");
}

#[derive(Debug, Clone)]
pub(super) struct ShortcutRouteConfig {
    pub(super) external_url: String,
    pub(super) lan: Option<ShortcutLanRouting>,
}

#[derive(Debug, Clone)]
pub struct ShortcutLanRouting {
    pub lan_url: String,
    pub lan_ssids: Vec<String>,
}

pub(super) fn resolve_shortcut_route_config(
    paths: &RuntimePaths,
    explicit_url: Option<String>,
) -> ShortcutRouteConfig {
    let external_url = explicit_url
        .or_else(|| std::env::var("DAVIS_SHORTCUT_WEBHOOK_URL").ok())
        .or_else(|| toml_string_value(&paths.local_config_path(), "shortcut", "external_url"))
        .or_else(|| {
            toml_string_value(&paths.local_config_path(), "tunnel", "hostname")
                .map(|host| format!("https://{}/shortcut", host.trim().trim_end_matches('/')))
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(default_shortcut_lan_url);

    let lan_ssids = toml_string_array_value(&paths.local_config_path(), "shortcut", "lan_ssids")
        .unwrap_or_default();
    let lan_ssids = lan_ssids
        .iter()
        .map(|ssid| ssid.trim())
        .filter(|ssid| !ssid.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let lan = if lan_ssids.is_empty() {
        None
    } else {
        let lan_url = toml_string_value(&paths.local_config_path(), "shortcut", "lan_url")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(default_shortcut_lan_url);
        Some(ShortcutLanRouting { lan_url, lan_ssids })
    };

    ShortcutRouteConfig { external_url, lan }
}

fn load_reply_phrases(paths: &RuntimePaths) -> Option<ReplyPhrases> {
    let path = paths.local_config_path();
    let raw = std::fs::read_to_string(&path).ok()?;
    let doc: toml::Value = toml::from_str(&raw).ok()?;
    let reply = doc.get("shortcut")?.get("reply")?.as_table()?;
    let phrases_tbl = reply.get("phrases")?.as_table()?;
    Some(ReplyPhrases {
        speak_brief_imessage_full: phrases_tbl
            .get("speak_brief_imessage_full")?
            .as_str()?
            .to_string(),
        error_generic: phrases_tbl.get("error_generic")?.as_str()?.to_string(),
    })
}

fn default_shortcut_lan_url() -> String {
    let host_ip = detect_host_ip().unwrap_or_else(|| {
        eprintln!("Warning: could not detect this Mac's LAN IP; leaving URL host as <mac-ip>.");
        "<mac-ip>".to_string()
    });
    let port = std::env::var("DAVIS_SHORTCUT_WEBHOOK_PORT").unwrap_or_else(|_| "3012".to_string());
    let path =
        std::env::var("DAVIS_SHORTCUT_WEBHOOK_PATH").unwrap_or_else(|_| "/shortcut".to_string());
    format!("http://{host_ip}:{port}{path}")
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

pub(super) fn toml_string_array_value(
    path: &Path,
    section: &str,
    key: &str,
) -> Option<Vec<String>> {
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
    customize_shortcut_json_with_routing(workflow, webhook_url, None, webhook_secret)
}

pub fn customize_shortcut_json_with_routing(
    workflow: &mut Value,
    external_url: &str,
    lan_routing: Option<&ShortcutLanRouting>,
    webhook_secret: Option<&str>,
) -> Result<()> {
    customize_shortcut_json_with_reply(workflow, external_url, lan_routing, webhook_secret, None)
}

/// Extended renderer with optional reply wiring. When `reply_phrases`
/// is `Some`, the renderer inserts the device-detect branch before the
/// download URL action, patches `thread_id` to use a variable, and
/// appends response-parse + speak actions after the download URL.
pub fn customize_shortcut_json_with_reply(
    workflow: &mut Value,
    external_url: &str,
    lan_routing: Option<&ShortcutLanRouting>,
    webhook_secret: Option<&str>,
    reply_phrases: Option<&ReplyPhrases>,
) -> Result<()> {
    *workflow
        .pointer_mut("/WFWorkflowImportQuestions/0/DefaultValue")
        .ok_or_else(|| {
            anyhow!("shortcut template missing WFWorkflowImportQuestions.0.DefaultValue")
        })? = Value::String(external_url.to_string());

    if let Some(lan) = lan_routing {
        customize_shortcut_json_dual_route(workflow, external_url, lan, webhook_secret)?;
    } else {
        let params = workflow
            .pointer_mut("/WFWorkflowActions/1/WFWorkflowActionParameters")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow!("shortcut template missing download URL action parameters"))?;
        apply_download_url_settings(params, external_url, webhook_secret);
    }

    if let Some(phrases) = reply_phrases {
        inject_reply_wiring(workflow, phrases)?;
    }
    retarget_import_question_action_index(workflow, external_url);
    Ok(())
}

fn customize_shortcut_json_dual_route(
    workflow: &mut Value,
    external_url: &str,
    lan: &ShortcutLanRouting,
    webhook_secret: Option<&str>,
) -> Result<()> {
    let actions = workflow
        .pointer_mut("/WFWorkflowActions")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("shortcut template missing WFWorkflowActions"))?;
    if actions.len() < 2 {
        bail!("shortcut template must contain at least ask and download URL actions");
    }

    let ask_action = actions[0].clone();
    let mut download_action = actions[1].clone();
    // Trailing actions beyond the download URL (e.g. speak) are preserved if present.
    let trailing_actions: Vec<Value> = actions.drain(2..).collect();

    let wifi_uuid = pseudo_uuid();
    let url_if_group_uuid = pseudo_uuid();
    let lan_url_uuid = pseudo_uuid();
    let external_url_uuid = pseudo_uuid();
    let url_if_result_uuid = pseudo_uuid();

    let mut routed_actions = vec![
        ask_action,
        get_wifi_network_name_action(&wifi_uuid),
        if_current_wifi_matches_any_action(&wifi_uuid, &url_if_group_uuid, &lan.lan_ssids),
        text_action(&lan_url_uuid, &lan.lan_url),
        otherwise_action(&url_if_group_uuid),
        text_action(&external_url_uuid, external_url),
        end_if_action_with_uuid(&url_if_group_uuid, &url_if_result_uuid),
    ];

    reset_action_uuid(&mut download_action)?;
    apply_download_url_settings(
        download_action
            .pointer_mut("/WFWorkflowActionParameters")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow!("shortcut template missing external download parameters"))?,
        "",
        webhook_secret,
    );
    set_download_url_to_variable(&mut download_action, &url_if_result_uuid, "If Result")?;
    routed_actions.push(download_action);
    let external_action_index = routed_actions.len() - 1;

    routed_actions.extend(trailing_actions);
    *actions = routed_actions;

    if let Some(action_index) = workflow.pointer_mut("/WFWorkflowImportQuestions/0/ActionIndex") {
        *action_index = Value::from(external_action_index);
    }

    Ok(())
}

fn reset_action_uuid(action: &mut Value) -> Result<String> {
    let uuid = pseudo_uuid();
    let params = action
        .pointer_mut("/WFWorkflowActionParameters")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("shortcut action missing parameters"))?;
    params.insert("UUID".to_string(), Value::String(uuid.clone()));
    Ok(uuid)
}

fn apply_download_url_settings(
    params: &mut serde_json::Map<String, Value>,
    webhook_url: &str,
    webhook_secret: Option<&str>,
) {
    params.insert("WFURL".to_string(), Value::String(webhook_url.to_string()));
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
}

fn get_wifi_network_name_action(uuid: &str) -> Value {
    json!({
        "WFWorkflowActionIdentifier": "is.workflow.actions.getwifi",
        "WFWorkflowActionParameters": {
            "UUID": uuid,
            "WFNetworkDetailsNetwork": "Wi-Fi",
            "WFWiFiDetail": "Network Name"
        }
    })
}

fn if_current_wifi_matches_any_action(
    wifi_uuid: &str,
    grouping_identifier: &str,
    ssids: &[String],
) -> Value {
    let templates: Vec<Value> = ssids
        .iter()
        .map(|ssid| {
            json!({
                "WFCondition": 4,
                "WFInput": action_output_variable(wifi_uuid, "Network Details"),
                "WFConditionalActionString": ssid
            })
        })
        .collect();
    json!({
        "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
        "WFWorkflowActionParameters": {
            "GroupingIdentifier": grouping_identifier,
            "WFControlFlowMode": 0,
            "WFConditions": {
                "Value": {
                    "WFActionParameterFilterPrefix": 0,
                    "WFActionParameterFilterTemplates": templates,
                    "WFContentPredicateBoundedDate": false
                },
                "WFSerializationType": "WFContentPredicateTableTemplate"
            }
        }
    })
}

fn text_action(uuid: &str, text: &str) -> Value {
    json!({
        "WFWorkflowActionIdentifier": "is.workflow.actions.gettext",
        "WFWorkflowActionParameters": {
            "UUID": uuid,
            "WFTextActionText": text
        }
    })
}

fn otherwise_action(grouping_identifier: &str) -> Value {
    json!({
        "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
        "WFWorkflowActionParameters": {
            "GroupingIdentifier": grouping_identifier,
            "WFControlFlowMode": 1
        }
    })
}

fn end_if_action_with_uuid(grouping_identifier: &str, uuid: &str) -> Value {
    json!({
        "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
        "WFWorkflowActionParameters": {
            "GroupingIdentifier": grouping_identifier,
            "UUID": uuid,
            "WFControlFlowMode": 2
        }
    })
}

fn set_download_url_to_variable(
    download_action: &mut Value,
    output_uuid: &str,
    output_name: &str,
) -> Result<()> {
    let params = download_action
        .pointer_mut("/WFWorkflowActionParameters")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("download action missing parameters"))?;
    params.insert(
        "WFURL".to_string(),
        text_token_action_output(output_uuid, output_name),
    );
    Ok(())
}

fn action_output_variable(output_uuid: &str, output_name: &str) -> Value {
    json!({
        "Type": "Variable",
        "Variable": {
            "WFSerializationType": "WFTextTokenAttachment",
            "Value": {
                "Type": "ActionOutput",
                "OutputName": output_name,
                "OutputUUID": output_uuid
            }
        }
    })
}

fn action_output_attachment(output_uuid: &str, output_name: &str) -> Value {
    json!({
        "Value": {
            "OutputUUID": output_uuid,
            "Type": "ActionOutput",
            "OutputName": output_name
        },
        "WFSerializationType": "WFTextTokenAttachment"
    })
}

fn text_token_action_output(output_uuid: &str, output_name: &str) -> Value {
    json!({
        "Value": {
            "string": "\u{FFFC}",
            "attachmentsByRange": {
                "{0, 1}": {
                    "OutputUUID": output_uuid,
                    "Type": "ActionOutput",
                    "OutputName": output_name
                }
            }
        },
        "WFSerializationType": "WFTextTokenString"
    })
}

fn retarget_import_question_action_index(workflow: &mut Value, webhook_url: &str) {
    let index = workflow
        .pointer("/WFWorkflowActions")
        .and_then(Value::as_array)
        .and_then(|actions| {
            actions.iter().position(|action| {
                action.pointer("/WFWorkflowActionParameters/WFURL")
                    == Some(&Value::String(webhook_url.to_string()))
            })
        });
    if let (Some(index), Some(action_index)) = (
        index,
        workflow.pointer_mut("/WFWorkflowImportQuestions/0/ActionIndex"),
    ) {
        *action_index = Value::from(index);
    }
}

/// Phrases spoken on the triggering device when reply wiring is enabled.
pub struct ReplyPhrases {
    /// Spoken when the reply is brief enough to speak directly but an iMessage
    /// full-length reply was also sent (server-rendered; not used in Shortcut).
    pub speak_brief_imessage_full: String,
    /// Spoken when the response body cannot be parsed or `speak_text` is empty.
    pub error_generic: String,
}

/// Thread-id prefix action variables: a `getdevicedetails` to read
/// Device Model, then an `if` branch to pick the prefix, emitting an
/// `If Result` variable that replaces the static `"ios:iphone"` string
/// in every download URL action.
///
/// Returns the sequence of actions to insert before the downloadurl
/// action, plus the UUID of the end-if action whose `If Result` output
/// carries the computed prefix.
fn build_device_prefix_actions() -> (Vec<Value>, String) {
    let get_model_uuid = pseudo_uuid();
    let if_group_uuid = pseudo_uuid();
    let if_body_uuid = pseudo_uuid();
    let else_body_uuid = pseudo_uuid();
    let end_if_uuid = pseudo_uuid();

    let actions = vec![
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.getdevicedetails",
            "WFWorkflowActionParameters": {
                "UUID": get_model_uuid,
                "WFDeviceDetail": "Device Model"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "WFControlFlowMode": 0,
                "WFInput": action_output_variable(&get_model_uuid, "Device Model"),
                "WFCondition": 8,
                "WFConditionalActionString": "HomePod"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.gettext",
            "WFWorkflowActionParameters": {
                "UUID": if_body_uuid,
                "WFTextActionText": "ios:homepod"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "WFControlFlowMode": 1
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.gettext",
            "WFWorkflowActionParameters": {
                "UUID": else_body_uuid,
                "WFTextActionText": "ios:iphone"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "UUID": end_if_uuid,
                "WFControlFlowMode": 2
            }
        }),
    ];
    (actions, end_if_uuid)
}

/// Post-POST actions: parse `speak_text` from response and speak it
/// unless empty; fall back to `error_phrase` when empty.
fn build_reply_actions(error_phrase: &str, downloadurl_uuid: &str) -> Vec<Value> {
    let dict_uuid = pseudo_uuid();
    let text_uuid = pseudo_uuid();
    let if_group_uuid = pseudo_uuid();
    let speak_value = text_token_action_output(&text_uuid, "Dictionary Value");
    vec![
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.detect.dictionary",
            "WFWorkflowActionParameters": {
                "UUID": dict_uuid,
                "WFInput": action_output_attachment(downloadurl_uuid, "Contents of URL")
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.getvalueforkey",
            "WFWorkflowActionParameters": {
                "UUID": text_uuid,
                "WFGetDictionaryValueType": "Value",
                "WFDictionaryKey": "speak_text",
                "WFInput": action_output_attachment(&dict_uuid, "Dictionary")
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "WFControlFlowMode": 0,
                "WFInput": action_output_variable(&text_uuid, "Dictionary Value"),
                "WFCondition": 100
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.speaktext",
            "WFWorkflowActionParameters": {
                "UUID": pseudo_uuid(),
                "Text": speak_value.clone(),
                "WFText": speak_value,
                "Language": "zh-CN"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "WFControlFlowMode": 1
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.speaktext",
            "WFWorkflowActionParameters": {
                "UUID": pseudo_uuid(),
                "Text": error_phrase,
                "WFText": error_phrase,
                "Language": "zh-CN"
            }
        }),
        json!({
            "WFWorkflowActionIdentifier": "is.workflow.actions.conditional",
            "WFWorkflowActionParameters": {
                "GroupingIdentifier": if_group_uuid,
                "UUID": pseudo_uuid(),
                "WFControlFlowMode": 2
            }
        }),
    ]
}

/// Patch the `thread_id` dictionary entry inside the download URL's
/// `WFJSONValues` to reference the device-prefix If Result instead of
/// the hardcoded `"ios:iphone"` string.
fn patch_thread_id_to_prefix_variable(
    download_action: &mut Value,
    prefix_if_result_uuid: &str,
) -> Result<()> {
    let new_thread_id_value = json!({
        "Value": {
            "string": "\u{FFFC}",
            "attachmentsByRange": {
                "{0, 1}": {
                    "OutputUUID": prefix_if_result_uuid,
                    "Type": "ActionOutput",
                    "OutputName": "If Result"
                }
            }
        },
        "WFSerializationType": "WFTextTokenString"
    });
    let items = download_action
        .pointer_mut("/WFWorkflowActionParameters/WFJSONValues/Value/WFDictionaryFieldValueItems")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("download action missing WFDictionaryFieldValueItems"))?;
    for item in items.iter_mut() {
        if item.get("WFKey").and_then(Value::as_str) == Some("thread_id") {
            if let Some(obj) = item.as_object_mut() {
                obj.insert("WFValue".to_string(), new_thread_id_value);
            }
            return Ok(());
        }
    }
    bail!("thread_id entry not found in download URL dictionary");
}

fn inject_reply_wiring(workflow: &mut Value, phrases: &ReplyPhrases) -> Result<()> {
    // Build the device-prefix branch once, before any LAN/external routing.
    let (prefix_actions, prefix_if_group_uuid) = build_device_prefix_actions();

    let actions = workflow
        .pointer_mut("/WFWorkflowActions")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("shortcut template missing WFWorkflowActions"))?;
    let first_download_idx = actions
        .iter()
        .position(|a| {
            a.get("WFWorkflowActionIdentifier").and_then(Value::as_str)
                == Some("is.workflow.actions.downloadurl")
        })
        .ok_or_else(|| anyhow!("no downloadurl action in workflow"))?;
    let prefix_insert_idx = if actions
        .first()
        .and_then(|a| a.get("WFWorkflowActionIdentifier"))
        .and_then(Value::as_str)
        == Some("is.workflow.actions.ask")
    {
        1
    } else {
        first_download_idx
    };

    let mut saw_download = false;
    let mut new_actions = Vec::with_capacity(actions.len() + prefix_actions.len() + 24);
    for (i, mut action) in actions.drain(..).enumerate() {
        if i == prefix_insert_idx {
            new_actions.extend(prefix_actions.clone());
        }
        if action
            .get("WFWorkflowActionIdentifier")
            .and_then(Value::as_str)
            == Some("is.workflow.actions.downloadurl")
        {
            saw_download = true;
            let download_uuid = action
                .pointer("/WFWorkflowActionParameters/UUID")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("downloadurl action missing UUID"))?
                .to_string();
            patch_thread_id_to_prefix_variable(&mut action, &prefix_if_group_uuid)?;
            new_actions.push(action);
            new_actions.extend(build_reply_actions(&phrases.error_generic, &download_uuid));
        } else {
            new_actions.push(action);
        }
    }
    if !saw_download {
        bail!("no downloadurl action in workflow");
    }
    *actions = new_actions;
    let _ = &phrases.speak_brief_imessage_full; // server-rendered, not Shortcut-side
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

#[cfg(test)]
mod reply_renderer_tests {
    use super::*;

    fn minimal_template() -> Value {
        json!({
            "WFWorkflowImportQuestions": [{
                "ActionIndex": 1,
                "Category": "Parameter",
                "DefaultValue": "http://x/shortcut",
                "ParameterKey": "WFURL",
                "Text": ""
            }],
            "WFWorkflowActions": [
                {
                    "WFWorkflowActionIdentifier": "is.workflow.actions.ask",
                    "WFWorkflowActionParameters": {"UUID": "ASK-UUID"}
                },
                {
                    "WFWorkflowActionIdentifier": "is.workflow.actions.downloadurl",
                    "WFWorkflowActionParameters": {
                        "UUID": "DL-UUID",
                        "WFURL": "http://x/shortcut",
                        "WFHTTPMethod": "POST",
                        "WFJSONValues": {
                            "Value": {
                                "WFDictionaryFieldValueItems": [
                                    {"WFKey": "sender", "WFValue": "ios-shortcuts", "WFItemType": 0},
                                    {"WFKey": "thread_id", "WFValue": "ios:iphone", "WFItemType": 0}
                                ]
                            },
                            "WFSerializationType": "WFDictionaryFieldValue"
                        }
                    }
                }
            ]
        })
    }

    #[test]
    fn reply_wiring_injects_device_detect_and_reply_actions() {
        let mut wf = minimal_template();
        let phrases = ReplyPhrases {
            speak_brief_imessage_full: "详情我通过短信发你".into(),
            error_generic: "戴维斯好像出问题了".into(),
        };
        customize_shortcut_json_with_reply(
            &mut wf,
            "http://x/shortcut",
            None,
            None,
            Some(&phrases),
        )
        .expect("inject ok");
        let actions = wf
            .pointer("/WFWorkflowActions")
            .and_then(Value::as_array)
            .unwrap();
        let ids: Vec<&str> = actions
            .iter()
            .filter_map(|a| a.get("WFWorkflowActionIdentifier").and_then(Value::as_str))
            .collect();
        assert!(
            ids.contains(&"is.workflow.actions.getdevicedetails"),
            "must insert getdevicedetails; got {ids:?}"
        );
        assert!(
            ids.contains(&"is.workflow.actions.speaktext"),
            "must append speaktext"
        );
        assert!(
            ids.contains(&"is.workflow.actions.getvalueforkey"),
            "must parse response dict"
        );
    }

    #[test]
    fn reply_wiring_omitted_when_phrases_none() {
        let mut wf = minimal_template();
        customize_shortcut_json_with_reply(&mut wf, "http://x/shortcut", None, None, None)
            .expect("ok");
        let actions = wf
            .pointer("/WFWorkflowActions")
            .and_then(Value::as_array)
            .unwrap();
        let ids: Vec<&str> = actions
            .iter()
            .filter_map(|a| a.get("WFWorkflowActionIdentifier").and_then(Value::as_str))
            .collect();
        assert!(
            !ids.contains(&"is.workflow.actions.getdevicedetails"),
            "no device detect without phrases"
        );
    }

    #[test]
    fn thread_id_entry_rewritten_to_variable() {
        let mut wf = minimal_template();
        let phrases = ReplyPhrases {
            speak_brief_imessage_full: "b".into(),
            error_generic: "e".into(),
        };
        customize_shortcut_json_with_reply(
            &mut wf,
            "http://x/shortcut",
            None,
            None,
            Some(&phrases),
        )
        .unwrap();
        let actions = wf
            .pointer("/WFWorkflowActions")
            .and_then(Value::as_array)
            .unwrap();
        let download = actions
            .iter()
            .find(|a| {
                a.get("WFWorkflowActionIdentifier").and_then(Value::as_str)
                    == Some("is.workflow.actions.downloadurl")
            })
            .unwrap();
        let items = download
            .pointer("/WFWorkflowActionParameters/WFJSONValues/Value/WFDictionaryFieldValueItems")
            .and_then(Value::as_array)
            .unwrap();
        let thread_id_item = items
            .iter()
            .find(|i| i.get("WFKey").and_then(Value::as_str) == Some("thread_id"))
            .unwrap();
        let val = thread_id_item.get("WFValue").unwrap();
        assert!(
            val.is_object(),
            "thread_id WFValue should be an attachment-token object, got {val}"
        );
        assert!(
            val.get("WFSerializationType").and_then(Value::as_str) == Some("WFTextTokenString"),
            "thread_id WFValue must be WFTextTokenString token"
        );
        assert_eq!(
            val.pointer("/Value/attachmentsByRange/{0, 1}/OutputName"),
            Some(&Value::String("If Result".to_string())),
            "thread_id token must reference the device-prefix If Result"
        );
    }

    #[test]
    fn reply_wiring_uses_single_download_after_lan_url_selection() {
        let mut wf = minimal_template();
        let lan = ShortcutLanRouting {
            lan_url: "http://192.168.1.2:3012/shortcut".into(),
            lan_ssids: vec!["FailLone".into(), "FailLone_5G".into()],
        };
        let phrases = ReplyPhrases {
            speak_brief_imessage_full: "b".into(),
            error_generic: "e".into(),
        };

        customize_shortcut_json_with_reply(
            &mut wf,
            "https://davis.example.com/shortcut",
            Some(&lan),
            None,
            Some(&phrases),
        )
        .unwrap();

        let actions = wf
            .pointer("/WFWorkflowActions")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(
            actions[1].pointer("/WFWorkflowActionIdentifier"),
            Some(&Value::String(
                "is.workflow.actions.getdevicedetails".to_string()
            )),
            "device prefix must run before LAN Wi-Fi routing"
        );

        let download_indices: Vec<usize> = actions
            .iter()
            .enumerate()
            .filter_map(|(idx, action)| {
                (action
                    .get("WFWorkflowActionIdentifier")
                    .and_then(Value::as_str)
                    == Some("is.workflow.actions.downloadurl"))
                .then_some(idx)
            })
            .collect();
        assert_eq!(
            download_indices.len(),
            1,
            "LAN routing should choose a URL first, then use one shared download/reply flow"
        );

        let download_idx = download_indices[0];
        let url_if_idx = actions
            .iter()
            .position(|action| {
                action.pointer("/WFWorkflowActionParameters/WFTextActionText")
                    == Some(&Value::String(
                        "https://davis.example.com/shortcut".to_string(),
                    ))
            })
            .unwrap();
        let url_if_end_uuid = actions[url_if_idx + 1]
            .pointer("/WFWorkflowActionParameters/UUID")
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(
            actions[download_idx].pointer(
                "/WFWorkflowActionParameters/WFURL/Value/attachmentsByRange/{0, 1}/OutputUUID"
            ),
            Some(&Value::String(url_if_end_uuid.to_string())),
            "download URL should come from the LAN/external If Result"
        );

        let thread_id_item = actions[download_idx]
            .pointer("/WFWorkflowActionParameters/WFJSONValues/Value/WFDictionaryFieldValueItems")
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("WFKey").and_then(Value::as_str) == Some("thread_id"))
            })
            .expect("download action has thread_id item");
        assert_eq!(
            thread_id_item.pointer("/WFValue/WFSerializationType"),
            Some(&Value::String("WFTextTokenString".to_string())),
            "download must use the device-prefix token"
        );
        assert_eq!(
            actions[download_idx + 1].pointer("/WFWorkflowActionIdentifier"),
            Some(&Value::String(
                "is.workflow.actions.detect.dictionary".to_string()
            )),
            "single download should feed the shared reply parser"
        );
        assert_eq!(
            actions[download_idx + 2].pointer("/WFWorkflowActionIdentifier"),
            Some(&Value::String(
                "is.workflow.actions.getvalueforkey".to_string()
            )),
            "shared reply parser should read speak_text"
        );
    }

    #[test]
    fn reply_actions_use_shortcuts_variable_serialization_for_inputs_and_speech_text() {
        let actions = build_reply_actions("err", "DL-UUID");
        let dict = &actions[0];
        let get_value = &actions[1];
        let has_value_if = &actions[2];
        let variable_speak = &actions[3];
        let fallback_speak = &actions[5];

        assert_eq!(
            dict.pointer("/WFWorkflowActionParameters/WFInput/WFSerializationType"),
            Some(&Value::String("WFTextTokenAttachment".to_string()))
        );
        assert_eq!(
            dict.pointer("/WFWorkflowActionParameters/WFInput/Value/OutputUUID"),
            Some(&Value::String("DL-UUID".to_string()))
        );
        assert_eq!(
            get_value.pointer("/WFWorkflowActionParameters/WFInput/WFSerializationType"),
            Some(&Value::String("WFTextTokenAttachment".to_string()))
        );
        let dict_uuid = dict
            .pointer("/WFWorkflowActionParameters/UUID")
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(
            get_value.pointer("/WFWorkflowActionParameters/WFInput/Value/OutputUUID"),
            Some(&Value::String(dict_uuid.to_string()))
        );
        assert_eq!(
            has_value_if.pointer("/WFWorkflowActionParameters/WFCondition"),
            Some(&Value::from(100))
        );
        assert_eq!(
            has_value_if.pointer("/WFWorkflowActionParameters/WFInput/Type"),
            Some(&Value::String("Variable".to_string()))
        );
        let dictionary_value_uuid = get_value
            .pointer("/WFWorkflowActionParameters/UUID")
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(
            has_value_if.pointer("/WFWorkflowActionParameters/WFInput/Variable/Value/OutputUUID"),
            Some(&Value::String(dictionary_value_uuid.to_string()))
        );
        assert!(
            has_value_if
                .pointer("/WFWorkflowActionParameters/WFConditionalActionString")
                .is_none(),
            "has-any-value If must not carry an empty comparison operand"
        );
        assert_eq!(
            variable_speak.pointer("/WFWorkflowActionParameters/Text/WFSerializationType"),
            Some(&Value::String("WFTextTokenString".to_string()))
        );
        assert_eq!(
            variable_speak.pointer(
                "/WFWorkflowActionParameters/Text/Value/attachmentsByRange/{0, 1}/OutputUUID"
            ),
            Some(&Value::String(dictionary_value_uuid.to_string()))
        );
        assert_eq!(
            variable_speak.pointer(
                "/WFWorkflowActionParameters/WFText/Value/attachmentsByRange/{0, 1}/OutputUUID"
            ),
            Some(&Value::String(dictionary_value_uuid.to_string()))
        );
        assert_eq!(
            fallback_speak.pointer("/WFWorkflowActionParameters/Text"),
            Some(&Value::String("err".to_string()))
        );
        assert_eq!(
            fallback_speak.pointer("/WFWorkflowActionParameters/WFText"),
            Some(&Value::String("err".to_string()))
        );
    }
}
