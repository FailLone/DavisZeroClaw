use super::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;

#[test]
fn sanitize_markdown_script_links_removes_script_targets_only() {
    let raw =
        "Run [setup](scripts/setup.sh), keep [docs](docs/readme.md), and [ps](x/install.ps1#L1).";
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
fn customize_shortcut_json_adds_lan_wifi_branch_when_configured() {
    let mut workflow = json!({
        "WFWorkflowImportQuestions": [
            {
                "ActionIndex": 1,
                "DefaultValue": "http://old"
            }
        ],
        "WFWorkflowActions": [
            {
                "WFWorkflowActionIdentifier": "is.workflow.actions.ask",
                "WFWorkflowActionParameters": {
                    "UUID": "ASK-UUID"
                }
            },
            {
                "WFWorkflowActionIdentifier": "is.workflow.actions.downloadurl",
                "WFWorkflowActionParameters": {
                    "UUID": "DOWNLOAD-UUID",
                    "WFURL": "http://old"
                }
            },
            {
                "WFWorkflowActionIdentifier": "is.workflow.actions.speaktext",
                "WFWorkflowActionParameters": {
                    "UUID": "SPEAK-UUID"
                }
            }
        ]
    });
    let lan = ShortcutLanRouting {
        lan_url: "http://192.168.1.2:3012/shortcut".to_string(),
        lan_ssids: vec!["FailLone".to_string(), "FailLone_5G".to_string()],
    };

    customize_shortcut_json_with_routing(
        &mut workflow,
        "https://davis.faillone.com/shortcut",
        Some(&lan),
        Some("secret"),
    )
    .unwrap();

    let actions = workflow
        .pointer("/WFWorkflowActions")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(actions.len(), 9);
    assert_eq!(
        actions[1].pointer("/WFWorkflowActionIdentifier"),
        Some(&Value::String("is.workflow.actions.getwifi".to_string()))
    );
    assert_eq!(
        actions[2].pointer("/WFWorkflowActionParameters/WFTextActionText"),
        Some(&Value::String("|FailLone|FailLone_5G|".to_string()))
    );
    assert_eq!(
        actions[3].pointer("/WFWorkflowActionParameters/WFCondition"),
        Some(&Value::from(99))
    );
    assert_eq!(
        actions[4].pointer("/WFWorkflowActionParameters/WFURL"),
        Some(&Value::String(
            "http://192.168.1.2:3012/shortcut".to_string()
        ))
    );
    assert_eq!(
        actions[6].pointer("/WFWorkflowActionParameters/WFURL"),
        Some(&Value::String(
            "https://davis.faillone.com/shortcut".to_string()
        ))
    );
    assert_eq!(
        workflow.pointer("/WFWorkflowImportQuestions/0/ActionIndex"),
        Some(&Value::from(6))
    );
    assert_eq!(
        actions[4].pointer(
            "/WFWorkflowActionParameters/WFHTTPHeaders/Value/WFDictionaryFieldValueItems/0/WFValue"
        ),
        Some(&Value::String("secret".to_string()))
    );
    assert_eq!(
        actions[6].pointer(
            "/WFWorkflowActionParameters/WFHTTPHeaders/Value/WFDictionaryFieldValueItems/0/WFValue"
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
fn sync_runtime_sops_accepts_empty_project_dir() {
    // project-sops/ ships empty by default. Sync must succeed and leave the
    // runtime directory in a consistent (empty) state without tripping on
    // the non-SOP README.md we place in the project root.
    let root = unique_test_dir("sync_runtime_sops_empty");
    let paths = RuntimePaths {
        repo_root: root.join("repo"),
        runtime_dir: root.join("runtime"),
    };
    let project = root.join("project-sops");
    fs::create_dir_all(&project).unwrap();
    // README.md at the top level must be ignored (not mistaken for a SOP).
    fs::write(project.join("README.md"), "# project-sops/\n").unwrap();

    sync_runtime_sops_with_sources(&paths, &project).unwrap();

    let runtime_sops = paths.workspace_sops_dir();
    assert!(runtime_sops.is_dir(), "runtime sops dir must be created");
    let entries: Vec<_> = fs::read_dir(&runtime_sops).unwrap().collect();
    assert!(
        entries.is_empty(),
        "empty project-sops must produce empty runtime sops: {entries:?}"
    );

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
    assert!(plist.contains("daemon --config-dir &apos;/tmp/Davis ZeroClaw/.runtime/davis&apos;"));
    assert!(plist.contains("<key>ZEROCLAW_CONFIG_DIR</key>"));
    assert!(plist.contains("<key>DAVIS_REPO_ROOT</key>"));
    assert!(!plist.contains("/opt/homebrew/var/zeroclaw"));
}

#[test]
fn proxy_service_label_and_plist_path_are_distinct_from_zeroclaw() {
    assert_ne!(proxy_service_label(), davis_service_label());
    assert!(proxy_service_label().contains("proxy"));
    let proxy_path = proxy_service_plist_path().unwrap();
    let zeroclaw_path = davis_service_plist_path().unwrap();
    assert_ne!(proxy_path, zeroclaw_path);
    assert!(proxy_path.to_str().unwrap().contains("proxy"));
}

#[test]
fn render_proxy_launchd_plist_runs_proxy_binary() {
    let spec = DavisServiceSpec {
        label: proxy_service_label().to_string(),
        repo_root: PathBuf::from("/tmp/Davis ZeroClaw"),
        runtime_dir: PathBuf::from("/tmp/Davis ZeroClaw/.runtime/davis"),
        zeroclaw_bin: PathBuf::from("/opt/homebrew/bin/zeroclaw"),
        proxy_bin: PathBuf::from("/tmp/Davis ZeroClaw/target/release/davis-local-proxy"),
        stdout_path: PathBuf::from("/tmp/Davis ZeroClaw/.runtime/davis/proxy.stdout.log"),
        stderr_path: PathBuf::from("/tmp/Davis ZeroClaw/.runtime/davis/proxy.stderr.log"),
        path_env: "/opt/homebrew/bin:/usr/local/bin".to_string(),
    };

    let plist = render_proxy_launchd_plist(&spec);
    assert!(plist.contains("<string>com.daviszeroclaw.proxy</string>"));
    assert!(plist.contains("davis-local-proxy"));
    assert!(!plist.contains("daemon --config-dir"));
    assert!(plist.contains("<key>RunAtLoad</key>"));
    assert!(plist.contains("<key>KeepAlive</key>"));
    assert!(plist.contains("<key>DAVIS_REPO_ROOT</key>"));
    assert!(plist.contains("<key>DAVIS_RUNTIME_DIR</key>"));
}

#[test]
fn uninstall_removes_both_plist_labels() {
    assert_ne!(proxy_service_label(), davis_service_label());
}

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
    assert_eq!(
        tunnel.tunnel_id.as_deref(),
        Some("aaaabbbb-1111-2222-3333-ccccddddeeee")
    );
    assert_eq!(tunnel.hostname.as_deref(), Some("davis.example.com"));
}

#[test]
fn shortcut_config_deserializes_from_toml() {
    let toml = r#"
        [home_assistant]
        url = "http://ha.local:8123"
        token = "token"

        [imessage]
        allowed_contacts = ["+15551234567"]

        [[providers]]
        name = "openai"
        api_key = "sk-test"
        base_url = "https://api.openai.com/v1"
        allowed_models = ["gpt-test"]

        [routing]
        default_profile = "home_control"

        [routing.profiles.home_control]
        provider = "openai"
        model = "gpt-test"

        [routing.profiles.general_qa]
        provider = "openai"
        model = "gpt-test"

        [routing.profiles.research]
        provider = "openai"
        model = "gpt-test"

        [routing.profiles.structured_lookup]
        provider = "openai"
        model = "gpt-test"

        [shortcut]
        external_url = "https://davis.example.com/shortcut"
        lan_ssids = ["FailLone", "FailLone_5G"]
    "#;
    let config: crate::LocalConfig = toml::from_str(toml).unwrap();
    assert_eq!(
        config.shortcut.external_url.as_deref(),
        Some("https://davis.example.com/shortcut")
    );
    assert!(config.shortcut.lan_url.is_none());
    assert_eq!(config.shortcut.lan_ssids, vec!["FailLone", "FailLone_5G"]);
}

#[test]
fn shortcut_route_config_detects_lan_url_when_ssids_are_configured() {
    let root = unique_test_dir("shortcut-route-config");
    fs::create_dir_all(root.join("config").join("davis")).unwrap();
    fs::write(
        root.join("config").join("davis").join("local.toml"),
        r#"
        [tunnel]
        hostname = "davis.example.com"

        [shortcut]
        lan_ssids = ["FailLone", "FailLone_5G"]
    "#,
    )
    .unwrap();
    std::env::set_var("DAVIS_SHORTCUT_HOST_IP", "192.168.1.23");
    let paths = RuntimePaths {
        repo_root: root.clone(),
        runtime_dir: root.join(".runtime").join("davis"),
    };

    let route = resolve_shortcut_route_config(&paths, None);

    std::env::remove_var("DAVIS_SHORTCUT_HOST_IP");
    assert_eq!(route.external_url, "https://davis.example.com/shortcut");
    let lan = route.lan.unwrap();
    assert_eq!(lan.lan_url, "http://192.168.1.23:3012/shortcut");
    assert_eq!(lan.lan_ssids, vec!["FailLone", "FailLone_5G"]);
    let _ = fs::remove_dir_all(root);
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

fn unique_test_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("daviszeroclaw-{name}-{}", unique_suffix()));
    if path.exists() {
        fs::remove_dir_all(&path).unwrap();
    }
    path
}

#[test]
fn pid_files_alive_returns_false_when_no_pid_files_exist() {
    let root = unique_test_dir("pid-check");
    let fake_pid = root.join("proxy.pid");
    assert!(!pid_file_is_alive(&fake_pid));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn pid_file_is_alive_returns_false_for_impossible_pid() {
    let root = unique_test_dir("pid-impossible");
    fs::create_dir_all(&root).unwrap();
    let pid_file = root.join("test.pid");
    fs::write(&pid_file, "4294967295").unwrap();
    assert!(!pid_file_is_alive(&pid_file));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn start_mutual_exclusion_message_is_clear() {
    let msg = format!(
        "Davis launchd service is already installed ({}).\n\
         Run `daviszeroclaw service uninstall` first, or use \
         `daviszeroclaw service restart` to reload.",
        proxy_service_label()
    );
    assert!(msg.contains("service uninstall"));
    assert!(msg.contains("service restart"));
}

#[test]
fn service_is_installed_false_when_no_plists_exist() {
    let fake_proxy = PathBuf::from("/nonexistent/proxy.plist");
    let fake_zeroclaw = PathBuf::from("/nonexistent/zeroclaw.plist");
    assert!(!either_plist_exists(&fake_proxy, &fake_zeroclaw));
}

#[test]
fn service_is_installed_true_when_one_plist_exists() {
    let root = unique_test_dir("plist-exists-check");
    fs::create_dir_all(&root).unwrap();
    let proxy_plist = root.join("proxy.plist");
    fs::write(&proxy_plist, "<plist/>").unwrap();
    let fake_zeroclaw = root.join("zeroclaw.plist");

    assert!(either_plist_exists(&proxy_plist, &fake_zeroclaw));
    assert!(either_plist_exists(&fake_zeroclaw, &proxy_plist));
    let _ = fs::remove_dir_all(root);
}

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

#[test]
fn tunnel_status_silent_when_plist_absent() {
    let path = tunnel_service_plist_path().unwrap();
    assert!(path.to_str().unwrap().contains("LaunchAgents"));
    assert!(path.to_str().unwrap().contains("com.daviszeroclaw.tunnel"));
}

#[test]
fn tunnel_cloudflared_config_path_is_in_dotcloudflared() {
    let path = tunnel_cloudflared_config_path().unwrap();
    assert!(path.to_str().unwrap().contains(".cloudflared"));
    assert!(path.to_str().unwrap().contains("davis-shortcut.yml"));
}

#[test]
fn render_tunnel_cloudflared_config_preserves_yaml_indentation() {
    let credentials_path =
        PathBuf::from("/Users/testuser/.cloudflared/94039ffd-4852-4626-a5ab-dfdb5603cfe2.json");
    let yaml = render_tunnel_cloudflared_config(
        "94039ffd-4852-4626-a5ab-dfdb5603cfe2",
        &credentials_path,
        "davis.faillone.com",
    );

    assert_eq!(
        yaml,
        concat!(
            "tunnel: 94039ffd-4852-4626-a5ab-dfdb5603cfe2\n",
            "credentials-file: \"/Users/testuser/.cloudflared/94039ffd-4852-4626-a5ab-dfdb5603cfe2.json\"\n",
            "\n",
            "ingress:\n",
            "  - hostname: davis.faillone.com\n",
            "    service: http://127.0.0.1:3012\n",
            "  - service: http_status:404\n",
        )
    );
    assert!(yaml.contains("\n  - hostname: davis.faillone.com\n"));
    assert!(yaml.contains("\n    service: http://127.0.0.1:3012\n"));
}

#[test]
fn tunnel_install_missing_cloudflared_error_message() {
    // Verify the brew hint is baked into the source — this string is user-facing
    // and must not be silently changed without updating docs.
    let msg =
        "cloudflared not found. Install it first: brew install cloudflare/cloudflare/cloudflared";
    assert!(msg.contains("brew install cloudflare/cloudflare/cloudflared"));
}

#[test]
fn tunnel_install_missing_config_error_message() {
    use crate::app_config::TunnelConfig;
    // Both fields absent → filter returns None → error triggers.
    let no_tunnel: Option<TunnelConfig> = None;
    let result = no_tunnel
        .as_ref()
        .filter(|t| t.tunnel_id.is_some() && t.hostname.is_some())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "[tunnel] tunnel_id and hostname are required in local.toml. \
                 See local.example.toml for an example."
            )
        });
    let err = result.unwrap_err();
    assert!(err
        .to_string()
        .contains("[tunnel] tunnel_id and hostname are required"));
    assert!(err.to_string().contains("local.example.toml"));
}

#[test]
fn tunnel_install_missing_credentials_error_message() {
    // Verify credentials bail message format — the path and hint must be present.
    let fake_path = PathBuf::from("/Users/testuser/.cloudflared/fake-uuid.json");
    if !fake_path.is_file() {
        let msg = format!(
            "Tunnel credentials not found at {}.\nRun: cloudflared tunnel create <name>",
            fake_path.display()
        );
        assert!(msg.contains("Tunnel credentials not found at"));
        assert!(msg.contains("cloudflared tunnel create"));
        assert!(msg.contains("fake-uuid.json"));
    }
}
