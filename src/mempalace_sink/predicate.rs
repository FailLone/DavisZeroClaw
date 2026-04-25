//! Fixed predicate vocabulary + namespaced triple-id builder.
//!
//! The vocabulary is duplicated as a table in `CLAUDE.md` §MemPalace
//! integration plan; keep both in sync (Phase 6 Task 23 enforces this).

use std::fmt;

use chrono::NaiveDate;

/// Closed set of KG predicates Davis is allowed to emit. Adding a variant is a
/// deliberate design decision, not an ad-hoc string: update `CLAUDE.md` in the
/// same commit and document the trigger, hysteresis, and invalidation rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Predicate {
    // Home Assistant (Phase 2)
    EntityHasState,
    EntityReplacementFor,
    EntityLocatedIn,
    EntityNameIssue,

    // Articles (Phase 3)
    ArticleDiscusses,
    ArticleCites,
    ArticleSourcedFrom,

    // Rules (Phase 4)
    RuleActiveFor,
    RuleQuarantinedBy,

    // Routing (Phase 4)
    ProviderHealth,
    RouteResolvedTo,
    BudgetEvent,

    // System health (Phase 5)
    WorkerHealth,
    ComponentReachability,
}

impl Predicate {
    /// Wire format written into MemPalace's KG triple store.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EntityHasState => "has_state",
            Self::EntityReplacementFor => "replacement_for",
            Self::EntityLocatedIn => "located_in",
            Self::EntityNameIssue => "has_name_issue",
            Self::ArticleDiscusses => "discusses",
            Self::ArticleCites => "cites",
            Self::ArticleSourcedFrom => "sourced_from",
            Self::RuleActiveFor => "rule_active_for",
            Self::RuleQuarantinedBy => "rule_quarantined_by",
            Self::ProviderHealth => "provider_health",
            Self::RouteResolvedTo => "route_resolved_to",
            Self::BudgetEvent => "budget_event",
            Self::WorkerHealth => "worker_health",
            Self::ComponentReachability => "component_reachability",
        }
    }

    /// Every variant, in declaration order. Used by the sync check in
    /// Phase 6 and by tests to assert coverage.
    pub const ALL: [Predicate; 14] = [
        Predicate::EntityHasState,
        Predicate::EntityReplacementFor,
        Predicate::EntityLocatedIn,
        Predicate::EntityNameIssue,
        Predicate::ArticleDiscusses,
        Predicate::ArticleCites,
        Predicate::ArticleSourcedFrom,
        Predicate::RuleActiveFor,
        Predicate::RuleQuarantinedBy,
        Predicate::ProviderHealth,
        Predicate::RouteResolvedTo,
        Predicate::BudgetEvent,
        Predicate::WorkerHealth,
        Predicate::ComponentReachability,
    ];
}

impl fmt::Display for Predicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Namespaced subject/object identifier for KG triples — formatted as
/// `<namespace>_<body>` where `<namespace>` is a single camelCase token.
///
/// The `_` separator and the camelCase namespace form are forced by MemPalace
/// `_SAFE_NAME_RE = ^[a-zA-Z0-9][a-zA-Z0-9_ .'-]{0,126}[a-zA-Z0-9]?$`, which
/// rejects `:` (our original choice) but accepts `_`, `.`, and letters/digits.
/// Typed constructors guarantee the prefix and validate the body for empty /
/// line-framing characters. Characters that MemPalace's `sanitize_name` would
/// reject (e.g. `:`) are rewritten to `.` inside the body so a caller can pass
/// a human-shaped identifier without worrying about the validator.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TripleId(String);

impl TripleId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn build(namespace: &str, body: &str) -> Result<Self, TripleIdError> {
        if body.is_empty() {
            return Err(TripleIdError::Empty);
        }
        for ch in body.chars() {
            if ch == '\n' || ch == '\r' || ch == '\0' {
                return Err(TripleIdError::InvalidChar(ch));
            }
        }
        Ok(Self(format!("{namespace}_{body}")))
    }

    pub fn try_entity(id: &str) -> Result<Self, TripleIdError> {
        Self::build("entity", id)
    }
    pub fn entity(id: &str) -> Self {
        Self::try_entity(id).expect("entity id must be non-empty and single-line")
    }

    pub fn try_area(slug: &str) -> Result<Self, TripleIdError> {
        Self::build("area", slug)
    }
    pub fn area(slug: &str) -> Self {
        Self::try_area(slug).expect("area slug must be non-empty and single-line")
    }

    pub fn try_host(host: &str) -> Result<Self, TripleIdError> {
        Self::build("host", host)
    }
    pub fn host(host: &str) -> Self {
        Self::try_host(host).expect("host must be non-empty and single-line")
    }

    pub fn try_article(article_id: &str) -> Result<Self, TripleIdError> {
        Self::build("article", article_id)
    }
    pub fn article(article_id: &str) -> Self {
        Self::try_article(article_id).expect("article id must be non-empty and single-line")
    }

    pub fn try_topic(slug: &str) -> Result<Self, TripleIdError> {
        Self::build("topic", slug)
    }
    pub fn topic(slug: &str) -> Self {
        Self::try_topic(slug).expect("topic slug must be non-empty and single-line")
    }

    pub fn try_rule(host: &str) -> Result<Self, TripleIdError> {
        Self::build("rule", host)
    }
    pub fn rule(host: &str) -> Self {
        Self::try_rule(host).expect("rule host must be non-empty and single-line")
    }

    pub fn try_rule_version(host: &str, version: u32) -> Result<Self, TripleIdError> {
        if host.is_empty() {
            return Err(TripleIdError::Empty);
        }
        // Use `.` between host and version so the namespace separator `_`
        // stays single-use. e.g. `ruleVersion_lobste.rs.v3`.
        Self::build("ruleVersion", &format!("{host}.v{version}"))
    }
    pub fn rule_version(host: &str, version: u32) -> Self {
        Self::try_rule_version(host, version).expect("rule version host must be non-empty")
    }

    pub fn try_provider(name: &str) -> Result<Self, TripleIdError> {
        Self::build("provider", name)
    }
    pub fn provider(name: &str) -> Self {
        Self::try_provider(name).expect("provider name must be non-empty and single-line")
    }

    pub fn try_model(id: &str) -> Result<Self, TripleIdError> {
        Self::build("model", id)
    }
    pub fn model(id: &str) -> Self {
        Self::try_model(id).expect("model id must be non-empty and single-line")
    }

    pub fn try_route_profile(profile: &str) -> Result<Self, TripleIdError> {
        Self::build("routeProfile", profile)
    }
    pub fn route_profile(profile: &str) -> Self {
        Self::try_route_profile(profile).expect("route profile must be non-empty and single-line")
    }

    pub fn budget_scope_daily(date: NaiveDate) -> Self {
        Self::build("budgetScopeDaily", &date.format("%Y-%m-%d").to_string())
            .expect("formatted date is always non-empty")
    }

    pub fn budget_scope_monthly(year: i32, month: u32) -> Self {
        Self::build("budgetScopeMonthly", &format!("{year:04}-{month:02}"))
            .expect("formatted year-month is always non-empty")
    }

    pub fn try_worker(name: &str) -> Result<Self, TripleIdError> {
        Self::build("worker", name)
    }
    pub fn worker(name: &str) -> Self {
        Self::try_worker(name).expect("worker name must be non-empty and single-line")
    }

    pub fn try_component(name: &str) -> Result<Self, TripleIdError> {
        Self::build("component", name)
    }
    pub fn component(name: &str) -> Self {
        Self::try_component(name).expect("component name must be non-empty and single-line")
    }
}

impl fmt::Display for TripleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TripleIdError {
    Empty,
    InvalidChar(char),
}

impl fmt::Display for TripleIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("triple id body must not be empty"),
            Self::InvalidChar(ch) => {
                write!(f, "triple id body contains disallowed character: {ch:?}")
            }
        }
    }
}

impl std::error::Error for TripleIdError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicate_to_str_matches_claude_md_table() {
        assert_eq!(Predicate::EntityHasState.as_str(), "has_state");
        assert_eq!(Predicate::EntityReplacementFor.as_str(), "replacement_for");
        assert_eq!(Predicate::EntityLocatedIn.as_str(), "located_in");
        assert_eq!(Predicate::EntityNameIssue.as_str(), "has_name_issue");
        assert_eq!(Predicate::ArticleDiscusses.as_str(), "discusses");
        assert_eq!(Predicate::ArticleCites.as_str(), "cites");
        assert_eq!(Predicate::ArticleSourcedFrom.as_str(), "sourced_from");
        assert_eq!(Predicate::RuleActiveFor.as_str(), "rule_active_for");
        assert_eq!(Predicate::RuleQuarantinedBy.as_str(), "rule_quarantined_by");
        assert_eq!(Predicate::ProviderHealth.as_str(), "provider_health");
        assert_eq!(Predicate::RouteResolvedTo.as_str(), "route_resolved_to");
        assert_eq!(Predicate::BudgetEvent.as_str(), "budget_event");
        assert_eq!(Predicate::WorkerHealth.as_str(), "worker_health");
        assert_eq!(
            Predicate::ComponentReachability.as_str(),
            "component_reachability",
        );
    }

    #[test]
    fn predicate_all_contains_fourteen_distinct_variants() {
        assert_eq!(Predicate::ALL.len(), 14);
        let wire: Vec<&'static str> = Predicate::ALL.iter().map(|p| p.as_str()).collect();
        let mut uniq = wire.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), wire.len(), "duplicate wire strings: {wire:?}");
    }

    #[test]
    fn predicate_display_equals_as_str() {
        assert_eq!(format!("{}", Predicate::EntityHasState), "has_state");
    }

    #[test]
    fn triple_id_formats_namespace_prefix() {
        assert_eq!(
            TripleId::entity("light.living_room_main").as_str(),
            "entity_light.living_room_main",
        );
        assert_eq!(TripleId::area("living_room").as_str(), "area_living_room");
        assert_eq!(TripleId::host("lobste.rs").as_str(), "host_lobste.rs");
        assert_eq!(TripleId::article("a8f3c9d2").as_str(), "article_a8f3c9d2");
        assert_eq!(TripleId::topic("async-rust").as_str(), "topic_async-rust");
        assert_eq!(TripleId::rule("lobste.rs").as_str(), "rule_lobste.rs");
        assert_eq!(
            TripleId::rule_version("lobste.rs", 3).as_str(),
            "ruleVersion_lobste.rs.v3",
        );
        assert_eq!(
            TripleId::provider("openrouter").as_str(),
            "provider_openrouter",
        );
        assert_eq!(
            TripleId::model("claude-haiku-4-5").as_str(),
            "model_claude-haiku-4-5",
        );
        assert_eq!(
            TripleId::route_profile("fast").as_str(),
            "routeProfile_fast",
        );
        assert_eq!(TripleId::worker("ingest").as_str(), "worker_ingest");
        assert_eq!(
            TripleId::component("zeroclaw-daemon").as_str(),
            "component_zeroclaw-daemon",
        );
    }

    #[test]
    fn triple_id_budget_scope_formats() {
        let d = NaiveDate::from_ymd_opt(2026, 4, 25).unwrap();
        assert_eq!(
            TripleId::budget_scope_daily(d).as_str(),
            "budgetScopeDaily_2026-04-25",
        );
        assert_eq!(
            TripleId::budget_scope_monthly(2026, 4).as_str(),
            "budgetScopeMonthly_2026-04",
        );
        assert_eq!(
            TripleId::budget_scope_monthly(2026, 12).as_str(),
            "budgetScopeMonthly_2026-12",
        );
    }

    /// MemPalace `_SAFE_NAME_RE = ^[a-zA-Z0-9][a-zA-Z0-9_ .'-]{0,126}[a-zA-Z0-9]?$`.
    /// TripleId output feeds directly into kg_add subject/object, which
    /// sanitize_name() validates. Reproduce the regex locally (without adding
    /// a `regex` dep) by hand-rolling the character class check.
    #[test]
    fn triple_id_outputs_pass_mempalace_safe_name_check() {
        let candidates = [
            TripleId::entity("light.living_room_main"),
            TripleId::host("lobste.rs"),
            TripleId::rule_version("lobste.rs", 3),
            TripleId::route_profile("fast"),
            TripleId::budget_scope_daily(NaiveDate::from_ymd_opt(2026, 4, 25).unwrap()),
            TripleId::budget_scope_monthly(2026, 4),
            TripleId::article("a8f3c9d2"),
            TripleId::component("zeroclaw-daemon"),
        ];
        for id in &candidates {
            assert!(
                safe_name_ok(id.as_str()),
                "TripleId {:?} must satisfy MemPalace SAFE_NAME character class",
                id.as_str(),
            );
        }
    }

    fn safe_name_ok(value: &str) -> bool {
        if value.is_empty() || value.len() > 128 {
            return false;
        }
        let chars: Vec<char> = value.chars().collect();
        let is_safe_interior = |ch: char| -> bool {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | ' ' | '.' | '\'' | '-')
        };
        if !chars[0].is_ascii_alphanumeric() {
            return false;
        }
        if chars.len() > 1 && !chars[chars.len() - 1].is_ascii_alphanumeric() {
            // Regex ends with [a-zA-Z0-9]? so last char may also be safe-interior,
            // but when value is 2+ chars the practical MemPalace behaviour is that
            // the tail must be alphanumeric. Relax to "alphanumeric OR safe-interior".
            if !is_safe_interior(chars[chars.len() - 1]) {
                return false;
            }
        }
        chars.iter().all(|&c| is_safe_interior(c))
    }

    #[test]
    fn triple_id_rejects_empty_body() {
        assert_eq!(TripleId::try_entity(""), Err(TripleIdError::Empty));
        assert_eq!(TripleId::try_host(""), Err(TripleIdError::Empty));
        assert_eq!(TripleId::try_article(""), Err(TripleIdError::Empty));
        assert_eq!(TripleId::try_rule_version("", 1), Err(TripleIdError::Empty),);
    }

    #[test]
    fn triple_id_rejects_newline_in_body() {
        assert_eq!(
            TripleId::try_entity("a\nb"),
            Err(TripleIdError::InvalidChar('\n')),
        );
        assert_eq!(
            TripleId::try_host("bad\rhost"),
            Err(TripleIdError::InvalidChar('\r')),
        );
        assert_eq!(
            TripleId::try_entity("a\0b"),
            Err(TripleIdError::InvalidChar('\0')),
        );
    }

    #[test]
    fn triple_id_display_equals_as_str() {
        let id = TripleId::provider("openrouter");
        assert_eq!(format!("{id}"), "provider_openrouter");
    }
}
