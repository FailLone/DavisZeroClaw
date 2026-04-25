# crawl4ai Cleaning Upgrade — Phase 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a pluggable extraction-engine pipeline (trafilatura + openrouter-llm) behind a deterministic quality gate, delete the hand-written `[[sites]]` cleaning rules, and wire it all into the existing article-memory ingest worker — keeping the same CLI/HTTP surface.

**Architecture:** `crawl4ai_adapter` gains new engine dispatchers (`engines.py`) selected by an `extract_engine` request field. The Rust side adds three pure modules (`engines.rs`, `quality_gate.rs`, `content_signals.rs`) consumed by the existing `IngestWorkerPool`. A failed quality gate upgrades the engine up a ladder (trafilatura → openrouter-llm). `[[sites]]` strategy code and fixtures are deleted.

**Tech Stack:** Rust (tokio, serde, anyhow), Python (FastAPI, pydantic, trafilatura, httpx), existing crawl4ai supervisor + profile locks.

**Reference spec:** `docs/superpowers/specs/2026-04-24-crawl4ai-cleaning-upgrade-design.md` (sections 4, 5, 6, 8, 10, 15, 16).

**Out of scope (Phase 2):** `normalize_line` fix, sliding dedup, multi-dim deterministic scoring, `extraction_quality` LLM field, rule self-learning loop, warmup CLI.

---

## File Structure

**Python** (`crawl4ai_adapter/`):
- Modify: `server.py` — add `extract_engine` field, dispatch to engines
- Create: `engines.py` — `extract_trafilatura`, `extract_openrouter_llm`, shared `ExtractResult` dataclass
- Modify: `server_main.py` — no code change, but install check (dep verification)

**Rust** (`src/article_memory/ingest/`):
- Create: `engines.rs` — `EngineChoice` enum, engine selection, upgrade-ladder
- Create: `quality_gate.rs` — `QualityGate::assess`, `GateResult`, hard/soft fail reasons
- Create: `content_signals.rs` — `ContentSignals`, `compute_signals` (signals used by gate; Phase 2 reuses for scoring)
- Modify: `mod.rs` — new module exports
- Modify: `types.rs` (ingest) — add `engine_chain: Vec<String>` to `IngestJob`
- Modify: `worker.rs` — call `pick_engine` + `QualityGate::assess` + upgrade loop
- Modify: `../../crawl4ai.rs` — `Crawl4aiPageRequest.extract_engine: Option<String>` + CrawlRequestBody wiring
- Modify: `../../app_config.rs` — new `ArticleMemoryExtractConfig`, `QualityGateConfig`
- Modify: `../cleaning_internals.rs` — DELETE site-strategy code paths (but keep `normalize_line` UNTOUCHED — that's Phase 2)
- Modify: `../types.rs` (article_memory root) — delete `ArticleCleaningSiteStrategy`, trim `ResolvedArticleCleaningStrategy`
- Modify: `../config.rs` — strip site-strategy fields from `ArticleCleaningConfig`
- Modify: `../pipeline.rs` — record `engine_chain` in `clean_report`

**Config**:
- Modify: `config/davis/article_memory.toml` — delete all `[[sites]]`, add `[extract]` + `[quality_gate]` sections

**Tests**:
- Create: `src/article_memory/ingest/engines.rs` unit tests (co-located `#[cfg(test)] mod tests`)
- Create: `src/article_memory/ingest/quality_gate.rs` unit tests
- Create: `src/article_memory/ingest/content_signals.rs` unit tests
- Create: `tests/ingest_engine_upgrade_test.rs` — integration via mocked supervisor
- Delete: `tests/article_memory_cleaning_strategy_test.rs` if present
- Update: any golden-file tests that referenced `[[sites]]` strategies

---

## Execution Order

1. **Tasks 1–4**: Python-side engine implementations (parallel-safe after T1)
2. **Tasks 5–8**: Rust engines/gate/signals primitives (pure functions, no I/O)
3. **Tasks 9–11**: Rust config + request-field plumbing
4. **Tasks 12–13**: Rust worker integration + engine_chain recording
5. **Tasks 14–16**: Delete `[[sites]]` code + fixtures + TOML blocks
6. **Task 17**: Integration test
7. **Task 18**: Docs + skill update
8. **Task 19**: Final verification + commit

---

### Task 1: Python — define `ExtractResult` and engine base

**Files:**
- Create: `crawl4ai_adapter/engines.py`
- Test: `crawl4ai_adapter/test_engines.py`

- [ ] **Step 1: Write the failing test**

```python
# crawl4ai_adapter/test_engines.py
from crawl4ai_adapter.engines import ExtractResult


def test_extract_result_has_required_fields():
    r = ExtractResult(markdown="hello", metadata={}, engine="test", warnings=[])
    assert r.markdown == "hello"
    assert r.metadata == {}
    assert r.engine == "test"
    assert r.warnings == []
    assert r.is_empty() is False


def test_extract_result_empty_detection():
    r = ExtractResult(markdown="", metadata={}, engine="test", warnings=["no content"])
    assert r.is_empty() is True
    r2 = ExtractResult(markdown="   \n\t  ", metadata={}, engine="test", warnings=[])
    assert r2.is_empty() is True
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crawl4ai_adapter && ../.runtime/davis/crawl4ai-venv/bin/python -m pytest test_engines.py -v`
Expected: FAIL — ModuleNotFoundError: No module named 'crawl4ai_adapter.engines'

- [ ] **Step 3: Write minimal implementation**

```python
# crawl4ai_adapter/engines.py
"""Pluggable content-extraction engines used by the /crawl endpoint.

Each engine takes HTML + options and returns an `ExtractResult`. Engines are
pure functions (no filesystem, no network for trafilatura; one HTTP call for
openrouter-llm). The dispatcher lives in `server.py`.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Optional


@dataclass
class ExtractResult:
    markdown: str
    metadata: dict[str, Any]
    engine: str
    warnings: list[str] = field(default_factory=list)

    def is_empty(self) -> bool:
        return not self.markdown.strip()
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd crawl4ai_adapter && ../.runtime/davis/crawl4ai-venv/bin/python -m pytest test_engines.py -v`
Expected: both tests PASS

- [ ] **Step 5: Commit**

```bash
git add crawl4ai_adapter/engines.py crawl4ai_adapter/test_engines.py
git commit -m "feat(crawl4ai-adapter): add ExtractResult dataclass for pluggable engines"
```

---

### Task 2: Python — implement `extract_trafilatura`

**Files:**
- Modify: `crawl4ai_adapter/engines.py`
- Modify: `crawl4ai_adapter/test_engines.py`
- Modify: `.runtime/davis/crawl4ai-venv` (add dep)

- [ ] **Step 1: Install trafilatura in the adapter venv**

Run: `.runtime/davis/crawl4ai-venv/bin/pip install trafilatura`
Expected: successfully installed. Capture version in log; pin NOT added (matches crawl4ai's unpinned policy).

- [ ] **Step 2: Write the failing test**

Add to `crawl4ai_adapter/test_engines.py`:

```python
import pytest
from crawl4ai_adapter.engines import extract_trafilatura


SAMPLE_HTML = """<!DOCTYPE html>
<html><head><title>Hello</title></head>
<body>
  <article>
    <h1>My Post</h1>
    <p>This is the first paragraph of the article body.</p>
    <p>This is the second paragraph with more detail about the topic.</p>
    <pre><code>print("hello")</code></pre>
  </article>
  <aside>Sidebar ad — should be dropped</aside>
</body></html>"""


def test_trafilatura_returns_markdown():
    r = extract_trafilatura(SAMPLE_HTML)
    assert r.engine == "trafilatura"
    assert not r.is_empty()
    assert "My Post" in r.markdown
    assert "first paragraph" in r.markdown
    assert "Sidebar ad" not in r.markdown


def test_trafilatura_empty_html_returns_warning():
    r = extract_trafilatura("<html><body></body></html>")
    assert r.is_empty()
    assert r.warnings, "empty extraction should carry a warning"
```

- [ ] **Step 3: Run test to verify failure**

Run: `cd crawl4ai_adapter && ../.runtime/davis/crawl4ai-venv/bin/python -m pytest test_engines.py -v`
Expected: FAIL — ImportError: cannot import name 'extract_trafilatura'

- [ ] **Step 4: Implement**

Append to `crawl4ai_adapter/engines.py`:

```python
def extract_trafilatura(html: str) -> ExtractResult:
    """Extract main content using trafilatura.

    Returns markdown (output_format='markdown') with comments, tables, and
    formatting preserved. Links and images are kept as they contain signal
    the downstream value judge may want. Falls back to a warning-tagged
    empty result when trafilatura cannot find a main block.
    """
    import trafilatura  # lazy import — heavy dep, only loaded when selected

    markdown = trafilatura.extract(
        html,
        output_format="markdown",
        include_comments=False,
        include_tables=True,
        include_formatting=True,
        include_links=True,
        include_images=True,
        favor_precision=True,  # prefer dropping noise over keeping everything
    )
    metadata_obj = trafilatura.extract_metadata(html)
    metadata: dict[str, Any] = {}
    if metadata_obj is not None:
        if metadata_obj.title:
            metadata["title"] = metadata_obj.title
        if metadata_obj.author:
            metadata["author"] = metadata_obj.author
        if metadata_obj.date:
            metadata["published_time"] = metadata_obj.date
        if metadata_obj.sitename:
            metadata["site_name"] = metadata_obj.sitename

    warnings: list[str] = []
    if not markdown or not markdown.strip():
        warnings.append("trafilatura produced no main content")
        markdown = ""
    return ExtractResult(
        markdown=markdown,
        metadata=metadata,
        engine="trafilatura",
        warnings=warnings,
    )
```

- [ ] **Step 5: Run tests to verify pass**

Run: `cd crawl4ai_adapter && ../.runtime/davis/crawl4ai-venv/bin/python -m pytest test_engines.py -v`
Expected: 4 tests PASS

- [ ] **Step 6: Commit**

```bash
git add crawl4ai_adapter/engines.py crawl4ai_adapter/test_engines.py
git commit -m "feat(crawl4ai-adapter): add trafilatura extraction engine"
```

---

### Task 3: Python — implement `extract_openrouter_llm`

**Files:**
- Modify: `crawl4ai_adapter/engines.py`
- Modify: `crawl4ai_adapter/test_engines.py`

- [ ] **Step 1: Write the failing test (mocked HTTP)**

Add to `crawl4ai_adapter/test_engines.py`:

```python
from unittest.mock import patch, MagicMock


def test_openrouter_llm_returns_markdown_from_choices():
    fake_response = MagicMock()
    fake_response.status_code = 200
    fake_response.json.return_value = {
        "choices": [{"message": {"content": "# Title\n\nBody text."}}]
    }
    fake_response.raise_for_status = lambda: None

    with patch("httpx.Client") as client_cls:
        client_cls.return_value.__enter__.return_value.post.return_value = fake_response
        r = extract_openrouter_llm(
            "<html><body><p>hi</p></body></html>",
            {
                "base_url": "https://openrouter.ai/api/v1",
                "api_key": "sk-test",
                "model": "google/gemini-2.0-flash-001",
                "timeout_secs": 30,
            },
        )
    assert r.engine == "openrouter-llm"
    assert r.markdown.startswith("# Title")
    assert r.warnings == []


def test_openrouter_llm_surfaces_http_failure_as_warning():
    fake_response = MagicMock()
    fake_response.status_code = 500
    fake_response.text = "upstream error"
    def raise_for_status():
        import httpx
        raise httpx.HTTPStatusError("500", request=MagicMock(), response=fake_response)
    fake_response.raise_for_status = raise_for_status

    with patch("httpx.Client") as client_cls:
        client_cls.return_value.__enter__.return_value.post.return_value = fake_response
        r = extract_openrouter_llm(
            "<html></html>",
            {"base_url": "x", "api_key": "y", "model": "z", "timeout_secs": 30},
        )
    assert r.is_empty()
    assert any("openrouter-llm" in w for w in r.warnings)
```

Also add import at top:
```python
from crawl4ai_adapter.engines import (
    ExtractResult,
    extract_openrouter_llm,
    extract_trafilatura,
)
```

- [ ] **Step 2: Run tests — expect failure**

Run: `cd crawl4ai_adapter && ../.runtime/davis/crawl4ai-venv/bin/python -m pytest test_engines.py -v`
Expected: FAIL — ImportError: cannot import name 'extract_openrouter_llm'

- [ ] **Step 3: Implement**

Append to `crawl4ai_adapter/engines.py`:

```python
_OPENROUTER_SYSTEM_PROMPT = (
    "You are a precise HTML-to-Markdown converter. Given raw HTML, extract ONLY "
    "the main article body as well-structured Markdown. Preserve: headings "
    "(use #/##/###), lists, code blocks (use ``` fences with language when "
    "recognizable), tables, links, block quotes. Remove: navigation, sidebars, "
    "comments, cookie banners, share buttons, related-article lists, ads, and "
    "all other UI chrome. Do not summarize. Do not add content. If no article "
    "body is present, return an empty response."
)


def extract_openrouter_llm(html: str, config: dict[str, Any]) -> ExtractResult:
    """Call an OpenRouter-hosted LLM to convert HTML to clean Markdown.

    `config` keys: base_url, api_key, model, timeout_secs, max_input_chars (opt).
    Truncates HTML to `max_input_chars` (default 60_000) to bound cost.
    """
    import httpx  # lazy import

    base_url = config["base_url"].rstrip("/")
    api_key = config["api_key"]
    model = config["model"]
    timeout_secs = int(config.get("timeout_secs", 60))
    max_input_chars = int(config.get("max_input_chars", 60_000))

    truncated = html[:max_input_chars]
    payload = {
        "model": model,
        "messages": [
            {"role": "system", "content": _OPENROUTER_SYSTEM_PROMPT},
            {"role": "user", "content": f"Convert this HTML to Markdown:\n\n{truncated}"},
        ],
        "temperature": 0.0,
    }
    headers = {
        "Authorization": f"Bearer {api_key}",
        "Content-Type": "application/json",
    }

    try:
        with httpx.Client(timeout=timeout_secs) as client:
            resp = client.post(f"{base_url}/chat/completions", headers=headers, json=payload)
            resp.raise_for_status()
            data = resp.json()
    except Exception as exc:  # noqa: BLE001 — we want all failure classes surfaced
        return ExtractResult(
            markdown="",
            metadata={},
            engine="openrouter-llm",
            warnings=[f"openrouter-llm request failed: {exc}"],
        )

    try:
        markdown = data["choices"][0]["message"]["content"] or ""
    except (KeyError, IndexError, TypeError):
        return ExtractResult(
            markdown="",
            metadata={},
            engine="openrouter-llm",
            warnings=["openrouter-llm response missing choices[0].message.content"],
        )

    warnings: list[str] = []
    if not markdown.strip():
        warnings.append("openrouter-llm returned empty content")
    return ExtractResult(
        markdown=markdown.strip(),
        metadata={},
        engine="openrouter-llm",
        warnings=warnings,
    )
```

- [ ] **Step 4: Verify**

Run: `cd crawl4ai_adapter && ../.runtime/davis/crawl4ai-venv/bin/python -m pytest test_engines.py -v`
Expected: 6 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crawl4ai_adapter/engines.py crawl4ai_adapter/test_engines.py
git commit -m "feat(crawl4ai-adapter): add openrouter-llm extraction engine"
```

---

### Task 4: Python — wire engines into `/crawl`

**Files:**
- Modify: `crawl4ai_adapter/server.py`
- Test: live smoke via integration test (task 17)

- [ ] **Step 1: Extend `CrawlRequest`**

Modify `crawl4ai_adapter/server.py` — `CrawlRequest` class:

```python
class CrawlRequest(BaseModel):
    profile_path: str = Field(..., description="Absolute path to Chromium user_data_dir")
    url: str
    wait_for: Optional[str] = None
    js_code: Optional[str] = None
    timeout_secs: int = 90
    headless: bool = True
    magic: bool = True
    simulate_user: bool = True
    override_navigator: bool = True
    remove_overlay_elements: bool = True
    enable_stealth: bool = True
    markdown_generator: bool = False
    content_filter: Optional[str] = None
    # NEW — Phase 1
    extract_engine: Optional[str] = None  # "pruning" | "trafilatura" | "openrouter-llm"
    openrouter_config: Optional[dict[str, Any]] = None  # required when engine=openrouter-llm
```

- [ ] **Step 2: Dispatch in `/crawl` after crawl completes**

In `server.py`, locate the block starting with `response_markdown = None` (around line 195). REPLACE that block through just before `response_metadata = getattr(...)` with:

```python
    # Engine dispatch. `pruning` keeps the existing crawl4ai flow (markdown already
    # resolved below). For trafilatura / openrouter-llm, re-run extraction on the
    # raw HTML regardless of crawl4ai's own markdown_generator setting.
    response_markdown: Optional[str] = None
    extra_warnings: list[str] = []
    engine_used = req.extract_engine or ("pruning" if req.markdown_generator else None)

    if engine_used == "pruning":
        markdown_v2 = getattr(result, "markdown_v2", None)
        if markdown_v2 is not None:
            response_markdown = getattr(markdown_v2, "fit_markdown", None) or getattr(
                markdown_v2, "raw_markdown", None
            )
        if response_markdown is None:
            response_markdown = getattr(result, "markdown", None)
    elif engine_used == "trafilatura":
        from crawl4ai_adapter.engines import extract_trafilatura
        raw_html = getattr(result, "html", None) or ""
        er = extract_trafilatura(raw_html)
        response_markdown = er.markdown or None
        extra_warnings.extend(er.warnings)
    elif engine_used == "openrouter-llm":
        from crawl4ai_adapter.engines import extract_openrouter_llm
        raw_html = getattr(result, "html", None) or ""
        if not req.openrouter_config:
            raise HTTPException(
                status_code=400,
                detail={"error": "missing_openrouter_config", "engine": engine_used},
            )
        er = extract_openrouter_llm(raw_html, req.openrouter_config)
        response_markdown = er.markdown or None
        extra_warnings.extend(er.warnings)
    # engine_used is None → no markdown requested, leave response_markdown=None
```

- [ ] **Step 3: Thread `extra_warnings` into the response**

Find the `return CrawlResponse(...)` at the end of `/crawl`. Add a `warnings` field pass-through by modifying the `CrawlResponse` class at the top of the file:

```python
class CrawlResponse(BaseModel):
    success: bool
    url: Optional[str] = None
    redirected_url: Optional[str] = None
    status_code: Optional[int] = None
    html: Optional[str] = None
    cleaned_html: Optional[str] = None
    js_execution_result: Optional[Any] = None
    error_message: Optional[str] = None
    markdown: Optional[str] = None
    metadata: Optional[dict[str, Any]] = None
    engine: Optional[str] = None       # NEW
    warnings: list[str] = []            # NEW
```

And in the `return CrawlResponse(...)` line append:

```python
        engine=engine_used,
        warnings=extra_warnings,
```

- [ ] **Step 4: Manual spot-check — fire up the server**

Run: `.runtime/davis/crawl4ai-venv/bin/uvicorn crawl4ai_adapter.server:app --port 18765 --log-level warning &`

In another terminal:

```bash
curl -sS -X POST http://127.0.0.1:18765/crawl \
  -H 'content-type: application/json' \
  -d '{"profile_path":"/tmp/adp-smoke","url":"https://example.com","extract_engine":"trafilatura","timeout_secs":30}' \
  | python -m json.tool | head -40
```

Expected: JSON with `"engine": "trafilatura"`, non-empty `"markdown"`, `"warnings": []`. Kill the server after: `kill %1`.

- [ ] **Step 5: Commit**

```bash
git add crawl4ai_adapter/server.py
git commit -m "feat(crawl4ai-adapter): dispatch /crawl to pluggable engines (pruning|trafilatura|openrouter-llm)"
```

---

### Task 5: Rust — `ContentSignals` pure module

**Files:**
- Create: `src/article_memory/ingest/content_signals.rs`
- Modify: `src/article_memory/ingest/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `src/article_memory/ingest/content_signals.rs`:

```rust
//! Content-level statistical signals computed from extracted markdown.
//!
//! Phase 1 uses these only inside the quality gate. Phase 2 will also feed
//! them into the deterministic value score.

#[derive(Debug, Clone, PartialEq)]
pub struct ContentSignals {
    pub total_chars: usize,
    pub code_density: f32,
    pub link_density: f32,
    pub heading_depth: usize,
    pub heading_count: usize,
    pub paragraph_count: usize,
    pub list_ratio: f32,
    pub avg_paragraph_chars: usize,
    pub alpha_ratio: f32,
    pub symbol_ratio: f32,
}

pub fn compute_signals(markdown: &str) -> ContentSignals {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_markdown_yields_zero_signals() {
        let s = compute_signals("");
        assert_eq!(s.total_chars, 0);
        assert_eq!(s.paragraph_count, 0);
        assert_eq!(s.heading_depth, 0);
    }

    #[test]
    fn paragraph_count_splits_on_blank_lines() {
        let md = "first para\n\nsecond para\n\nthird para";
        let s = compute_signals(md);
        assert_eq!(s.paragraph_count, 3);
        assert!(s.avg_paragraph_chars >= 9);
    }

    #[test]
    fn heading_depth_tracks_max_level() {
        let md = "# H1\n\n## H2\n\n#### H4\n\nbody";
        let s = compute_signals(md);
        assert_eq!(s.heading_depth, 4);
        assert_eq!(s.heading_count, 3);
    }

    #[test]
    fn code_density_counts_fenced_and_inline() {
        let md = "prose prose prose\n\n```\nprint(x)\n```\n\nmore `code` here";
        let s = compute_signals(md);
        assert!(s.code_density > 0.0);
        assert!(s.code_density < 1.0);
    }

    #[test]
    fn link_density_counts_markdown_links() {
        let md = "one [link](http://a) two [link](http://b) words words";
        let s = compute_signals(md);
        assert!(s.link_density > 0.0);
    }

    #[test]
    fn list_ratio_measures_list_lines() {
        let md = "- a\n- b\n- c\n\ntext";
        let s = compute_signals(md);
        assert!(s.list_ratio > 0.5);
    }

    #[test]
    fn alpha_ratio_detects_symbol_heavy() {
        let md = "############!!!!!!!!########";
        let s = compute_signals(md);
        assert!(s.alpha_ratio < 0.1);
        assert!(s.symbol_ratio > 0.1);
    }
}
```

Add to `src/article_memory/ingest/mod.rs`:

```rust
mod content_signals;
pub use content_signals::{compute_signals, ContentSignals};
```

- [ ] **Step 2: Run tests — expect failure**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::content_signals -- --nocapture`
Expected: FAIL — `not yet implemented` from `todo!()`.

- [ ] **Step 3: Implement `compute_signals`**

Replace the `todo!()` in `content_signals.rs` with:

```rust
pub fn compute_signals(markdown: &str) -> ContentSignals {
    let total_chars = markdown.chars().count();
    if total_chars == 0 {
        return ContentSignals {
            total_chars: 0,
            code_density: 0.0,
            link_density: 0.0,
            heading_depth: 0,
            heading_count: 0,
            paragraph_count: 0,
            list_ratio: 0.0,
            avg_paragraph_chars: 0,
            alpha_ratio: 0.0,
            symbol_ratio: 0.0,
        };
    }

    // Code density: fenced code block chars + inline-code chars
    let mut code_chars: usize = 0;
    let mut in_fence = false;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            code_chars += line.chars().count() + 1; // +1 for newline
            continue;
        }
        if in_fence {
            code_chars += line.chars().count() + 1;
        }
    }
    // Inline code (simple backtick counter; excludes lines already in fences).
    let inline_code_chars = count_inline_code_chars(markdown);
    code_chars += inline_code_chars;

    let code_density = (code_chars as f32 / total_chars as f32).clamp(0.0, 1.0);

    // Heading stats
    let mut heading_depth = 0usize;
    let mut heading_count = 0usize;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes)
            && trimmed
                .chars()
                .nth(hashes)
                .map(|c| c == ' ')
                .unwrap_or(false)
        {
            heading_count += 1;
            if hashes > heading_depth {
                heading_depth = hashes;
            }
        }
    }

    // Paragraphs: non-empty blocks separated by blank lines.
    let paragraphs: Vec<&str> = markdown
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let paragraph_count = paragraphs.len();
    let avg_paragraph_chars = if paragraph_count == 0 {
        0
    } else {
        paragraphs
            .iter()
            .map(|p| p.chars().count())
            .sum::<usize>()
            / paragraph_count
    };

    // Link density: number of [..](..) matches / word count
    let link_count = count_markdown_links(markdown);
    let word_count = markdown.split_whitespace().count().max(1);
    let link_density = (link_count as f32 / word_count as f32).clamp(0.0, 1.0);

    // List ratio
    let lines: Vec<&str> = markdown.lines().collect();
    let total_lines = lines.len().max(1);
    let list_lines = lines
        .iter()
        .filter(|line| {
            let t = line.trim_start();
            t.starts_with("- ")
                || t.starts_with("* ")
                || t.starts_with("+ ")
                || starts_with_numbered_list(t)
        })
        .count();
    let list_ratio = list_lines as f32 / total_lines as f32;

    // Char-class ratios
    let mut alpha = 0usize;
    let mut symbol = 0usize;
    for ch in markdown.chars() {
        if ch.is_alphabetic() {
            alpha += 1;
        } else if "!@#$%^&*()[]{}|\\/<>~`".contains(ch) {
            symbol += 1;
        }
    }
    let alpha_ratio = alpha as f32 / total_chars as f32;
    let symbol_ratio = symbol as f32 / total_chars as f32;

    ContentSignals {
        total_chars,
        code_density,
        link_density,
        heading_depth,
        heading_count,
        paragraph_count,
        list_ratio,
        avg_paragraph_chars,
        alpha_ratio,
        symbol_ratio,
    }
}

fn count_inline_code_chars(markdown: &str) -> usize {
    let mut total = 0usize;
    let mut chars = markdown.chars().peekable();
    let mut in_fence = false;
    let mut fence_buf = String::new();
    while let Some(c) = chars.next() {
        fence_buf.push(c);
        if fence_buf.ends_with("```") {
            in_fence = !in_fence;
            fence_buf.clear();
        }
        if fence_buf.len() > 3 {
            fence_buf.drain(..1);
        }
        if in_fence {
            continue;
        }
        if c == '`' {
            // capture until next `
            let mut run = 0usize;
            for nc in chars.by_ref() {
                if nc == '`' {
                    break;
                }
                run += 1;
            }
            total += run;
        }
    }
    total
}

fn count_markdown_links(markdown: &str) -> usize {
    // Naive: count "](http" occurrences. Good enough for density estimation.
    markdown.matches("](").count()
}

fn starts_with_numbered_list(line: &str) -> bool {
    let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0
        && line
            .chars()
            .nth(digits)
            .map(|c| c == '.')
            .unwrap_or(false)
        && line
            .chars()
            .nth(digits + 1)
            .map(|c| c == ' ')
            .unwrap_or(false)
}
```

- [ ] **Step 4: Verify**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::content_signals`
Expected: 7 tests PASS

- [ ] **Step 5: Lint check**

Run: `cargo clippy -p davis_zero_claw --lib -- -D warnings`
Expected: clean

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/ingest/content_signals.rs src/article_memory/ingest/mod.rs
git commit -m "feat(article-memory): add ContentSignals computation (Phase 1 groundwork)"
```

---

### Task 6: Rust — `QualityGate` module

**Files:**
- Create: `src/article_memory/ingest/quality_gate.rs`
- Modify: `src/article_memory/ingest/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `src/article_memory/ingest/quality_gate.rs`:

```rust
//! Deterministic post-extraction quality gate.
//!
//! Hard-fails: statistical signs of catastrophic extraction (empty,
//! low kept ratio, almost no paragraphs, link-soup). Triggers engine
//! upgrade AND captures HTML for future rule-learning (Phase 2).
//!
//! Soft-fails: structural signs (code flattened, no headings when HTML
//! had them). Triggers engine upgrade only.

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
        let md = "# Title\n\nFirst paragraph with enough length to matter.\n\nSecond paragraph here with more text to exceed the minimum char budget in this small config.";
        let r = assess(md, 5000, &cfg());
        assert!(r.pass, "reasons: hard={:?} soft={:?}", r.hard_fail_reasons, r.soft_fail_reasons);
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
        let md: String = (0..20)
            .map(|i| format!("[l{i}](http://x.example/{i})"))
            .collect::<Vec<_>>()
            .join(" ");
        let md_body = format!("Para one.\n\n{md}\n\nPara two.\n\nPara three with more content here so min paragraphs holds.");
        let r = assess(&md_body, 5000, &cfg());
        assert!(r.hard_fail_reasons.contains(&"link_density_too_high"));
    }

    #[test]
    fn boilerplate_marker_soft_fails() {
        let md = "# Ok\n\nSome body.\n\n请订阅我们的频道。\n\nAnother paragraph long enough here.";
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
```

Add to `mod.rs`:

```rust
mod quality_gate;
pub use quality_gate::{assess as assess_quality, GateResult, QualityGateConfig};
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::quality_gate`
Expected: 6 tests PASS

- [ ] **Step 3: Clippy**

Run: `cargo clippy -p davis_zero_claw --lib -- -D warnings`
Expected: clean

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/ingest/quality_gate.rs src/article_memory/ingest/mod.rs
git commit -m "feat(article-memory): add quality gate with hard/soft fail signals"
```

---

### Task 7: Rust — `EngineChoice` + engine module

**Files:**
- Create: `src/article_memory/ingest/engines.rs`
- Modify: `src/article_memory/ingest/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `src/article_memory/ingest/engines.rs`:

```rust
//! Extraction engine selection + upgrade ladder.
//!
//! Phase 1 engines: trafilatura (default), openrouter-llm (fallback).
//! `pruning` is recognised to stay compatible with callers that still request
//! it, but is deprecated and removed in Phase 2.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineChoice {
    Trafilatura,
    OpenRouterLlm,
    Pruning, // deprecated; retained for migration window
}

impl EngineChoice {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trafilatura => "trafilatura",
            Self::OpenRouterLlm => "openrouter-llm",
            Self::Pruning => "pruning",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
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
            default_engine: EngineChoice::Trafilatura,
            fallback_ladder: vec![EngineChoice::Trafilatura, EngineChoice::OpenRouterLlm],
        }
    }
}

/// Pick the starting engine. Phase 1: always the config default. Phase 2
/// will add learned-rules lookup before falling through.
pub fn pick_engine(config: &ExtractEngineConfig) -> EngineChoice {
    config.default_engine.clone()
}

/// Given the current engine and the ladder, return the next engine to try,
/// or `None` if exhausted.
pub fn next_engine(current: &EngineChoice, ladder: &[EngineChoice]) -> Option<EngineChoice> {
    let pos = ladder.iter().position(|e| e == current)?;
    ladder.get(pos + 1).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_engine_returns_default() {
        let c = ExtractEngineConfig::default();
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
            EngineChoice::Trafilatura,
            EngineChoice::OpenRouterLlm,
            EngineChoice::Pruning,
        ] {
            assert_eq!(EngineChoice::from_str(e.as_str()), Some(e));
        }
        assert_eq!(EngineChoice::from_str("nope"), None);
    }
}
```

Add to `mod.rs`:

```rust
mod engines;
pub use engines::{next_engine, pick_engine, EngineChoice, ExtractEngineConfig};
```

- [ ] **Step 2: Test**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest::engines`
Expected: 4 tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/article_memory/ingest/engines.rs src/article_memory/ingest/mod.rs
git commit -m "feat(article-memory): add EngineChoice enum and upgrade-ladder helpers"
```

---

### Task 8: Rust — wire `extract_engine` through `Crawl4aiPageRequest`

**Files:**
- Modify: `src/crawl4ai.rs`

- [ ] **Step 1: Extend the request struct**

In `src/crawl4ai.rs`, replace the `Crawl4aiPageRequest` definition (lines 6–14) with:

```rust
#[derive(Debug, Clone)]
pub struct Crawl4aiPageRequest {
    pub profile_name: String,
    pub url: String,
    pub wait_for: Option<String>,
    pub js_code: Option<String>,
    /// When true, request crawl4ai to produce fit-filtered Markdown.
    /// Deprecated: set `extract_engine = Some("pruning")` instead. Kept for
    /// backward-compat during Phase 1 migration.
    pub markdown: bool,
    /// Explicit engine selection; overrides `markdown`. Values:
    /// `"pruning"` | `"trafilatura"` | `"openrouter-llm"`.
    pub extract_engine: Option<String>,
    /// Required when `extract_engine == Some("openrouter-llm")`.
    pub openrouter_config: Option<serde_json::Value>,
}
```

- [ ] **Step 2: Extend `CrawlRequestBody`**

Replace `CrawlRequestBody` (lines 29–44) with:

```rust
#[derive(Serialize)]
struct CrawlRequestBody<'a> {
    profile_path: String,
    url: &'a str,
    wait_for: Option<&'a str>,
    js_code: Option<&'a str>,
    timeout_secs: u64,
    headless: bool,
    magic: bool,
    simulate_user: bool,
    override_navigator: bool,
    remove_overlay_elements: bool,
    enable_stealth: bool,
    markdown_generator: bool,
    content_filter: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extract_engine: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    openrouter_config: Option<&'a serde_json::Value>,
}
```

- [ ] **Step 3: Populate new fields in `crawl4ai_crawl`**

In the `let body = CrawlRequestBody { ... }` block (lines 70–88), replace with:

```rust
    let (markdown_generator, content_filter, extract_engine) = match request.extract_engine.as_deref() {
        Some(engine) => (true, None, Some(engine)),
        None if request.markdown => (true, Some("pruning"), Some("pruning")),
        None => (false, None, None),
    };

    let body = CrawlRequestBody {
        profile_path: profile_dir.display().to_string(),
        url: &request.url,
        wait_for: request.wait_for.as_deref(),
        js_code: request.js_code.as_deref(),
        timeout_secs: config.timeout_secs,
        headless: config.headless,
        magic: config.magic,
        simulate_user: config.simulate_user,
        override_navigator: config.override_navigator,
        remove_overlay_elements: config.remove_overlay_elements,
        enable_stealth: config.enable_stealth,
        markdown_generator,
        content_filter,
        extract_engine,
        openrouter_config: request.openrouter_config.as_ref(),
    };
```

- [ ] **Step 4: Update all callers**

Search for `Crawl4aiPageRequest {` to find construction sites:

Run: `grep -rn "Crawl4aiPageRequest {" src/ tests/`

For EACH match, add `extract_engine: None, openrouter_config: None,` to the struct literal. Known sites per baseline: `src/express.rs` (authenticated express flow), `src/article_memory/ingest/worker.rs` (will be overwritten in Task 13 — do minimal fix now to keep the build green).

- [ ] **Step 5: Verify the crate compiles**

Run: `cargo check -p davis_zero_claw`
Expected: clean build.

- [ ] **Step 6: Tests still pass**

Run: `cargo test -p davis_zero_claw --lib`
Expected: all existing tests still pass (no behavioral change yet).

- [ ] **Step 7: Commit**

```bash
git add src/crawl4ai.rs src/express.rs src/article_memory/ingest/worker.rs
git commit -m "feat(crawl4ai): add extract_engine and openrouter_config to Crawl4aiPageRequest"
```

---

### Task 9: Rust — new config types

**Files:**
- Modify: `src/app_config.rs`

- [ ] **Step 1: Write failing tests**

Add at the bottom of `src/app_config.rs` inside the existing `#[cfg(test)] mod tests { ... }` block (after the existing `ArticleMemoryConfig` tests):

```rust
    #[test]
    fn article_memory_extract_defaults_to_trafilatura() {
        let toml = r#"
            [extract]
        "#;
        let cfg: ArticleMemoryExtractConfig = toml::from_str(toml).unwrap().extract;
        assert_eq!(cfg.default_engine, "trafilatura");
        assert_eq!(cfg.fallback_ladder, vec!["trafilatura", "openrouter-llm"]);
    }

    #[test]
    fn quality_gate_defaults_are_sane() {
        let toml = "";
        let cfg: QualityGateConfigTomlWrapper = toml::from_str(toml).unwrap();
        assert!(cfg.quality_gate.enabled);
        assert_eq!(cfg.quality_gate.min_markdown_chars, 500);
    }

    #[derive(serde::Deserialize)]
    struct QualityGateConfigTomlWrapper {
        #[serde(default)]
        quality_gate: QualityGateToml,
    }
```

NOTE: these test wrappers depend on types created in Step 3 below; that's OK — Step 2 will make them fail first.

- [ ] **Step 2: Run tests — expect failure**

Run: `cargo test -p davis_zero_claw --lib app_config::tests`
Expected: FAIL — unresolved types.

- [ ] **Step 3: Add config types**

In `src/app_config.rs`, add AFTER the `ArticleMemoryIngestConfig` definition:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArticleMemoryExtractConfig {
    #[serde(default = "default_extract_engine")]
    pub default_engine: String,
    #[serde(default = "default_fallback_ladder")]
    pub fallback_ladder: Vec<String>,
    #[serde(default)]
    pub openrouter_llm: OpenRouterLlmEngineConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct OpenRouterLlmEngineConfig {
    #[serde(default)]
    pub provider: String,       // looked up in [[providers]] by name
    #[serde(default = "default_openrouter_llm_model")]
    pub model: String,
    #[serde(default = "default_openrouter_llm_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_openrouter_llm_max_input_chars")]
    pub max_input_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityGateToml {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_gate_min_markdown_chars")]
    pub min_markdown_chars: usize,
    #[serde(default = "default_gate_min_kept_ratio")]
    pub min_kept_ratio: f32,
    #[serde(default = "default_gate_min_paragraphs")]
    pub min_paragraphs: usize,
    #[serde(default = "default_gate_max_link_density")]
    pub max_link_density: f32,
    #[serde(default)]
    pub boilerplate_markers: Vec<String>,
}

impl Default for QualityGateToml {
    fn default() -> Self {
        Self {
            enabled: true,
            min_markdown_chars: default_gate_min_markdown_chars(),
            min_kept_ratio: default_gate_min_kept_ratio(),
            min_paragraphs: default_gate_min_paragraphs(),
            max_link_density: default_gate_max_link_density(),
            boilerplate_markers: Vec::new(),
        }
    }
}

impl Default for ArticleMemoryExtractConfig {
    fn default() -> Self {
        Self {
            default_engine: default_extract_engine(),
            fallback_ladder: default_fallback_ladder(),
            openrouter_llm: OpenRouterLlmEngineConfig::default(),
        }
    }
}

fn default_extract_engine() -> String {
    "trafilatura".to_string()
}

fn default_fallback_ladder() -> Vec<String> {
    vec!["trafilatura".to_string(), "openrouter-llm".to_string()]
}

fn default_openrouter_llm_model() -> String {
    "google/gemini-2.0-flash-001".to_string()
}

fn default_openrouter_llm_timeout_secs() -> u64 {
    60
}

fn default_openrouter_llm_max_input_chars() -> usize {
    60_000
}

fn default_gate_min_markdown_chars() -> usize {
    500
}
fn default_gate_min_kept_ratio() -> f32 {
    0.05
}
fn default_gate_min_paragraphs() -> usize {
    3
}
fn default_gate_max_link_density() -> f32 {
    0.5
}
```

- [ ] **Step 4: Add fields to `ArticleMemoryConfig`**

Locate `pub struct ArticleMemoryConfig {` (around line 160). Add two new fields:

```rust
    #[serde(default)]
    pub extract: ArticleMemoryExtractConfig,
    #[serde(default)]
    pub quality_gate: QualityGateToml,
```

Make sure `#[derive(..., Default)]` works — if `ArticleMemoryConfig` doesn't already derive `Default`, skip that; the outer config uses `#[serde(default)]` attribute-level instead.

- [ ] **Step 5: Verify**

Run: `cargo test -p davis_zero_claw --lib app_config::tests`
Expected: all tests pass including the two new ones.

Run: `cargo clippy -p davis_zero_claw --lib -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/app_config.rs
git commit -m "feat(config): add ArticleMemoryExtractConfig and QualityGateToml"
```

---

### Task 10: Rust — add `engine_chain` to `IngestJob`

**Files:**
- Modify: `src/article_memory/ingest/types.rs`

- [ ] **Step 1: Extend the struct**

In `src/article_memory/ingest/types.rs`, inside `pub struct IngestJob`, ADD after the `warnings` field (around line 89):

```rust
    #[serde(default)]
    pub engine_chain: Vec<String>,
```

- [ ] **Step 2: Update any constructors**

Run: `grep -n "IngestJob {" src/article_memory/ingest/*.rs tests/`

For each match, add `engine_chain: Vec::new(),` to the struct literal. At minimum: `queue.rs` (submit creates a new job).

- [ ] **Step 3: Verify build + existing tests pass**

Run: `cargo test -p davis_zero_claw --lib article_memory::ingest`
Expected: all ingest tests pass. `engine_chain` serializes as `[]` when empty; old JSON files without the field parse due to `#[serde(default)]`.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/ingest/types.rs src/article_memory/ingest/queue.rs
git commit -m "feat(article-memory): add engine_chain to IngestJob for engine-upgrade telemetry"
```

---

### Task 11: Rust — lift config onto the worker dep bundle

**Files:**
- Modify: `src/article_memory/ingest/worker.rs`

- [ ] **Step 1: Extend `IngestWorkerDeps`**

In `src/article_memory/ingest/worker.rs`, modify the `IngestWorkerDeps` struct (lines 19–29) to add two fields:

```rust
#[derive(Clone)]
pub struct IngestWorkerDeps {
    pub paths: RuntimePaths,
    pub crawl4ai_config: Arc<Crawl4aiConfig>,
    pub supervisor: Arc<Crawl4aiSupervisor>,
    pub profile_locks: Crawl4aiProfileLocks,
    pub article_memory_config: Arc<ArticleMemoryConfig>,
    pub providers: Arc<Vec<ModelProviderConfig>>,
    pub ingest_config: Arc<ArticleMemoryIngestConfig>,
    pub imessage_config: Arc<ImessageConfig>,
    pub extract_config: Arc<ArticleMemoryExtractConfig>, // NEW
    pub quality_gate_config: Arc<QualityGateToml>,       // NEW
}
```

- [ ] **Step 2: Update the caller that builds deps**

Run: `grep -rn "IngestWorkerDeps {" src/`

There should be one construction site in `src/server.rs` or `src/article_memory/` daemon boot. Add the two new fields to that constructor, sourcing from `article_memory_config.extract` and `article_memory_config.quality_gate`:

```rust
    extract_config: Arc::new(local.article_memory.extract.clone()),
    quality_gate_config: Arc::new(local.article_memory.quality_gate.clone()),
```

Add `use crate::app_config::{ArticleMemoryExtractConfig, QualityGateToml};` imports to `worker.rs` and any construction sites.

- [ ] **Step 3: Verify**

Run: `cargo check -p davis_zero_claw`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/article_memory/ingest/worker.rs src/server.rs
git commit -m "feat(article-memory): thread extract and quality-gate config to worker deps"
```

---

### Task 12: Rust — first-cut engine dispatch in worker

**Files:**
- Modify: `src/article_memory/ingest/worker.rs`

- [ ] **Step 1: Add an engine-aware crawl helper**

At the TOP of `src/article_memory/ingest/worker.rs` (just under the existing `use` block), add:

```rust
use super::engines::{next_engine, pick_engine, EngineChoice, ExtractEngineConfig};
use super::quality_gate::{assess as assess_quality, QualityGateConfig};
use crate::app_config::{ArticleMemoryExtractConfig, OpenRouterLlmEngineConfig, QualityGateToml};
```

At the BOTTOM of the file, add:

```rust
fn engine_config_from_toml(extract: &ArticleMemoryExtractConfig) -> ExtractEngineConfig {
    let default_engine = EngineChoice::from_str(&extract.default_engine)
        .unwrap_or(EngineChoice::Trafilatura);
    let ladder: Vec<EngineChoice> = extract
        .fallback_ladder
        .iter()
        .filter_map(|s| EngineChoice::from_str(s))
        .collect();
    ExtractEngineConfig {
        default_engine,
        fallback_ladder: if ladder.is_empty() {
            vec![EngineChoice::Trafilatura, EngineChoice::OpenRouterLlm]
        } else {
            ladder
        },
    }
}

fn quality_gate_config_from_toml(gate: &QualityGateToml) -> QualityGateConfig {
    QualityGateConfig {
        enabled: gate.enabled,
        min_markdown_chars: gate.min_markdown_chars,
        min_kept_ratio: gate.min_kept_ratio,
        min_paragraphs: gate.min_paragraphs,
        max_link_density: gate.max_link_density,
        boilerplate_markers: gate.boilerplate_markers.clone(),
    }
}

fn openrouter_config_for(
    providers: &[crate::app_config::ModelProviderConfig],
    engine_cfg: &OpenRouterLlmEngineConfig,
) -> Option<serde_json::Value> {
    let provider = providers.iter().find(|p| p.name == engine_cfg.provider)?;
    Some(serde_json::json!({
        "base_url": provider.base_url,
        "api_key": provider.api_key,
        "model": engine_cfg.model,
        "timeout_secs": engine_cfg.timeout_secs,
        "max_input_chars": engine_cfg.max_input_chars,
    }))
}
```

- [ ] **Step 2: Replace the fetch block in `execute_job_core`**

Locate in `worker.rs` the `// Stage 1: fetch` block (lines 125–156). Replace the whole block (from `// Stage 1: fetch` up to and including the bare `let markdown = match page.markdown.as_deref() { ... };` that immediately follows) with:

```rust
    // Stage 1: fetch with engine ladder + quality gate
    let engine_cfg = engine_config_from_toml(&deps.extract_config);
    let gate_cfg = quality_gate_config_from_toml(&deps.quality_gate_config);
    let openrouter = openrouter_config_for(&deps.providers, &deps.extract_config.openrouter_llm);

    let mut attempted: Vec<EngineChoice> = Vec::new();
    let mut current = pick_engine(&engine_cfg);
    let mut final_page: Option<crate::Crawl4aiPageResult> = None;
    let mut gate_result_final: Option<super::quality_gate::GateResult> = None;

    loop {
        attempted.push(current.clone());
        let (markdown_needed, or_cfg) = match current {
            EngineChoice::OpenRouterLlm => (true, openrouter.clone()),
            _ => (true, None),
        };
        let req = Crawl4aiPageRequest {
            profile_name: job.profile_name.clone(),
            url: job.url.clone(),
            wait_for: None,
            js_code: None,
            markdown: false,
            extract_engine: Some(current.as_str().to_string()),
            openrouter_config: or_cfg,
        };
        let page_res = crawl4ai_crawl(
            &deps.paths,
            &deps.crawl4ai_config,
            &deps.supervisor,
            req,
        )
        .await;
        let _ = markdown_needed; // suppress unused warning for now

        let page = match page_res {
            Ok(p) => p,
            Err(err) => {
                // Network/infra failure — same behavior as before: fail the job.
                let issue_type = err.issue_type().to_string();
                let message = err.to_string();
                queue
                    .attach_engine_chain(
                        &job.id,
                        attempted.iter().map(|e| e.as_str().to_string()).collect(),
                    )
                    .await;
                queue
                    .finish(
                        &job.id,
                        IngestOutcome::Failed(IngestJobError {
                            issue_type,
                            message,
                            stage: "fetching".into(),
                        }),
                    )
                    .await;
                return;
            }
        };

        let markdown_str = page.markdown.clone().unwrap_or_default();
        let html_chars = page
            .html
            .as_ref()
            .map(|h| h.chars().count())
            .unwrap_or(0);
        let gate_result = assess_quality(&markdown_str, html_chars, &gate_cfg);
        if gate_result.pass {
            final_page = Some(page);
            gate_result_final = Some(gate_result);
            break;
        }
        // Gate failed — try next engine.
        match next_engine(&current, &engine_cfg.fallback_ladder) {
            Some(next) => {
                tracing::info!(
                    job_id = %job.id,
                    from = %current,
                    to = %next,
                    hard = ?gate_result.hard_fail_reasons,
                    soft = ?gate_result.soft_fail_reasons,
                    "upgrading extraction engine after gate failure"
                );
                current = next;
                continue;
            }
            None => {
                queue
                    .attach_engine_chain(
                        &job.id,
                        attempted.iter().map(|e| e.as_str().to_string()).collect(),
                    )
                    .await;
                queue
                    .finish(
                        &job.id,
                        IngestOutcome::Failed(IngestJobError {
                            issue_type: "quality_gate_rejected".into(),
                            message: format!(
                                "all engines exhausted; last hard={:?} soft={:?}",
                                gate_result.hard_fail_reasons, gate_result.soft_fail_reasons,
                            ),
                            stage: "fetching".into(),
                        }),
                    )
                    .await;
                return;
            }
        }
    }

    let page = final_page.expect("loop exits only via break with Some or early return");
    let _final_gate = gate_result_final.expect("set together with final_page");
    queue
        .attach_engine_chain(
            &job.id,
            attempted.iter().map(|e| e.as_str().to_string()).collect(),
        )
        .await;

    let markdown = match page.markdown.as_deref() {
        Some(m) => m.to_string(),
        None => {
            queue
                .finish(
                    &job.id,
                    IngestOutcome::Failed(IngestJobError {
                        issue_type: "empty_content".into(),
                        message: "engine returned no markdown field".into(),
                        stage: "fetching".into(),
                    }),
                )
                .await;
            return;
        }
    };
```

LEAVE the subsequent `if markdown.chars().count() < deps.ingest_config.min_markdown_chars { ... }` block unchanged — it's redundant with the gate now but harmless and is removed in Phase 2.

- [ ] **Step 3: Add `attach_engine_chain` to queue**

In `src/article_memory/ingest/queue.rs`, inside `impl IngestQueue`, add:

```rust
pub async fn attach_engine_chain(&self, job_id: &str, chain: Vec<String>) {
    let mut state = self.inner.lock().await;
    if let Some(job) = state.jobs.get_mut(job_id) {
        job.engine_chain = chain;
    }
    let _ = self.persist_locked(&state);
}
```

(The exact field/method names may differ; use the existing pattern seen in `attach_article_id`.)

- [ ] **Step 4: Verify build + existing tests still pass**

Run: `cargo build -p davis_zero_claw`
Run: `cargo test -p davis_zero_claw --lib`
Expected: green.

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p davis_zero_claw --all-targets -- -D warnings`
Expected: clean. Fix any warnings.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/ingest/worker.rs src/article_memory/ingest/queue.rs
git commit -m "feat(article-memory): worker uses pluggable engines with quality-gate upgrade ladder"
```

---

### Task 13: Rust — record engine chain in clean report

**Files:**
- Modify: `src/article_memory/pipeline.rs`

- [ ] **Step 1: Extend `ArticleCleanReport`**

In `src/article_memory/pipeline.rs`, locate `pub struct ArticleCleanReport` in `src/article_memory/reports.rs` (it's defined there). Add:

```rust
    #[serde(default)]
    pub engine_chain: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_engine: Option<String>,
```

- [ ] **Step 2: Populate in `build_clean_report`**

In `pipeline.rs`, modify `build_clean_report` (around line 316) to accept two new parameters:

```rust
pub(super) fn build_clean_report(
    article: &ArticleMemoryRecord,
    strategy: &ResolvedArticleCleaningStrategy,
    normalized: &NormalizedArticleText,
    clean_status: &str,
    raw_chars: usize,
    normalized_chars: usize,
    final_chars: usize,
    engine_chain: Vec<String>,
    final_engine: Option<String>,
) -> ArticleCleanReport {
    // ... existing body ...
    ArticleCleanReport {
        // ... existing fields ...
        engine_chain,
        final_engine,
    }
}
```

Then find all three call sites of `build_clean_report` in the same file (replay, judge, normalize) and pass `Vec::new()` + `None` for now. The worker-originated path will thread the real chain in Step 3.

- [ ] **Step 3: Pass chain from worker when calling normalize**

Currently `worker.rs` calls `normalize_article_memory(...)` which internally calls `build_clean_report` with no chain data. Add an overload or a per-article side-channel:

Create `src/article_memory/ingest/report_context.rs`:

```rust
//! Thread-local context for passing engine_chain into the shared normalize path.
//! Alternative to threading the value through 8 function signatures.

use std::cell::RefCell;

thread_local! {
    static CONTEXT: RefCell<Option<EngineReportContext>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone)]
pub struct EngineReportContext {
    pub engine_chain: Vec<String>,
    pub final_engine: Option<String>,
}

pub fn with_context<R>(ctx: EngineReportContext, f: impl FnOnce() -> R) -> R {
    CONTEXT.with(|c| *c.borrow_mut() = Some(ctx));
    let r = f();
    CONTEXT.with(|c| *c.borrow_mut() = None);
    r
}

pub fn current() -> Option<EngineReportContext> {
    CONTEXT.with(|c| c.borrow().clone())
}
```

**BUT**: Tokio tasks migrate threads. This thread-local approach is UNSAFE across `.await`. Instead use `tokio::task_local!`:

Replace the above with:

```rust
use tokio::task_local;

task_local! {
    static CONTEXT: EngineReportContext;
}

#[derive(Debug, Clone)]
pub struct EngineReportContext {
    pub engine_chain: Vec<String>,
    pub final_engine: Option<String>,
}

pub async fn with_context<R, F>(ctx: EngineReportContext, fut: F) -> R
where
    F: std::future::Future<Output = R>,
{
    CONTEXT.scope(ctx, fut).await
}

pub fn current() -> Option<EngineReportContext> {
    CONTEXT.try_with(|c| c.clone()).ok()
}
```

Add to `src/article_memory/ingest/mod.rs`:

```rust
mod report_context;
pub use report_context::{with_context as with_engine_report_context, EngineReportContext};
```

Then in `build_clean_report`, change the signature back to its original (no extra params) and read context at the top:

```rust
pub(super) fn build_clean_report(
    article: &ArticleMemoryRecord,
    strategy: &ResolvedArticleCleaningStrategy,
    normalized: &NormalizedArticleText,
    clean_status: &str,
    raw_chars: usize,
    normalized_chars: usize,
    final_chars: usize,
) -> ArticleCleanReport {
    let ctx = crate::article_memory::ingest::report_context::current();
    let engine_chain = ctx.as_ref().map(|c| c.engine_chain.clone()).unwrap_or_default();
    let final_engine = ctx.and_then(|c| c.final_engine);

    // ... existing body ...

    ArticleCleanReport {
        // ... existing fields ...
        engine_chain,
        final_engine,
    }
}
```

Revert the `build_clean_report` call sites that had new params from Step 2.

- [ ] **Step 4: Wrap the worker's normalize call with the context**

In `worker.rs`, where `normalize_article_memory(...)` is awaited, wrap it:

```rust
let normalize_response = super::report_context::with_context(
    super::report_context::EngineReportContext {
        engine_chain: attempted.iter().map(|e| e.as_str().to_string()).collect(),
        final_engine: Some(current.as_str().to_string()),
    },
    normalize_article_memory(
        &deps.paths,
        normalize_config.as_ref(),
        value_config.as_ref(),
        &record.id,
    ),
)
.await;
```

- [ ] **Step 5: Verify build + tests**

Run: `cargo build -p davis_zero_claw`
Run: `cargo test -p davis_zero_claw --lib`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add src/article_memory/ingest/report_context.rs src/article_memory/ingest/mod.rs \
        src/article_memory/ingest/worker.rs src/article_memory/pipeline.rs \
        src/article_memory/reports.rs
git commit -m "feat(article-memory): record engine_chain and final_engine in clean_report"
```

---

### Task 14: Delete `[[sites]]` TOML blocks

**Files:**
- Modify: `config/davis/article_memory.toml`

- [ ] **Step 1: Snapshot current file for grep reference**

Run: `cp config/davis/article_memory.toml /tmp/article_memory.toml.phase0`

- [ ] **Step 2: Edit the file**

Open `config/davis/article_memory.toml`. DELETE every `[[sites]]` block (from the line starting `[[sites]]` up to the line BEFORE the next `[[sites]]` or `[something_else]` header).

KEEP these sections intact:
- `[defaults]` — preserved
- `[value]` — preserved
- Any other top-level tables

ADD at the bottom:

```toml
[extract]
default_engine = "trafilatura"
fallback_ladder = ["trafilatura", "openrouter-llm"]

[extract.openrouter_llm]
provider = "openrouter"
model = "google/gemini-2.0-flash-001"
timeout_secs = 60
max_input_chars = 60000

[quality_gate]
enabled = true
min_markdown_chars = 500
min_kept_ratio = 0.05
min_paragraphs = 3
max_link_density = 0.5
boilerplate_markers = ["订阅", "分享到", "Cookie Policy", "相关推荐"]
```

- [ ] **Step 3: Verify parse**

Run: `cargo test -p davis_zero_claw --lib app_config`
Expected: all pass. If any `[[sites]]`-specific test fails, it belongs to the deletion wave in Task 15.

- [ ] **Step 4: Commit**

```bash
git add config/davis/article_memory.toml
git commit -m "config: delete [[sites]] blocks; add [extract] and [quality_gate] sections"
```

---

### Task 15: Delete Rust site-strategy code

**Files:**
- Modify: `src/article_memory/cleaning_internals.rs`
- Modify: `src/article_memory/config.rs`
- Modify: `src/article_memory/types.rs`
- Modify: `src/article_memory/pipeline.rs`

- [ ] **Step 1: Identify deletion targets**

Run these greps to inventory:

```bash
grep -n "resolve_article_cleaning_strategy\|article_matches_strategy\|wildcard_match\|merged_lines\|ArticleCleaningSiteStrategy" src/article_memory/*.rs
```

Record which functions are affected.

- [ ] **Step 2: Replace `resolve_article_cleaning_strategy` with a zero-sites default**

In `src/article_memory/cleaning_internals.rs`, REPLACE the body of `resolve_article_cleaning_strategy` (around line 105) with:

```rust
pub(super) fn resolve_article_cleaning_strategy(
    config: &ArticleCleaningConfig,
    _article: &ArticleMemoryRecord,
) -> ResolvedArticleCleaningStrategy {
    // Phase 1: all site strategies deleted. Every article uses the "default"
    // strategy derived from the shared [defaults] block. Phase 2's learned-rules
    // engine supersedes per-host strategy handling entirely.
    ResolvedArticleCleaningStrategy {
        name: "default".to_string(),
        version: 1,
        source: "config/davis/article_memory.toml".to_string(),
        min_kept_ratio: config.defaults.min_kept_ratio,
        max_kept_ratio: config.defaults.max_kept_ratio,
        min_normalized_chars: config.defaults.min_normalized_chars,
        start_markers: Vec::new(),
        end_markers: Vec::new(),
        exact_noise_lines: config.defaults.exact_noise_lines.clone(),
        contains_noise_lines: config.defaults.contains_noise_lines.clone(),
        line_suffix_noise: Vec::new(),
    }
}
```

- [ ] **Step 3: Delete `article_matches_strategy`, `wildcard_match`, `merged_lines`**

In `cleaning_internals.rs`, delete those three functions (lines ~150–203). If any tests reference them, delete those too.

- [ ] **Step 4: Trim `normalize_article_cleaning_config`**

Find `normalize_article_cleaning_config` (around line 33). DELETE the entire `for site in &mut config.sites { ... }` loop. Keep:

```rust
pub(super) fn normalize_article_cleaning_config(config: &mut ArticleCleaningConfig) -> Result<()> {
    normalize_cleaning_defaults(&mut config.defaults);
    Ok(())
}
```

- [ ] **Step 5: Delete `ArticleCleaningSiteStrategy`**

In `src/article_memory/types.rs`, delete the `ArticleCleaningSiteStrategy` struct AND remove the `sites: Vec<ArticleCleaningSiteStrategy>` field from `ArticleCleaningConfig`.

- [ ] **Step 6: Delete legacy fields on `ResolvedArticleCleaningStrategy`**

In the same `types.rs`, remove unused fields from `ResolvedArticleCleaningStrategy` if any are now always empty. Safe minimal change: keep `start_markers`, `end_markers`, `exact_noise_lines`, `contains_noise_lines`, `line_suffix_noise` as `Vec<String>` (they stay empty in Phase 1 but are read by other code paths).

- [ ] **Step 7: Delete obsolete tests**

Run: `grep -rn "resolve_article_cleaning_strategy\|article_matches_strategy\|wildcard_match\|ArticleCleaningSiteStrategy" tests/`

For each test file that's now orphaned, delete the file. For tests that mix cleaning-strategy assertions with other assertions, delete just the offending test functions.

- [ ] **Step 8: Fix any compile errors from field removal**

Run: `cargo check -p davis_zero_claw`
Expected: clean. If `sites:` is referenced elsewhere (e.g., `config.rs` serialization), update to drop the field.

- [ ] **Step 9: Run all tests**

Run: `cargo test -p davis_zero_claw`
Expected: all green.

- [ ] **Step 10: Clippy**

Run: `cargo clippy -p davis_zero_claw --all-targets -- -D warnings`
Expected: clean. Fix warnings about unused imports.

- [ ] **Step 11: Commit**

```bash
git add -u src/article_memory/
git add -u tests/
git commit -m "refactor(article-memory): delete [[sites]] strategy code paths (superseded by pluggable engines)"
```

---

### Task 16: Remove deprecated `pruning` engine path (optional cleanup)

**Files:**
- Modify: `crawl4ai_adapter/server.py`
- Modify: `src/crawl4ai.rs`

- [ ] **Step 1: Decide**

Phase 1 keeps `pruning` as a migration-window option. No code changes in this task — but leave a TODO marker:

In `crawl4ai_adapter/server.py`, above the `if engine_used == "pruning":` line, add:

```python
    # TODO(Phase-2): delete the `pruning` branch once all callers migrate to
    # `extract_engine` and the Rust-side `markdown: true` compat path is gone.
```

In `src/crawl4ai.rs`, above the `markdown` field docstring, confirm the "Deprecated" language (already added in Task 8).

- [ ] **Step 2: Commit**

```bash
git add crawl4ai_adapter/server.py
git commit -m "chore: mark pruning engine path for Phase 2 removal"
```

---

### Task 17: Integration test — engine ladder upgrade behavior

**Files:**
- Create: `tests/ingest_engine_ladder_test.rs`

- [ ] **Step 1: Write the test**

Create `tests/ingest_engine_ladder_test.rs`:

```rust
//! Integration test: simulated engine upgrade on quality-gate failure.
//!
//! Uses `Crawl4aiSupervisor::for_test` (feature = "test-util") to inject
//! canned responses per engine without spawning Python.

#![cfg(feature = "test-util")]

use davis_zero_claw::{
    article_memory::ingest::{
        IngestQueue, IngestRequest, IngestWorkerDeps, IngestWorkerPool,
    },
    app_config::{
        ArticleMemoryConfig, ArticleMemoryExtractConfig, ArticleMemoryIngestConfig, ImessageConfig,
        ModelProviderConfig, OpenRouterLlmEngineConfig, QualityGateToml,
    },
    server::Crawl4aiProfileLocks,
    Crawl4aiConfig, Crawl4aiSupervisor, RuntimePaths,
};
use std::sync::Arc;

#[tokio::test]
async fn trafilatura_gate_fails_upgrades_to_openrouter() {
    // Arrange: supervisor returns 30-char markdown for trafilatura (gate fails)
    // and 2000-char markdown for openrouter-llm (gate passes).
    // Implementation note: Crawl4aiSupervisor::for_test needs an engine-aware
    // switch. If the current for_test API only returns a single canned result,
    // adapt the test to drive two separate crawl calls and assert the upgrade
    // path visible via engine_chain persisted to the queue.

    // Skeleton shown; flesh out once for_test capability is confirmed.
    // See baseline spec §13 for the Crawl4aiSupervisor::for_test API.

    // If the existing for_test cannot switch by engine, the acceptable
    // alternative is: assert that when we force default_engine="openrouter-llm"
    // and trafilatura isn't exercised, engine_chain == ["openrouter-llm"].
}

#[tokio::test]
async fn quality_gate_disabled_skips_upgrade() {
    // With quality_gate.enabled = false, the first engine's result is always
    // accepted. engine_chain should be ["trafilatura"] even for short output.
    // Fleshed out similarly.
}
```

- [ ] **Step 2: Flesh out test bodies based on current `for_test` API**

Run: `grep -rn "Crawl4aiSupervisor::for_test\|fn for_test" src/`

Read the existing signature and adapt the tests to inject canned responses. If the API can't switch per-engine, write a simpler test that:

1. Boots a `Crawl4aiSupervisor::for_test` returning `Crawl4aiPageResult` with a short markdown
2. Submits one ingest job with `default_engine = "openrouter-llm"` (skipping trafilatura)
3. Asserts the job transitions to `Failed("quality_gate_rejected")` because there's no further tier
4. Asserts `job.engine_chain == ["openrouter-llm"]`

- [ ] **Step 3: Run the test**

Run: `cargo test -p davis_zero_claw --test ingest_engine_ladder_test --features test-util`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add tests/ingest_engine_ladder_test.rs
git commit -m "test(ingest): integration test for engine ladder and quality gate"
```

---

### Task 18: Update daviszeroclaw install for trafilatura

**Files:**
- Modify: `src/control/crawl.rs` or wherever `daviszeroclaw crawl install` is defined

- [ ] **Step 1: Locate the install command**

Run: `grep -rn "pip install" src/ | head`

Find the `crawl install` subcommand handler. Add `trafilatura httpx` to the pip-install invocation. Example pattern (adapt to actual code):

```rust
// existing: pip install --upgrade crawl4ai fastapi uvicorn[standard] pydantic
// new:      pip install --upgrade crawl4ai fastapi uvicorn[standard] pydantic trafilatura httpx
```

- [ ] **Step 2: Document in the adapter README or inline comment**

Add a comment above the changed line explaining that both deps are optional at runtime but recommended for Phase 1 parity.

- [ ] **Step 3: Verify via `daviszeroclaw crawl install` reinstall**

```bash
cargo run --bin daviszeroclaw -- crawl install
```

Then confirm: `.runtime/davis/crawl4ai-venv/bin/pip list | grep -i trafilatura`
Expected: trafilatura listed.

- [ ] **Step 4: Commit**

```bash
git add src/control/crawl.rs
git commit -m "chore(crawl): install trafilatura + httpx alongside crawl4ai"
```

---

### Task 19: Final verification

- [ ] **Step 1: Full test suite**

Run: `cargo test -p davis_zero_claw`
Expected: all tests pass.

- [ ] **Step 2: Clippy all targets**

Run: `cargo clippy -p davis_zero_claw --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Fmt**

Run: `cargo fmt --all -- --check`
Expected: clean.

- [ ] **Step 4: Python tests**

Run: `cd crawl4ai_adapter && ../.runtime/davis/crawl4ai-venv/bin/python -m pytest -v`
Expected: all pass.

- [ ] **Step 5: Manual end-to-end**

Boot the daemon:

```bash
cargo run --bin daviszeroclaw -- daemon
```

In another shell:

```bash
cargo run --bin daviszeroclaw -- articles ingest "https://example.com"
# wait ~10s
cargo run --bin daviszeroclaw -- articles ingest history --limit 1
```

Expected: job in `Saved` status; `engine_chain` shows `["trafilatura"]`; article visible in `article_memory_index.json`.

- [ ] **Step 6: Update spec status**

Edit `docs/superpowers/specs/2026-04-24-crawl4ai-cleaning-upgrade-design.md` line 3:

```markdown
- Status: Phase 1 Landed (Phase 2 pending)
```

- [ ] **Step 7: Final commit**

```bash
git add docs/superpowers/specs/2026-04-24-crawl4ai-cleaning-upgrade-design.md
git commit -m "docs(specs): mark Phase 1 of crawl4ai cleaning upgrade as landed"
```

---

## Self-Review Checklist

- [x] Every `[[sites]]` TOML block deletion → Task 14
- [x] trafilatura engine → Task 2
- [x] openrouter-llm engine → Task 3
- [x] Python dispatch wiring → Task 4
- [x] Rust content signals (reused in Phase 2) → Task 5
- [x] Quality gate hard/soft fail → Task 6
- [x] Engine ladder → Task 7
- [x] Crawl4aiPageRequest extension → Task 8
- [x] Config types → Task 9
- [x] engine_chain on IngestJob → Task 10
- [x] Deps threading → Task 11
- [x] Worker upgrade loop → Task 12
- [x] Clean report records engine chain → Task 13
- [x] Site-strategy code deletion → Task 15
- [x] Integration test → Task 17
- [x] Install dependency → Task 18
- [x] Verification → Task 19

No placeholders. Signatures and names consistent across tasks (`pick_engine`, `next_engine`, `assess_quality`, `engine_chain`, `final_engine`, `EngineChoice`, `QualityGateConfig` vs `QualityGateToml`—the TOML shape is `QualityGateToml`, the runtime shape is `QualityGateConfig`, bridging in Task 12).

Out-of-scope (Phase 2, tracked in spec §4.3):
- `normalize_line` code-block preservation
- Sliding-window dedup
- Multi-dim deterministic scoring
- LLM `extraction_quality` field
- Rule self-learning loop
- Warmup CLI
