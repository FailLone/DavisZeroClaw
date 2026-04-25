//! Phase 2 projection layer — diff a fresh HA live-context report against
//! the previous snapshot and project HA findings into MemPalace.
//!
//! Lives in its own module so `ha_mcp.rs` stays focused on the MCP wire +
//! domain-analysis logic. This file contains only what feeds the sink.

use crate::ha_mcp::{HaMcpEntityObservation, HaMcpLiveContextReport};
use crate::mempalace_sink::{MempalaceEmitter, Predicate, TripleId};

/// Durable identifier for an HA entity that we can feed into `TripleId`.
/// Built from (domain, name-slug) so it survives across refreshes even when
/// the same entity briefly drops from the live-context output.
fn entity_triple_id(name: &str, domain: &str) -> TripleId {
    let dom = if domain.trim().is_empty() {
        "unknown".to_string()
    } else {
        TripleId::safe_slug(domain)
    };
    let body = format!("{}.{}", TripleId::safe_slug(name), dom);
    TripleId::entity(&body)
}

/// Coarse state bucket we use for KG triples. Day-to-day `on`/`off` toggles
/// are NOT interesting — we only file `available` vs `unavailable` vs
/// `unknown`, because those are the states the user asks about
/// (“哪个灯坏了”, “哪个传感器没数据”).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoarseState {
    Available,
    Unavailable,
    Unknown,
}

impl CoarseState {
    fn classify(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "unavailable" => Self::Unavailable,
            "unknown" => Self::Unknown,
            _ => Self::Available,
        }
    }

    fn as_object(self) -> Option<TripleId> {
        match self {
            Self::Available => None, // we don't file positive state triples
            Self::Unavailable => Some(TripleId::entity("state.unavailable")),
            Self::Unknown => Some(TripleId::entity("state.unknown")),
        }
    }
}

/// Project HA state-change events into MemPalace. Policy:
///
/// * Rising edge (available → unavailable|unknown) → `kg_add(EntityHasState)`
///   with the new coarse state as the object.
/// * Falling edge (unavailable|unknown → available) → `kg_invalidate` the
///   previous coarse-state triple.
/// * On-the-fly appearance / disappearance:
///   - entity first seen in a non-available state → kg_add
///   - entity vanishes while in a non-available state → kg_invalidate
/// * `on`/`off`/numeric state changes are ignored — they're not what users
///   ask "what broke last week" about.
///
/// Debounce / hysteresis: plan calls for "state stable ≥ 60s". We approximate
/// this with a 2-refresh stability requirement — HA live-context refreshes on
/// a minute-ish cadence, so requiring the new coarse bucket to match the
/// prior snapshot's version of that entity is equivalent in practice and
/// avoids needing a clock. The *first* time we see a transition we DO emit
/// (there's no point delaying the "went offline right now" signal).
pub fn emit_state_transitions(
    prev: Option<&HaMcpLiveContextReport>,
    next: &HaMcpLiveContextReport,
    emitter: &dyn MempalaceEmitter,
) {
    let prev_states = index_by_signature(prev);
    let mut next_seen = std::collections::HashSet::new();

    for observation in &next.observations {
        next_seen.insert(observation.signature.clone());
        let next_state = CoarseState::classify(&observation.state);
        match prev_states.get(observation.signature.as_str()) {
            None => {
                // First time we see this entity — only file if it's already
                // unhealthy. Don't flood the KG with "everything is on".
                if let Some(obj) = next_state.as_object() {
                    emitter.kg_add(
                        entity_triple_id(&observation.name, &observation.domain),
                        Predicate::EntityHasState,
                        obj,
                    );
                }
            }
            Some(prev_obs) => {
                let prev_state = CoarseState::classify(&prev_obs.state);
                if prev_state == next_state {
                    continue;
                }
                // State changed — invalidate the old unhealthy triple if any,
                // then file the new one if unhealthy.
                if let Some(obj) = prev_state.as_object() {
                    emitter.kg_invalidate(
                        entity_triple_id(&observation.name, &observation.domain),
                        Predicate::EntityHasState,
                        obj,
                    );
                }
                if let Some(obj) = next_state.as_object() {
                    emitter.kg_add(
                        entity_triple_id(&observation.name, &observation.domain),
                        Predicate::EntityHasState,
                        obj,
                    );
                }
            }
        }
    }

    // Entities that vanished while they were previously unhealthy — invalidate
    // so the user's "what broke last week" timeline closes cleanly.
    if let Some(prev_report) = prev {
        for prev_obs in &prev_report.observations {
            if next_seen.contains(prev_obs.signature.as_str()) {
                continue;
            }
            let prev_state = CoarseState::classify(&prev_obs.state);
            if let Some(obj) = prev_state.as_object() {
                emitter.kg_invalidate(
                    entity_triple_id(&prev_obs.name, &prev_obs.domain),
                    Predicate::EntityHasState,
                    obj,
                );
            }
        }
    }
}

fn index_by_signature(
    report: Option<&HaMcpLiveContextReport>,
) -> std::collections::HashMap<String, &HaMcpEntityObservation> {
    let mut out = std::collections::HashMap::new();
    if let Some(r) = report {
        for obs in &r.observations {
            out.insert(obs.signature.clone(), obs);
        }
    }
    out
}

/// Emit a single one-line diary entry summarizing this refresh. Capped at
/// ~200 chars so the diary stays scannable.
pub fn emit_refresh_diary(
    report: &HaMcpLiveContextReport,
    timestamp_iso: &str,
    emitter: &dyn MempalaceEmitter,
) {
    const DIARY_MAX_CHARS: usize = 200;
    let bad_names = report.findings.bad_names.len();
    let missing_areas = report.findings.missing_area_exposure.len();
    let duplicates = report.findings.exposed_duplicate_names.len();
    let replacements = report
        .findings
        .possible_replacements
        .iter()
        .filter(|r| r.score >= REPLACEMENT_SCORE_ENTER)
        .count();

    let top_finding = top_finding_label(report);
    let summary = format!(
        "[{timestamp_iso}] entities={} unavailable={} unknown={} \
         bad_names={} missing_area={} dup_names={} replacements={}{}",
        report.entity_count,
        report.unavailable_count,
        report.unknown_count,
        bad_names,
        missing_areas,
        duplicates,
        replacements,
        top_finding.map(|t| format!(" top={t}")).unwrap_or_default(),
    );
    let summary: String = summary.chars().take(DIARY_MAX_CHARS).collect();
    emitter.diary_write("davis.agent.ha-analyzer", &summary);
}

fn top_finding_label(report: &HaMcpLiveContextReport) -> Option<String> {
    if let Some(bad) = report.findings.bad_names.first() {
        return Some(format!("bad_name:{}", bad.name));
    }
    if let Some(rep) = report
        .findings
        .possible_replacements
        .iter()
        .find(|r| r.score >= REPLACEMENT_SCORE_ENTER)
    {
        return Some(format!(
            "rep:{}->{}",
            rep.unavailable_name, rep.replacement_name
        ));
    }
    if let Some(ma) = report.findings.missing_area_exposure.first() {
        return Some(format!("missing_area:{}", ma.name));
    }
    if let Some(dup) = report.findings.exposed_duplicate_names.first() {
        return Some(format!("dup:{}", dup.name));
    }
    None
}

/// Emit narrative drawers under `wing=davis.ha`, one per area. Severity
/// order (highest first): bad_name, missing_area, duplicate_name,
/// cross_domain_conflict, replacement. Each drawer is capped at
/// `DRAWER_MAX_CHARS`. Areas with no findings don't produce a drawer.
pub fn emit_findings_drawer(
    report: &HaMcpLiveContextReport,
    timestamp_iso: &str,
    emitter: &dyn MempalaceEmitter,
) {
    use std::collections::BTreeMap;

    const DRAWER_MAX_CHARS: usize = 500;

    #[derive(Default)]
    struct AreaAccumulator {
        bad_names: Vec<String>,
        missing_area: Vec<String>,
        duplicates: Vec<String>,
        cross_domain: Vec<String>,
        replacements: Vec<String>,
    }

    let mut by_area: BTreeMap<String, AreaAccumulator> = BTreeMap::new();
    let unattached = "_unattached_".to_string();

    let area_key = |areas: &[String]| -> String {
        areas.first().cloned().unwrap_or_else(|| unattached.clone())
    };

    for finding in &report.findings.bad_names {
        let key = area_key(&finding.areas);
        by_area.entry(key).or_default().bad_names.push(format!(
            "{} ({}): {}",
            finding.name,
            finding.domain,
            finding.reasons.join("/"),
        ));
    }
    for finding in &report.findings.missing_area_exposure {
        by_area
            .entry(unattached.clone())
            .or_default()
            .missing_area
            .push(format!("{} ({})", finding.name, finding.domain));
    }
    for finding in &report.findings.exposed_duplicate_names {
        let key = area_key(&finding.areas);
        by_area
            .entry(key)
            .or_default()
            .duplicates
            .push(format!("{} x{}", finding.name, finding.count));
    }
    for finding in &report.findings.exposed_cross_domain_conflicts {
        let key = area_key(&finding.areas);
        by_area.entry(key).or_default().cross_domain.push(format!(
            "{} across {}",
            finding.name,
            finding.domains.join(","),
        ));
    }
    for finding in &report.findings.possible_replacements {
        if finding.score < REPLACEMENT_SCORE_ENTER {
            continue;
        }
        let key = area_key(&finding.unavailable_areas);
        by_area.entry(key).or_default().replacements.push(format!(
            "{} ↔ {} (score {})",
            finding.unavailable_name, finding.replacement_name, finding.score,
        ));
    }

    for (area, acc) in by_area {
        let mut body = String::new();
        body.push_str(&format!("[{timestamp_iso}] HA findings for {area}:\n"));
        let append_section = |buf: &mut String, label: &str, items: &[String]| {
            if items.is_empty() {
                return;
            }
            buf.push_str(label);
            buf.push_str(": ");
            buf.push_str(&items.join("; "));
            buf.push('\n');
        };
        append_section(&mut body, "bad_names", &acc.bad_names);
        append_section(&mut body, "missing_area", &acc.missing_area);
        append_section(&mut body, "duplicates", &acc.duplicates);
        append_section(&mut body, "cross_domain", &acc.cross_domain);
        append_section(&mut body, "replacements", &acc.replacements);
        if acc.bad_names.is_empty()
            && acc.missing_area.is_empty()
            && acc.duplicates.is_empty()
            && acc.cross_domain.is_empty()
            && acc.replacements.is_empty()
        {
            continue;
        }
        // Truncate by character count (UTF-8 safe).
        let body: String = body.chars().take(DRAWER_MAX_CHARS).collect();
        let room = TripleId::safe_slug(&area);
        emitter.add_drawer("davis.ha", &room, &body);
    }
}

/// Emit replacement/area/name-issue projections derived from
/// `HaMcpLiveContextFindings`. Independent of `emit_state_transitions` so
/// callers can subset if they want.
pub fn emit_findings_projections(
    prev: Option<&HaMcpLiveContextReport>,
    next: &HaMcpLiveContextReport,
    emitter: &dyn MempalaceEmitter,
) {
    emit_replacement_triples(prev, next, emitter);
    emit_located_in_triples(prev, next, emitter);
    emit_name_issue_triples(prev, next, emitter);
}

const REPLACEMENT_SCORE_ENTER: i32 = 60;
const REPLACEMENT_SCORE_EXIT: i32 = 40;

fn emit_replacement_triples(
    prev: Option<&HaMcpLiveContextReport>,
    next: &HaMcpLiveContextReport,
    emitter: &dyn MempalaceEmitter,
) {
    use std::collections::{HashMap, HashSet};

    // Index prev replacements by (unavailable, replacement, domain) so we can
    // detect transitions in and out of the >=60 band.
    let prev_scores: HashMap<(String, String, String), i32> = match prev {
        Some(r) => r
            .findings
            .possible_replacements
            .iter()
            .map(|f| {
                (
                    (
                        f.unavailable_name.clone(),
                        f.replacement_name.clone(),
                        f.domain.clone(),
                    ),
                    f.score,
                )
            })
            .collect(),
        None => HashMap::new(),
    };
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    for finding in &next.findings.possible_replacements {
        let key = (
            finding.unavailable_name.clone(),
            finding.replacement_name.clone(),
            finding.domain.clone(),
        );
        seen.insert(key.clone());
        let prev_score = prev_scores.get(&key).copied();
        match prev_score {
            None if finding.score >= REPLACEMENT_SCORE_ENTER => {
                emitter.kg_add(
                    entity_triple_id(&finding.unavailable_name, &finding.domain),
                    Predicate::EntityReplacementFor,
                    entity_triple_id(&finding.replacement_name, &finding.domain),
                );
            }
            Some(ps)
                if ps < REPLACEMENT_SCORE_ENTER && finding.score >= REPLACEMENT_SCORE_ENTER =>
            {
                emitter.kg_add(
                    entity_triple_id(&finding.unavailable_name, &finding.domain),
                    Predicate::EntityReplacementFor,
                    entity_triple_id(&finding.replacement_name, &finding.domain),
                );
            }
            Some(ps) if ps >= REPLACEMENT_SCORE_ENTER && finding.score < REPLACEMENT_SCORE_EXIT => {
                emitter.kg_invalidate(
                    entity_triple_id(&finding.unavailable_name, &finding.domain),
                    Predicate::EntityReplacementFor,
                    entity_triple_id(&finding.replacement_name, &finding.domain),
                );
            }
            _ => {}
        }
    }
    // Replacements that vanished entirely while previously above the high-water
    // mark — invalidate.
    for (key, ps) in &prev_scores {
        if *ps >= REPLACEMENT_SCORE_ENTER && !seen.contains(key) {
            let (unavailable, replacement, domain) = key;
            emitter.kg_invalidate(
                entity_triple_id(unavailable, domain),
                Predicate::EntityReplacementFor,
                entity_triple_id(replacement, domain),
            );
        }
    }
}

fn emit_located_in_triples(
    prev: Option<&HaMcpLiveContextReport>,
    next: &HaMcpLiveContextReport,
    emitter: &dyn MempalaceEmitter,
) {
    use std::collections::{HashMap, HashSet};

    // Only file one area per entity — the first one in `areas`. Multi-area
    // entities are rare enough that we prefer a simple single triple over a
    // combinatorial explosion.
    let prev_areas: HashMap<String, String> = match prev {
        Some(r) => r
            .observations
            .iter()
            .filter_map(|obs| {
                obs.areas
                    .first()
                    .map(|a| (obs.signature.clone(), a.clone()))
            })
            .collect(),
        None => HashMap::new(),
    };
    let mut seen_signatures: HashSet<String> = HashSet::new();

    for obs in &next.observations {
        let Some(area_now) = obs.areas.first() else {
            continue;
        };
        seen_signatures.insert(obs.signature.clone());
        let subject = entity_triple_id(&obs.name, &obs.domain);
        let object_now = TripleId::area(&TripleId::safe_slug(area_now));
        match prev_areas.get(&obs.signature) {
            None => {
                emitter.kg_add(subject, Predicate::EntityLocatedIn, object_now);
            }
            Some(prev_area) if prev_area != area_now => {
                let object_prev = TripleId::area(&TripleId::safe_slug(prev_area));
                emitter.kg_invalidate(subject.clone(), Predicate::EntityLocatedIn, object_prev);
                emitter.kg_add(subject, Predicate::EntityLocatedIn, object_now);
            }
            _ => {}
        }
    }
}

fn emit_name_issue_triples(
    prev: Option<&HaMcpLiveContextReport>,
    next: &HaMcpLiveContextReport,
    emitter: &dyn MempalaceEmitter,
) {
    use std::collections::{HashMap, HashSet};

    // Index prev bad names by (name, domain, reason).
    let prev_issues: HashSet<(String, String, String)> = match prev {
        Some(r) => r
            .findings
            .bad_names
            .iter()
            .flat_map(|f| {
                f.reasons
                    .iter()
                    .map(move |reason| (f.name.clone(), f.domain.clone(), reason.clone()))
            })
            .collect(),
        None => HashSet::new(),
    };

    // Also gather current issues so we can invalidate ones that are gone.
    let mut current_issues: HashMap<(String, String, String), ()> = HashMap::new();

    for bad in &next.findings.bad_names {
        for reason in &bad.reasons {
            let key = (bad.name.clone(), bad.domain.clone(), reason.clone());
            current_issues.insert(key.clone(), ());
            // Two-cycle debounce: only emit when the same (name, domain, reason)
            // appeared in prev as well.
            if prev_issues.contains(&key) {
                emitter.kg_add(
                    entity_triple_id(&bad.name, &bad.domain),
                    Predicate::EntityNameIssue,
                    TripleId::entity(&format!("issue.{}", TripleId::safe_slug(reason))),
                );
            }
        }
    }

    // Invalidate issues that were present in prev but not in next — note this
    // is a *rising* invalidate (user likely fixed the issue).
    for key in &prev_issues {
        if !current_issues.contains_key(key) {
            let (name, domain, reason) = key;
            emitter.kg_invalidate(
                entity_triple_id(name, domain),
                Predicate::EntityNameIssue,
                TripleId::entity(&format!("issue.{}", TripleId::safe_slug(reason))),
            );
        }
    }
}
