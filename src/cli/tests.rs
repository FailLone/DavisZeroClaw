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
    assert!(plist.contains("daemon --config-dir &apos;/tmp/Davis ZeroClaw/.runtime/davis&apos;"));
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
