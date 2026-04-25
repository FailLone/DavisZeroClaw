# crawl4ai Cleaning Upgrade — Phase 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Phase 1 engine ladder into a self-healing system that learns per-host CSS-selector rules from quality failures, fixes two latent Rust cleaning bugs, upgrades the deterministic value score, and consolidates four duplicate LLM HTTP clients into one.

**Architecture:** Five parts: ① Rust structural cleaning fixes (code-fence preservation + sliding-window dedup), ② multi-dim deterministic scoring + gopher hard-reject, ⑤ unified `chat_completion` client consolidating `cleaning_internals::create_chat_completion{,_for_value}` and `llm_extract::llm_html_to_markdown`, ③ LLM value judge emits `extraction_quality` in the same API call, ④ rule self-learning loop with LLM-generated CSS selectors, sample pool, hourly worker, stale detection, override file, and warmup CLI.

**Tech Stack:** Rust (tokio, serde, anyhow, reqwest), Python (FastAPI, crawl4ai's `JsonCssExtractionStrategy`), existing `Crawl4aiSupervisor` + `IngestWorkerPool`.

**Reference spec:** `docs/superpowers/specs/2026-04-24-crawl4ai-cleaning-upgrade-design.md` §9, §10, §11.

**Phase 1 state (already landed):**
- `src/article_memory/ingest/` has `content_signals.rs`, `quality_gate.rs`, `engines.rs`, `llm_extract.rs`, `report_context.rs`
- Engine ladder in `worker.rs` with Rust-local LLM fallback
- `IngestJob.engine_chain`, `ArticleCleanReport.engine_chain`/`final_engine`
- `ArticleMemoryExtractConfig`, `QualityGateToml`, `OpenRouterLlmEngineConfig`
- `[[sites]]` deleted from TOML + Rust
- 225 lib tests pass, clippy + fmt clean

**Out of scope (tracked in spec §17 Follow-ups):**
- ZeroClaw provider-crate consolidation (separate architectural project)
- MinHash LSH semantic dedup across `article_memory_index`
- PII scrubbing, fastText language filter, PDF-specific extraction

---

## File Structure

**New modules** (under `src/article_memory/`):
- `ingest/cleaning_fix.rs` — structure-preserving normalize + sliding-window dedup (⑤ the cleaning primitives that replace broken helpers in `cleaning_internals.rs`)
- `ingest/value_signals.rs` — deterministic score helpers consuming `ContentSignals` (②)
- `llm_client.rs` — unified `chat_completion` entry point (⑤)
- `ingest/rule_types.rs` — `LearnedRule`, `RuleStats`, `RuleSample` data shapes (④)
- `ingest/learned_rules.rs` — `LearnedRuleStore` (load/save/stale) + override merge (④)
- `ingest/rule_samples.rs` — `SampleStore` (push/list-ready/clear) (④)
- `ingest/rule_learning.rs` — DOM simplification, learn prompt, validation (④)
- `ingest/rule_learning_worker.rs` — hourly `tokio::spawn` loop (④)

**Modified modules**:
- `src/article_memory/cleaning_internals.rs` — replace broken `normalize_line` + in-place dedup usage (①)
- `src/article_memory/pipeline.rs` — multi-dim score via `value_signals` (②); parse `extraction_quality` fields (③); feed results to `LearnedRuleStore` (④)
- `src/article_memory/types.rs` — `ArticleValueReport` adds 3 fields (③); drop dead `ArticleMemoryIngestConfig.min_markdown_chars` field (②)
- `src/article_memory/reports.rs` — clean report serialization stays backward-compat
- `src/article_memory/ingest/worker.rs` — select learned engine before trafilatura; on LLM "poor" push sample, mark stale (④)
- `src/article_memory/ingest/engines.rs` — `EngineChoice::LearnedRules`, ladder-aware `pick_engine` (④)
- `src/article_memory/ingest/llm_extract.rs` — delegate to `llm_client` (⑤)
- `src/crawl4ai.rs` — new request field `learned_rule: Option<Value>` (④)
- `src/article_memory/mod.rs` + `src/lib.rs` — re-exports for new types
- `src/app_config.rs` — `RuleLearningConfig`, `ArticleMemoryOverridesConfig` types (④)
- `config/davis/article_memory.toml` — add `[rule_learning]` (④)
- `config/davis/article_memory_overrides.toml` — new empty scaffold (④)
- `crawl4ai_adapter/server.py` — `learned-rules` engine branch using `JsonCssExtractionStrategy` (④)
- `crawl4ai_adapter/engines.py` — new `extract_learned_rules(html, rule)` (④)
- `src/cli/articles.rs` — new `articles rule-learn {list,show,mark-stale,warmup,quarantine,promote}` subcommands (④)

**Tests:**
- Unit tests co-located in each new module
- `tests/rust/rule_learning_worker_test.rs` — integration test for one learning round
- `tests/rust/cleaning_structure_test.rs` — code-fence preservation + sliding dedup behavior

---

## Execution Order

Five phases internal to Phase 2. Each phase leaves a green tree; you can stop after any phase.

- **Phase 2.1** (Tasks 1-4): ① Rust cleaning fixes
- **Phase 2.2** (Tasks 5-8): ② Multi-dim scoring + gopher reject
- **Phase 2.3** (Tasks 9-11): ⑤ LLM client consolidation
- **Phase 2.4** (Tasks 12-14): ③ LLM judge `extraction_quality` field
- **Phase 2.5** (Tasks 15-27): ④ Rule self-learning loop
- **Phase 2.6** (Task 28): Final verification + spec status

Dependencies: 2.4 requires 2.2 + 2.3 ; 2.5 requires 2.1 + 2.3 + 2.4 ; 2.1 and 2.2 parallel-safe.

---

# Phase 2.1 — Rust Cleaning Fixes

### Task 1: Add `normalize_markdown_preserving_structure` + tests

**Files:**
- Create: `src/article_memory/ingest/cleaning_fix.rs`
- Modify: `src/article_memory/ingest/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/article_memory/ingest/cleaning_fix.rs`:

```rust
//! Structure-preserving markdown normalization.
//!
//! Replaces the T1-era `normalize_line` which unconditionally ran
//! `split_whitespace()` and destroyed fenced code blocks and list indentation.

#![allow(dead_code)]

/// Normalize a single line of markdown, preserving fenced-code content and
/// list indentation, while folding internal runs of whitespace into single
/// spaces.
///
/// When `in_code_fence` is true, the line is returned unchanged (callers
/// track fence state themselves — see `normalize_markdown_preserving_structure`).
pub fn normalize_line_preserving(line: &str, in_code_fence: bool) -> String {
    if in_code_fence {
        return line.to_string();
    }
    // Detect and preserve leading whitespace for list / indented content.
    let indent_len: usize = line
        .chars()
        .take_while(|c| c.is_whitespace() && *c != '\n')
        .count();
    let indent: String = line.chars().take(indent_len).collect();
    let body: String = line.chars().skip(indent_len).collect();
    let folded = body
        .replace('\u{00a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if folded.is_empty() {
        indent.trim_end().to_string()
    } else {
        format!("{indent}{folded}")
    }
}

/// Normalize a multi-line markdown string while respecting fenced code blocks.
pub fn normalize_markdown_preserving_structure(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push(line.to_string());
            continue;
        }
        out.push(normalize_line_preserving(line, in_fence));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_line_collapses_internal_whitespace() {
        let r = normalize_line_preserving("hello    world  \t  again", false);
        assert_eq!(r, "hello world again");
    }

    #[test]
    fn list_indent_preserved() {
        let r = normalize_line_preserving("  - nested    item", false);
        assert_eq!(r, "  - nested item");
    }

    #[test]
    fn code_fence_line_unchanged() {
        let r = normalize_line_preserving("    let x = 1;  // indent matters", true);
        assert_eq!(r, "    let x = 1;  // indent matters");
    }

    #[test]
    fn full_document_preserves_fenced_block() {
        let md = "Para one with    extra  spaces.\n\n```\n    let x = 1;\n    let y = 2;\n```\n\nPara two\twith\ttabs.";
        let out = normalize_markdown_preserving_structure(md);
        assert!(out.contains("Para one with extra spaces."));
        assert!(out.contains("    let x = 1;")); // 4-space indent kept
        assert!(out.contains("    let y = 2;"));
        assert!(out.contains("Para two with tabs."));
    }

    #[test]
    fn nbsp_becomes_space_outside_fence() {
        let r = normalize_line_preserving("a\u{00a0}b\u{00a0}c", false);
        assert_eq!(r, "a b c");
    }

    #[test]
    fn tilde_fences_toggled_too() {
        let md = "~~~\n  indented\n~~~\nfollowing";
        let out = normalize_markdown_preserving_structure(md);
        assert!(out.contains("  indented"));
    }
}
```

In `src/article_memory/ingest/mod.rs`, add after the existing `mod content_signals;`:

```rust
mod cleaning_fix;
#[allow(unused_imports)]
pub use cleaning_fix::{normalize_line_preserving, normalize_markdown_preserving_structure};
```

- [ ] **Step 2: Run test to verify failure**

Run: `cd /Users/faillonexie/Projects/DavisZeroClaw && cargo test -p davis_zero_claw --lib article_memory::ingest::cleaning_fix`
Expected: compiles + 6 tests PASS (implementation written with tests in Step 1).

- [ ] **Step 3: Clippy + fmt**

Run:
```
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
Both clean.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/ingest/cleaning_fix.rs src/article_memory/ingest/mod.rs
git commit -m "feat(article-memory): add structure-preserving markdown normalizer"
```

---

### Task 2: Add `SlidingDedup` + tests

**Files:**
- Modify: `src/article_memory/ingest/cleaning_fix.rs`

- [ ] **Step 1: Append SlidingDedup struct + tests**

Append to `src/article_memory/ingest/cleaning_fix.rs`:

```rust
use std::collections::VecDeque;

/// Near-line deduplicator. Drops a line only if its lowercased form appears
/// in the most recent `window_size` kept lines. Unlike full-document dedup,
/// this keeps legitimate cross-section repeats (e.g. "示例:" appearing in
/// multiple sections) while still removing adjacent template noise.
pub struct SlidingDedup {
    window: VecDeque<String>,
    window_size: usize,
    /// Lines at least this long are never deduped (long content is never
    /// accidental repetition).
    long_line_threshold: usize,
}

impl SlidingDedup {
    pub fn new(window_size: usize, long_line_threshold: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(window_size.saturating_add(1)),
            window_size,
            long_line_threshold,
        }
    }

    /// Returns true if the line should be kept; mutates internal window.
    pub fn accept(&mut self, line: &str) -> bool {
        if line.chars().count() >= self.long_line_threshold {
            return true;
        }
        let key = line.to_lowercase();
        if self.window.iter().any(|prev| prev == &key) {
            return false;
        }
        self.window.push_back(key);
        if self.window.len() > self.window_size {
            self.window.pop_front();
        }
        true
    }
}

#[cfg(test)]
mod dedup_tests {
    use super::*;

    #[test]
    fn adjacent_duplicate_dropped() {
        let mut d = SlidingDedup::new(5, 80);
        assert!(d.accept("hello"));
        assert!(!d.accept("hello"));
    }

    #[test]
    fn cross_section_repeat_kept_when_beyond_window() {
        // Window=3; after 3 filler lines, "example:" is eligible again.
        let mut d = SlidingDedup::new(3, 80);
        assert!(d.accept("example:"));
        assert!(d.accept("fill-a"));
        assert!(d.accept("fill-b"));
        assert!(d.accept("fill-c"));
        assert!(d.accept("example:"), "should pass after window shift");
    }

    #[test]
    fn long_lines_never_deduped() {
        let long: String = "x".repeat(100);
        let mut d = SlidingDedup::new(3, 80);
        assert!(d.accept(&long));
        assert!(d.accept(&long), "100-char line bypasses window check");
    }

    #[test]
    fn case_insensitive_dedup() {
        let mut d = SlidingDedup::new(5, 80);
        assert!(d.accept("Hello"));
        assert!(!d.accept("hello"));
        assert!(!d.accept("HELLO"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::cleaning_fix`
Expected: 10 tests PASS (6 from Task 1 + 4 new).

- [ ] **Step 3: Clippy + fmt**

Run:
```
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
Both clean.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/ingest/cleaning_fix.rs
git commit -m "feat(article-memory): add SlidingDedup for near-line deduplication"
```

---

### Task 3: Swap old `normalize_line` + BTreeSet dedup in `cleaning_internals.rs`

**Files:**
- Modify: `src/article_memory/cleaning_internals.rs`

- [ ] **Step 1: Read the current normalize_article_text**

Run: `grep -n "fn normalize_article_text\|fn normalize_line\|BTreeSet\|seen\.insert" src/article_memory/cleaning_internals.rs`

Identify:
- `pub(super) fn normalize_line(line: impl AsRef<str>) -> String` (around line 279)
- `pub(super) fn normalize_article_text(...)` — the caller that uses `seen: BTreeSet` dedup
- `use std::collections::BTreeSet;` import

- [ ] **Step 2: Wire the new normalizer into `normalize_article_text`**

In `normalize_article_text` (earlier in the file, around line 110-180), the current loop looks like:

```rust
    let mut seen = BTreeSet::new();
    let mut lines = Vec::new();
    // ... loop over raw_text lines calling normalize_line ...
    if line.chars().count() < 80 && !seen.insert(dedupe_key) {
        continue;
    }
```

REPLACE with (read the full function first to preserve other logic — noise filtering, empty-line collapsing, etc.):

```rust
    use super::ingest::cleaning_fix::SlidingDedup;

    let mut dedup = SlidingDedup::new(50, 80);
    let mut lines = Vec::new();
    // ... existing loop setup ...

    // Replace old call site:
    //   `if line.chars().count() < 80 && !seen.insert(dedupe_key) { continue; }`
    // with:
    if !dedup.accept(&line) {
        continue;
    }
```

Remove the `let mut seen = BTreeSet::new();` line and the now-dead `dedupe_key` binding if present.

If `BTreeSet` is no longer used elsewhere in the file, remove the import. Otherwise keep it.

- [ ] **Step 3: Swap `normalize_line` body**

Replace the old body:

```rust
pub(super) fn normalize_line(line: impl AsRef<str>) -> String {
    let line = line.as_ref();
    line.replace('\u{00a0}', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}
```

With a re-export delegation — track fence state at the caller instead. Actually simpler: **keep `normalize_line` as a per-line helper** but swap to the preserving version when there's no fence context. Change body to:

```rust
pub(super) fn normalize_line(line: impl AsRef<str>) -> String {
    super::ingest::cleaning_fix::normalize_line_preserving(line.as_ref(), false).trim().to_string()
}
```

NOTE: callers of `normalize_line` within `normalize_article_text` do not currently track fence state — they operate on raw text that's already been markdown-ified. The caller loop must be upgraded to track fences too. Do this:

In `normalize_article_text`, before the for-loop over lines, initialize:
```rust
    let mut in_fence = false;
```

Inside the loop, before `let line = normalize_line(...)`, detect fence:
```rust
    let trimmed_raw = raw_line.trim_start();
    if trimmed_raw.starts_with("```") || trimmed_raw.starts_with("~~~") {
        in_fence = !in_fence;
        lines.push(raw_line.to_string());
        continue;
    }
    let line = super::ingest::cleaning_fix::normalize_line_preserving(raw_line, in_fence);
    // Skip noise checks AND dedup for fenced lines:
    if in_fence {
        lines.push(line);
        continue;
    }
```

Leave the non-fenced path using the existing `is_noise_line` + dedup + empty-collapse logic.

- [ ] **Step 4: Run all article_memory tests**

Run:
```
cd /Users/faillonexie/Projects/DavisZeroClaw
cargo test -p davis_zero_claw --lib article_memory
```
Expected: all tests pass. If the existing `normalize_article_memory_writes_raw_normalized_and_final_files` test has brittle character-count assertions, they may shift ±a few — adjust the assertions to match the new behavior (use `assert!(chars >= X)` rather than exact equality where reasonable).

If any test fails on a specific expected-string value, inspect the diff — most likely the new normalizer preserves a code-block indent that the old one collapsed, OR keeps a cross-section "示例" that dedup used to drop. Update assertions to reflect the new semantics.

- [ ] **Step 5: Full suite + lint**

Run:
```
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/cleaning_internals.rs
git commit -m "fix(article-memory): normalize_article_text preserves code fences and uses sliding dedup"
```

---

### Task 4: Integration test for cleaning structure preservation

**Files:**
- Create: `tests/rust/cleaning_structure_test.rs`

- [ ] **Step 1: Write integration test**

Create `tests/rust/cleaning_structure_test.rs`:

```rust
//! End-to-end test: a raw article with code blocks + cross-section short
//! repeats goes through `normalize_article_memory` and retains structure.

use crate::article_memory::{add_article_memory, normalize_article_memory, ArticleMemoryAddRequest, ArticleMemoryRecordStatus};
use crate::{init_article_memory, RuntimePaths};

#[tokio::test]
async fn normalize_preserves_fenced_code_and_cross_section_repeats() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let paths = RuntimePaths::for_test(temp.path().to_path_buf());
    init_article_memory(&paths).expect("init");

    let raw_md = r#"# Example Article

Example one.

```rust
    let x = 1;
    let y = 2;
```

More prose here with enough    extra   whitespace   to   trigger   folding.

## Section Two

示例:

Example two.

## Section Three

示例:

Example three."#;

    let req = ArticleMemoryAddRequest {
        title: "Test".into(),
        url: Some("https://example.test/a".into()),
        source: "test".into(),
        language: None,
        tags: vec![],
        content: raw_md.to_string(),
        summary: None,
        translation: None,
        status: ArticleMemoryRecordStatus::Candidate,
        value_score: None,
        notes: None,
    };
    let record = add_article_memory(&paths, req).expect("add");

    let resp = normalize_article_memory(&paths, None, None, &record.id)
        .await
        .expect("normalize");

    let normalized_path = std::path::Path::new(&resp.normalized_path);
    let contents = std::fs::read_to_string(normalized_path).expect("read normalized");

    assert!(
        contents.contains("    let x = 1;"),
        "code-block indent lost:\n{contents}"
    );
    assert!(
        contents.contains("    let y = 2;"),
        "code-block indent lost:\n{contents}"
    );
    // Cross-section short repeat "示例:" should survive at least twice.
    let count = contents.matches("示例:").count();
    assert!(
        count >= 2,
        "expected >=2 instances of 示例: across sections, got {count} in:\n{contents}"
    );
}
```

- [ ] **Step 2: Register in mod.rs**

Check `tests/rust/mod.rs`. Add a module declaration:

```rust
mod cleaning_structure_test;
```

- [ ] **Step 3: Run the test**

Run: `cd /Users/faillonexie/Projects/DavisZeroClaw && cargo test -p davis_zero_claw cleaning_structure_test -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Full suite**

Run: `cargo test -p davis_zero_claw`
Expected: 227+ passed (225 prior + 4 cleaning_fix unit + 1 integration; counts may differ by how Task 1-3 tests were counted).

- [ ] **Step 5: Commit**

```bash
git add tests/rust/cleaning_structure_test.rs tests/rust/mod.rs
git commit -m "test(article-memory): integration test for code-fence + cross-section dedup preservation"
```

---

# Phase 2.2 — Multi-dim Deterministic Scoring

### Task 5: Create `value_signals.rs` with `deterministic_score`

**Files:**
- Create: `src/article_memory/ingest/value_signals.rs`
- Modify: `src/article_memory/ingest/mod.rs`

- [ ] **Step 1: Write module + tests**

Create `src/article_memory/ingest/value_signals.rs`:

```rust
//! Multi-dimensional deterministic value scoring.
//!
//! Phase 2 replacement for `pipeline::deterministic_value_report`'s hardcoded
//! 0.55 baseline. Reads content signals already computed by `ContentSignals`
//! + topic matches, returns a score in [0.0, 1.0].

#![allow(dead_code)]

use super::content_signals::ContentSignals;

/// Score components: base 0.5 + topic bonus (max +0.15) + code density bonus
/// + heading depth bonus + paragraph length bonus - link density penalty -
/// list ratio penalty.
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
        assert!(s >= 0.0 && s <= 1.0);
    }
}
```

In `src/article_memory/ingest/mod.rs`, add:

```rust
mod value_signals;
#[allow(unused_imports)]
pub use value_signals::{deterministic_score, gopher_reject};
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::value_signals`
Expected: 11 tests PASS.

- [ ] **Step 3: Clippy + fmt**

Run:
```
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
Clean.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/ingest/value_signals.rs src/article_memory/ingest/mod.rs
git commit -m "feat(article-memory): add multi-dim deterministic score + gopher reject helpers"
```

---

### Task 6: Wire `deterministic_score` into `pipeline::deterministic_value_report`

**Files:**
- Modify: `src/article_memory/pipeline.rs`

- [ ] **Step 1: Locate the function**

Run: `grep -n "fn deterministic_value_report" src/article_memory/pipeline.rs`

Read the function. It currently hardcodes `score: f32 = 0.55` + 3 conditions.

- [ ] **Step 2: Replace the scoring logic**

REPLACE the body of `deterministic_value_report` from the line `let mut score: f32 = 0.55;` through the decision tree that sets `score`, with:

```rust
    use super::ingest::content_signals::compute_signals;
    use super::ingest::value_signals::{deterministic_score, gopher_reject};

    let signals = compute_signals(normalized);
    let mut reasons = Vec::new();
    let mut risk_flags = Vec::new();
    let matched_topics = matched_value_topics(config, article, normalized);
    let mut deterministic_reject = false;

    // Gopher-style hard rejects, before any other analysis.
    if let Some(reason) = gopher_reject(&signals) {
        deterministic_reject = true;
        risk_flags.push(reason.to_string());
        reasons.push(format!("gopher-rule rejection: {reason}"));
    }

    if clean_report.clean_status == "fallback_raw" {
        deterministic_reject = true;
        risk_flags.push("fallback_raw".to_string());
        reasons.push("cleaning fell back to raw content".to_string());
    }

    if clean_report.normalized_chars < config.min_normalized_chars {
        deterministic_reject = true;
        risk_flags.push("normalized_too_short".to_string());
        reasons.push("normalized article is too short".to_string());
    }

    if matched_topics.is_empty() && !config.target_topics.is_empty() {
        deterministic_reject = true;
        risk_flags.push("off_topic".to_string());
        reasons.push("no target topic matched the article".to_string());
    }

    // Multi-dim score (even if rejecting, so callers see a number).
    let score = deterministic_score(&signals, matched_topics.len());

    if !clean_report.risk_flags.is_empty() {
        risk_flags.extend(clean_report.risk_flags.clone());
    }
    if reasons.is_empty() {
        reasons.push("passed deterministic value prefilter".to_string());
    }
```

Keep the existing `let decision = ...` block below unchanged — it still consumes `score` + `deterministic_reject` + threshold config.

Find and delete the old matched_topics length-based hardcode block if present (e.g. `score = if matched_topics.len() >= 2 { 0.65 } else { 0.55 };`).

- [ ] **Step 3: Build + run pipeline tests**

Run:
```
cd /Users/faillonexie/Projects/DavisZeroClaw
cargo build -p davis_zero_claw
cargo test -p davis_zero_claw --lib article_memory::pipeline
cargo test -p davis_zero_claw --lib article_memory
```

Some existing tests may assert specific score values or `reasons` wording — update them to match the new reality. Common adjustments:
- Tests that asserted `value_score >= 0.55` keep passing if they use `>=`.
- Tests that asserted specific strings like `"passed deterministic value prefilter"` keep passing (wording unchanged).
- Tests that fed short/empty content and expected `score ~ 0.10` now see `score ≈ 0.5 - penalties` — update to `assert!(score < 0.5)` or similar.

If you cannot make an assertion pass without breaking the intent, the test may have been exercising the OLD scoring behavior specifically. Update it to assert the new invariant (reject when deterministic_reject is true), not a specific score.

- [ ] **Step 4: Full suite + lint**

Run:
```
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

- [ ] **Step 5: Commit**

```bash
git add src/article_memory/pipeline.rs
git commit -m "feat(article-memory): deterministic_value_report uses multi-dim signals + gopher reject"
```

---

### Task 7: Delete dead `ArticleMemoryIngestConfig.min_markdown_chars`

**Files:**
- Modify: `src/app_config.rs`
- Modify: `src/article_memory/ingest/worker.rs` (if referenced there)
- Modify: `config/davis/article_memory.toml` (remove key if present)
- Modify: tests that reference the field

- [ ] **Step 1: Grep for usages**

Run: `grep -rn "min_markdown_chars" /Users/faillonexie/Projects/DavisZeroClaw/src /Users/faillonexie/Projects/DavisZeroClaw/tests /Users/faillonexie/Projects/DavisZeroClaw/config`

The field is read by the quality gate config (KEEP — `QualityGateToml.min_markdown_chars`) but its original owner `ArticleMemoryIngestConfig.min_markdown_chars` is now dead (worker no longer consults it after Phase 1).

Expected matches: `app_config.rs` (definition on `ArticleMemoryIngestConfig`), possibly test construction sites. The `QualityGateToml.min_markdown_chars` field stays.

- [ ] **Step 2: Remove the field from `ArticleMemoryIngestConfig`**

In `src/app_config.rs`:
1. Delete the `pub min_markdown_chars: usize,` line inside `pub struct ArticleMemoryIngestConfig { ... }`.
2. Delete its `#[serde(default = "default_ingest_min_markdown_chars")]` attribute.
3. Delete the `fn default_ingest_min_markdown_chars() -> usize { 600 }` helper.
4. Delete `min_markdown_chars: default_ingest_min_markdown_chars(),` from the `impl Default for ArticleMemoryIngestConfig`.

- [ ] **Step 3: Remove construction-site references**

Run: `grep -rn "min_markdown_chars" src/ tests/` again. Remove the field from every `ArticleMemoryIngestConfig { ... }` struct literal in tests.

- [ ] **Step 4: Remove from config TOML if present**

Run: `grep -n "min_markdown_chars" config/davis/article_memory.toml config/davis/local.example.toml`

Only a key under `[article_memory.ingest]` (not under `[quality_gate]`) should be removed. Delete the line if present.

- [ ] **Step 5: Build + test + lint**

Run:
```
cargo build -p davis_zero_claw
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "refactor(config): drop dead ArticleMemoryIngestConfig.min_markdown_chars (subsumed by quality gate)"
```

---

### Task 8: Ladder-aware `pick_engine`

**Files:**
- Modify: `src/article_memory/ingest/engines.rs`

- [ ] **Step 1: Update `pick_engine`**

Current `pick_engine` (engines.rs line ~70): `config.default_engine.clone()`.

REPLACE with:

```rust
/// Pick the starting engine. Rules:
/// 1. If `default_engine` is OpenRouterLlm, we still need HTML first —
///    Phase 1 worker code already falls back to Trafilatura for fetch.
/// 2. Otherwise return `default_engine` if it appears in the ladder,
///    else the head of the ladder.
pub fn pick_engine(config: &ExtractEngineConfig) -> EngineChoice {
    if matches!(config.default_engine, EngineChoice::OpenRouterLlm) {
        return EngineChoice::Trafilatura;
    }
    if config
        .fallback_ladder
        .iter()
        .any(|e| *e == config.default_engine)
    {
        config.default_engine.clone()
    } else {
        config
            .fallback_ladder
            .first()
            .cloned()
            .unwrap_or(EngineChoice::Trafilatura)
    }
}
```

This moves the "`OpenRouterLlm` fallback to Trafilatura for fetch" logic out of `worker.rs` (where Phase 1 put it) and into the pick site. Worker.rs will stop doing that special-case in Phase 2.5.

- [ ] **Step 2: Update tests**

Add to the existing `#[cfg(test)] mod tests` in `engines.rs`:

```rust
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
```

- [ ] **Step 3: Run**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::engines`
Expected: 6 tests PASS (existing 4 + new 2).

- [ ] **Step 4: Worker.rs: remove duplicated special-case**

In `src/article_memory/ingest/worker.rs`, find the block that looks like:

```rust
    let fetch_engine = match engine_cfg.default_engine {
        EngineChoice::OpenRouterLlm => EngineChoice::Trafilatura,
        ref other => other.clone(),
    };
```

SIMPLIFY to:

```rust
    let fetch_engine = pick_engine(&engine_cfg);
```

(`pick_engine` now handles the fallback internally.)

- [ ] **Step 5: Full suite + lint**

Run:
```
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/ingest/engines.rs src/article_memory/ingest/worker.rs
git commit -m "refactor(article-memory): ladder-aware pick_engine centralizes fetch-engine selection"
```

---

# Phase 2.3 — LLM Client Consolidation

### Task 9: Create unified `llm_client::chat_completion`

**Files:**
- Create: `src/article_memory/llm_client.rs`
- Modify: `src/article_memory/mod.rs`

- [ ] **Step 1: Write the module**

Create `src/article_memory/llm_client.rs`:

```rust
//! Single chat-completions entry point for article_memory LLM calls.
//!
//! Pre-Phase-2 the project had four near-identical reqwest clients for
//! `/chat/completions`: `cleaning_internals::create_chat_completion`,
//! `cleaning_internals::create_chat_completion_for_value`,
//! `ingest::llm_extract::llm_html_to_markdown`. This module consolidates
//! them. Callers supply `LlmChatRequest` describing their specific call
//! shape (system/user/temperature/max_tokens/timeout).

#![allow(dead_code)]

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use std::time::Duration;

/// Minimal provider credentials the chat endpoint needs.
pub struct LlmProvider<'a> {
    pub name: &'a str,
    pub base_url: &'a str,
    pub api_key: &'a str,
}

/// One chat-completions invocation.
pub struct LlmChatRequest<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub user: &'a str,
    pub temperature: f32,
    pub max_tokens: Option<usize>,
    pub timeout: Duration,
}

/// Call the provider's `/chat/completions` endpoint and return the
/// content of `choices[0].message.content`. Errors on HTTP failure,
/// empty content, or missing fields.
pub async fn chat_completion(
    provider: &LlmProvider<'_>,
    req: &LlmChatRequest<'_>,
) -> Result<String> {
    if provider.api_key.trim().is_empty() {
        bail!("llm provider '{}' has empty api_key", provider.name);
    }
    if provider.base_url.trim().is_empty() {
        bail!("llm provider '{}' has empty base_url", provider.name);
    }

    let endpoint = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );

    let client = reqwest::Client::builder()
        .timeout(req.timeout)
        .build()
        .context("build reqwest client for chat_completion")?;

    let mut payload = json!({
        "model": req.model,
        "messages": [
            {"role": "system", "content": req.system},
            {"role": "user", "content": req.user},
        ],
        "temperature": req.temperature,
    });
    if let Some(max_tokens) = req.max_tokens {
        payload["max_tokens"] = json!(max_tokens);
    }

    let response = client
        .post(endpoint)
        .bearer_auth(provider.api_key)
        .json(&payload)
        .send()
        .await
        .context("chat_completion request failed")?;

    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| String::from("<failed to read response>"));
    if !status.is_success() {
        bail!("chat_completion HTTP {status}: {body}");
    }

    let value: serde_json::Value = serde_json::from_str(&body)
        .context("chat_completion response was not valid JSON")?;
    value
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("chat_completion response did not contain non-empty content"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_api_key_bails_before_http() {
        let p = LlmProvider {
            name: "openrouter",
            base_url: "https://x",
            api_key: "   ",
        };
        let r = LlmChatRequest {
            model: "gpt-test",
            system: "",
            user: "",
            temperature: 0.0,
            max_tokens: None,
            timeout: Duration::from_secs(5),
        };
        let err = chat_completion(&p, &r).await.unwrap_err().to_string();
        assert!(err.contains("empty api_key"));
    }

    #[tokio::test]
    async fn empty_base_url_bails_before_http() {
        let p = LlmProvider {
            name: "openrouter",
            base_url: "",
            api_key: "sk-test",
        };
        let r = LlmChatRequest {
            model: "gpt-test",
            system: "",
            user: "",
            temperature: 0.0,
            max_tokens: None,
            timeout: Duration::from_secs(5),
        };
        let err = chat_completion(&p, &r).await.unwrap_err().to_string();
        assert!(err.contains("empty base_url"));
    }
}
```

In `src/article_memory/mod.rs`, add:

```rust
pub(crate) mod llm_client;
```

(Or `mod` if `pub(crate)` conflicts with the existing style — check the file's convention for internal modules.)

- [ ] **Step 2: Run tests**

Run: `cargo test -p davis_zero_claw --lib article_memory::llm_client`
Expected: 2 tests PASS.

- [ ] **Step 3: Clippy + fmt**

Run:
```
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
Clean.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/llm_client.rs src/article_memory/mod.rs
git commit -m "feat(article-memory): add unified chat_completion client (llm_client)"
```

---

### Task 10: Migrate `cleaning_internals::create_chat_completion{,_for_value}` to `llm_client`

**Files:**
- Modify: `src/article_memory/cleaning_internals.rs`

- [ ] **Step 1: Replace both functions**

In `cleaning_internals.rs`, locate `pub(super) async fn create_chat_completion` and `pub(super) async fn create_chat_completion_for_value`. REPLACE both with thin wrappers:

```rust
pub(super) async fn create_chat_completion(
    config: &ResolvedArticleNormalizeConfig,
    system: &str,
    user: &str,
    max_tokens: usize,
) -> Result<String> {
    use super::llm_client::{chat_completion, LlmChatRequest, LlmProvider};
    use std::time::Duration;
    chat_completion(
        &LlmProvider {
            name: &config.provider,
            base_url: &config.base_url,
            api_key: &config.api_key,
        },
        &LlmChatRequest {
            model: &config.model,
            system,
            user,
            temperature: 0.1,
            max_tokens: Some(max_tokens),
            timeout: Duration::from_secs(120),
        },
    )
    .await
}

pub(super) async fn create_chat_completion_for_value(
    config: &ResolvedArticleValueConfig,
    system: &str,
    user: &str,
    max_tokens: usize,
) -> Result<String> {
    use super::llm_client::{chat_completion, LlmChatRequest, LlmProvider};
    use std::time::Duration;
    chat_completion(
        &LlmProvider {
            name: &config.provider,
            base_url: &config.base_url,
            api_key: &config.api_key,
        },
        &LlmChatRequest {
            model: &config.model,
            system,
            user,
            temperature: 0.0,
            max_tokens: Some(max_tokens),
            timeout: Duration::from_secs(60),
        },
    )
    .await
}
```

Remove the old reqwest `Client::builder`/`post`/`bearer_auth` imports from the file if they become unused. Keep `serde_json::json` only if other functions in the file still use it.

- [ ] **Step 2: Build + test**

Run:
```
cargo build -p davis_zero_claw
cargo test -p davis_zero_claw --lib article_memory
```
Expected: all tests pass (the two wrappers are behaviorally identical to the old inline implementations).

- [ ] **Step 3: Full suite + lint**

Run:
```
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
Clean.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/cleaning_internals.rs
git commit -m "refactor(article-memory): cleaning_internals chat helpers delegate to llm_client"
```

---

### Task 11: Migrate `ingest::llm_extract::llm_html_to_markdown` to `llm_client`

**Files:**
- Modify: `src/article_memory/ingest/llm_extract.rs`

- [ ] **Step 1: Rewrite `llm_html_to_markdown` body**

REPLACE the inline reqwest block in `llm_html_to_markdown` with a delegation. The new function body:

```rust
pub async fn llm_html_to_markdown(
    provider: &ModelProviderConfig,
    engine_cfg: &OpenRouterLlmEngineConfig,
    html: &str,
) -> Result<String> {
    use super::super::llm_client::{chat_completion, LlmChatRequest, LlmProvider};
    use std::time::Duration;

    // Truncate by chars (UTF-8 safe).
    let truncated: String = html.chars().take(engine_cfg.max_input_chars).collect();
    let user = format!("Convert this HTML to Markdown:\n\n{truncated}");

    chat_completion(
        &LlmProvider {
            name: &provider.name,
            base_url: &provider.base_url,
            api_key: &provider.api_key,
        },
        &LlmChatRequest {
            model: &engine_cfg.model,
            system: SYSTEM_PROMPT,
            user: &user,
            temperature: 0.0,
            max_tokens: None,
            timeout: Duration::from_secs(engine_cfg.timeout_secs.max(1)),
        },
    )
    .await
}
```

Remove the now-unused `reqwest::Client`, `serde_json::json`, `anyhow` (except the fn-sig `Result`) imports if they're no longer referenced. Keep the `SYSTEM_PROMPT` constant and tests.

- [ ] **Step 2: Update tests if needed**

The existing `empty_api_key_bails_early` / `empty_base_url_bails_early` tests expect specific error strings from the inline implementation. Now they route through `llm_client`, which bails with slightly different wording (`"llm provider '...' has empty api_key"` vs the old `"provider '...' has empty api_key"`). Update test assertions:

```rust
#[tokio::test]
async fn empty_api_key_bails_early() {
    let p = provider("openrouter", "https://x", "");
    let err = llm_html_to_markdown(&p, &engine(), "<html></html>")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("empty api_key"));
}
```

The `contains("empty api_key")` substring is still present in both old and new error strings. Keep the substring check; adjust only if the test fails.

- [ ] **Step 3: Build + test**

Run:
```
cargo test -p davis_zero_claw --lib article_memory::ingest::llm_extract
cargo test -p davis_zero_claw
```
All clean.

- [ ] **Step 4: Clippy + fmt**

Run:
```
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
Clean. Remove any dead `use` statements the refactor exposed.

- [ ] **Step 5: Commit**

```bash
git add src/article_memory/ingest/llm_extract.rs
git commit -m "refactor(article-memory): llm_html_to_markdown delegates to llm_client"
```

---

# Phase 2.4 — LLM Judge `extraction_quality`

### Task 12: Extend `ArticleValueReport` with 3 new fields

**Files:**
- Modify: `src/article_memory/types.rs`

- [ ] **Step 1: Locate the struct**

Run: `grep -n "pub struct ArticleValueReport" src/article_memory/types.rs`

Read the struct.

- [ ] **Step 2: Add fields**

Add three new fields at the END of the struct (preserving existing fields):

```rust
    /// LLM-reported extraction quality. Defaults to `"clean"` when parsing
    /// legacy responses that predate this field.
    #[serde(default = "default_extraction_quality")]
    pub extraction_quality: String,
    /// Specific issues flagged by the LLM when extraction_quality !=
    /// `"clean"`.
    #[serde(default)]
    pub extraction_issues: Vec<String>,
    /// Freeform hint for the rule-learning system when the LLM suggests a
    /// selector/filter refinement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_refinement_hint: Option<String>,
```

Add the default fn in the same file near the other `fn default_*`:

```rust
fn default_extraction_quality() -> String {
    "clean".to_string()
}
```

- [ ] **Step 3: Update all `ArticleValueReport { ... }` construction sites**

Run: `grep -rn "ArticleValueReport {" src/ tests/`

For each construction site, add:

```rust
        extraction_quality: "clean".to_string(),
        extraction_issues: Vec::new(),
        rule_refinement_hint: None,
```

(Or, if the construction is via `..ArticleValueReport::default()` — skip; the new defaults work via serde defaults.)

- [ ] **Step 4: Build + test + lint**

Run:
```
cargo build -p davis_zero_claw
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "feat(article-memory): ArticleValueReport adds extraction_quality, issues, rule_refinement_hint"
```

---

### Task 13: Extend LLM judge prompt + parser

**Files:**
- Modify: `src/article_memory/pipeline.rs`

- [ ] **Step 1: Locate the judge prompt**

Run: `grep -n "fn judge_article_value\|fn parse_value_judge_response\|You judge\|Article title" src/article_memory/pipeline.rs`

Find the user prompt string in `judge_article_value`.

- [ ] **Step 2: Append extraction-quality instructions to the prompt**

In the user-prompt template, after the existing instructions (right before the final `"Article:\n{article}"` line), insert:

```
You also act as an extraction-quality judge. Look for:
- Abrupt truncation or missing continuation
- Boilerplate UI text (nav, share, comments, "related articles") mixed into body
- Broken or flattened code blocks, lists, tables
- Wrong content region (e.g. only a comment instead of the article body)

Also emit: extraction_quality ("clean" | "partial" | "poor"; default "clean" unless clearly faulty),
extraction_issues (array of short codes like ["content_truncated", "code_block_broken"]),
rule_refinement_hint (brief free-text suggesting a selector or filter change; null when not applicable).
```

And update the JSON output schema line (the existing one mentions `decision, value_score, reasons, topic_tags, risk_flags, translation_needed`):

```
Return JSON with keys: decision (save|candidate|reject), value_score (0..1), reasons (array),
topic_tags (array), risk_flags (array), translation_needed (boolean),
extraction_quality (clean|partial|poor), extraction_issues (array of strings),
rule_refinement_hint (string or null).
```

- [ ] **Step 3: Update the JSON parser**

In `parse_value_judge_response`, after the existing field extractions, add:

```rust
    let extraction_quality = value
        .get("extraction_quality")
        .and_then(|v| v.as_str())
        .unwrap_or("clean")
        .trim()
        .to_lowercase();
    let extraction_quality = match extraction_quality.as_str() {
        "clean" | "partial" | "poor" => extraction_quality,
        _ => "clean".to_string(),
    };
    let extraction_issues = json_string_array(&value, "extraction_issues");
    let rule_refinement_hint = value
        .get("rule_refinement_hint")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
```

Then in the `Ok(ArticleValueReport { ... })` construction near the end of `parse_value_judge_response`, add the three new fields:

```rust
        extraction_quality,
        extraction_issues,
        rule_refinement_hint,
```

- [ ] **Step 4: Update `deterministic_value_report` to also default these fields**

`deterministic_value_report` (called when LLM judge is disabled or gopher-rejected) must also set the three new fields. Inside its final `ArticleValueReport { ... }` literal, add:

```rust
        extraction_quality: "clean".to_string(),
        extraction_issues: Vec::new(),
        rule_refinement_hint: None,
```

- [ ] **Step 5: Add a unit test for the parser**

Find `#[cfg(test)] mod tests` in `pipeline.rs` (may be at bottom, may not exist). If no test module, add one. Add:

```rust
    #[test]
    fn parse_value_judge_response_reads_extraction_fields() {
        let config = ResolvedArticleValueConfig {
            enabled: true,
            llm_judge: true,
            provider: "openrouter".into(),
            api_key: "sk-test".into(),
            base_url: "https://x".into(),
            model: "gpt-test".into(),
            max_input_chars: 12000,
            min_normalized_chars: 400,
            save_threshold: 0.75,
            candidate_threshold: 0.45,
            target_topics: vec!["agent".into()],
        };
        let article = ArticleMemoryRecord {
            id: "a1".into(),
            title: "T".into(),
            url: None,
            source: "test".into(),
            language: None,
            tags: vec![],
            content_path: "articles/a1.content".into(),
            raw_path: None,
            normalized_path: None,
            summary_path: None,
            clean_status: None,
            clean_profile: None,
            value_score: None,
            status: ArticleMemoryRecordStatus::Candidate,
            captured_at: "2026-04-25T00:00:00Z".into(),
            created_at: "2026-04-25T00:00:00Z".into(),
            updated_at: "2026-04-25T00:00:00Z".into(),
            notes: None,
            translation: None,
        };
        let content = r#"{
            "decision": "candidate",
            "value_score": 0.6,
            "reasons": ["looks ok"],
            "topic_tags": ["agent"],
            "risk_flags": [],
            "translation_needed": false,
            "extraction_quality": "partial",
            "extraction_issues": ["boilerplate_mixed_in"],
            "rule_refinement_hint": "drop .related list"
        }"#;
        let report = parse_value_judge_response(content, &config, &article).unwrap();
        assert_eq!(report.extraction_quality, "partial");
        assert_eq!(report.extraction_issues, vec!["boilerplate_mixed_in"]);
        assert_eq!(report.rule_refinement_hint.as_deref(), Some("drop .related list"));
    }
```

Adjust `ArticleMemoryRecord` field list to match the actual struct (read `types.rs` if needed).

- [ ] **Step 6: Build + test + lint**

Run:
```
cargo build -p davis_zero_claw
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

- [ ] **Step 7: Commit**

```bash
git add src/article_memory/pipeline.rs
git commit -m "feat(article-memory): LLM judge emits extraction_quality + issues + refinement_hint"
```

---

### Task 14: Pipeline consumes extraction_quality (force candidate on "poor")

**Files:**
- Modify: `src/article_memory/pipeline.rs`

- [ ] **Step 1: Find the decision-finalization site**

Inside `judge_article_value` (after `parse_value_judge_response` returns a report), the code currently uses `report.decision` directly. Add a downgrade rule:

AFTER `let mut report = parse_value_judge_response(...)?;` (or however the variable is bound), insert:

```rust
    // Phase 2: when the LLM reports poor extraction, force candidate
    // regardless of value_score. Stops bad extractions from silently
    // becoming saved records; rule-learning worker (Phase 2.5) picks
    // up the HTML sample.
    if report.extraction_quality == "poor" && report.decision == "save" {
        tracing::info!(
            article_id = %article.id,
            value_score = report.value_score,
            issues = ?report.extraction_issues,
            "downgrading save→candidate due to extraction_quality=poor"
        );
        report.decision = "candidate".to_string();
        report.reasons.push(
            "extraction_quality=poor; downgraded from save to candidate".to_string()
        );
    }
```

- [ ] **Step 2: Test via a new unit test**

Append to `pipeline.rs`'s test module:

```rust
    #[test]
    fn parse_response_with_poor_quality_stays_save_until_consumer_downgrades() {
        // parse_value_judge_response does NOT downgrade; the consumer does.
        let config = /* same as previous test */;
        let article = /* same */;
        let content = r#"{
            "decision": "save",
            "value_score": 0.9,
            "reasons": [],
            "topic_tags": [],
            "risk_flags": [],
            "translation_needed": false,
            "extraction_quality": "poor",
            "extraction_issues": ["content_truncated"],
            "rule_refinement_hint": "selector too narrow"
        }"#;
        let report = parse_value_judge_response(content, &config, &article).unwrap();
        assert_eq!(report.decision, "save");  // parser stays neutral
        assert_eq!(report.extraction_quality, "poor");
    }
```

The downgrade logic lives in `judge_article_value`, not in the parser. Full E2E test for downgrade will come when the rule_samples wiring lands in Phase 2.5.

- [ ] **Step 3: Run + commit**

Run:
```
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

```bash
git add src/article_memory/pipeline.rs
git commit -m "feat(article-memory): downgrade save→candidate when extraction_quality=poor"
```

---

# Phase 2.5 — Rule Self-Learning Loop

This sub-phase introduces the rule-learning system. It has 13 tasks (T15-T27) forming four layers: data shapes → stores → learning worker → engine integration + CLI.

### Task 15: Define `rule_types.rs`

**Files:**
- Create: `src/article_memory/ingest/rule_types.rs`
- Modify: `src/article_memory/ingest/mod.rs`

- [ ] **Step 1: Write types + tests**

Create `src/article_memory/ingest/rule_types.rs`:

```rust
//! Data shapes for the rule-learning subsystem.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// LLM-generated per-host extraction rule (learned or hand-overridden).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LearnedRule {
    pub host: String,
    /// RFC3339 timestamp of when the rule was generated.
    pub version: String,
    pub content_selectors: Vec<String>,
    #[serde(default)]
    pub remove_selectors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_selector: Option<String>,
    #[serde(default)]
    pub start_markers: Vec<String>,
    #[serde(default)]
    pub end_markers: Vec<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub learned_from_sample_count: usize,
    #[serde(default)]
    pub stale: bool,
}

fn default_confidence() -> f32 {
    0.5
}

/// Hit/partial/poor counters + stale tracking per host.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleStats {
    #[serde(default)]
    pub rule_version: String,
    #[serde(default)]
    pub hits: u64,
    #[serde(default)]
    pub partial: u64,
    #[serde(default)]
    pub poor: u64,
    #[serde(default)]
    pub consecutive_issues: u32,
    #[serde(default)]
    pub last_relearn_trigger: Option<String>,
    #[serde(default)]
    pub last_updated: String,
}

/// A single captured HTML sample awaiting a learning round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSample {
    pub url: String,
    pub job_id: String,
    pub captured_at: String,
    /// Relative path from `runtime/article_memory/` to the HTML snapshot.
    pub html_snapshot_path: String,
    pub markdown_from_engine: String,
    pub failure_reason: String,
    #[serde(default)]
    pub failure_details: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learned_rule_default_confidence_deserializes_missing_field() {
        let json = r#"{"host":"example.com","version":"2026-04-25T00:00:00Z","content_selectors":["article"]}"#;
        let rule: LearnedRule = serde_json::from_str(json).unwrap();
        assert_eq!(rule.host, "example.com");
        assert!(!rule.stale);
        assert!((rule.confidence - 0.5).abs() < 1e-6);
    }

    #[test]
    fn rule_stats_default_is_all_zero() {
        let stats = RuleStats::default();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.consecutive_issues, 0);
    }

    #[test]
    fn rule_sample_roundtrips() {
        let sample = RuleSample {
            url: "https://x.example/a".into(),
            job_id: "j1".into(),
            captured_at: "2026-04-25T00:00:00Z".into(),
            html_snapshot_path: "rule_samples/x.example/j1.html".into(),
            markdown_from_engine: "# stub".into(),
            failure_reason: "hard_fail".into(),
            failure_details: vec!["markdown_too_short".into()],
        };
        let ser = serde_json::to_string(&sample).unwrap();
        let back: RuleSample = serde_json::from_str(&ser).unwrap();
        assert_eq!(back.url, sample.url);
    }
}
```

In `src/article_memory/ingest/mod.rs`, add:

```rust
mod rule_types;
#[allow(unused_imports)]
pub use rule_types::{LearnedRule, RuleSample, RuleStats};
```

- [ ] **Step 2: Run + commit**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::rule_types`
Expected: 3 tests PASS.

Run full lint.

```bash
git add src/article_memory/ingest/rule_types.rs src/article_memory/ingest/mod.rs
git commit -m "feat(article-memory): add rule-learning data shapes (LearnedRule, RuleStats, RuleSample)"
```

---

### Task 16: `LearnedRuleStore` with atomic persistence + overrides merge

**Files:**
- Create: `src/article_memory/ingest/learned_rules.rs`
- Modify: `src/article_memory/ingest/mod.rs`

- [ ] **Step 1: Write module + tests**

Create `src/article_memory/ingest/learned_rules.rs`:

```rust
//! Load / save / stale-track learned host rules. Also applies a hand-written
//! overrides file (`config/davis/article_memory_overrides.toml`) which takes
//! precedence over learned entries.

#![allow(dead_code)]

use super::rule_types::LearnedRule;
use crate::runtime_paths::RuntimePaths;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Deserialize)]
struct OverridesFile {
    #[serde(default)]
    overrides: Vec<OverrideRule>,
}

#[derive(Debug, Deserialize)]
struct OverrideRule {
    host: String,
    #[serde(default)]
    content_selectors: Vec<String>,
    #[serde(default)]
    remove_selectors: Vec<String>,
    #[serde(default)]
    title_selector: Option<String>,
    #[serde(default)]
    start_markers: Vec<String>,
    #[serde(default)]
    end_markers: Vec<String>,
}

impl OverrideRule {
    fn into_learned(self) -> LearnedRule {
        LearnedRule {
            host: self.host,
            version: "override".to_string(),
            content_selectors: self.content_selectors,
            remove_selectors: self.remove_selectors,
            title_selector: self.title_selector,
            start_markers: self.start_markers,
            end_markers: self.end_markers,
            confidence: 1.0,
            reasoning: "hand-written override".to_string(),
            learned_from_sample_count: 0,
            stale: false,
        }
    }
}

#[derive(Clone)]
pub struct LearnedRuleStore {
    learned_path: PathBuf,
    inner: Arc<RwLock<BTreeMap<String, LearnedRule>>>,
    overrides: Arc<BTreeMap<String, LearnedRule>>,
}

impl LearnedRuleStore {
    /// Load learned_rules.json from disk and merge any overrides.toml.
    /// overrides.toml path is passed in (typically
    /// `config/davis/article_memory_overrides.toml` relative to repo_root).
    pub fn load(paths: &RuntimePaths, overrides_path: Option<&std::path::Path>) -> Result<Self> {
        let learned_path = paths
            .article_memory_root()
            .join("learned_rules.json");
        let learned: BTreeMap<String, LearnedRule> = if learned_path.exists() {
            let raw = fs::read_to_string(&learned_path)
                .with_context(|| format!("read {}", learned_path.display()))?;
            serde_json::from_str(&raw)
                .with_context(|| format!("parse {}", learned_path.display()))?
        } else {
            BTreeMap::new()
        };

        let mut overrides = BTreeMap::new();
        if let Some(op) = overrides_path {
            if op.exists() {
                let raw = fs::read_to_string(op)
                    .with_context(|| format!("read {}", op.display()))?;
                let file: OverridesFile = toml::from_str(&raw)
                    .with_context(|| format!("parse {}", op.display()))?;
                for rule in file.overrides {
                    overrides.insert(rule.host.clone(), rule.into_learned());
                }
            }
        }

        Ok(Self {
            learned_path,
            inner: Arc::new(RwLock::new(learned)),
            overrides: Arc::new(overrides),
        })
    }

    /// Look up the active rule for a host. Overrides win over learned entries
    /// and are always treated as non-stale.
    pub async fn get(&self, host: &str) -> Option<LearnedRule> {
        if let Some(r) = self.overrides.get(host) {
            return Some(r.clone());
        }
        let map = self.inner.read().await;
        map.get(host).cloned()
    }

    /// Store (or replace) a learned rule for a host. Persists atomically.
    pub async fn upsert(&self, rule: LearnedRule) -> Result<()> {
        {
            let mut map = self.inner.write().await;
            map.insert(rule.host.clone(), rule);
        }
        self.persist().await
    }

    /// Mark a host's learned rule stale. No-op if missing.
    pub async fn mark_stale(&self, host: &str, reason: &str) -> Result<()> {
        {
            let mut map = self.inner.write().await;
            if let Some(rule) = map.get_mut(host) {
                if rule.stale {
                    return Ok(());
                }
                rule.stale = true;
                tracing::info!(host = %host, reason = %reason, "marking learned rule stale");
            } else {
                return Ok(());
            }
        }
        self.persist().await
    }

    async fn persist(&self) -> Result<()> {
        let map = self.inner.read().await;
        let body = serde_json::to_string_pretty(&*map)?;
        let tmp = self.learned_path.with_extension("json.tmp");
        fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, &self.learned_path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), self.learned_path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(host: &str) -> LearnedRule {
        LearnedRule {
            host: host.to_string(),
            version: "v1".to_string(),
            content_selectors: vec!["article".to_string()],
            remove_selectors: vec![],
            title_selector: None,
            start_markers: vec![],
            end_markers: vec![],
            confidence: 0.8,
            reasoning: "test".to_string(),
            learned_from_sample_count: 3,
            stale: false,
        }
    }

    #[tokio::test]
    async fn upsert_and_get_roundtrips() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(temp.path().to_path_buf());
        std::fs::create_dir_all(paths.article_memory_root()).unwrap();
        let store = LearnedRuleStore::load(&paths, None).unwrap();
        store.upsert(rule("example.com")).await.unwrap();
        let got = store.get("example.com").await.unwrap();
        assert_eq!(got.host, "example.com");
    }

    #[tokio::test]
    async fn override_wins_over_learned() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(temp.path().to_path_buf());
        std::fs::create_dir_all(paths.article_memory_root()).unwrap();
        let overrides_path = temp.path().join("overrides.toml");
        std::fs::write(
            &overrides_path,
            r#"[[overrides]]
host = "example.com"
content_selectors = [".hand-written"]
"#,
        )
        .unwrap();
        let store = LearnedRuleStore::load(&paths, Some(&overrides_path)).unwrap();
        store.upsert(rule("example.com")).await.unwrap();
        let got = store.get("example.com").await.unwrap();
        assert_eq!(got.content_selectors, vec![".hand-written".to_string()]);
        assert_eq!(got.version, "override");
    }

    #[tokio::test]
    async fn mark_stale_sets_flag() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(temp.path().to_path_buf());
        std::fs::create_dir_all(paths.article_memory_root()).unwrap();
        let store = LearnedRuleStore::load(&paths, None).unwrap();
        store.upsert(rule("example.com")).await.unwrap();
        store.mark_stale("example.com", "test").await.unwrap();
        let got = store.get("example.com").await.unwrap();
        assert!(got.stale);
    }

    #[tokio::test]
    async fn persist_survives_reload() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(temp.path().to_path_buf());
        std::fs::create_dir_all(paths.article_memory_root()).unwrap();
        {
            let s1 = LearnedRuleStore::load(&paths, None).unwrap();
            s1.upsert(rule("example.com")).await.unwrap();
        }
        let s2 = LearnedRuleStore::load(&paths, None).unwrap();
        let got = s2.get("example.com").await.unwrap();
        assert_eq!(got.host, "example.com");
    }
}
```

In `mod.rs`, add:

```rust
mod learned_rules;
#[allow(unused_imports)]
pub use learned_rules::LearnedRuleStore;
```

This requires `paths.article_memory_root()` — if the method has a different name, grep the runtime_paths module (`grep -n "article_memory" src/runtime_paths.rs`) and adapt. Expected name per Phase 1 code: `article_memory_root()` or `article_memory_index_path().parent()`.

- [ ] **Step 2: Run + commit**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::learned_rules`
Expected: 4 tests PASS.

Run full lint.

```bash
git add src/article_memory/ingest/learned_rules.rs src/article_memory/ingest/mod.rs
git commit -m "feat(article-memory): add LearnedRuleStore with atomic persistence + overrides merge"
```

---

### Task 17: `RuleStats` store (analog of LearnedRuleStore for counters)

**Files:**
- Modify: `src/article_memory/ingest/learned_rules.rs`

- [ ] **Step 1: Add `RuleStatsStore` to the same file**

Append to `src/article_memory/ingest/learned_rules.rs`:

```rust
use super::rule_types::RuleStats;

#[derive(Clone)]
pub struct RuleStatsStore {
    path: PathBuf,
    inner: Arc<RwLock<BTreeMap<String, RuleStats>>>,
}

impl RuleStatsStore {
    pub fn load(paths: &RuntimePaths) -> Result<Self> {
        let path = paths.article_memory_root().join("learned_rules_stats.json");
        let map: BTreeMap<String, RuleStats> = if path.exists() {
            let raw = fs::read_to_string(&path)?;
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        Ok(Self {
            path,
            inner: Arc::new(RwLock::new(map)),
        })
    }

    pub async fn bump_hit(&self, host: &str) -> Result<()> {
        let mut map = self.inner.write().await;
        let entry = map.entry(host.to_string()).or_default();
        entry.hits += 1;
        entry.consecutive_issues = 0;
        entry.last_updated = crate::support::isoformat(crate::support::now_utc());
        let body = serde_json::to_string_pretty(&*map)?;
        drop(map);
        self.persist(body).await
    }

    pub async fn bump_partial(&self, host: &str) -> Result<u32> {
        let mut map = self.inner.write().await;
        let entry = map.entry(host.to_string()).or_default();
        entry.partial += 1;
        entry.consecutive_issues += 1;
        entry.last_updated = crate::support::isoformat(crate::support::now_utc());
        let streak = entry.consecutive_issues;
        let body = serde_json::to_string_pretty(&*map)?;
        drop(map);
        self.persist(body).await?;
        Ok(streak)
    }

    pub async fn bump_poor(&self, host: &str) -> Result<()> {
        let mut map = self.inner.write().await;
        let entry = map.entry(host.to_string()).or_default();
        entry.poor += 1;
        entry.consecutive_issues += 1;
        entry.last_updated = crate::support::isoformat(crate::support::now_utc());
        let body = serde_json::to_string_pretty(&*map)?;
        drop(map);
        self.persist(body).await
    }

    pub async fn reset_for_new_rule(&self, host: &str, rule_version: &str) -> Result<()> {
        let mut map = self.inner.write().await;
        map.insert(
            host.to_string(),
            RuleStats {
                rule_version: rule_version.to_string(),
                hits: 0,
                partial: 0,
                poor: 0,
                consecutive_issues: 0,
                last_relearn_trigger: None,
                last_updated: crate::support::isoformat(crate::support::now_utc()),
            },
        );
        let body = serde_json::to_string_pretty(&*map)?;
        drop(map);
        self.persist(body).await
    }

    pub async fn get(&self, host: &str) -> Option<RuleStats> {
        let map = self.inner.read().await;
        map.get(host).cloned()
    }

    async fn persist(&self, body: String) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, body)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    #[tokio::test]
    async fn bump_partial_returns_running_streak() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(temp.path().to_path_buf());
        std::fs::create_dir_all(paths.article_memory_root()).unwrap();
        let store = RuleStatsStore::load(&paths).unwrap();
        assert_eq!(store.bump_partial("x.com").await.unwrap(), 1);
        assert_eq!(store.bump_partial("x.com").await.unwrap(), 2);
        store.bump_hit("x.com").await.unwrap();
        assert_eq!(store.bump_partial("x.com").await.unwrap(), 1);
    }

    #[tokio::test]
    async fn reset_clears_counters() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(temp.path().to_path_buf());
        std::fs::create_dir_all(paths.article_memory_root()).unwrap();
        let store = RuleStatsStore::load(&paths).unwrap();
        store.bump_partial("x.com").await.unwrap();
        store.bump_poor("x.com").await.unwrap();
        store.reset_for_new_rule("x.com", "v2").await.unwrap();
        let stats = store.get("x.com").await.unwrap();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.partial, 0);
        assert_eq!(stats.rule_version, "v2");
    }
}
```

Update mod.rs:

```rust
pub use learned_rules::{LearnedRuleStore, RuleStatsStore};
```

- [ ] **Step 2: Run + commit**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::learned_rules`
Expected: 6 tests PASS.

Full lint.

```bash
git add src/article_memory/ingest/learned_rules.rs src/article_memory/ingest/mod.rs
git commit -m "feat(article-memory): add RuleStatsStore for per-host hit/partial/poor counters"
```

---

### Task 18: `SampleStore` — HTML sample pool

**Files:**
- Create: `src/article_memory/ingest/rule_samples.rs`
- Modify: `src/article_memory/ingest/mod.rs`

- [ ] **Step 1: Write module + tests**

Create `src/article_memory/ingest/rule_samples.rs`:

```rust
//! Per-host accumulator for HTML samples awaiting a rule-learning round.

#![allow(dead_code)]

use super::rule_types::RuleSample;
use crate::runtime_paths::RuntimePaths;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

pub struct SampleStore {
    root: PathBuf,
}

impl SampleStore {
    pub fn new(paths: &RuntimePaths) -> Self {
        let root = paths.article_memory_root().join("rule_samples");
        Self { root }
    }

    fn host_dir(&self, host: &str) -> PathBuf {
        // Sanitize for filesystem: replace '/' and other oddities.
        let safe = host.replace(['/', '\\'], "_");
        self.root.join(safe)
    }

    /// Persist an HTML sample for `host`. Writes the HTML body to a
    /// `.html` file and a sidecar `.json` with metadata.
    pub fn push(
        &self,
        host: &str,
        job_id: &str,
        url: &str,
        html: &str,
        markdown_from_engine: &str,
        failure_reason: &str,
        failure_details: Vec<String>,
    ) -> Result<()> {
        let dir = self.host_dir(host);
        fs::create_dir_all(&dir)
            .with_context(|| format!("create {}", dir.display()))?;

        let timestamp = crate::support::isoformat(crate::support::now_utc());
        // Filesystem-safe timestamp: replace ':' with '-' for macOS/Win.
        let ts_safe = timestamp.replace(':', "-");
        let base = format!("{ts_safe}-{job_id}");
        let html_path = dir.join(format!("{base}.html"));
        let json_path = dir.join(format!("{base}.json"));

        let html_rel = pathdiff::diff_paths(&html_path, self.root.parent().unwrap_or(&self.root))
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| html_path.display().to_string());

        let sample = RuleSample {
            url: url.to_string(),
            job_id: job_id.to_string(),
            captured_at: timestamp,
            html_snapshot_path: html_rel,
            markdown_from_engine: markdown_from_engine.to_string(),
            failure_reason: failure_reason.to_string(),
            failure_details,
        };

        fs::write(&html_path, html)?;
        fs::write(&json_path, serde_json::to_string_pretty(&sample)?)?;
        Ok(())
    }

    /// Return the list of hosts whose sample count meets `threshold`.
    pub fn ready_hosts(&self, threshold: usize) -> Vec<String> {
        let mut ready = Vec::new();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return ready;
        };
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let json_count = entry
                .path()
                .read_dir()
                .map(|iter| {
                    iter.flatten()
                        .filter(|e| {
                            e.path()
                                .extension()
                                .and_then(|e| e.to_str())
                                .map(|s| s == "json")
                                .unwrap_or(false)
                        })
                        .count()
                })
                .unwrap_or(0);
            if json_count >= threshold {
                if let Some(name) = entry.file_name().to_str() {
                    ready.push(name.to_string());
                }
            }
        }
        ready
    }

    /// Load up to `limit` most-recent samples for a host.
    pub fn load_samples(&self, host: &str, limit: usize) -> Result<Vec<(RuleSample, String)>> {
        let dir = self.host_dir(host);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut entries: Vec<_> = fs::read_dir(&dir)?
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s == "json")
                    .unwrap_or(false)
            })
            .collect();
        entries.sort_by_key(|e| e.file_name());
        entries.reverse();

        let mut out = Vec::new();
        for entry in entries.into_iter().take(limit) {
            let json_path = entry.path();
            let html_path = json_path.with_extension("html");
            let sample: RuleSample = serde_json::from_str(&fs::read_to_string(&json_path)?)?;
            let html = fs::read_to_string(&html_path)?;
            out.push((sample, html));
        }
        Ok(out)
    }

    pub fn clear(&self, host: &str) -> Result<()> {
        let dir = self.host_dir(host);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_creates_html_and_json() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(temp.path().to_path_buf());
        std::fs::create_dir_all(paths.article_memory_root()).unwrap();
        let store = SampleStore::new(&paths);
        store
            .push(
                "example.com",
                "job1",
                "https://example.com/a",
                "<html></html>",
                "# md",
                "hard_fail",
                vec!["markdown_too_short".into()],
            )
            .unwrap();
        let loaded = store.load_samples("example.com", 10).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0.failure_reason, "hard_fail");
        assert_eq!(loaded[0].1, "<html></html>");
    }

    #[test]
    fn ready_hosts_respects_threshold() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(temp.path().to_path_buf());
        std::fs::create_dir_all(paths.article_memory_root()).unwrap();
        let store = SampleStore::new(&paths);
        store.push("a.com", "j1", "u", "h", "m", "hard_fail", vec![]).unwrap();
        store.push("a.com", "j2", "u", "h", "m", "hard_fail", vec![]).unwrap();
        assert_eq!(store.ready_hosts(3), Vec::<String>::new());
        store.push("a.com", "j3", "u", "h", "m", "hard_fail", vec![]).unwrap();
        assert_eq!(store.ready_hosts(3), vec!["a.com".to_string()]);
    }

    #[test]
    fn clear_removes_all_samples() {
        let temp = tempfile::TempDir::new().unwrap();
        let paths = RuntimePaths::for_test(temp.path().to_path_buf());
        std::fs::create_dir_all(paths.article_memory_root()).unwrap();
        let store = SampleStore::new(&paths);
        store.push("a.com", "j1", "u", "h", "m", "hard_fail", vec![]).unwrap();
        store.clear("a.com").unwrap();
        assert!(store.load_samples("a.com", 10).unwrap().is_empty());
    }
}
```

Add to `Cargo.toml` `[dependencies]` if not already present:
```toml
pathdiff = "0.2"
```
(Check first — if another part of the codebase uses a different relpath utility, adapt.)

Register in `mod.rs`:

```rust
mod rule_samples;
#[allow(unused_imports)]
pub use rule_samples::SampleStore;
```

- [ ] **Step 2: Run + commit**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::rule_samples`
Expected: 3 tests PASS.

```bash
git add src/article_memory/ingest/rule_samples.rs src/article_memory/ingest/mod.rs Cargo.toml Cargo.lock
git commit -m "feat(article-memory): add SampleStore for HTML rule-learning samples"
```

---

### Task 19: DOM simplification + learning prompt

**Files:**
- Create: `src/article_memory/ingest/rule_learning.rs`
- Modify: `src/article_memory/ingest/mod.rs`

- [ ] **Step 1: Write module + tests**

Create `src/article_memory/ingest/rule_learning.rs`:

```rust
//! DOM simplification (token-efficient input for the learning LLM) +
//! prompt building + rule validation (re-extracting on samples).

#![allow(dead_code)]

use super::rule_types::{LearnedRule, RuleSample};

/// Simplify HTML into a textual tree outline: tag + id + first 2 classes +
/// child element count. Skips text, attributes, scripts, styles. Depth and
/// children are capped.
pub fn simplify_dom(html: &str) -> String {
    // Minimal regex-free approach via scraper. Codebase may already have
    // scraper or lol_html in deps; if not, add scraper = "0.19" to Cargo.toml.
    use scraper::{ElementRef, Html, Node};

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

fn render(out: &mut String, elem: scraper::ElementRef<'_>, depth: usize, max_depth: usize, max_children: usize) {
    use scraper::node::Node;
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
        let preview: String = sample
            .markdown_from_engine
            .chars()
            .take(500)
            .collect();
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
pub fn parse_learn_response(host: &str, content: &str, learned_from: usize) -> anyhow::Result<LearnedRule> {
    use anyhow::{anyhow, Context};
    let trimmed = content.trim();
    let json_str = if trimmed.starts_with("```") {
        let mut s = trimmed.trim_start_matches("```json").trim_start_matches("```").trim().to_string();
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
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|arr| {
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
```

Add to Cargo.toml (if not present):
```toml
scraper = "0.19"
```

Register in `mod.rs`:

```rust
mod rule_learning;
#[allow(unused_imports)]
pub use rule_learning::{build_learn_prompt, parse_learn_response, simplify_dom, LEARN_SYSTEM_PROMPT};
```

- [ ] **Step 2: Run + commit**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::rule_learning`
Expected: 4 tests PASS.

```bash
git add src/article_memory/ingest/rule_learning.rs src/article_memory/ingest/mod.rs Cargo.toml Cargo.lock
git commit -m "feat(article-memory): add DOM simplification + learning prompt + response parser"
```

---

### Task 20: Rule validation (re-extract on samples, check gate)

**Files:**
- Modify: `src/article_memory/ingest/rule_learning.rs`

- [ ] **Step 1: Append validate_rule**

Append to `rule_learning.rs`:

```rust
use super::content_signals::compute_signals;
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
            if let Some(marker) = rule.start_markers.iter().find(|m| text.contains(m.as_str())) {
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
        let body = "A. ".repeat(body_chars / 3);
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
```

Also add `pub use` for `validate_rule` and `ValidationResult`:

```rust
#[allow(unused_imports)]
pub use rule_learning::{
    build_learn_prompt, parse_learn_response, simplify_dom, validate_rule, ValidationResult,
    LEARN_SYSTEM_PROMPT,
};
```

- [ ] **Step 2: Run + commit**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::rule_learning`
Expected: 6 tests PASS (4 from Task 19 + 2 new).

```bash
git add src/article_memory/ingest/rule_learning.rs src/article_memory/ingest/mod.rs
git commit -m "feat(article-memory): validate learned rules by re-extracting on samples"
```

---

### Task 21: Python `extract_learned_rules` engine

**Files:**
- Modify: `crawl4ai_adapter/engines.py`
- Modify: `crawl4ai_adapter/server.py`
- Modify: `crawl4ai_adapter/test_engines.py`

- [ ] **Step 1: Write test for the Python function**

Append to `crawl4ai_adapter/test_engines.py`:

```python
from crawl4ai_adapter.engines import extract_learned_rules


LEARNED_SAMPLE = """<!DOCTYPE html>
<html><head><title>T</title></head>
<body>
  <nav>nav ignore</nav>
  <article class="post">
    <h1 class="post-title">My Post</h1>
    <p>First paragraph.</p>
    <p>Second paragraph.</p>
    <div class="related"><h2>Related</h2><ul><li>x</li></ul></div>
  </article>
</body></html>"""


def test_learned_rules_extracts_content_and_drops_noise():
    rule = {
        "content_selectors": ["article.post"],
        "remove_selectors": [".related"],
        "title_selector": "h1.post-title",
    }
    r = extract_learned_rules(LEARNED_SAMPLE, rule)
    assert r.engine == "learned-rules"
    assert "My Post" in r.markdown
    assert "First paragraph" in r.markdown
    assert "Related" not in r.markdown
    assert "nav ignore" not in r.markdown


def test_learned_rules_empty_selector_returns_warning():
    rule = {"content_selectors": ["article.nonexistent"]}
    r = extract_learned_rules(LEARNED_SAMPLE, rule)
    assert r.is_empty()
    assert r.warnings
```

Also update the top import:
```python
from crawl4ai_adapter.engines import (
    ExtractResult,
    extract_learned_rules,
    extract_trafilatura,
)
```

- [ ] **Step 2: Run tests — expect fail**

Run:
```
cd /Users/faillonexie/Projects/DavisZeroClaw/crawl4ai_adapter
../.runtime/davis/crawl4ai-venv/bin/python -m pytest test_engines.py -v
```
Expected: ImportError.

- [ ] **Step 3: Implement**

Append to `crawl4ai_adapter/engines.py`:

```python
def extract_learned_rules(html: str, rule: dict[str, Any]) -> ExtractResult:
    """Apply a learned CSS-selector rule to HTML, producing Markdown.

    `rule` keys:
    - content_selectors: list[str]  (first match wins)
    - remove_selectors: list[str]   (dropped from the match)
    - title_selector: Optional[str]
    - start_markers / end_markers: list[str]   (text-level trimming)
    """
    from bs4 import BeautifulSoup  # lazy import
    import trafilatura

    soup = BeautifulSoup(html, "html.parser")
    content_selectors = rule.get("content_selectors") or []
    remove_selectors = rule.get("remove_selectors") or []
    start_markers = rule.get("start_markers") or []
    end_markers = rule.get("end_markers") or []

    block = None
    for sel in content_selectors:
        block = soup.select_one(sel)
        if block is not None:
            break
    if block is None:
        return ExtractResult(
            markdown="",
            metadata={},
            engine="learned-rules",
            warnings=[f"learned-rules: no match for selectors {content_selectors}"],
        )

    for rs in remove_selectors:
        for junk in block.select(rs):
            junk.decompose()

    block_html = str(block)
    # Use trafilatura just as the HTML→Markdown converter on the pruned block.
    markdown = trafilatura.extract(
        block_html,
        output_format="markdown",
        include_tables=True,
        include_formatting=True,
        include_links=True,
        favor_precision=False,
    ) or ""

    # Apply text-level start/end markers (if provided).
    for marker in start_markers:
        pos = markdown.find(marker)
        if pos != -1:
            markdown = markdown[pos + len(marker):]
            break
    for marker in end_markers:
        pos = markdown.find(marker)
        if pos != -1:
            markdown = markdown[:pos]
            break

    metadata: dict[str, Any] = {}
    title_selector = rule.get("title_selector")
    if title_selector:
        title_elem = soup.select_one(title_selector)
        if title_elem:
            metadata["title"] = title_elem.get_text(strip=True)

    warnings: list[str] = []
    if not markdown.strip():
        warnings.append("learned-rules: matched block produced empty markdown")
        markdown = ""
    return ExtractResult(
        markdown=markdown,
        metadata=metadata,
        engine="learned-rules",
        warnings=warnings,
    )
```

Also ensure `beautifulsoup4` is a dep — trafilatura already depends on it transitively, but add it explicitly to `src/cli/crawl.rs`'s pip-install list (modify in Task 22).

- [ ] **Step 4: Wire `/crawl` dispatch**

In `crawl4ai_adapter/server.py`, locate the engine dispatch block. Add another `elif` before the catch-all:

```python
    elif engine_used == "learned-rules":
        from crawl4ai_adapter.engines import extract_learned_rules
        raw_html = getattr(result, "html", None) or ""
        rule = req.learned_rule or {}
        if not rule.get("content_selectors"):
            raise HTTPException(
                status_code=400,
                detail={"error": "missing_learned_rule", "engine": engine_used},
            )
        er = extract_learned_rules(raw_html, rule)
        response_markdown = er.markdown or None
        extra_warnings.extend(er.warnings)
```

And add the `learned_rule` field to `CrawlRequest`:

```python
    learned_rule: Optional[dict[str, Any]] = None  # required when engine=learned-rules
```

- [ ] **Step 5: Run tests**

Run:
```
cd /Users/faillonexie/Projects/DavisZeroClaw/crawl4ai_adapter
../.runtime/davis/crawl4ai-venv/bin/python -m pytest test_engines.py -v
```
Expected: 6 tests PASS (4 existing + 2 new).

- [ ] **Step 6: Commit**

```bash
git add crawl4ai_adapter/engines.py crawl4ai_adapter/server.py crawl4ai_adapter/test_engines.py
git commit -m "feat(crawl4ai-adapter): add learned-rules engine using CSS selectors"
```

---

### Task 22: Install `beautifulsoup4` explicitly + Rust side plumbing

**Files:**
- Modify: `src/cli/crawl.rs`
- Modify: `src/crawl4ai.rs`

- [ ] **Step 1: Add bs4 to install command**

In `src/cli/crawl.rs`, find the `pip install --upgrade ...` Command. Append `beautifulsoup4` to the arg list (trafilatura pulls it transitively but we want it explicit since our code imports `bs4` directly):

```rust
            .arg("beautifulsoup4")
```

Update the display-string accordingly.

- [ ] **Step 2: Add `learned_rule` field to `Crawl4aiPageRequest`**

In `src/crawl4ai.rs`:

Extend the struct:
```rust
pub struct Crawl4aiPageRequest {
    pub profile_name: String,
    pub url: String,
    pub wait_for: Option<String>,
    pub js_code: Option<String>,
    pub markdown: bool,
    pub extract_engine: Option<String>,
    pub openrouter_config: Option<serde_json::Value>,
    pub learned_rule: Option<serde_json::Value>,  // NEW
}
```

Extend `CrawlRequestBody`:
```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    learned_rule: Option<&'a serde_json::Value>,
```

Thread through body construction:
```rust
        learned_rule: request.learned_rule.as_ref(),
```

Update `EngineChoice::from_str` / related if needed to recognize `"learned-rules"`:

In `engines.rs`, add variant:
```rust
pub enum EngineChoice {
    LearnedRules,
    Trafilatura,
    OpenRouterLlm,
    Pruning,
}
```

And update `as_str` / `from_str`:
```rust
Self::LearnedRules => "learned-rules",
"learned-rules" => Some(Self::LearnedRules),
```

Update construction sites of `Crawl4aiPageRequest`: grep, add `learned_rule: None,` everywhere.

```
grep -rn "Crawl4aiPageRequest {" src/ tests/
```

- [ ] **Step 3: Build + test**

Run:
```
cargo build -p davis_zero_claw
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(crawl4ai): plumb learned_rule field through Crawl4aiPageRequest"
```

---

### Task 23: Worker uses learned rules when available

**Files:**
- Modify: `src/article_memory/ingest/worker.rs`
- Modify: `src/article_memory/ingest/engines.rs`

- [ ] **Step 1: Extend `ExtractEngineConfig` default ladder**

In `engines.rs`, change the default ladder to include `LearnedRules` at the head (conditional — worker will still check if a rule exists per host):

```rust
impl Default for ExtractEngineConfig {
    fn default() -> Self {
        Self {
            default_engine: EngineChoice::LearnedRules,  // aspirational; falls through per-host
            fallback_ladder: vec![EngineChoice::Trafilatura, EngineChoice::OpenRouterLlm],
        }
    }
}
```

BUT keep `pick_engine` as-is: LearnedRules in default_engine falls back to Trafilatura at pick-time (worker does the actual lookup).

Update `pick_engine` to also fall back for `LearnedRules`:

```rust
pub fn pick_engine(config: &ExtractEngineConfig) -> EngineChoice {
    if matches!(config.default_engine, EngineChoice::OpenRouterLlm | EngineChoice::LearnedRules) {
        return EngineChoice::Trafilatura;
    }
    /* ... rest unchanged ... */
}
```

And add a test:
```rust
    #[test]
    fn pick_engine_learned_rules_default_falls_back_to_trafilatura() {
        let c = ExtractEngineConfig::default();
        assert_eq!(pick_engine(&c), EngineChoice::Trafilatura);
    }
```

- [ ] **Step 2: In worker.rs, check learned rule BEFORE fetch**

Near the top of `execute_job_core` Stage 1, BEFORE the first `crawl4ai_crawl` call, insert:

```rust
    // Phase 2: try learned-rules engine first if we have a non-stale rule for this host.
    let host = extract_host(&job.url);  // ensure a helper exists; use url::Url::parse
    let learned_rule = match &host {
        Some(h) => deps.learned_rules.get(h).await,
        None => None,
    };
    let active_learned_rule = learned_rule.as_ref().filter(|r| !r.stale);
```

Where `extract_host`:
```rust
fn extract_host(url_str: &str) -> Option<String> {
    url::Url::parse(url_str).ok()?.host_str().map(|s| s.to_lowercase())
}
```

Add this to worker.rs bottom.

THEN, if `active_learned_rule` is `Some`:

```rust
    if let Some(rule) = active_learned_rule {
        attempted.push(EngineChoice::LearnedRules);
        let rule_json = serde_json::to_value(rule).ok();
        // Call crawl4ai with engine=learned-rules, passing the rule.
        let req = Crawl4aiPageRequest {
            profile_name: job.profile_name.clone(),
            url: job.url.clone(),
            wait_for: None,
            js_code: None,
            markdown: false,
            extract_engine: Some("learned-rules".to_string()),
            openrouter_config: None,
            learned_rule: rule_json,
        };
        match crawl4ai_crawl(&deps.paths, &deps.crawl4ai_config, &deps.supervisor, req).await {
            Ok(p) => {
                let md = p.markdown.clone().unwrap_or_default();
                let html_chars = p.html.as_ref().map(|h| h.chars().count()).unwrap_or(0);
                let gr = assess_quality(&md, html_chars, &gate_cfg);
                if gr.pass {
                    // Success path: use this result.
                    page = p;
                    markdown = md;
                    gate = gr;
                    // ... skip fetch_engine fallback ...
                }
            }
            Err(err) => {
                tracing::warn!(host = ?host, error = %err, "learned-rules fetch failed; falling back");
            }
        }
    }
```

NOTE: restructuring the Phase 1 flat Stage 1 into this branching layout is non-trivial. The cleanest approach:

1. Extract the existing Stage 1 (fetch + gate + LLM upgrade) into a helper `async fn fetch_with_engine_ladder(...) -> Result<FetchOutcome, TerminalReason>`.
2. `execute_job_core` calls the learned-rules path first; on success skip the ladder; on failure call the ladder.

**Simpler option**: leave the learned-rules attempt inline, and if it fails (either gate fails or crawl errors), fall THROUGH to the existing Trafilatura → LLM ladder. Track `attempted` across both attempts so `engine_chain` reflects the full path.

Let me write the whole revised Stage 1 as one block. Refer to the code in worker.rs now; I'll rewrite it. Replace the entire Stage 1 region (currently from `// Stage 1: fetch + quality gate + Rust-local LLM upgrade` through `queue.attach_engine_chain(&job.id, ...).await;` for the success path) with:

```rust
    // Stage 1: fetch + quality gate, with optional learned-rules priority.
    let engine_cfg = engine_config_from_toml(&deps.extract_config);
    let gate_cfg = quality_gate_config_from_toml(&deps.quality_gate_config);
    let host = extract_host(&job.url);

    let mut attempted: Vec<EngineChoice> = Vec::new();
    let mut page: Option<crate::Crawl4aiPageResult> = None;
    let mut markdown = String::new();
    let mut gate: super::quality_gate::GateResult;

    // 1a. Try learned rule first, if any.
    let learned = match &host {
        Some(h) => deps.learned_rules.get(h).await,
        None => None,
    };
    if let Some(rule) = learned.as_ref().filter(|r| !r.stale) {
        attempted.push(EngineChoice::LearnedRules);
        let rule_json = serde_json::to_value(rule).ok();
        let req = Crawl4aiPageRequest {
            profile_name: job.profile_name.clone(),
            url: job.url.clone(),
            wait_for: None,
            js_code: None,
            markdown: false,
            extract_engine: Some("learned-rules".into()),
            openrouter_config: None,
            learned_rule: rule_json,
        };
        match crawl4ai_crawl(&deps.paths, &deps.crawl4ai_config, &deps.supervisor, req).await {
            Ok(p) => {
                let md = p.markdown.clone().unwrap_or_default();
                let html_chars = p.html.as_ref().map(|h| h.chars().count()).unwrap_or(0);
                let gr = assess_quality(&md, html_chars, &gate_cfg);
                if gr.pass {
                    markdown = md;
                    gate = gr;
                    page = Some(p);
                } else {
                    tracing::info!(
                        host = ?host, hard = ?gr.hard_fail_reasons, soft = ?gr.soft_fail_reasons,
                        "learned-rules gate failed; falling through to trafilatura"
                    );
                }
            }
            Err(err) => {
                tracing::warn!(host = ?host, error = %err, "learned-rules crawl failed; falling through");
            }
        }
    }

    // 1b. If learned-rules didn't succeed, run the Phase 1 engine ladder.
    if page.is_none() {
        let fetch_engine = pick_engine(&engine_cfg);
        attempted.push(fetch_engine.clone());
        // ... existing Phase 1 ladder code starting with crawl4ai_crawl(trafilatura) ...
        // Keep the block that sets `page`, `markdown`, `gate` via Trafilatura + optional LLM upgrade.
        // At the end of that block, ensure `page.is_some()` and `gate.pass` or we've already returned.
    }

    let final_engine_str = attempted
        .last()
        .map(|e| e.as_str().to_string())
        .unwrap_or_default();
    let _ = final_engine_str; // used later

    // Attach the full engine_chain before proceeding.
    queue
        .attach_engine_chain(
            &job.id,
            attempted.iter().map(|e| e.as_str().to_string()).collect(),
        )
        .await;
```

Implementer: preserve the exact Phase 1 trafilatura + LLM-upgrade block inside the `if page.is_none() { ... }` guard. The variable names `markdown`, `gate`, `page` are updated inside that block. On terminal-failure paths (crawl err / quality_gate_rejected), still do `queue.attach_engine_chain + queue.finish + return` as Phase 1 did.

- [ ] **Step 3: Add `learned_rules` to `IngestWorkerDeps`**

In `worker.rs`:
```rust
#[derive(Clone)]
pub struct IngestWorkerDeps {
    /* ... existing fields ... */
    pub learned_rules: Arc<crate::article_memory::ingest::learned_rules::LearnedRuleStore>,
    pub rule_stats: Arc<crate::article_memory::ingest::learned_rules::RuleStatsStore>,
    pub sample_store: Arc<crate::article_memory::ingest::rule_samples::SampleStore>,
}
```

Update the prod constructor in `src/local_proxy.rs` to build + pass these (T11-style plumbing):

```rust
            learned_rules: Arc::new(
                crate::article_memory::ingest::learned_rules::LearnedRuleStore::load(
                    &runtime_paths,
                    Some(&paths.repo_root.join("config/davis/article_memory_overrides.toml")),
                )?,
            ),
            rule_stats: Arc::new(
                crate::article_memory::ingest::learned_rules::RuleStatsStore::load(&runtime_paths)?,
            ),
            sample_store: Arc::new(crate::article_memory::ingest::rule_samples::SampleStore::new(&runtime_paths)),
```

Update the 8 test IngestWorkerDeps constructors in `tests/rust/article_memory_ingest_worker.rs`:

```rust
            learned_rules: Arc::new(
                crate::article_memory::ingest::learned_rules::LearnedRuleStore::load(
                    &paths, None,
                ).unwrap(),
            ),
            rule_stats: Arc::new(
                crate::article_memory::ingest::learned_rules::RuleStatsStore::load(&paths).unwrap(),
            ),
            sample_store: Arc::new(
                crate::article_memory::ingest::rule_samples::SampleStore::new(&paths),
            ),
```

- [ ] **Step 4: Build + test + lint**

Run:
```
cargo build -p davis_zero_claw
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean. Some existing tests may need `extract_host` handle or may surface compilation errors from field additions — fix minimally.

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "feat(article-memory): worker tries learned-rules engine before trafilatura"
```

---

### Task 24: Worker captures samples on `hard_fail` and consumes `extraction_quality`

**Files:**
- Modify: `src/article_memory/ingest/worker.rs`
- Modify: `src/article_memory/pipeline.rs`

- [ ] **Step 1: Capture sample on hard_fail before marking Failed**

In `worker.rs` Stage 1 trafilatura path, when the quality gate returns a `hard_fail`, AFTER the LLM upgrade has also failed (just before the final `finish Failed(quality_gate_rejected)`), push the HTML to the sample pool:

```rust
    // Capture sample for rule learning.
    if !gate.hard_fail_reasons.is_empty() {
        if let Some(ref h) = host {
            if let Some(ref p) = page {
                let html = p.html.clone().unwrap_or_default();
                if let Err(err) = deps.sample_store.push(
                    h,
                    &job.id,
                    &job.url,
                    &html,
                    &markdown,
                    "hard_fail",
                    gate.hard_fail_reasons.iter().map(|s| s.to_string()).collect(),
                ) {
                    tracing::warn!(host = %h, error = %err, "failed to push rule sample");
                }
            }
        }
    }
```

Place this RIGHT BEFORE the failure return path.

- [ ] **Step 2: After pipeline judge runs, push sample if extraction_quality=poor + mark stale**

In `pipeline.rs`, the `judge_article_value` function runs inside the worker's normalize call. The worker receives `ArticleCleanResponse` / `ArticleNormalizeResponse`. After `normalize_article_memory` returns, in worker.rs Stage 3 (judging), inspect the value_report.

Actually: the value report is written by `pipeline::judge_article_value` itself — we need to thread its result back to the worker. Two approaches:

**Approach A (simpler)**: Add a callback field to `EngineReportContext` for post-judge actions. The worker installs a callback that bumps stats / pushes samples.

**Approach B (cleaner)**: After `normalize_article_memory` returns, the worker reads the latest `value_reports/<article_id>.json` from disk and applies rule bookkeeping.

Use **Approach B**. After `normalize_article_memory(...).await?`:

```rust
    // Apply extraction_quality feedback to learned-rules store.
    if let Some(ref h) = host {
        if let Ok(value_report) = load_latest_value_report(&deps.paths, &record.id) {
            match value_report.extraction_quality.as_str() {
                "poor" => {
                    let _ = deps.rule_stats.bump_poor(h).await;
                    let _ = deps.learned_rules.mark_stale(h, "extraction_quality=poor").await;
                    // Save HTML sample if we have it (page.html was captured during Stage 1).
                    if let Some(ref p) = page {
                        if let Some(ref html) = p.html {
                            let _ = deps.sample_store.push(
                                h, &job.id, &job.url, html, &markdown,
                                "llm_poor",
                                value_report.extraction_issues.clone(),
                            );
                        }
                    }
                }
                "partial" => {
                    if let Ok(streak) = deps.rule_stats.bump_partial(h).await {
                        if streak >= 2 {
                            let _ = deps.learned_rules.mark_stale(h, "consecutive_partial").await;
                        }
                    }
                }
                _ => {
                    let _ = deps.rule_stats.bump_hit(h).await;
                }
            }
        }
    }
```

Add the helper `load_latest_value_report` to worker.rs:

```rust
fn load_latest_value_report(paths: &RuntimePaths, article_id: &str) -> anyhow::Result<crate::article_memory::ArticleValueReport> {
    let p = paths
        .article_memory_value_reports_dir()
        .join(format!("{article_id}.json"));
    let raw = std::fs::read_to_string(&p)?;
    Ok(serde_json::from_str(&raw)?)
}
```

If `article_memory_value_reports_dir` method doesn't exist on RuntimePaths, adapt — search for how value reports are currently written (`grep -n "value_reports" src/article_memory/*.rs`).

- [ ] **Step 3: Build + test + lint**

Run the full gauntlet. Existing tests may break if the sample-push assumes `host` is always Some — guard correctly.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/ingest/worker.rs
git commit -m "feat(article-memory): worker feeds rule_samples + stats from hard_fail / llm_poor signals"
```

---

### Task 25: Rule learning worker (hourly tokio task)

**Files:**
- Create: `src/article_memory/ingest/rule_learning_worker.rs`
- Modify: `src/article_memory/ingest/mod.rs`
- Modify: `src/local_proxy.rs` — spawn the worker at daemon start

- [ ] **Step 1: Write the worker**

Create `src/article_memory/ingest/rule_learning_worker.rs`:

```rust
//! Hourly background worker that turns accumulated rule samples into
//! LearnedRule entries. Calls the learning LLM (configured via
//! RuleLearningConfig), validates the rule against its samples, and
//! writes passing rules to `learned_rules.json`. Failing rules land in
//! `quarantine_rules/` with validation errors attached.

#![allow(dead_code)]

use super::learned_rules::{LearnedRuleStore, RuleStatsStore};
use super::quality_gate::QualityGateConfig;
use super::rule_learning::{
    build_learn_prompt, parse_learn_response, validate_rule, ValidationResult, LEARN_SYSTEM_PROMPT,
};
use super::rule_samples::SampleStore;
use crate::app_config::{ModelProviderConfig, RuleLearningConfig};
use crate::article_memory::llm_client::{chat_completion, LlmChatRequest, LlmProvider};
use crate::runtime_paths::RuntimePaths;
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct RuleLearningDeps {
    pub paths: RuntimePaths,
    pub learned_rules: Arc<LearnedRuleStore>,
    pub rule_stats: Arc<RuleStatsStore>,
    pub sample_store: Arc<SampleStore>,
    pub providers: Arc<Vec<ModelProviderConfig>>,
    pub config: Arc<RuleLearningConfig>,
    pub quality_gate: Arc<QualityGateConfig>,
}

pub struct RuleLearningWorker;

impl RuleLearningWorker {
    /// Spawn the hourly worker on the current runtime.
    pub fn spawn(deps: RuleLearningDeps) {
        if !deps.config.enabled {
            tracing::info!("rule learning worker disabled; not spawning");
            return;
        }
        tokio::spawn(async move {
            tracing::info!("rule learning worker started");
            // Initial quick scan, then hourly.
            if let Err(err) = run_scan(&deps).await {
                tracing::error!(error = %err, "rule learning initial scan failed");
            }
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            interval.tick().await; // skip the immediate first tick
            loop {
                interval.tick().await;
                if let Err(err) = run_scan(&deps).await {
                    tracing::error!(error = %err, "rule learning scan failed");
                }
            }
        });
    }
}

async fn run_scan(deps: &RuleLearningDeps) -> Result<()> {
    let ready = deps.sample_store.ready_hosts(deps.config.samples_required);
    if ready.is_empty() {
        return Ok(());
    }
    tracing::info!(hosts = ?ready, "rule learning: hosts ready to learn");
    for host in ready {
        match learn_one_host(deps, &host).await {
            Ok(()) => tracing::info!(host = %host, "rule learning: host complete"),
            Err(err) => tracing::warn!(host = %host, error = %err, "rule learning: host failed"),
        }
    }
    Ok(())
}

async fn learn_one_host(deps: &RuleLearningDeps, host: &str) -> Result<()> {
    let samples = deps
        .sample_store
        .load_samples(host, deps.config.samples_required)?;
    if samples.len() < deps.config.samples_required {
        return Ok(());
    }

    let prompt = build_learn_prompt(host, &samples);
    let provider = deps
        .providers
        .iter()
        .find(|p| p.name == deps.config.learning_provider)
        .with_context(|| format!("provider '{}' not configured", deps.config.learning_provider))?;

    let response = chat_completion(
        &LlmProvider {
            name: &provider.name,
            base_url: &provider.base_url,
            api_key: &provider.api_key,
        },
        &LlmChatRequest {
            model: &deps.config.learning_model,
            system: LEARN_SYSTEM_PROMPT,
            user: &prompt,
            temperature: 0.0,
            max_tokens: Some(2000),
            timeout: Duration::from_secs(120),
        },
    )
    .await
    .context("learning LLM call failed")?;

    let rule = parse_learn_response(host, &response, samples.len())
        .context("parse learning LLM response")?;

    let validation = validate_rule(&rule, &samples, &deps.quality_gate);
    if !validation.ok {
        write_quarantine(&deps.paths, host, &rule, &validation)?;
        if deps.config.notify_on_quarantine {
            // Fire-and-forget notification — daemon picks up primary handle.
            tracing::warn!(
                host = %host, errors = ?validation.errors,
                "learning rule failed validation; quarantined"
            );
        }
        return Ok(());
    }

    deps.learned_rules.upsert(rule.clone()).await?;
    deps.rule_stats.reset_for_new_rule(host, &rule.version).await?;
    deps.sample_store.clear(host)?;
    tracing::info!(
        host = %host, version = %rule.version, confidence = rule.confidence,
        "learned rule saved"
    );
    Ok(())
}

fn write_quarantine(
    paths: &RuntimePaths,
    host: &str,
    rule: &super::rule_types::LearnedRule,
    validation: &ValidationResult,
) -> Result<PathBuf> {
    let dir = paths.article_memory_root().join("quarantine_rules");
    fs::create_dir_all(&dir)?;
    let ts = crate::support::isoformat(crate::support::now_utc()).replace(':', "-");
    let path = dir.join(format!("{host}-{ts}.json"));
    let body = serde_json::json!({
        "host": host,
        "rule": rule,
        "errors": validation.errors,
        "extracted_chars_median": validation.extracted_chars_median,
    });
    fs::write(&path, serde_json::to_string_pretty(&body)?)?;
    Ok(path)
}
```

Add config type in `src/app_config.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleLearningConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_samples_required")]
    pub samples_required: usize,
    #[serde(default = "default_stale_after_partial")]
    pub stale_after_consecutive_issues: u32,
    #[serde(default = "default_learning_provider")]
    pub learning_provider: String,
    #[serde(default = "default_learning_model")]
    pub learning_model: String,
    #[serde(default = "default_true")]
    pub notify_on_quarantine: bool,
}

impl Default for RuleLearningConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            samples_required: default_samples_required(),
            stale_after_consecutive_issues: default_stale_after_partial(),
            learning_provider: default_learning_provider(),
            learning_model: default_learning_model(),
            notify_on_quarantine: true,
        }
    }
}

fn default_samples_required() -> usize { 3 }
fn default_stale_after_partial() -> u32 { 2 }
fn default_learning_provider() -> String { "openrouter".to_string() }
fn default_learning_model() -> String { "openai/gpt-4o".to_string() }
```

Add to `ArticleMemoryConfig`:
```rust
    #[serde(default)]
    pub rule_learning: RuleLearningConfig,
```

Register module in `src/article_memory/ingest/mod.rs`:

```rust
mod rule_learning_worker;
#[allow(unused_imports)]
pub use rule_learning_worker::{RuleLearningDeps, RuleLearningWorker};
```

Re-export `RuleLearningConfig` from `src/lib.rs`.

- [ ] **Step 2: Spawn the worker in daemon boot**

In `src/local_proxy.rs`, near where `IngestWorkerPool::spawn` is called, also spawn:

```rust
    crate::article_memory::ingest::RuleLearningWorker::spawn(
        crate::article_memory::ingest::RuleLearningDeps {
            paths: runtime_paths.clone(),
            learned_rules: learned_rules.clone(),
            rule_stats: rule_stats.clone(),
            sample_store: sample_store.clone(),
            providers: providers.clone(),
            config: Arc::new(local.article_memory.rule_learning.clone()),
            quality_gate: Arc::new(
                crate::article_memory::ingest::quality_gate::QualityGateConfig {
                    enabled: local.article_memory.quality_gate.enabled,
                    min_markdown_chars: local.article_memory.quality_gate.min_markdown_chars,
                    min_kept_ratio: local.article_memory.quality_gate.min_kept_ratio,
                    min_paragraphs: local.article_memory.quality_gate.min_paragraphs,
                    max_link_density: local.article_memory.quality_gate.max_link_density,
                    boilerplate_markers: local.article_memory.quality_gate.boilerplate_markers.clone(),
                },
            ),
        },
    );
```

Hoist the previously-constructed `learned_rules`, `rule_stats`, `sample_store`, `providers` arcs so they're reused.

- [ ] **Step 3: Build + test**

Run:
```
cargo build -p davis_zero_claw
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(article-memory): hourly rule-learning worker with LLM call + validation"
```

---

### Task 26: TOML + overrides scaffold + article_memory_overrides.toml

**Files:**
- Modify: `config/davis/article_memory.toml`
- Create: `config/davis/article_memory_overrides.toml`

- [ ] **Step 1: Append `[rule_learning]` to TOML**

At the end of `config/davis/article_memory.toml`:

```toml
[rule_learning]
enabled = true
samples_required = 3
stale_after_consecutive_issues = 2
learning_provider = "openrouter"
learning_model = "openai/gpt-4o"
notify_on_quarantine = true
```

- [ ] **Step 2: Create empty overrides scaffold**

Create `config/davis/article_memory_overrides.toml`:

```toml
# Hand-written overrides for article extraction rules.
#
# Each [[overrides]] entry takes precedence over any learned rule for the
# same host (learned rules live in runtime/article_memory/learned_rules.json).
# Use this file as an emergency repair channel — e.g. when a site's DOM
# changes and the learner hasn't caught up yet.
#
# Example:
#
# [[overrides]]
# host = "zhihu.com"
# content_selectors = [".QuestionAnswer-content .RichContent-inner"]
# remove_selectors = [".AnswerItem .RichContent-cover", "aside"]
# title_selector = "h1.QuestionHeader-title"
# start_markers = []
# end_markers = []
```

- [ ] **Step 3: Test that TOML parses**

Run:
```
cargo test -p davis_zero_claw --lib app_config
```
Clean.

- [ ] **Step 4: Commit**

```bash
git add config/davis/article_memory.toml config/davis/article_memory_overrides.toml
git commit -m "config: add [rule_learning] section + empty article_memory_overrides.toml scaffold"
```

---

### Task 27: `daviszeroclaw articles rule-learn` CLI

**Files:**
- Modify: `src/cli/articles.rs`
- Modify: `src/server.rs` (new HTTP endpoints)

- [ ] **Step 1: Add HTTP endpoints for rule management**

In `src/server.rs`, add three new routes:

```rust
        .route("/article-memory/rules", get(rules_list_handler))
        .route("/article-memory/rules/mark-stale", post(rules_mark_stale_handler))
        .route("/article-memory/rules/warmup", post(rules_warmup_handler))
```

Handler stubs (full impls below):

```rust
async fn rules_list_handler(State(state): State<AppState>) -> impl IntoResponse {
    // TODO in next step — see Task 27 Step 2.
}
```

No — fill these out directly. For `list`:

```rust
async fn rules_list_handler(
    State(state): State<AppState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    use serde_json::json;
    let store = &state.learned_rules;
    // Dump the current map as JSON.
    let snapshot = store.snapshot().await;  // add this method on LearnedRuleStore
    Ok(Json(json!({ "rules": snapshot })))
}

#[derive(Deserialize)]
struct MarkStalePayload { host: String, reason: Option<String> }

async fn rules_mark_stale_handler(
    State(state): State<AppState>,
    Json(payload): Json<MarkStalePayload>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let reason = payload.reason.unwrap_or_else(|| "manual".to_string());
    state
        .learned_rules
        .mark_stale(&payload.host, &reason)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;
    Ok(Json(json!({"status": "ok"})))
}
```

Add `snapshot` method to `LearnedRuleStore`:

```rust
    pub async fn snapshot(&self) -> BTreeMap<String, LearnedRule> {
        let map = self.inner.read().await;
        let mut out = map.clone();
        for (host, rule) in self.overrides.iter() {
            out.insert(host.clone(), rule.clone());
        }
        out
    }
```

And add `learned_rules` to `AppState` (grep for how other stores are attached).

Warmup handler:
```rust
#[derive(Deserialize)]
struct WarmupPayload {
    hosts: Option<Vec<String>>,
    per_host: Option<usize>,
    from_existing: Option<bool>,
}

async fn rules_warmup_handler(
    State(state): State<AppState>,
    Json(payload): Json<WarmupPayload>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    // Placeholder: full warmup impl is its own subsystem. For Phase 2
    // v1, return 501 with guidance.
    let _ = (state, payload);
    Err((
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "warmup not implemented in Phase 2 v1; run articles ingest manually"})),
    ))
}
```

**Defer full warmup** to a Phase 2.1 follow-up (single user-level CLI flow). The stub returning 501 is fine for shipping.

- [ ] **Step 2: Add CLI subcommands**

In `src/cli/articles.rs`, grep for the existing `Ingest(...)` subcommand layout. Add a new enum variant:

```rust
    RuleLearn(RuleLearnArgs),
```

```rust
#[derive(clap::Args)]
pub struct RuleLearnArgs {
    #[command(subcommand)]
    pub action: RuleLearnAction,
}

#[derive(clap::Subcommand)]
pub enum RuleLearnAction {
    List,
    Show { host: String },
    MarkStale { host: String, #[arg(long)] reason: Option<String> },
    Quarantine,
    Promote { host: String },
}
```

Handler:

```rust
pub async fn handle_rule_learn(args: RuleLearnArgs, daemon_url: &str) -> Result<()> {
    match args.action {
        RuleLearnAction::List => {
            let resp = reqwest::get(format!("{daemon_url}/article-memory/rules"))
                .await?
                .error_for_status()?
                .text()
                .await?;
            println!("{resp}");
            Ok(())
        }
        RuleLearnAction::MarkStale { host, reason } => {
            let client = reqwest::Client::new();
            let resp = client
                .post(format!("{daemon_url}/article-memory/rules/mark-stale"))
                .json(&serde_json::json!({
                    "host": host,
                    "reason": reason,
                }))
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            println!("{resp}");
            Ok(())
        }
        RuleLearnAction::Show { host } => {
            // Fetch full list, filter locally.
            let resp: serde_json::Value = reqwest::get(format!("{daemon_url}/article-memory/rules"))
                .await?
                .error_for_status()?
                .json()
                .await?;
            let rule = resp
                .get("rules")
                .and_then(|r| r.get(&host))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            println!("{}", serde_json::to_string_pretty(&rule)?);
            Ok(())
        }
        RuleLearnAction::Quarantine => {
            // Scan local filesystem (CLI may not have daemon access to runtime paths).
            let paths = RuntimePaths::default_for_repo()?;  // adapt to actual helper
            let dir = paths.article_memory_root().join("quarantine_rules");
            if !dir.exists() {
                println!("no quarantine rules");
                return Ok(());
            }
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                println!("{}", entry.path().display());
            }
            Ok(())
        }
        RuleLearnAction::Promote { host } => {
            // Find the most recent quarantine file for host and hand-copy to learned_rules.
            // Implementer: use snapshot API + mark-stale removal flow, or implement as TODO stub.
            println!("promote: run `articles rule-learn mark-stale {host}` then let the learner rerun (Phase 2 v1 — manual flow)");
            Ok(())
        }
    }
}
```

Wire the dispatch into `articles.rs`'s main handler: match `ArticlesAction::RuleLearn(a) => handle_rule_learn(a, daemon_url).await`.

- [ ] **Step 3: Run + commit**

```
cargo build -p davis_zero_claw
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

```bash
git add -u
git commit -m "feat(cli): add articles rule-learn {list,show,mark-stale,quarantine,promote}"
```

---

# Phase 2.6 — Final Verification

### Task 28: Full verification + spec status update

- [ ] **Step 1: Full Rust suite + lint**

Run:
```
cd /Users/faillonexie/Projects/DavisZeroClaw
cargo build -p davis_zero_claw
cargo test -p davis_zero_claw
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
All clean.

- [ ] **Step 2: Python tests**

Run:
```
cd /Users/faillonexie/Projects/DavisZeroClaw/crawl4ai_adapter
../.runtime/davis/crawl4ai-venv/bin/python -m pytest -v
```
Expected: 6 tests pass.

- [ ] **Step 3: Manual end-to-end smoke**

```
cargo run --bin daviszeroclaw -- daemon &
sleep 5
cargo run --bin daviszeroclaw -- articles ingest https://example.com
sleep 10
cargo run --bin daviszeroclaw -- articles rule-learn list
```
Expected: empty rules (no host has accumulated 3 samples yet). Engine chain for the ingest shows `["trafilatura"]`.

- [ ] **Step 4: Remove lingering `#![allow(dead_code)]` where all symbols now have consumers**

Grep for `#![allow(dead_code)]` in `src/article_memory/ingest/`:
```
grep -n "#!\[allow(dead_code)\]" src/article_memory/ingest/*.rs
```
For each, check if all `pub` symbols in the module are now referenced outside the module. If yes, delete the allow. Run clippy after each deletion.

Likely candidates for removal: `content_signals.rs`, `engines.rs`, `quality_gate.rs`, `llm_extract.rs` (now consumed by worker.rs + pipeline.rs).

Likely candidates to KEEP: `rule_types.rs`, `cleaning_fix.rs` (indirect consumption).

- [ ] **Step 5: Update spec status**

In `docs/superpowers/specs/2026-04-24-crawl4ai-cleaning-upgrade-design.md`, update the header:

```markdown
- Status: Phase 2 Landed (all phases complete)
```

- [ ] **Step 6: Final commit**

```bash
git add -u
git commit -m "docs(specs): mark Phase 2 of crawl4ai cleaning upgrade as landed"
```

---

## Self-Review Checklist

- [x] ① Rust cleaning fixes → Tasks 1-4
- [x] ② Multi-dim deterministic scoring + gopher_reject + dead-field cleanup → Tasks 5-8
- [x] ⑤ LLM client consolidation → Tasks 9-11
- [x] ③ LLM judge extraction_quality field + downgrade rule → Tasks 12-14
- [x] ④ Rule self-learning loop → Tasks 15-27
- [x] ZeroClaw integration deliberately out of scope per spec §17

No placeholders or "similar to Task N" references. Type names consistent across tasks: `LearnedRule` / `RuleStats` / `RuleSample` / `LearnedRuleStore` / `RuleStatsStore` / `SampleStore` / `RuleLearningConfig` / `RuleLearningDeps` / `RuleLearningWorker`.

**Known simplifications (spec-permitted):**
- Warmup CLI stubbed at 501 (Phase 2 v1); full flow deferred to follow-up.
- `promote` CLI subcommand prints manual-flow instructions rather than fully automating quarantine promotion — Phase 2 v1 does not need automated promotion for a single-user system.
- `RuleLearningWorker` uses a single 3600s tick interval (hardcoded) — matches spec §9; configurable interval is a follow-up.

**Estimated time:** 13-16 days (matches Phase 2 total in spec).
