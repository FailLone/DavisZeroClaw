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
