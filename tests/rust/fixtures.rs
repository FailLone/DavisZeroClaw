use crate::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_PATH_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub(super) fn sample_states() -> Vec<Value> {
    vec![
        json!({"entity_id":"light.study_strip","state":"off","attributes":{"friendly_name":"书房灯带"}}),
        json!({"entity_id":"light.study_main","state":"off","attributes":{"friendly_name":"书房主灯"}}),
        json!({"entity_id":"switch.parents_chandelier","state":"off","attributes":{"friendly_name":"父母间吊灯"}}),
        json!({"entity_id":"switch.parents_chandelier_2","state":"on","attributes":{"friendly_name":"父母间吊灯"}}),
    ]
}

pub(super) fn sample_states_with_brightness() -> Vec<Value> {
    vec![
        json!({
            "entity_id":"light.study_strip",
            "state":"on",
            "attributes":{"friendly_name":"书房灯带","brightness":130}
        }),
        json!({"entity_id":"light.study_main","state":"off","attributes":{"friendly_name":"书房主灯"}}),
    ]
}

pub(super) fn sample_typed_states() -> Vec<HaState> {
    sample_states()
        .into_iter()
        .map(|value| serde_json::from_value(value).unwrap())
        .collect()
}

pub(super) fn sample_paths() -> RuntimePaths {
    let root = std::env::temp_dir().join(format!(
        "davis-zero-claw-test-{}-{}",
        std::process::id(),
        TEST_PATH_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let runtime_dir = root.join("runtime");
    let _ = std::fs::create_dir_all(runtime_dir.join("state"));
    RuntimePaths {
        repo_root: root,
        runtime_dir,
    }
}

pub(super) fn sample_local_config() -> LocalConfig {
    toml::from_str(
        r#"
[home_assistant]
url = "http://127.0.0.1:8123/api/mcp"
token = "test-token"

[imessage]
allowed_contacts = ["+8613800138000"]

[[providers]]
name = "openrouter"
api_key = "test-key"
base_url = ""
allowed_models = ["openai/gpt-4o"]

[routing]
recompute_interval_minutes = 30
restart_debounce_minutes = 10

[routing.profiles.home_control]
weights = { task_success = 0.45, safety = 0.30, stability = 0.15, latency = 0.08, cost = 0.02 }
minimums = { task_success = 80, safety = 90 }
max_fallbacks = 1

[routing.profiles.general_qa]
weights = { task_success = 0.42, latency = 0.28, stability = 0.15, safety = 0.10, cost = 0.05 }
minimums = { task_success = 60, safety = 40 }
max_fallbacks = 1

[routing.profiles.research]
weights = { task_success = 0.50, stability = 0.20, latency = 0.15, safety = 0.10, cost = 0.05 }
minimums = { task_success = 70, safety = 50 }
max_fallbacks = 1

[routing.profiles.structured_lookup]
weights = { task_success = 0.40, latency = 0.25, stability = 0.20, safety = 0.10, cost = 0.05 }
minimums = { task_success = 75, safety = 60 }
max_fallbacks = 1
"#,
    )
    .unwrap()
}

pub(super) fn sample_local_config_with_crawl4ai_base_url(base_url: &str) -> LocalConfig {
    let mut config = sample_local_config();
    config.crawl4ai.enabled = true;
    config.crawl4ai.transport = Crawl4aiTransport::Server;
    config.crawl4ai.base_url = base_url.trim_end_matches('/').to_string();
    config
}

pub(super) fn write_control_config(paths: &RuntimePaths, content: &str) {
    let path = paths.control_aliases_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

pub(super) fn sample_config() -> ControlConfig {
    let mut config = ControlConfig::default();
    config.entity_aliases.insert(
        "light.study_strip".to_string(),
        vec!["书房灯条".to_string()],
    );
    config
        .area_aliases
        .insert("父母间".to_string(), vec!["次卧".to_string()]);
    config.groups.insert(
        "书房的灯".to_string(),
        GroupConfig {
            entities: vec![
                "light.study_strip".to_string(),
                "light.study_main".to_string(),
            ],
            aliases: vec!["书房灯".to_string(), "书房灯光".to_string()],
        },
    );
    config
}

pub(super) fn sample_broken_group_config() -> ControlConfig {
    let mut config = sample_config();
    config.groups.insert(
        "坏掉的书房灯组".to_string(),
        GroupConfig {
            entities: vec![
                "light.study_strip".to_string(),
                "light.study_missing".to_string(),
            ],
            aliases: vec!["坏书房灯".to_string()],
        },
    );
    config
}

pub(super) fn sample_failure_summary(failure_count: usize) -> FailureSummary {
    let mut counts_by_reason = std::collections::BTreeMap::new();
    counts_by_reason.insert(FailureReason::ResolutionFailed, failure_count);
    FailureSummary {
        status: "ok".to_string(),
        window_hours: CONTROL_FAILURE_WINDOW_HOURS,
        threshold: CONTROL_FAILURE_THRESHOLD,
        failure_count,
        counts_by_reason,
        top_failed_queries: vec![TopFailedQuery {
            query_entity: "书房灯带".to_string(),
            count: failure_count,
        }],
        events: Vec::new(),
        suggestion_due: failure_count >= CONTROL_FAILURE_THRESHOLD,
        last_suggested_at: None,
    }
}
