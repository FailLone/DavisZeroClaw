//! Multi-dimensional deterministic value scoring.
//!
//! Phase 2 replacement for `pipeline::deterministic_value_report`'s hardcoded
//! 0.55 baseline. Reads content signals already computed by `ContentSignals`
//! + topic matches, returns a score in [0.0, 1.0].

use super::content_signals::ContentSignals;

/// Score components: base 0.5 + topic bonus (max +0.15) + code density bonus
/// + heading depth bonus + paragraph length bonus - link density penalty -
///   list ratio penalty.
pub fn deterministic_score(signals: &ContentSignals, matched_topics_count: usize) -> f32 {
    let mut s = 0.5;
    s += (matched_topics_count as f32 * 0.05).min(0.15);

    if (0.05..0.30).contains(&signals.code_density) {
        s += 0.08;
    }

    if signals.heading_depth >= 2 {
        s += 0.05;
    }
    if signals.heading_depth >= 4 {
        s += 0.03;
    }

    if signals.link_density > 0.3 {
        s -= 0.10;
    }

    if (300..1500).contains(&signals.avg_paragraph_chars) {
        s += 0.05;
    }

    if signals.list_ratio > 0.5 {
        s -= 0.05;
    }

    s.clamp(0.0, 1.0)
}

/// Gopher-rules hard reject — if any trigger fires, decision is immediately
/// reject with the returned reason, skipping LLM judge entirely.
pub fn gopher_reject(signals: &ContentSignals) -> Option<&'static str> {
    if signals.link_density > 0.6 {
        return Some("link_density_too_high");
    }
    if signals.alpha_ratio < 0.3 {
        return Some("alphabetic_ratio_too_low");
    }
    if signals.symbol_ratio > 0.1 {
        return Some("symbol_ratio_too_high");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zeros() -> ContentSignals {
        ContentSignals {
            total_chars: 0,
            code_density: 0.0,
            link_density: 0.0,
            heading_depth: 0,
            heading_count: 0,
            paragraph_count: 0,
            list_ratio: 0.0,
            avg_paragraph_chars: 0,
            alpha_ratio: 1.0,
            symbol_ratio: 0.0,
        }
    }

    #[test]
    fn no_signals_scores_baseline() {
        let s = deterministic_score(&zeros(), 0);
        assert!((s - 0.5).abs() < 1e-6);
    }

    #[test]
    fn topic_match_adds_up_to_0_15() {
        assert!((deterministic_score(&zeros(), 1) - 0.55).abs() < 1e-6);
        assert!((deterministic_score(&zeros(), 3) - 0.65).abs() < 1e-6);
        // Cap at 0.15 bonus
        assert!((deterministic_score(&zeros(), 10) - 0.65).abs() < 1e-6);
    }

    #[test]
    fn code_density_in_technical_band_rewards() {
        let mut sig = zeros();
        sig.code_density = 0.15;
        let s = deterministic_score(&sig, 0);
        assert!((s - 0.58).abs() < 1e-6);
    }

    #[test]
    fn code_density_outside_band_no_bonus() {
        let mut sig = zeros();
        sig.code_density = 0.50; // too code-heavy
        let s = deterministic_score(&sig, 0);
        assert!((s - 0.5).abs() < 1e-6);
    }

    #[test]
    fn deep_heading_hierarchy_adds_bonuses() {
        let mut sig = zeros();
        sig.heading_depth = 4;
        let s = deterministic_score(&sig, 0);
        // +0.05 for >=2 AND +0.03 for >=4 = 0.58
        assert!((s - 0.58).abs() < 1e-6);
    }

    #[test]
    fn link_heavy_penalized() {
        let mut sig = zeros();
        sig.link_density = 0.4;
        let s = deterministic_score(&sig, 0);
        assert!((s - 0.4).abs() < 1e-6);
    }

    #[test]
    fn gopher_reject_link_soup() {
        let mut sig = zeros();
        sig.link_density = 0.7;
        assert_eq!(gopher_reject(&sig), Some("link_density_too_high"));
    }

    #[test]
    fn gopher_reject_symbol_heavy() {
        let mut sig = zeros();
        sig.symbol_ratio = 0.2;
        assert_eq!(gopher_reject(&sig), Some("symbol_ratio_too_high"));
    }

    #[test]
    fn gopher_reject_alpha_poor() {
        let mut sig = zeros();
        sig.alpha_ratio = 0.1;
        assert_eq!(gopher_reject(&sig), Some("alphabetic_ratio_too_low"));
    }

    #[test]
    fn gopher_no_trigger_on_healthy_signals() {
        let mut sig = zeros();
        sig.alpha_ratio = 0.8;
        sig.link_density = 0.1;
        sig.symbol_ratio = 0.02;
        assert_eq!(gopher_reject(&sig), None);
    }

    #[test]
    fn score_clamps_to_0_1() {
        let mut sig = zeros();
        sig.link_density = 0.5; // -0.10
        let s = deterministic_score(&sig, 0);
        assert!((0.0..=1.0).contains(&s));
    }
}
