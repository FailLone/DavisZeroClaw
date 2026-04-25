//! Tests for src/ha_mcp_projection.rs (Phase 2 HA→MemPalace projection).
//! Separated from the impl module so the module file stays under the 800-line
//! cap mandated by CLAUDE.md.

use crate::ha_mcp::{
    HaMcpBadNameFinding, HaMcpDuplicateNameFinding, HaMcpEntityObservation,
    HaMcpLiveContextFindings, HaMcpLiveContextReport, HaMcpMissingAreaExposureFinding,
    HaMcpPossibleReplacementFinding,
};
use crate::ha_mcp_projection::*;
use crate::mempalace_sink::{Predicate, SpySink};

fn obs(sig: &str, name: &str, domain: &str, state: &str) -> HaMcpEntityObservation {
    HaMcpEntityObservation {
        signature: sig.to_string(),
        name: name.to_string(),
        domain: domain.to_string(),
        state: state.to_string(),
        areas: vec![],
    }
}

fn report_with(observations: Vec<HaMcpEntityObservation>) -> HaMcpLiveContextReport {
    HaMcpLiveContextReport {
        status: "ok".to_string(),
        endpoint: "http://example.com".to_string(),
        source_tool: "GetLiveContext".to_string(),
        observations,
        findings: HaMcpLiveContextFindings::default(),
        ..Default::default()
    }
}

#[test]
fn transitioning_to_unavailable_emits_kg_add() {
    let prev = report_with(vec![obs("light|a|", "A", "light", "on")]);
    let next = report_with(vec![obs("light|a|", "A", "light", "unavailable")]);
    let spy = SpySink::default();
    emit_state_transitions(Some(&prev), &next, &spy);
    let adds = spy.kg_adds();
    assert_eq!(adds.len(), 1, "{adds:?}");
    assert_eq!(adds[0].predicate, Predicate::EntityHasState);
    assert!(adds[0].subject.starts_with("entity_"));
    assert_eq!(adds[0].object, "entity_state.unavailable");
    assert!(
        spy.kg_invalidates().is_empty(),
        "{:?}",
        spy.kg_invalidates()
    );
}

#[test]
fn recovery_from_unavailable_emits_kg_invalidate() {
    let prev = report_with(vec![obs("light|a|", "A", "light", "unavailable")]);
    let next = report_with(vec![obs("light|a|", "A", "light", "on")]);
    let spy = SpySink::default();
    emit_state_transitions(Some(&prev), &next, &spy);
    let inv = spy.kg_invalidates();
    assert_eq!(inv.len(), 1, "{inv:?}");
    assert_eq!(inv[0].predicate, Predicate::EntityHasState);
    assert_eq!(inv[0].object, "entity_state.unavailable");
    assert!(spy.kg_adds().is_empty(), "{:?}", spy.kg_adds());
}

#[test]
fn state_transition_unavailable_to_unknown_swaps_triples() {
    let prev = report_with(vec![obs("light|a|", "A", "light", "unavailable")]);
    let next = report_with(vec![obs("light|a|", "A", "light", "unknown")]);
    let spy = SpySink::default();
    emit_state_transitions(Some(&prev), &next, &spy);
    let inv = spy.kg_invalidates();
    let adds = spy.kg_adds();
    assert_eq!(inv.len(), 1);
    assert_eq!(inv[0].object, "entity_state.unavailable");
    assert_eq!(adds.len(), 1);
    assert_eq!(adds[0].object, "entity_state.unknown");
}

#[test]
fn daily_on_off_transitions_are_ignored() {
    let prev = report_with(vec![obs("light|a|", "A", "light", "on")]);
    let next = report_with(vec![obs("light|a|", "A", "light", "off")]);
    let spy = SpySink::default();
    emit_state_transitions(Some(&prev), &next, &spy);
    assert!(spy.kg_adds().is_empty(), "{:?}", spy.kg_adds());
    assert!(spy.kg_invalidates().is_empty());
}

#[test]
fn first_snapshot_files_only_unhealthy_entities() {
    let next = report_with(vec![
        obs("light|a|", "A", "light", "on"),
        obs("light|b|", "B", "light", "unavailable"),
    ]);
    let spy = SpySink::default();
    emit_state_transitions(None, &next, &spy);
    let adds = spy.kg_adds();
    assert_eq!(adds.len(), 1);
    assert_eq!(adds[0].object, "entity_state.unavailable");
    // The healthy entity did not generate a triple.
    assert!(adds.iter().all(|t| !t.subject.contains("A.")) || adds[0].subject.contains("B."));
}

#[test]
fn vanishing_entity_in_unhealthy_state_invalidates() {
    let prev = report_with(vec![obs("sensor|z|", "Z", "sensor", "unknown")]);
    let next = report_with(vec![]);
    let spy = SpySink::default();
    emit_state_transitions(Some(&prev), &next, &spy);
    let inv = spy.kg_invalidates();
    assert_eq!(inv.len(), 1, "{inv:?}");
    assert_eq!(inv[0].object, "entity_state.unknown");
}

#[test]
fn cjk_entity_name_produces_safe_subject() {
    let next = report_with(vec![obs(
        "light|客厅主灯|",
        "客厅主灯",
        "light",
        "unavailable",
    )]);
    let spy = SpySink::default();
    emit_state_transitions(None, &next, &spy);
    let adds = spy.kg_adds();
    assert_eq!(adds.len(), 1);
    let s = &adds[0].subject;
    assert!(s.starts_with("entity_"), "{s}");
    // The ASCII portion (domain "light") must be present for readability.
    assert!(s.contains("light"), "{s}");
}

fn report_with_findings(
    observations: Vec<HaMcpEntityObservation>,
    findings: HaMcpLiveContextFindings,
) -> HaMcpLiveContextReport {
    HaMcpLiveContextReport {
        status: "ok".to_string(),
        endpoint: "x".into(),
        source_tool: "t".into(),
        observations,
        findings,
        ..Default::default()
    }
}

#[test]
fn replacement_above_threshold_emits_kg_add_on_first_seen() {
    let mut findings = HaMcpLiveContextFindings::default();
    findings
        .possible_replacements
        .push(HaMcpPossibleReplacementFinding {
            unavailable_name: "A".into(),
            replacement_name: "B".into(),
            domain: "light".into(),
            unavailable_areas: vec![],
            replacement_areas: vec![],
            score: 72,
            reasons: vec![],
            time_signals: vec![],
        });
    let next = report_with_findings(vec![], findings);
    let spy = SpySink::default();
    emit_findings_projections(None, &next, &spy);
    let adds = spy.kg_adds();
    let reps: Vec<_> = adds
        .iter()
        .filter(|t| t.predicate == Predicate::EntityReplacementFor)
        .collect();
    assert_eq!(reps.len(), 1, "{adds:?}");
    assert!(reps[0].subject.contains("A") || reps[0].subject.starts_with("entity_"));
}

#[test]
fn replacement_below_threshold_does_not_emit() {
    let mut findings = HaMcpLiveContextFindings::default();
    findings
        .possible_replacements
        .push(HaMcpPossibleReplacementFinding {
            unavailable_name: "A".into(),
            replacement_name: "B".into(),
            domain: "light".into(),
            unavailable_areas: vec![],
            replacement_areas: vec![],
            score: 45, // above EXIT but below ENTER
            reasons: vec![],
            time_signals: vec![],
        });
    let next = report_with_findings(vec![], findings);
    let spy = SpySink::default();
    emit_findings_projections(None, &next, &spy);
    assert!(spy
        .kg_adds()
        .iter()
        .all(|t| t.predicate != Predicate::EntityReplacementFor));
}

#[test]
fn replacement_dropping_below_exit_emits_invalidate() {
    let mut prev_findings = HaMcpLiveContextFindings::default();
    prev_findings
        .possible_replacements
        .push(HaMcpPossibleReplacementFinding {
            unavailable_name: "A".into(),
            replacement_name: "B".into(),
            domain: "light".into(),
            unavailable_areas: vec![],
            replacement_areas: vec![],
            score: 70,
            reasons: vec![],
            time_signals: vec![],
        });
    let mut next_findings = HaMcpLiveContextFindings::default();
    next_findings
        .possible_replacements
        .push(HaMcpPossibleReplacementFinding {
            unavailable_name: "A".into(),
            replacement_name: "B".into(),
            domain: "light".into(),
            unavailable_areas: vec![],
            replacement_areas: vec![],
            score: 30,
            reasons: vec![],
            time_signals: vec![],
        });
    let prev = report_with_findings(vec![], prev_findings);
    let next = report_with_findings(vec![], next_findings);
    let spy = SpySink::default();
    emit_findings_projections(Some(&prev), &next, &spy);
    let inv: Vec<_> = spy
        .kg_invalidates()
        .into_iter()
        .filter(|t| t.predicate == Predicate::EntityReplacementFor)
        .collect();
    assert_eq!(inv.len(), 1, "{inv:?}");
}

#[test]
fn located_in_first_seen_emits_add() {
    let next = report_with_findings(
        vec![HaMcpEntityObservation {
            signature: "light|a|living_room".into(),
            name: "A".into(),
            domain: "light".into(),
            state: "on".into(),
            areas: vec!["Living Room".into()],
        }],
        HaMcpLiveContextFindings::default(),
    );
    let spy = SpySink::default();
    emit_findings_projections(None, &next, &spy);
    let adds: Vec<_> = spy
        .kg_adds()
        .into_iter()
        .filter(|t| t.predicate == Predicate::EntityLocatedIn)
        .collect();
    assert_eq!(adds.len(), 1);
    assert!(adds[0].object.starts_with("area_"));
    assert!(adds[0].object.contains("Living-Room"));
}

#[test]
fn located_in_area_change_invalidates_old_then_adds_new() {
    let prev = report_with_findings(
        vec![HaMcpEntityObservation {
            signature: "light|a|living_room".into(),
            name: "A".into(),
            domain: "light".into(),
            state: "on".into(),
            areas: vec!["Living Room".into()],
        }],
        HaMcpLiveContextFindings::default(),
    );
    let next = report_with_findings(
        vec![HaMcpEntityObservation {
            signature: "light|a|living_room".into(),
            name: "A".into(),
            domain: "light".into(),
            state: "on".into(),
            areas: vec!["Bedroom".into()],
        }],
        HaMcpLiveContextFindings::default(),
    );
    let spy = SpySink::default();
    emit_findings_projections(Some(&prev), &next, &spy);
    let inv: Vec<_> = spy
        .kg_invalidates()
        .into_iter()
        .filter(|t| t.predicate == Predicate::EntityLocatedIn)
        .collect();
    let adds: Vec<_> = spy
        .kg_adds()
        .into_iter()
        .filter(|t| t.predicate == Predicate::EntityLocatedIn)
        .collect();
    assert_eq!(inv.len(), 1, "inv={inv:?}");
    assert!(inv[0].object.contains("Living-Room"));
    assert_eq!(adds.len(), 1);
    assert!(adds[0].object.contains("Bedroom"));
}

#[test]
fn name_issue_requires_two_consecutive_detections() {
    let issue = HaMcpBadNameFinding {
        name: "光1".into(),
        domain: "light".into(),
        areas: vec![],
        reasons: vec!["mixed_cjk_ascii".into()],
    };
    let findings_first_cycle = HaMcpLiveContextFindings {
        bad_names: vec![issue.clone()],
        ..Default::default()
    };
    let findings_second_cycle = HaMcpLiveContextFindings {
        bad_names: vec![issue.clone()],
        ..Default::default()
    };

    // First cycle alone: no prev → no emit yet.
    let spy = SpySink::default();
    emit_findings_projections(
        None,
        &report_with_findings(vec![], findings_first_cycle.clone()),
        &spy,
    );
    assert!(spy
        .kg_adds()
        .iter()
        .all(|t| t.predicate != Predicate::EntityNameIssue));

    // Second cycle, prev had the same issue: emit.
    let spy2 = SpySink::default();
    emit_findings_projections(
        Some(&report_with_findings(vec![], findings_first_cycle)),
        &report_with_findings(vec![], findings_second_cycle),
        &spy2,
    );
    let adds: Vec<_> = spy2
        .kg_adds()
        .into_iter()
        .filter(|t| t.predicate == Predicate::EntityNameIssue)
        .collect();
    assert_eq!(adds.len(), 1, "{adds:?}");
    assert!(adds[0].object.contains("mixed"), "{:?}", adds[0]);
}

#[test]
fn name_issue_resolution_emits_invalidate() {
    let prev = report_with_findings(
        vec![],
        HaMcpLiveContextFindings {
            bad_names: vec![HaMcpBadNameFinding {
                name: "光1".into(),
                domain: "light".into(),
                areas: vec![],
                reasons: vec!["mixed_cjk_ascii".into()],
            }],
            ..Default::default()
        },
    );
    let next = report_with_findings(vec![], HaMcpLiveContextFindings::default());
    let spy = SpySink::default();
    emit_findings_projections(Some(&prev), &next, &spy);
    let inv: Vec<_> = spy
        .kg_invalidates()
        .into_iter()
        .filter(|t| t.predicate == Predicate::EntityNameIssue)
        .collect();
    assert_eq!(inv.len(), 1);
}

#[test]
fn findings_narrative_emits_one_drawer_per_area() {
    let findings = HaMcpLiveContextFindings {
        bad_names: vec![HaMcpBadNameFinding {
            name: "光1".into(),
            domain: "light".into(),
            areas: vec!["Living Room".into()],
            reasons: vec!["mixed_cjk_ascii".into()],
        }],
        missing_area_exposure: vec![HaMcpMissingAreaExposureFinding {
            name: "aircon".into(),
            domain: "climate".into(),
            state: "on".into(),
            reasons: vec!["no_area".into()],
        }],
        ..Default::default()
    };
    let report = report_with_findings(vec![], findings);
    let spy = SpySink::default();
    emit_findings_drawer(&report, "2026-04-25T12:00:00Z", &spy);
    let drawers = spy.drawers();
    assert_eq!(drawers.len(), 2, "{drawers:?}");
    assert!(drawers.iter().all(|d| d.wing == "davis.ha"));
    let living = drawers
        .iter()
        .find(|d| d.room.contains("Living"))
        .expect("living room drawer");
    assert!(living.content.contains("光1"), "{}", living.content);
    assert!(
        living.content.contains("mixed_cjk_ascii"),
        "{}",
        living.content
    );
}

#[test]
fn findings_drawer_is_capped_at_500_chars() {
    // Craft 50 bad names so the body trivially exceeds 500 chars.
    let findings = HaMcpLiveContextFindings {
        bad_names: (0..50)
            .map(|i| HaMcpBadNameFinding {
                name: format!("bad_name_{i}"),
                domain: "light".into(),
                areas: vec!["Area".into()],
                reasons: vec!["mixed_cjk_ascii".into()],
            })
            .collect(),
        ..Default::default()
    };
    let report = report_with_findings(vec![], findings);
    let spy = SpySink::default();
    emit_findings_drawer(&report, "2026-04-25T12:00:00Z", &spy);
    let drawers = spy.drawers();
    assert_eq!(drawers.len(), 1);
    let chars = drawers[0].content.chars().count();
    assert!(
        chars <= 500,
        "drawer body was {chars} chars: {}",
        drawers[0].content
    );
}

#[test]
fn findings_drawer_skips_empty_report() {
    let report = report_with_findings(vec![], HaMcpLiveContextFindings::default());
    let spy = SpySink::default();
    emit_findings_drawer(&report, "2026-04-25T12:00:00Z", &spy);
    assert!(spy.drawers().is_empty(), "{:?}", spy.drawers());
}

#[test]
fn findings_drawer_includes_duplicate_names_section() {
    let findings = HaMcpLiveContextFindings {
        exposed_duplicate_names: vec![HaMcpDuplicateNameFinding {
            name: "灯".into(),
            count: 3,
            domains: vec!["light".into()],
            areas: vec!["Kitchen".into()],
        }],
        ..Default::default()
    };
    let report = report_with_findings(vec![], findings);
    let spy = SpySink::default();
    emit_findings_drawer(&report, "ts", &spy);
    let drawers = spy.drawers();
    assert_eq!(drawers.len(), 1);
    assert!(
        drawers[0].content.contains("duplicates"),
        "{}",
        drawers[0].content
    );
    assert!(
        drawers[0].content.contains("灯 x3"),
        "{}",
        drawers[0].content
    );
}

#[test]
fn diary_entry_counts_are_written_to_ha_analyzer_wing() {
    let findings = HaMcpLiveContextFindings {
        bad_names: vec![HaMcpBadNameFinding {
            name: "光1".into(),
            domain: "light".into(),
            areas: vec![],
            reasons: vec!["mixed_cjk_ascii".into()],
        }],
        ..Default::default()
    };
    let mut report = report_with_findings(vec![], findings);
    report.entity_count = 42;
    report.unavailable_count = 3;
    report.unknown_count = 1;
    let spy = SpySink::default();
    emit_refresh_diary(&report, "2026-04-25T12:00:00Z", &spy);
    let entries = spy.diary_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].wing, "davis.agent.ha-analyzer");
    assert!(
        entries[0].entry.contains("entities=42"),
        "{}",
        entries[0].entry
    );
    assert!(
        entries[0].entry.contains("unavailable=3"),
        "{}",
        entries[0].entry
    );
    assert!(
        entries[0].entry.contains("bad_names=1"),
        "{}",
        entries[0].entry
    );
    assert!(entries[0].entry.contains("top="), "{}", entries[0].entry);
}

#[test]
fn diary_entry_is_capped_at_200_chars() {
    let findings = HaMcpLiveContextFindings {
        bad_names: (0..200)
            .map(|i| HaMcpBadNameFinding {
                name: format!("extremely_long_entity_name_{i}"),
                domain: "light".into(),
                areas: vec![],
                reasons: vec!["mixed_cjk_ascii".into()],
            })
            .collect(),
        ..Default::default()
    };
    let report = report_with_findings(vec![], findings);
    let spy = SpySink::default();
    emit_refresh_diary(&report, "2026-04-25T12:00:00Z", &spy);
    let entries = spy.diary_entries();
    assert_eq!(entries.len(), 1);
    let n = entries[0].entry.chars().count();
    assert!(n <= 200, "diary entry was {n} chars: {}", entries[0].entry);
}

#[test]
fn same_state_second_refresh_is_a_no_op() {
    let prev = report_with(vec![obs("light|a|", "A", "light", "unavailable")]);
    let next = report_with(vec![obs("light|a|", "A", "light", "unavailable")]);
    let spy = SpySink::default();
    emit_state_transitions(Some(&prev), &next, &spy);
    assert!(spy.kg_adds().is_empty());
    assert!(spy.kg_invalidates().is_empty());
}
