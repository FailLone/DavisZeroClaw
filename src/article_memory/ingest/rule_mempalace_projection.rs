//! Phase 4 projection helpers — surface rule-learning events into MemPalace.

use super::rule_types::LearnedRule;
use crate::mempalace_sink::{MempalaceEmitter, Predicate, TripleId};

const DIARY_MAX_CHARS: usize = 300;

/// Project a rule promotion. When a new learned rule lands for a host we
/// kg_add the new `RuleActiveFor(host → rule_version)` triple. If the host
/// already had an active rule in the prev snapshot, invalidate that mapping
/// first so the timeline stays clean.
pub fn emit_rule_promoted(
    host: &str,
    new_rule: &LearnedRule,
    previous: Option<&LearnedRule>,
    emitter: &dyn MempalaceEmitter,
) {
    let host_id = TripleId::host(&TripleId::safe_slug(host));
    if let Some(prev) = previous {
        // Only invalidate if the version actually changed — otherwise a
        // re-upsert of the same rule would look like a flap.
        if prev.version != new_rule.version {
            let prev_version_id = TripleId::rule_version_str(host, &prev.version);
            emitter.kg_invalidate(host_id.clone(), Predicate::RuleActiveFor, prev_version_id);
        }
    }
    let new_version_id = TripleId::rule_version_str(host, &new_rule.version);
    emitter.kg_add(host_id, Predicate::RuleActiveFor, new_version_id);
}

/// Project a rule quarantine. Emits `RuleQuarantinedBy(rule_version →
/// reason_tag)`. The reason is slugged so free-form validator messages still
/// produce MemPalace-safe objects.
pub fn emit_rule_quarantined(
    host: &str,
    rule: &LearnedRule,
    reason: &str,
    emitter: &dyn MempalaceEmitter,
) {
    let version_id = TripleId::rule_version_str(host, &rule.version);
    let reason_slug = {
        let trimmed = reason.trim();
        if trimmed.is_empty() {
            "unspecified".to_string()
        } else {
            TripleId::safe_slug(trimmed)
        }
    };
    let reason_id = TripleId::entity(&format!("reason.{reason_slug}"));
    emitter.kg_add(version_id, Predicate::RuleQuarantinedBy, reason_id);
}

/// Per-scan diary for the rule-learning worker. Captures one-line outcomes
/// so agents can answer "这条规则什么时候上线的 / 为什么被隔离".
pub fn emit_rule_learner_diary(entry: &RuleLearnerDiaryEntry, emitter: &dyn MempalaceEmitter) {
    let mut s = format!(
        "[{ts}] host={host} outcome={outcome}",
        ts = entry.timestamp_iso,
        host = entry.host,
        outcome = entry.outcome,
    );
    if let Some(version) = &entry.version {
        s.push_str(&format!(" version={version}"));
    }
    if let Some(samples) = entry.sample_count {
        s.push_str(&format!(" samples={samples}"));
    }
    if let Some(reason) = &entry.reason {
        s.push_str(&format!(" reason={reason}"));
    }
    let s: String = s.chars().take(DIARY_MAX_CHARS).collect();
    emitter.diary_write("davis.agent.rule-learner", &s);
}

#[derive(Debug, Clone)]
pub struct RuleLearnerDiaryEntry {
    pub timestamp_iso: String,
    pub host: String,
    pub outcome: RuleLearnerOutcome,
    pub version: Option<String>,
    pub sample_count: Option<usize>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum RuleLearnerOutcome {
    Saved,
    Quarantined,
}

impl std::fmt::Display for RuleLearnerOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Saved => "saved",
            Self::Quarantined => "quarantined",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mempalace_sink::SpySink;

    fn fixture_rule(host: &str, version: &str) -> LearnedRule {
        LearnedRule {
            host: host.to_string(),
            version: version.to_string(),
            content_selectors: vec!["article".into()],
            remove_selectors: vec![],
            title_selector: None,
            start_markers: vec![],
            end_markers: vec![],
            confidence: 0.85,
            reasoning: "sample reasoning".into(),
            learned_from_sample_count: 10,
            stale: false,
        }
    }

    #[test]
    fn first_rule_promotion_emits_active_for_triple() {
        let spy = SpySink::default();
        let rule = fixture_rule("lobste.rs", "2026-04-25T12:00:00Z");
        emit_rule_promoted("lobste.rs", &rule, None, &spy);
        let adds = spy.kg_adds();
        assert_eq!(adds.len(), 1, "{adds:?}");
        assert_eq!(adds[0].predicate, Predicate::RuleActiveFor);
        assert_eq!(adds[0].subject, "host_lobste.rs");
        assert!(adds[0].object.starts_with("ruleVersion_lobste.rs."));
        assert!(spy.kg_invalidates().is_empty());
    }

    #[test]
    fn rule_version_change_invalidates_previous_then_adds_new() {
        let spy = SpySink::default();
        let prev = fixture_rule("lobste.rs", "2026-04-20T12:00:00Z");
        let next = fixture_rule("lobste.rs", "2026-04-25T12:00:00Z");
        emit_rule_promoted("lobste.rs", &next, Some(&prev), &spy);
        assert_eq!(spy.kg_invalidates().len(), 1);
        assert_eq!(spy.kg_adds().len(), 1);
        let inv_obj = &spy.kg_invalidates()[0].object;
        let add_obj = &spy.kg_adds()[0].object;
        assert_ne!(
            inv_obj, add_obj,
            "different versions must produce different ids"
        );
    }

    #[test]
    fn idempotent_upsert_of_same_version_does_not_emit_flap() {
        let spy = SpySink::default();
        let same = fixture_rule("lobste.rs", "2026-04-25T12:00:00Z");
        emit_rule_promoted("lobste.rs", &same, Some(&same), &spy);
        // We still add the active-for triple so newly-booting sink observers
        // see the current state, but we don't invalidate the unchanged prev.
        assert_eq!(spy.kg_adds().len(), 1);
        assert!(
            spy.kg_invalidates().is_empty(),
            "{:?}",
            spy.kg_invalidates()
        );
    }

    #[test]
    fn quarantine_emits_kg_add_with_slugged_reason() {
        let spy = SpySink::default();
        let rule = fixture_rule("lobste.rs", "2026-04-25T12:00:00Z");
        emit_rule_quarantined("lobste.rs", &rule, "extraction_quality=poor", &spy);
        let adds = spy.kg_adds();
        assert_eq!(adds.len(), 1);
        assert_eq!(adds[0].predicate, Predicate::RuleQuarantinedBy);
        assert!(adds[0].subject.starts_with("ruleVersion_lobste.rs."));
        assert_eq!(adds[0].object, "entity_reason.extraction_quality-poor");
    }

    #[test]
    fn quarantine_with_blank_reason_uses_unspecified() {
        let spy = SpySink::default();
        let rule = fixture_rule("lobste.rs", "2026-04-25T12:00:00Z");
        emit_rule_quarantined("lobste.rs", &rule, "   ", &spy);
        assert_eq!(spy.kg_adds()[0].object, "entity_reason.unspecified");
    }

    #[test]
    fn rule_learner_diary_captures_saved_outcome() {
        let spy = SpySink::default();
        let entry = RuleLearnerDiaryEntry {
            timestamp_iso: "2026-04-25T12:00:00Z".into(),
            host: "lobste.rs".into(),
            outcome: RuleLearnerOutcome::Saved,
            version: Some("2026-04-25T12:00:00Z".into()),
            sample_count: Some(8),
            reason: None,
        };
        emit_rule_learner_diary(&entry, &spy);
        let entries = spy.diary_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].wing, "davis.agent.rule-learner");
        let e = &entries[0].entry;
        assert!(e.contains("host=lobste.rs"));
        assert!(e.contains("outcome=saved"));
        assert!(e.contains("samples=8"));
    }

    #[test]
    fn rule_learner_diary_captures_quarantined_reason() {
        let spy = SpySink::default();
        let entry = RuleLearnerDiaryEntry {
            timestamp_iso: "2026-04-25T12:00:00Z".into(),
            host: "lobste.rs".into(),
            outcome: RuleLearnerOutcome::Quarantined,
            version: None,
            sample_count: Some(5),
            reason: Some("validation_failed".into()),
        };
        emit_rule_learner_diary(&entry, &spy);
        let e = &spy.diary_entries()[0].entry;
        assert!(e.contains("outcome=quarantined"));
        assert!(e.contains("reason=validation_failed"));
    }
}
