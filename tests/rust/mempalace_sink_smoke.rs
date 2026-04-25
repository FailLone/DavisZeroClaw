//! End-to-end smoke test: Davis sink → real MemPalace MCP server.
//!
//! `#[ignore]` by default because it needs a working MemPalace venv. Run with:
//!
//! ```bash
//! DAVIS_MEMPALACE_VENV=/path/to/.runtime/davis/mempalace-venv \
//!   cargo test --lib -- --ignored smoke --nocapture
//! ```
//!
//! The venv must have `mempalace` importable and `python -m mempalace.mcp_server`
//! must launch cleanly.
//!
//! What the smoke test covers:
//! - All four driver tool mappings actually work (`mempalace_add_drawer`,
//!   `mempalace_kg_add`, `mempalace_kg_invalidate`, `mempalace_diary_write`).
//! - `success=false` business errors propagate into `failed` + `last_error`
//!   (regression test for the Phase 1 driver dispatch bug).
//! - A drawer written via the sink is retrievable via `mempalace_search`, so we
//!   know the data actually landed in the palace, not just that the JSON-RPC
//!   hop succeeded.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::mempalace_sink::{MemPalaceSink, Predicate, TripleId};
use crate::runtime_paths::RuntimePaths;

fn runtime_paths_from_env() -> Option<RuntimePaths> {
    let venv = std::env::var_os("DAVIS_MEMPALACE_VENV")?;
    let venv_dir = PathBuf::from(venv);
    // Parent of the venv is assumed to be the runtime dir ({runtime}/mempalace-venv).
    let runtime_dir = venv_dir.parent()?.to_path_buf();
    let repo_root = std::env::current_dir().ok()?;
    Some(RuntimePaths {
        repo_root,
        runtime_dir,
    })
}

async fn wait_for_metric<F: Fn(&crate::mempalace_sink::SinkMetrics) -> bool>(
    sink: &MemPalaceSink,
    predicate: F,
    label: &str,
    timeout: Duration,
) -> crate::mempalace_sink::SinkMetrics {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let m = sink.metrics().await;
        if predicate(&m) {
            return m;
        }
        if std::time::Instant::now() > deadline {
            panic!("smoke: timed out waiting for {label}: {m:?}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Single smoke test exercising all four tool mappings. We intentionally do
/// NOT split this into multiple `#[tokio::test]` functions: tokio runs tests
/// in parallel and two sinks pointed at the same MemPalace palace dir will
/// race on Chroma's sqlite lock, producing flaky `stdio reader closed` errors.
#[tokio::test]
#[ignore]
async fn smoke_exercises_all_four_tool_mappings_and_verifies_drawer_in_palace() {
    let Some(paths) = runtime_paths_from_env() else {
        eprintln!("DAVIS_MEMPALACE_VENV not set; skipping");
        return;
    };
    let sink = MemPalaceSink::spawn(&paths);

    // Unique marker so `mempalace_search` finds this run's drawer specifically.
    // Keep it pure ASCII — MemPalace serializes search results with
    // `ensure_ascii=True`, so non-ASCII chars come back as `\uXXXX` escapes
    // which defeat a naive `contains` check.
    let tag = format!("davisSmokeTag{}", chrono::Utc::now().timestamp());
    let marker = format!("davis smoke {tag} cross-tool mapping check");

    sink.add_drawer("davis.test", "smoke", &marker);
    sink.diary_write("davis.agent.smoke", &format!("smoke diary {tag}"));
    sink.kg_add(
        TripleId::entity(&format!("smoke.entity.{tag}")),
        Predicate::EntityHasState,
        TripleId::entity("smoke.state.on"),
    );
    sink.kg_invalidate(
        TripleId::entity(&format!("smoke.entity.{tag}")),
        Predicate::EntityHasState,
        TripleId::entity("smoke.state.on"),
    );

    let m = wait_for_metric(
        &sink,
        |m| m.sent + m.failed >= 4,
        "4 tool calls",
        Duration::from_secs(45),
    )
    .await;
    assert_eq!(
        m.sent, 4,
        "expected all four tools to succeed: {m:?}. If one is failed, \
         inspect last_error — it likely flags a tool-name or schema drift.",
    );
    assert_eq!(m.failed, 0, "unexpected failures: {m:?}");

    // Now verify the drawer actually materialised in the palace by spinning up
    // a second short-lived MCP client and calling `mempalace_search`.
    verify_drawer_searchable(&paths, &tag, &marker).await;
    println!("cross-tool smoke OK: {m:?}");
}

async fn verify_drawer_searchable(paths: &RuntimePaths, tag: &str, marker: &str) {
    use crate::mempalace_sink::McpStdioClient;
    let (program, args) = paths.mempalace_mcp_server_cmd();
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&args);
    let client = McpStdioClient::spawn(cmd)
        .await
        .expect("spawn second MCP client for verification");
    client
        .initialize(&crate::mempalace_sink::InitializeParams {
            client_name: "davis-smoke-verifier".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
        })
        .await
        .expect("initialize second MCP client");

    // MemPalace needs a brief moment for Chroma to persist + reopen for reads.
    // We retry a few times before giving up.
    let mut last: Option<Value> = None;
    for _ in 0..5 {
        let value = client
            .call_tool(
                "mempalace_search",
                json!({"query": tag, "wing": "davis.test", "limit": 3}),
            )
            .await
            .expect("search call");
        last = Some(value.clone());
        if value
            .get("content")
            .and_then(Value::as_array)
            .and_then(|arr| arr.iter().find_map(|i| i.get("text")))
            .and_then(Value::as_str)
            .is_some_and(|t| t.contains(marker))
        {
            client.shutdown().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    client.shutdown().await;
    panic!(
        "smoke: drawer for tag {tag} not found via search; last response: {}",
        last.map(|v| v.to_string()).unwrap_or_default(),
    );
}

// =====================================================================
// Integrated Phase 2-5 smoke: drive every projection path against a real
// MemPalace venv, then verify the resulting drawers + KG triples via a
// second MCP client.
// =====================================================================

#[tokio::test]
#[ignore]
async fn smoke_all_phases_exercise_every_projection_path() {
    let Some(paths) = runtime_paths_from_env() else {
        eprintln!("DAVIS_MEMPALACE_VENV not set; skipping");
        return;
    };
    let sink = MemPalaceSink::spawn(&paths);
    let tag = format!("davisSmokeAll{}", chrono::Utc::now().timestamp());

    // --- Phase 2: HA projection ---
    drive_ha_projection(&sink, &tag);

    // --- Phase 3: Article projection ---
    drive_article_projection(&sink, &tag);

    // --- Phase 4: Rule-learning projection ---
    drive_rule_projection(&sink, &tag);

    // --- Phase 5: Debouncer projections ---
    drive_debouncer_projections(&sink, &tag);

    // All phases together fire the following counts — keep this in lockstep
    // with the drive_* helpers.
    let expected_sent =
        ha_emit_count() + article_emit_count() + rule_emit_count() + debouncer_emit_count();
    let m = wait_for_metric(
        &sink,
        move |m| m.sent + m.failed >= expected_sent,
        "all projection calls",
        Duration::from_secs(90),
    )
    .await;
    assert_eq!(
        m.sent, expected_sent,
        "expected {expected_sent} successful projection calls; metrics {m:?}. \
         If `failed > 0`, inspect last_error for the first offending tool."
    );
    assert_eq!(m.failed, 0, "no projection call should fail: {m:?}");
    assert_eq!(m.dropped, 0, "mpsc queue should not overflow: {m:?}");
    println!("integrated smoke OK: {m:?}");

    // --- Verification: cross-check via a second MCP client ---
    verify_integrated_projections(&paths, &tag).await;
}

// ---- Phase 2: HA ---------------------------------------------------------

fn ha_emit_count() -> u64 {
    // First snapshot emits 1 EntityHasState add (one unavailable entity in
    // fixture). Findings drawer: one area (`Living-Room`) with bad_names →
    // 1 drawer. Refresh diary: always 1. Findings projections emit 2 KG
    // triples (replacement above threshold + located_in + name_issue needs
    // 2 cycles so 0) — in this single-snapshot drive, only EntityLocatedIn
    // and EntityReplacementFor fire.
    1 + 1 + 1 + 1 + 1 // state_add + drawer + diary + located_in + replacement
}

fn drive_ha_projection(sink: &MemPalaceSink, tag: &str) {
    use crate::ha_mcp::{
        HaMcpBadNameFinding, HaMcpEntityObservation, HaMcpLiveContextFindings,
        HaMcpLiveContextReport, HaMcpPossibleReplacementFinding,
    };
    use crate::ha_mcp_projection;

    let observations = vec![HaMcpEntityObservation {
        signature: format!("light|{tag}-broken|living_room"),
        name: format!("{tag}-broken"),
        domain: "light".into(),
        state: "unavailable".into(),
        areas: vec!["Living Room".into()],
    }];
    let findings = HaMcpLiveContextFindings {
        bad_names: vec![HaMcpBadNameFinding {
            name: format!("{tag}-name"),
            domain: "light".into(),
            areas: vec!["Living Room".into()],
            reasons: vec!["mixed_cjk_ascii".into()],
        }],
        possible_replacements: vec![HaMcpPossibleReplacementFinding {
            unavailable_name: format!("{tag}-broken"),
            replacement_name: format!("{tag}-spare"),
            domain: "light".into(),
            unavailable_areas: vec!["Living Room".into()],
            replacement_areas: vec!["Living Room".into()],
            score: 75,
            reasons: vec!["name-similarity".into()],
            time_signals: vec![],
        }],
        ..Default::default()
    };
    let report = HaMcpLiveContextReport {
        status: "ok".into(),
        endpoint: "http://example.com".into(),
        source_tool: "GetLiveContext".into(),
        entity_count: 1,
        unavailable_count: 1,
        observations,
        findings,
        ..Default::default()
    };
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    ha_mcp_projection::emit_state_transitions(None, &report, sink);
    ha_mcp_projection::emit_findings_projections(None, &report, sink);
    ha_mcp_projection::emit_findings_drawer(&report, &ts, sink);
    ha_mcp_projection::emit_refresh_diary(&report, &ts, sink);
}

// ---- Phase 3: Article ---------------------------------------------------

fn article_emit_count() -> u64 {
    // SourcedFrom (1) + 2 Discusses (2 topics) + drawer (1) + diary (1).
    1 + 2 + 1 + 1
}

fn drive_article_projection(sink: &MemPalaceSink, tag: &str) {
    use crate::article_memory::mempalace_projection;
    use crate::article_memory::ArticleValueReport;

    let article_id = format!("{tag}-art");
    let report = ArticleValueReport {
        article_id: article_id.clone(),
        title: format!("Smoke test article {tag}"),
        url: Some("https://lobste.rs/s/davissmoke".into()),
        judged_at: "2026-04-25T12:00:00Z".into(),
        decision: "save".into(),
        value_score: 0.82,
        deterministic_reject: false,
        reasons: vec!["integrated smoke coverage".into()],
        topic_tags: vec!["smoke-testing".into(), "davis-integration".into()],
        risk_flags: vec![],
        translation_needed: false,
        model: Some("claude-haiku-4-5".into()),
        extraction_quality: "clean".into(),
        extraction_issues: vec![],
        rule_refinement_hint: None,
    };
    mempalace_projection::emit_article_success(&report, sink);

    let diary_entry = mempalace_projection::IngestDiaryEntry {
        timestamp_iso: "2026-04-25T12:00:00Z".into(),
        job_id: format!("{tag}-job"),
        status: mempalace_projection::IngestDiaryStatus::Saved,
        host: Some("lobste.rs".into()),
        article_id: Some(article_id),
        value_decision: Some("save".into()),
        value_score: Some(0.82),
        reason: None,
    };
    mempalace_projection::emit_ingest_diary(&diary_entry, sink);
}

// ---- Phase 4: Rule ------------------------------------------------------

fn rule_emit_count() -> u64 {
    // Promote (1 active_for) + diary (1) + quarantine (1 quarantined_by) +
    // diary (1).
    1 + 1 + 1 + 1
}

fn drive_rule_projection(sink: &MemPalaceSink, tag: &str) {
    use crate::article_memory::ingest::rule_mempalace_projection::{
        emit_rule_learner_diary, emit_rule_promoted, emit_rule_quarantined, RuleLearnerDiaryEntry,
        RuleLearnerOutcome,
    };
    use crate::article_memory::LearnedRule;

    let host = format!("{tag}.example.com");
    let rule = LearnedRule {
        host: host.clone(),
        version: "2026-04-25T12:00:00Z".into(),
        content_selectors: vec!["article".into()],
        remove_selectors: vec![],
        title_selector: None,
        start_markers: vec![],
        end_markers: vec![],
        confidence: 0.85,
        reasoning: "smoke fixture".into(),
        learned_from_sample_count: 5,
        stale: false,
    };
    emit_rule_promoted(&host, &rule, None, sink);
    emit_rule_learner_diary(
        &RuleLearnerDiaryEntry {
            timestamp_iso: "2026-04-25T12:00:00Z".into(),
            host: host.clone(),
            outcome: RuleLearnerOutcome::Saved,
            version: Some(rule.version.clone()),
            sample_count: Some(5),
            reason: None,
        },
        sink,
    );

    // Quarantine a different host so the version hash differs.
    let bad_host = format!("{tag}-bad.example.com");
    let bad_rule = LearnedRule {
        host: bad_host.clone(),
        version: "2026-04-25T13:00:00Z".into(),
        content_selectors: vec!["main".into()],
        remove_selectors: vec![],
        title_selector: None,
        start_markers: vec![],
        end_markers: vec![],
        confidence: 0.35,
        reasoning: "validation failed".into(),
        learned_from_sample_count: 3,
        stale: false,
    };
    emit_rule_quarantined(&bad_host, &bad_rule, "extraction_quality=poor", sink);
    emit_rule_learner_diary(
        &RuleLearnerDiaryEntry {
            timestamp_iso: "2026-04-25T13:00:00Z".into(),
            host: bad_host,
            outcome: RuleLearnerOutcome::Quarantined,
            version: Some(bad_rule.version),
            sample_count: Some(3),
            reason: Some("extraction_quality=poor".into()),
        },
        sink,
    );
}

// ---- Phase 5: Debouncers -------------------------------------------------

fn debouncer_emit_count() -> u64 {
    // SampleDebouncer flips after 2 unhealthy → 1 kg_add. TimeDebouncer
    // with sustained=0 flips on first unhealthy → 1 kg_add.
    1 + 1
}

fn drive_debouncer_projections(sink: &MemPalaceSink, tag: &str) {
    use crate::mempalace_sink::{Predicate, SampleDebouncer, TimeDebouncer, TripleId};

    let worker_subject = TripleId::worker(&format!("{tag}-ingest"));
    let backlog = TripleId::entity("state.backlogged");
    let sample = SampleDebouncer::new(2);
    // 2 unhealthy samples → 1 emit.
    for _ in 0..2 {
        sample.record(
            "ingest",
            &worker_subject,
            Predicate::WorkerHealth,
            &backlog,
            true,
            sink,
        );
    }

    let component_subject = TripleId::component(&format!("{tag}-component"));
    let unreachable = TripleId::entity("state.unreachable");
    // sustained=0 → any unhealthy sample flips immediately.
    let time = TimeDebouncer::new(Duration::from_secs(0));
    time.record(
        "component",
        &component_subject,
        Predicate::ComponentReachability,
        &unreachable,
        true,
        std::time::Instant::now(),
        sink,
    );
}

// ---- Verification via second MCP client ---------------------------------

async fn verify_integrated_projections(paths: &RuntimePaths, tag: &str) {
    use crate::mempalace_sink::{InitializeParams, McpStdioClient};
    let (program, args) = paths.mempalace_mcp_server_cmd();
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(&args);
    let client = McpStdioClient::spawn(cmd).await.expect("spawn verifier");
    client
        .initialize(&InitializeParams {
            client_name: "davis-integrated-smoke-verifier".into(),
            client_version: env!("CARGO_PKG_VERSION").into(),
        })
        .await
        .expect("initialize verifier");

    // 1. HA findings drawer should be searchable under davis.ha.
    expect_drawer_contains(&client, tag, "davis.ha", &format!("{tag}-name")).await;
    // 2. Article value drawer should be searchable under davis.articles.
    //    Drawer body contains the title/url/topics but not the raw article_id,
    //    so we assert the title-side marker the drawer actually carries.
    expect_drawer_contains(
        &client,
        tag,
        "davis.articles",
        &format!("Smoke test article {tag}"),
    )
    .await;

    // 3. KG has article → host and article → topic triples.
    expect_kg_subject_present(&client, &format!("article_{tag}-art"), "sourced_from").await;
    expect_kg_subject_present(&client, &format!("article_{tag}-art"), "discusses").await;

    // 4. Rule active-for + quarantined-by triples are present. We query by
    //    `host_*` (kg_query accepts the exact subject) for active_for, and
    //    use kg_timeline to locate the quarantined-by fact because its
    //    subject is a versioned `ruleVersion_<host>.<version-slug>` that
    //    the smoke doesn't know exactly.
    expect_kg_subject_present(
        &client,
        &format!("host_{tag}.example.com"),
        "rule_active_for",
    )
    .await;
    expect_kg_predicate_somewhere(&client, "rule_quarantined_by", tag).await;

    // 5. Worker health + component reachability triples fired.
    expect_kg_subject_present(&client, &format!("worker_{tag}-ingest"), "worker_health").await;
    expect_kg_subject_present(
        &client,
        &format!("component_{tag}-component"),
        "component_reachability",
    )
    .await;

    client.shutdown().await;
}

async fn expect_drawer_contains(
    client: &crate::mempalace_sink::McpStdioClient,
    tag: &str,
    wing: &str,
    expected_marker: &str,
) {
    for attempt in 0..5 {
        let value = client
            .call_tool(
                "mempalace_search",
                json!({"query": tag, "wing": wing, "limit": 5}),
            )
            .await
            .expect("search call");
        if let Some(text) = value
            .get("content")
            .and_then(Value::as_array)
            .and_then(|arr| arr.iter().find_map(|i| i.get("text")))
            .and_then(Value::as_str)
        {
            if text.contains(expected_marker) {
                return;
            }
            if attempt == 4 {
                panic!(
                    "smoke: wing={wing} missing marker `{expected_marker}` after retries.\n\
                     last response: {text}"
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    panic!("smoke: wing={wing} search never returned a text payload");
}

/// Scan the whole KG timeline for at least one fact with the given
/// predicate whose subject or object mentions the run tag. Used when the
/// smoke doesn't know the exact slugged subject (e.g. versioned rule ids).
async fn expect_kg_predicate_somewhere(
    client: &crate::mempalace_sink::McpStdioClient,
    predicate: &str,
    tag: &str,
) {
    let tag_lc = tag.to_ascii_lowercase();
    for attempt in 0..5 {
        let value = client
            .call_tool("mempalace_kg_timeline", json!({}))
            .await
            .expect("kg_timeline call");
        if let Some(text) = value
            .get("content")
            .and_then(Value::as_array)
            .and_then(|arr| arr.iter().find_map(|i| i.get("text")))
            .and_then(Value::as_str)
        {
            if let Ok(inner) = serde_json::from_str::<Value>(text) {
                let facts = inner
                    .get("timeline")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                // MemPalace timeline echoes the original-case entity strings
                // (kg_query lowercases for lookups only). Match case-insensitively.
                let hit = facts.iter().any(|f| {
                    let pred_ok = f.get("predicate").and_then(Value::as_str) == Some(predicate);
                    let subject_mentions_tag = f
                        .get("subject")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s.to_ascii_lowercase().contains(&tag_lc));
                    let object_mentions_tag = f
                        .get("object")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s.to_ascii_lowercase().contains(&tag_lc));
                    pred_ok && (subject_mentions_tag || object_mentions_tag)
                });
                if hit {
                    return;
                }
                if attempt == 4 {
                    panic!(
                        "smoke: no KG fact with predicate={predicate} mentions tag \
                         {tag_lc}. timeline count: {}",
                        facts.len()
                    );
                }
            } else if attempt == 4 {
                panic!("smoke: kg_timeline text not JSON: {text}");
            }
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    panic!("smoke: kg_timeline never returned parseable response");
}

async fn expect_kg_subject_present(
    client: &crate::mempalace_sink::McpStdioClient,
    subject: &str,
    predicate: &str,
) {
    // MemPalace 3.1.0 kg_query takes `entity` (not subject/predicate),
    // lowercases its inputs before storage, and returns
    // `{"entity": ..., "facts": [...]}`. Lowercase here to match.
    let normalized = subject.to_ascii_lowercase();
    for attempt in 0..5 {
        let value = client
            .call_tool("mempalace_kg_query", json!({"entity": normalized}))
            .await
            .expect("kg_query call");
        if let Some(text) = value
            .get("content")
            .and_then(Value::as_array)
            .and_then(|arr| arr.iter().find_map(|i| i.get("text")))
            .and_then(Value::as_str)
        {
            if let Ok(inner) = serde_json::from_str::<Value>(text) {
                let facts = inner.get("facts").and_then(Value::as_array);
                let hit = facts
                    .map(|arr| {
                        arr.iter()
                            .any(|f| f.get("predicate").and_then(Value::as_str) == Some(predicate))
                    })
                    .unwrap_or(false);
                if hit {
                    return;
                }
                if attempt == 4 {
                    panic!(
                        "smoke: KG query for entity={subject} found no fact with \
                         predicate={predicate}. facts: {}",
                        serde_json::to_string_pretty(&inner).unwrap_or_default()
                    );
                }
            } else if attempt == 4 {
                panic!("smoke: KG query text was not JSON for entity={subject}. text: {text}");
            }
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    panic!("smoke: KG query never returned a parseable response");
}
