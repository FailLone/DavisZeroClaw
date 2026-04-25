//! Deterministic post-extraction quality gate.
//!
//! Hard-fails: statistical signs of catastrophic extraction (empty,
//! low kept ratio, almost no paragraphs, link-soup). Triggers engine
//! upgrade AND captures HTML for future rule-learning (Phase 2).
//!
//! Soft-fails: structural signs (code flattened, no headings when HTML
//! had them). Triggers engine upgrade only.

// Consumers land in T12 (worker engine-ladder loop); until then the
// public surface is exercised only by unit tests here.
#![allow(dead_code)]

use super::content_signals::{compute_signals, ContentSignals};

#[derive(Debug, Clone, PartialEq)]
pub struct QualityGateConfig {
    pub enabled: bool,
    pub min_markdown_chars: usize,
    pub min_kept_ratio: f32,
    pub min_paragraphs: usize,
    pub max_link_density: f32,
    pub boilerplate_markers: Vec<String>,
}

impl Default for QualityGateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_markdown_chars: 500,
            min_kept_ratio: 0.05,
            min_paragraphs: 3,
            max_link_density: 0.5,
            boilerplate_markers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GateResult {
    pub pass: bool,
    pub hard_fail_reasons: Vec<&'static str>,
    pub soft_fail_reasons: Vec<&'static str>,
    pub signals: ContentSignals,
    pub kept_ratio: f32,
}

pub fn assess(markdown: &str, html_chars: usize, config: &QualityGateConfig) -> GateResult {
    let signals = compute_signals(markdown);
    let kept_ratio = if html_chars == 0 {
        0.0
    } else {
        signals.total_chars as f32 / html_chars as f32
    };

    if !config.enabled {
        return GateResult {
            pass: true,
            hard_fail_reasons: Vec::new(),
            soft_fail_reasons: Vec::new(),
            signals,
            kept_ratio,
        };
    }

    let mut hard: Vec<&'static str> = Vec::new();
    let mut soft: Vec<&'static str> = Vec::new();

    if signals.total_chars < config.min_markdown_chars {
        hard.push("markdown_too_short");
    }
    if kept_ratio > 0.0 && kept_ratio < config.min_kept_ratio {
        hard.push("kept_ratio_too_low");
    }
    if signals.paragraph_count < config.min_paragraphs {
        hard.push("too_few_paragraphs");
    }
    if signals.link_density > config.max_link_density {
        hard.push("link_density_too_high");
    }

    // Soft: boilerplate markers present
    let lower = markdown.to_lowercase();
    if config
        .boilerplate_markers
        .iter()
        .any(|needle| lower.contains(&needle.to_lowercase()))
    {
        soft.push("boilerplate_marker_present");
    }

    GateResult {
        pass: hard.is_empty() && soft.is_empty(),
        hard_fail_reasons: hard,
        soft_fail_reasons: soft,
        signals,
        kept_ratio,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> QualityGateConfig {
        QualityGateConfig {
            enabled: true,
            min_markdown_chars: 100,
            min_kept_ratio: 0.05,
            min_paragraphs: 2,
            max_link_density: 0.5,
            boilerplate_markers: vec!["订阅".to_string(), "cookie policy".to_string()],
        }
    }

    #[test]
    fn good_markdown_passes() {
        // html_chars=2000 and markdown~160 → kept_ratio ≈ 0.08, above 0.05 floor.
        let md = "# Title\n\nFirst paragraph with enough length to matter.\n\nSecond paragraph here with more text to exceed the minimum char budget in this small config.";
        let r = assess(md, 2000, &cfg());
        assert!(
            r.pass,
            "reasons: hard={:?} soft={:?} kept_ratio={}",
            r.hard_fail_reasons, r.soft_fail_reasons, r.kept_ratio
        );
    }

    #[test]
    fn too_short_hard_fails() {
        let r = assess("tiny", 100, &cfg());
        assert!(!r.pass);
        assert!(r.hard_fail_reasons.contains(&"markdown_too_short"));
    }

    #[test]
    fn low_kept_ratio_hard_fails() {
        // 200 chars out of 100_000 HTML → ratio 0.002
        let md = "x".repeat(200);
        let mut config = cfg();
        config.min_markdown_chars = 10;
        config.min_paragraphs = 0;
        let r = assess(&md, 100_000, &config);
        assert!(r.hard_fail_reasons.contains(&"kept_ratio_too_low"));
    }

    #[test]
    fn link_soup_hard_fails() {
        let links: String = (0..20)
            .map(|i| format!("[l{i}](http://x.example/{i})"))
            .collect::<Vec<_>>()
            .join(" ");
        let md_body = format!("Para one.\n\n{links}\n\nPara two.\n\nPara three with more content here so min paragraphs holds.");
        let r = assess(&md_body, 5000, &cfg());
        assert!(r.hard_fail_reasons.contains(&"link_density_too_high"));
    }

    #[test]
    fn boilerplate_marker_soft_fails() {
        let md = "# Ok\n\nSome body long enough to clear min_markdown_chars 100.\n\n请订阅我们的频道。\n\nAnother paragraph long enough here to keep paragraph count sensible.";
        let r = assess(md, 5000, &cfg());
        assert!(r.soft_fail_reasons.contains(&"boilerplate_marker_present"));
        assert!(!r.pass);
    }

    #[test]
    fn disabled_gate_always_passes() {
        let mut c = cfg();
        c.enabled = false;
        let r = assess("x", 1, &c);
        assert!(r.pass);
        assert!(r.hard_fail_reasons.is_empty());
    }
}
