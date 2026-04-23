"""Long-lived HTTP adapter for crawl4ai.

Runs as a child of the Rust daemon (see src/crawl4ai_supervisor.rs).
Exposes POST /crawl and GET /health. A fresh AsyncWebCrawler is
opened per request — crawl4ai's BrowserConfig + persistent_context
tie a crawler to one Chromium user_data_dir, so reuse across
requests would conflate profiles. The long-lived FastAPI process
still wins by keeping Python + the crawl4ai module imports warm;
only the Chromium launch is paid per call, and concurrent same-profile
calls are serialized by the Rust-side mutex (src/express.rs).
"""

from __future__ import annotations

import asyncio
import logging
import os
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, Optional

from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel, Field

logger = logging.getLogger("crawl4ai_adapter.server")


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


class CrawlResponse(BaseModel):
    success: bool
    url: Optional[str] = None
    redirected_url: Optional[str] = None
    status_code: Optional[int] = None
    html: Optional[str] = None
    cleaned_html: Optional[str] = None
    js_execution_result: Optional[Any] = None
    error_message: Optional[str] = None


def _collect_versions() -> dict[str, str]:
    """Best-effort version capture for the packages we care about.

    Unpinned by design — we want breakages to fail loudly rather than be
    hidden by a frozen requirements.txt. Surfacing versions at runtime
    means 'which version regressed?' is answerable from daemon.log.
    """
    import importlib.metadata as md
    versions: dict[str, str] = {}
    for pkg in ("crawl4ai", "playwright", "patchright", "fastapi", "pydantic", "uvicorn"):
        try:
            versions[pkg] = md.version(pkg)
        except md.PackageNotFoundError:
            versions[pkg] = "missing"
    import sys
    versions["python"] = sys.version.split()[0]
    return versions


@asynccontextmanager
async def lifespan(app: FastAPI):
    runtime_dir = Path(os.environ.get("CRAWL4_AI_BASE_DIRECTORY", ".")).resolve()
    runtime_dir.mkdir(parents=True, exist_ok=True)
    os.environ["CRAWL4_AI_BASE_DIRECTORY"] = str(runtime_dir)
    app.state.versions = _collect_versions()
    logger.info(
        "crawl4ai_adapter.server starting base_dir=%s versions=%s",
        runtime_dir,
        app.state.versions,
    )
    # Lazy-import crawl4ai so startup failures surface in /health rather than
    # at import time (daemon can report a typed error to the user).
    try:
        from crawl4ai import AsyncWebCrawler  # noqa: F401
        app.state.crawl4ai_ok = True
    except Exception as exc:  # pragma: no cover
        app.state.crawl4ai_ok = False
        app.state.crawl4ai_import_error = str(exc)
        logger.exception("crawl4ai import failed")
    yield
    logger.info("crawl4ai_adapter.server stopping")


app = FastAPI(title="crawl4ai_adapter", lifespan=lifespan)


@app.get("/health")
async def health() -> dict[str, Any]:
    versions = getattr(app.state, "versions", {})
    if not getattr(app.state, "crawl4ai_ok", False):
        return JSONResponse(
            status_code=503,
            content={
                "status": "unhealthy",
                "reason": "crawl4ai_import_failed",
                "details": getattr(app.state, "crawl4ai_import_error", "unknown"),
                "versions": versions,
            },
        )
    return {"status": "ok", "versions": versions}


@app.post("/crawl", response_model=CrawlResponse)
async def crawl(req: CrawlRequest) -> CrawlResponse:
    if not getattr(app.state, "crawl4ai_ok", False):
        raise HTTPException(
            status_code=503,
            detail={
                "error": "crawl4ai_unavailable",
                "details": getattr(app.state, "crawl4ai_import_error", "unknown"),
            },
        )

    from crawl4ai import AsyncWebCrawler, BrowserConfig, CacheMode, CrawlerRunConfig

    profile_path = Path(req.profile_path).expanduser().resolve()
    profile_path.mkdir(parents=True, exist_ok=True)
    logger.info("crawl url=%s profile=%s timeout_secs=%d", req.url, profile_path, req.timeout_secs)

    browser_config = BrowserConfig(
        browser_type="chromium",
        headless=req.headless,
        use_managed_browser=True,
        use_persistent_context=True,
        user_data_dir=str(profile_path),
        enable_stealth=req.enable_stealth,
        viewport_width=1440,
        viewport_height=960,
        verbose=False,
    )
    crawler_config = CrawlerRunConfig(
        cache_mode=CacheMode.BYPASS,
        page_timeout=req.timeout_secs * 1000,
        delay_before_return_html=1.0,
        magic=req.magic,
        simulate_user=req.simulate_user,
        override_navigator=req.override_navigator,
        remove_overlay_elements=req.remove_overlay_elements,
        wait_for=req.wait_for,
        js_code=req.js_code,
    )

    try:
        async with AsyncWebCrawler(config=browser_config) as crawler:
            # Outer timeout guards against crawl4ai hanging past its own page_timeout.
            result = await asyncio.wait_for(
                crawler.arun(url=req.url, config=crawler_config),
                timeout=req.timeout_secs + 15,
            )
    except asyncio.TimeoutError:
        logger.warning("crawl timeout url=%s budget_secs=%d", req.url, req.timeout_secs + 15)
        raise HTTPException(
            status_code=504,
            detail={"error": "crawl_timeout", "details": f"exceeded {req.timeout_secs + 15}s"},
        )
    except Exception as exc:
        logger.exception("crawl failed url=%s", req.url)
        raise HTTPException(
            status_code=500,
            detail={"error": "crawl_failed", "details": str(exc)},
        )

    return CrawlResponse(
        success=bool(getattr(result, "success", False)),
        url=getattr(result, "url", req.url),
        redirected_url=getattr(result, "redirected_url", None),
        status_code=getattr(result, "status_code", None),
        html=getattr(result, "html", None),
        cleaned_html=getattr(result, "cleaned_html", None),
        js_execution_result=getattr(result, "js_execution_result", None),
        error_message=getattr(result, "error_message", None),
    )
