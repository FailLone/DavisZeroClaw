"""Pluggable content-extraction engines used by the /crawl endpoint.

Each engine takes HTML + options and returns an `ExtractResult`. Engines are
pure functions (no filesystem, no network). The dispatcher lives in
`server.py`.
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
    from trafilatura.settings import use_config

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

    # Extract title up-front so we can both stash it in metadata AND prepend it
    # back onto the markdown — trafilatura strips the block's only <h1> as a
    # "doc title" during conversion, so we re-inject it to keep the rendered
    # article self-contained.
    metadata: dict[str, Any] = {}
    title_text: str = ""
    title_selector = rule.get("title_selector")
    if title_selector:
        title_elem = soup.select_one(title_selector)
        if title_elem:
            title_text = title_elem.get_text(strip=True)
            if title_text:
                metadata["title"] = title_text

    block_html = str(block)
    # Use trafilatura just as the HTML→Markdown converter on the pruned block.
    # Loosen MIN_EXTRACTED_SIZE / MIN_OUTPUT_SIZE because learned-rules callers
    # have already narrowed the DOM to exactly the block they want; applying
    # trafilatura's default "too-short → discard" heuristic on top would reject
    # short-but-legitimate posts.
    cfg = use_config()
    cfg.set("DEFAULT", "MIN_EXTRACTED_SIZE", "10")
    cfg.set("DEFAULT", "MIN_OUTPUT_SIZE", "10")
    cfg.set("DEFAULT", "MIN_EXTRACTED_COMM_SIZE", "10")
    cfg.set("DEFAULT", "MIN_OUTPUT_COMM_SIZE", "10")
    body_markdown = trafilatura.extract(
        block_html,
        output_format="markdown",
        include_tables=True,
        include_formatting=True,
        include_links=True,
        favor_precision=False,
        config=cfg,
    ) or ""

    # Re-prepend the title (trafilatura drops the block's <h1> as doc-title).
    if title_text and title_text not in body_markdown:
        markdown = f"# {title_text}\n\n{body_markdown}" if body_markdown else f"# {title_text}"
    else:
        markdown = body_markdown

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
