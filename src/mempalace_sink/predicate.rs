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
/// `<namespace>:<body>`. Typed constructors guarantee the namespace prefix;
/// the body is only rejected when empty or when it contains newline/NUL
/// (which would corrupt the JSON-RPC line framing).
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
        Ok(Self(format!("{namespace}:{body}")))
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
        Self::build("rule_version", &format!("{host}:v{version}"))
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
        Self::build("route_profile", profile)
    }
    pub fn route_profile(profile: &str) -> Self {
        Self::try_route_profile(profile).expect("route profile must be non-empty and single-line")
    }

    pub fn budget_scope_daily(date: NaiveDate) -> Self {
        Self::build(
            "budget_scope",
            &format!("daily:{}", date.format("%Y-%m-%d")),
        )
        .expect("formatted date is always non-empty")
    }

    pub fn budget_scope_monthly(year: i32, month: u32) -> Self {
        Self::build("budget_scope", &format!("monthly:{year:04}-{month:02}"))
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
            "entity:light.living_room_main",
        );
        assert_eq!(TripleId::area("living_room").as_str(), "area:living_room");
        assert_eq!(TripleId::host("lobste.rs").as_str(), "host:lobste.rs");
        assert_eq!(TripleId::article("a8f3c9d2").as_str(), "article:a8f3c9d2");
        assert_eq!(TripleId::topic("async-rust").as_str(), "topic:async-rust");
        assert_eq!(TripleId::rule("lobste.rs").as_str(), "rule:lobste.rs");
        assert_eq!(
            TripleId::rule_version("lobste.rs", 3).as_str(),
            "rule_version:lobste.rs:v3",
        );
        assert_eq!(
            TripleId::provider("openrouter").as_str(),
            "provider:openrouter",
        );
        assert_eq!(
            TripleId::model("claude-haiku-4-5").as_str(),
            "model:claude-haiku-4-5",
        );
        assert_eq!(
            TripleId::route_profile("fast").as_str(),
            "route_profile:fast"
        );
        assert_eq!(TripleId::worker("ingest").as_str(), "worker:ingest");
        assert_eq!(
            TripleId::component("zeroclaw-daemon").as_str(),
            "component:zeroclaw-daemon",
        );
    }

    #[test]
    fn triple_id_budget_scope_formats() {
        let d = NaiveDate::from_ymd_opt(2026, 4, 25).unwrap();
        assert_eq!(
            TripleId::budget_scope_daily(d).as_str(),
            "budget_scope:daily:2026-04-25",
        );
        assert_eq!(
            TripleId::budget_scope_monthly(2026, 4).as_str(),
            "budget_scope:monthly:2026-04",
        );
        assert_eq!(
            TripleId::budget_scope_monthly(2026, 12).as_str(),
            "budget_scope:monthly:2026-12",
        );
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
        assert_eq!(format!("{id}"), "provider:openrouter");
    }
}
