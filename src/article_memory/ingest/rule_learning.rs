//! DOM simplification (token-efficient input for the learning LLM) +
//! prompt building + rule validation (re-extracting on samples).

use super::rule_types::{LearnedRule, RuleSample};

/// Simplify HTML into a textual tree outline: tag + id + first 2 classes +
/// child element count. Skips text, attributes, scripts, styles. Depth and
/// children are capped.
pub fn simplify_dom(html: &str) -> String {
    // Minimal regex-free approach via scraper. Codebase may already have
    // scraper or lol_html in deps; if not, add scraper = "0.19" to Cargo.toml.
    use scraper::{ElementRef, Html};

    const MAX_DEPTH: usize = 8;
    const MAX_CHILDREN: usize = 10;

    let doc = Html::parse_document(html);
    let mut out = String::new();
    if let Some(root) = doc.tree.root().children().find(|n| n.value().is_element()) {
        if let Some(elem) = ElementRef::wrap(root) {
            render(&mut out, elem, 0, MAX_DEPTH, MAX_CHILDREN);
        }
    }
    out
}

fn render(
    out: &mut String,
    elem: scraper::ElementRef<'_>,
    depth: usize,
    max_depth: usize,
    max_children: usize,
) {
    let tag = elem.value().name();
    if matches!(tag, "script" | "style" | "noscript") {
        return;
    }
    let id = elem.value().id();
    let classes: Vec<&str> = elem.value().classes().take(2).collect();
    let child_count = elem.children().filter(|n| n.value().is_element()).count();
    let indent = "  ".repeat(depth);
    out.push_str(&indent);
    out.push_str(tag);
    if let Some(id) = id {
        out.push('#');
        out.push_str(id);
    }
    for c in &classes {
        out.push('.');
        out.push_str(c);
    }
    if child_count > 0 {
        out.push_str(&format!(" ({child_count} children)"));
    }
    out.push('\n');
    if depth + 1 >= max_depth {
        return;
    }
    let mut shown = 0;
    for child in elem.children() {
        if let Some(ch) = scraper::ElementRef::wrap(child) {
            if shown >= max_children {
                out.push_str(&indent);
                out.push_str(&format!("  ... ({} more)\n", child_count - shown));
                break;
            }
            render(out, ch, depth + 1, max_depth, max_children);
            shown += 1;
        }
    }
}

/// Build the user-prompt body for the learning LLM.
pub fn build_learn_prompt(host: &str, samples: &[(RuleSample, String)]) -> String {
    let mut p = String::new();
    p.push_str(&format!("Host: {host}\n"));
    p.push_str(&format!(
        "Failure context: {} article(s) failed quality gate.\n\n",
        samples.len()
    ));
    for (idx, (sample, html)) in samples.iter().enumerate() {
        p.push_str(&format!(
            "=== Sample {n} (url={url}, reason={reason}) ===\n",
            n = idx + 1,
            url = sample.url,
            reason = sample.failure_reason
        ));
        p.push_str("Simplified DOM:\n");
        p.push_str(&simplify_dom(html));
        p.push('\n');
        let preview: String = sample.markdown_from_engine.chars().take(500).collect();
        p.push_str("Bad markdown preview (first 500 chars):\n");
        p.push_str(&preview);
        p.push_str("\n\n");
    }
    p.push_str(
        "Emit JSON describing CSS selectors that would cleanly extract the main article body across ALL samples:\n\
         {\n  \"content_selectors\": [\"primary selector for article body\", ...],\n  \"remove_selectors\": [\"selectors to drop noise blocks\", ...],\n  \"title_selector\": \"selector for article title or null\",\n  \"start_markers\": [],\n  \"end_markers\": [],\n  \"confidence\": 0.0..1.0,\n  \"reasoning\": \"brief explanation\"\n}\n\n\
         Prefer selectors that appear in all samples. Use stable tag+class combos. Avoid :nth-child and dynamic IDs.",
    );
    p
}

pub const LEARN_SYSTEM_PROMPT: &str = "You are a web extraction rule generator. \
Given simplified DOM outlines from multiple pages of the same site, emit CSS \
selectors that would cleanly extract the main article content. Return strict \
JSON only. No markdown fences.";

/// Parse a LLM response into a LearnedRule. Strips any ```json fences the
/// model may have added despite the instruction.
pub fn parse_learn_response(
    host: &str,
    content: &str,
    learned_from: usize,
) -> anyhow::Result<LearnedRule> {
    use anyhow::{anyhow, Context};
    let trimmed = content.trim();
    let json_str = if trimmed.starts_with("```") {
        let mut s = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim()
            .to_string();
        if s.ends_with("```") {
            s.truncate(s.len() - 3);
        }
        s.trim().to_string()
    } else {
        trimmed.to_string()
    };
    let v: serde_json::Value = serde_json::from_str(&json_str)
        .with_context(|| format!("parse learn response as JSON: {json_str}"))?;
    let content_selectors = array_of_strings(&v, "content_selectors")
        .ok_or_else(|| anyhow!("content_selectors missing"))?;
    if content_selectors.is_empty() {
        return Err(anyhow!("content_selectors empty"));
    }
    let remove_selectors = array_of_strings(&v, "remove_selectors").unwrap_or_default();
    let title_selector = v
        .get("title_selector")
        .and_then(|s| s.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let start_markers = array_of_strings(&v, "start_markers").unwrap_or_default();
    let end_markers = array_of_strings(&v, "end_markers").unwrap_or_default();
    let confidence = v
        .get("confidence")
        .and_then(|x| x.as_f64())
        .map(|f| f.clamp(0.0, 1.0) as f32)
        .unwrap_or(0.5);
    let reasoning = v
        .get("reasoning")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    Ok(LearnedRule {
        host: host.to_string(),
        version: crate::support::isoformat(crate::support::now_utc()),
        content_selectors,
        remove_selectors,
        title_selector,
        start_markers,
        end_markers,
        confidence,
        reasoning,
        learned_from_sample_count: learned_from,
        stale: false,
    })
}

fn array_of_strings(v: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    v.get(key).and_then(|x| x.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|s| s.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simplify_dom_compresses_simple_html() {
        let html = r#"<html><body><header>x</header><main class="a b"><article>y</article></main></body></html>"#;
        let out = simplify_dom(html);
        assert!(out.contains("html"));
        assert!(out.contains("body"));
        assert!(out.contains("main.a.b"));
        assert!(out.contains("article"));
        assert!(!out.contains("x"), "text content must not leak");
    }

    #[test]
    fn parse_learn_response_happy_path() {
        let content = r#"{
            "content_selectors": ["article.post"],
            "remove_selectors": [".ad"],
            "title_selector": "h1",
            "start_markers": [],
            "end_markers": [],
            "confidence": 0.85,
            "reasoning": "shared across all samples"
        }"#;
        let rule = parse_learn_response("x.com", content, 3).unwrap();
        assert_eq!(rule.content_selectors, vec!["article.post"]);
        assert_eq!(rule.title_selector.as_deref(), Some("h1"));
        assert!((rule.confidence - 0.85).abs() < 1e-6);
        assert_eq!(rule.learned_from_sample_count, 3);
    }

    #[test]
    fn parse_learn_response_strips_json_fence() {
        let content = "```json\n{\"content_selectors\":[\"main\"]}\n```";
        let rule = parse_learn_response("x.com", content, 1).unwrap();
        assert_eq!(rule.content_selectors, vec!["main"]);
    }

    #[test]
    fn parse_learn_response_rejects_empty_selectors() {
        let content = r#"{"content_selectors": []}"#;
        assert!(parse_learn_response("x.com", content, 1).is_err());
    }
}

use super::quality_gate::{assess, QualityGateConfig};

/// Try applying `rule.content_selectors[0]` (and remove_selectors) to `html`.
/// Returns the extracted text on success, or None if no match.
fn apply_rule(rule: &LearnedRule, html: &str) -> Option<String> {
    use scraper::{Html, Selector};

    let doc = Html::parse_document(html);
    for selector_str in &rule.content_selectors {
        let Ok(sel) = Selector::parse(selector_str) else {
            continue;
        };
        if let Some(elem) = doc.select(&sel).next() {
            let mut text = elem.text().collect::<Vec<_>>().join("\n");
            // Apply remove_selectors by rebuilding without those subtrees.
            for rs in &rule.remove_selectors {
                if let Ok(rsel) = Selector::parse(rs) {
                    for junk in elem.select(&rsel) {
                        let junk_text = junk.text().collect::<Vec<_>>().join("\n");
                        text = text.replace(&junk_text, "");
                    }
                }
            }
            // Apply start/end markers.
            if let Some(marker) = rule
                .start_markers
                .iter()
                .find(|m| text.contains(m.as_str()))
            {
                if let Some(pos) = text.find(marker.as_str()) {
                    text = text[pos + marker.len()..].to_string();
                }
            }
            if let Some(marker) = rule.end_markers.iter().find(|m| text.contains(m.as_str())) {
                if let Some(pos) = text.find(marker.as_str()) {
                    text.truncate(pos);
                }
            }
            return Some(text.trim().to_string());
        }
    }
    None
}

pub struct ValidationResult {
    pub ok: bool,
    pub errors: Vec<String>,
    pub extracted_chars_median: usize,
}

/// Run the proposed rule against each sample's HTML; require:
///  - all samples yield non-empty extraction
///  - at least 2/3 pass the quality gate
///  - median extracted chars >= 1500
pub fn validate_rule(
    rule: &LearnedRule,
    samples: &[(RuleSample, String)],
    gate: &QualityGateConfig,
) -> ValidationResult {
    let mut errors = Vec::new();
    let mut char_counts = Vec::new();
    let mut gate_passes = 0usize;

    for (i, (_s, html)) in samples.iter().enumerate() {
        match apply_rule(rule, html) {
            Some(extracted) if !extracted.is_empty() => {
                char_counts.push(extracted.chars().count());
                let html_chars = html.chars().count();
                let gr = assess(&extracted, html_chars, gate);
                if gr.pass {
                    gate_passes += 1;
                }
            }
            _ => {
                errors.push(format!("sample {i} yielded no extraction"));
                char_counts.push(0);
            }
        }
    }

    let median = if char_counts.is_empty() {
        0
    } else {
        let mut sorted = char_counts.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    };

    let ok = errors.is_empty() && gate_passes >= (samples.len() * 2 / 3).max(1) && median >= 1500;
    if !ok && errors.is_empty() {
        errors.push(format!(
            "gate_passes={gate_passes}/{total}, median_chars={median} < 1500",
            total = samples.len()
        ));
    }
    ValidationResult {
        ok,
        errors,
        extracted_chars_median: median,
    }
}

#[cfg(test)]
mod validate_tests {
    use super::*;

    fn sample_with_html(html: &str) -> (RuleSample, String) {
        let s = RuleSample {
            url: "u".into(),
            job_id: "j".into(),
            captured_at: "t".into(),
            html_snapshot_path: "p".into(),
            markdown_from_engine: "".into(),
            failure_reason: "hard_fail".into(),
            failure_details: vec![],
        };
        (s, html.to_string())
    }

    fn big_html(body_chars: usize) -> String {
        // Split the body into 3 blank-line-separated chunks so the quality
        // gate's paragraph_count (split on "\n\n") sees 3 paragraphs.
        let per = body_chars / 9; // each chunk ~= body_chars/3 chars of "A. "
        let chunk_a = "A. ".repeat(per);
        let chunk_b = "B. ".repeat(per);
        let chunk_c = "C. ".repeat(per);
        let body = format!("{chunk_a}\n\n{chunk_b}\n\n{chunk_c}");
        format!("<html><body><article class=\"post\">{body}</article></body></html>")
    }

    #[test]
    fn rule_hits_all_samples_and_passes() {
        let rule = LearnedRule {
            host: "x.com".into(),
            version: "v1".into(),
            content_selectors: vec!["article.post".into()],
            remove_selectors: vec![],
            title_selector: None,
            start_markers: vec![],
            end_markers: vec![],
            confidence: 0.9,
            reasoning: "".into(),
            learned_from_sample_count: 3,
            stale: false,
        };
        let samples = vec![
            sample_with_html(&big_html(2000)),
            sample_with_html(&big_html(2000)),
            sample_with_html(&big_html(2000)),
        ];
        let gate = QualityGateConfig::default();
        let r = validate_rule(&rule, &samples, &gate);
        assert!(r.ok, "errors: {:?}", r.errors);
    }

    #[test]
    fn rule_misses_samples_fails_validation() {
        let rule = LearnedRule {
            host: "x.com".into(),
            version: "v1".into(),
            content_selectors: vec!["div.nonexistent".into()],
            remove_selectors: vec![],
            title_selector: None,
            start_markers: vec![],
            end_markers: vec![],
            confidence: 0.5,
            reasoning: "".into(),
            learned_from_sample_count: 1,
            stale: false,
        };
        let samples = vec![sample_with_html(&big_html(2000))];
        let gate = QualityGateConfig::default();
        let r = validate_rule(&rule, &samples, &gate);
        assert!(!r.ok);
    }
}
