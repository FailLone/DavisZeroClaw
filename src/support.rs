use crate::Issue;
use chrono::{DateTime, Utc};

pub fn normalize_text(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|ch| !matches!(ch, ' ' | '_' | '-' | '.'))
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn isoformat(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(crate) fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

pub(crate) fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

pub(crate) fn entity_domain(entity_id: &str) -> String {
    entity_id
        .split_once('.')
        .map(|pair| pair.0.to_string())
        .unwrap_or_default()
}

pub fn build_issue(issue_type: &str, query_entity: &str, suggestions: Vec<String>) -> Issue {
    let (category, actions, missing) = match issue_type {
        "missing_credentials" => (
            "configuration",
            vec![
                "Set home_assistant.url in config/davis/local.toml",
                "Set home_assistant.token in config/davis/local.toml",
            ],
            vec!["ha_url", "ha_token"],
        ),
        "ha_unreachable" => (
            "connectivity",
            vec![
                "Verify home_assistant.url in config/davis/local.toml is reachable",
                "Check network, DNS, reverse proxy, and TLS configuration",
            ],
            vec![],
        ),
        "ha_auth_failed" => (
            "authorization",
            vec![
                "Verify the current Long-Lived Access Token",
                "Regenerate home_assistant.token only if the current token cannot access HA REST endpoints",
            ],
            vec![],
        ),
        "recorder_not_enabled" => (
            "configuration",
            vec![
                "Enable recorder in Home Assistant",
                "Confirm the target entity is not excluded from recorder",
                "Confirm history/logbook retention covers the requested window",
            ],
            vec!["recorder", "history", "logbook"],
        ),
        "entity_not_found" => (
            "resolution",
            vec![
                "Use a more specific entity_id",
                "Try the entity's friendly name",
                "Inspect Home Assistant states to confirm the real entity_id",
            ],
            vec![],
        ),
        "entity_ambiguous" => (
            "resolution",
            vec![
                "Use the full entity_id instead of a shorthand",
                "Choose one of the suggested candidate entities",
            ],
            vec![],
        ),
        "group_members_missing" => (
            "configuration",
            vec![
                "Review the configured group members in control_aliases.json",
                "Remove stale entity_ids or rename the group to match current Home Assistant entities",
                "Regenerate the config report and fix the affected group before retrying control",
            ],
            vec!["group_entities"],
        ),
        "browser_automation_unavailable" | "crawl4ai_unavailable" => (
            "configuration",
            vec![
                "Verify the configured Python can import Crawl4AI and Playwright",
                "Run the crawl profile login helper again after the Crawl4AI adapter is available",
            ],
            vec!["crawl4ai"],
        ),
        "auth_required" => (
            "authorization",
            vec![
                "Run the crawl profile login helper for the affected platform",
                "Complete the login flow in the Crawl4AI-compatible browser window",
            ],
            vec!["site_session"],
        ),
        "site_changed" => (
            "integration",
            vec![
                "Open the affected order page in a headed browser and verify the latest DOM structure",
                "Update the extractor script selectors before retrying",
            ],
            vec!["site_dom"],
        ),
        "remote_debugging_required" => (
            "configuration",
            vec![
                "Enable Chrome remote debugging before using the user browser profile",
                "Or rely on the AppleScript read-only fallback on macOS",
            ],
            vec!["chrome_remote_debugging"],
        ),
        "write_confirmation_required" => (
            "authorization",
            vec![
                "Ask the user to confirm the pending browser write action",
                "Restrict write automation to explicitly trusted surfaces",
            ],
            vec!["user_confirmation"],
        ),
        "write_blocked" => (
            "authorization",
            vec![
                "Restrict the request to a trusted automation surface",
                "Review the automation policy before enabling direct writes",
            ],
            vec!["allowed_origin"],
        ),
        "unsupported_surface" => (
            "integration",
            vec![
                "Retry the action on a Crawl4AI-backed surface",
                "Verify the required browser automation runtime is installed",
            ],
            vec!["playwright"],
        ),
        _ => (
            "request",
            vec![
                "Provide entity_id",
                "Use a valid time range where start is earlier than end",
            ],
            vec!["entity_id"],
        ),
    };
    Issue {
        issue_type: issue_type.to_string(),
        issue_category: category.to_string(),
        query_entity: query_entity.to_string(),
        recommended_actions: actions.into_iter().map(str::to_string).collect(),
        missing_requirements: missing.into_iter().map(str::to_string).collect(),
        suggestions,
    }
}
