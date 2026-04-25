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
