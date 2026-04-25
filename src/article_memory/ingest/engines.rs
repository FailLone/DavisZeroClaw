//! Extraction engine selection + upgrade ladder.
//!
//! Phase 1 engines: trafilatura (default), openrouter-llm (fallback).
//! `pruning` is recognised to stay compatible with callers that still request
//! it, but is deprecated and removed in Phase 2.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineChoice {
    LearnedRules,
    Trafilatura,
    OpenRouterLlm,
    Pruning, // deprecated; retained for migration window
}

impl EngineChoice {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LearnedRules => "learned-rules",
            Self::Trafilatura => "trafilatura",
            Self::OpenRouterLlm => "openrouter-llm",
            Self::Pruning => "pruning",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "learned-rules" => Some(Self::LearnedRules),
            "trafilatura" => Some(Self::Trafilatura),
            "openrouter-llm" => Some(Self::OpenRouterLlm),
            "pruning" => Some(Self::Pruning),
            _ => None,
        }
    }
}

impl fmt::Display for EngineChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct ExtractEngineConfig {
    pub default_engine: EngineChoice,
    pub fallback_ladder: Vec<EngineChoice>,
}

impl Default for ExtractEngineConfig {
    fn default() -> Self {
        Self {
            // Aspirational default: the worker will attempt learned-rules per
            // host; when no rule exists (or fails), `pick_engine` falls this
            // back to Trafilatura so the engine-ladder still starts from a
            // concrete HTML-fetching engine.
            default_engine: EngineChoice::LearnedRules,
            fallback_ladder: vec![EngineChoice::Trafilatura, EngineChoice::OpenRouterLlm],
        }
    }
}

/// Pick the starting engine. Rules:
/// 1. If `default_engine` is OpenRouterLlm or LearnedRules, we still need
///    HTML first — the worker invokes learned-rules explicitly per host, and
///    openrouter-llm needs HTML from Trafilatura before it can rewrite.
/// 2. Otherwise return `default_engine` if it appears in the ladder,
///    else the head of the ladder.
pub fn pick_engine(config: &ExtractEngineConfig) -> EngineChoice {
    if matches!(
        config.default_engine,
        EngineChoice::OpenRouterLlm | EngineChoice::LearnedRules
    ) {
        return EngineChoice::Trafilatura;
    }
    if config.fallback_ladder.contains(&config.default_engine) {
        config.default_engine.clone()
    } else {
        config
            .fallback_ladder
            .first()
            .cloned()
            .unwrap_or(EngineChoice::Trafilatura)
    }
}

/// Given the current engine and the ladder, return the next engine to try,
/// or `None` if exhausted.
#[allow(dead_code)] // worker uses its own iteration; kept for tests + future use
pub fn next_engine(current: &EngineChoice, ladder: &[EngineChoice]) -> Option<EngineChoice> {
    let pos = ladder.iter().position(|e| e == current)?;
    ladder.get(pos + 1).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_engine_returns_default() {
        // With the Phase 2 default (`LearnedRules`), `pick_engine` falls back
        // to Trafilatura because the worker handles per-host learned-rules
        // selection explicitly before consulting the ladder.
        let c = ExtractEngineConfig::default();
        assert_eq!(pick_engine(&c), EngineChoice::Trafilatura);
    }

    #[test]
    fn pick_engine_learned_rules_default_falls_back_to_trafilatura() {
        let c = ExtractEngineConfig::default();
        assert_eq!(c.default_engine, EngineChoice::LearnedRules);
        assert_eq!(pick_engine(&c), EngineChoice::Trafilatura);
    }

    #[test]
    fn pick_engine_openrouter_default_falls_back_to_trafilatura() {
        let c = ExtractEngineConfig {
            default_engine: EngineChoice::OpenRouterLlm,
            fallback_ladder: vec![EngineChoice::Trafilatura, EngineChoice::OpenRouterLlm],
        };
        assert_eq!(pick_engine(&c), EngineChoice::Trafilatura);
    }

    #[test]
    fn pick_engine_defaults_to_ladder_head_when_default_missing() {
        let c = ExtractEngineConfig {
            default_engine: EngineChoice::Pruning,
            fallback_ladder: vec![EngineChoice::Trafilatura, EngineChoice::OpenRouterLlm],
        };
        assert_eq!(pick_engine(&c), EngineChoice::Trafilatura);
    }

    #[test]
    fn next_engine_walks_ladder() {
        let ladder = vec![EngineChoice::Trafilatura, EngineChoice::OpenRouterLlm];
        assert_eq!(
            next_engine(&EngineChoice::Trafilatura, &ladder),
            Some(EngineChoice::OpenRouterLlm)
        );
        assert_eq!(next_engine(&EngineChoice::OpenRouterLlm, &ladder), None);
    }

    #[test]
    fn next_engine_missing_returns_none() {
        let ladder = vec![EngineChoice::Trafilatura];
        assert_eq!(next_engine(&EngineChoice::OpenRouterLlm, &ladder), None);
    }

    #[test]
    fn engine_choice_roundtrip() {
        for e in [
            EngineChoice::LearnedRules,
            EngineChoice::Trafilatura,
            EngineChoice::OpenRouterLlm,
            EngineChoice::Pruning,
        ] {
            assert_eq!(EngineChoice::from_str(e.as_str()), Some(e));
        }
        assert_eq!(EngineChoice::from_str("nope"), None);
    }
}
