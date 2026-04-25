"""Pluggable content-extraction engines used by the /crawl endpoint.

Each engine takes HTML + options and returns an `ExtractResult`. Engines are
pure functions (no filesystem, no network for trafilatura; one HTTP call for
openrouter-llm). The dispatcher lives in `server.py`.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass
class ExtractResult:
    markdown: str
    metadata: dict[str, Any]
    engine: str
    warnings: list[str] = field(default_factory=list)

    def is_empty(self) -> bool:
        return not self.markdown.strip()


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
