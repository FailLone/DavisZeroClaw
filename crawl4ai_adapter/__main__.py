from __future__ import annotations

import argparse
import asyncio
import json
import os
import select
import sys
from pathlib import Path
from typing import Any


def _emit(payload: dict[str, Any]) -> None:
    json.dump(payload, sys.stdout, ensure_ascii=False)
    sys.stdout.write("\n")


def _error(message: str, *, code: int = 1, details: str | None = None) -> int:
    payload: dict[str, Any] = {"success": False, "error": message}
    if details:
        payload["details"] = details
    _emit(payload)
    return code


def _ensure_runtime_env(args: argparse.Namespace) -> Path:
    runtime_dir = Path(args.runtime_dir).expanduser().resolve()
    runtime_dir.mkdir(parents=True, exist_ok=True)
    os.environ["CRAWL4_AI_BASE_DIRECTORY"] = str(runtime_dir)
    return runtime_dir


async def _terminate_browser_process(browser_process: Any | None) -> None:
    if browser_process is None or browser_process.poll() is not None:
        return

    try:
        browser_process.terminate()
        await asyncio.wait_for(asyncio.to_thread(browser_process.wait), timeout=5.0)
        return
    except Exception:
        pass

    if browser_process.poll() is None:
        try:
            browser_process.kill()
            await asyncio.wait_for(asyncio.to_thread(browser_process.wait), timeout=3.0)
        except Exception:
            pass


async def _cleanup_login_session(
    managed_browser: Any,
    playwright: Any | None,
    browser: Any | None,
    *,
    prefer_process_termination: bool = False,
) -> None:
    browser_process = getattr(managed_browser, "browser_process", None)

    if prefer_process_termination:
        await _terminate_browser_process(browser_process)

    if browser is not None:
        try:
            await asyncio.wait_for(browser.close(), timeout=3.0)
        except Exception:
            try:
                browser.disconnect()
            except Exception:
                pass

    if playwright is not None:
        try:
            await asyncio.wait_for(playwright.stop(), timeout=3.0)
        except Exception:
            pass

    try:
        await asyncio.wait_for(managed_browser.cleanup(), timeout=5.0)
        return
    except Exception:
        pass

    await _terminate_browser_process(browser_process)


async def _run_login(args: argparse.Namespace) -> int:
    _ensure_runtime_env(args)

    try:
        from crawl4ai.async_configs import BrowserConfig
        from crawl4ai.browser_manager import ManagedBrowser
        from playwright.async_api import TimeoutError as PlaywrightTimeoutError
        from playwright.async_api import async_playwright
    except Exception as exc:  # pragma: no cover - import failure path
        return _error(
            "failed to import Crawl4AI login dependencies",
            details=str(exc),
        )

    profile_name = args.profile_name.strip()
    if not profile_name:
        return _error("profile_name must not be empty")

    profile_path = Path(args.profile_path).expanduser().resolve()
    profile_path.mkdir(parents=True, exist_ok=True)

    browser_config = BrowserConfig(
        browser_type="chromium",
        headless=False,
        verbose=True,
        use_managed_browser=True,
        use_persistent_context=True,
        user_data_dir=str(profile_path),
        extra_args=[
            "--password-store=basic",
            "--use-mock-keychain",
        ],
    )
    managed_browser = ManagedBrowser(browser_config=browser_config)
    playwright = None
    browser = None

    try:
        cdp_url = await managed_browser.start()
        playwright = await async_playwright().start()
        browser = await playwright.chromium.connect_over_cdp(cdp_url)
        context = browser.contexts[0] if browser.contexts else await browser.new_context()
        page = context.pages[0] if context.pages else await context.new_page()
        try:
            # Login pages often keep background requests open long enough that
            # Playwright never observes a full "load" completion. We only need
            # the page to become interactive, so fall back to manual navigation
            # instead of blocking the terminal forever.
            await page.goto(
                args.url,
                wait_until="domcontentloaded",
                timeout=45_000,
            )
        except PlaywrightTimeoutError:
            print(
                "Timed out waiting for the login page to finish loading. "
                "The browser window stays open; if needed, finish navigation manually "
                f"to {args.url} and continue logging in.",
                file=sys.stderr,
            )

        print(f"Crawl4AI profile login opened for {profile_name}", file=sys.stderr)
        print(f"Profile path: {profile_path}", file=sys.stderr)
        print(f"Page: {args.url}", file=sys.stderr)
        print(
            "Finish login in the browser. Then press Enter in this terminal to save and close.",
            file=sys.stderr,
        )

        browser_process = managed_browser.browser_process
        while True:
            ready, _, _ = await asyncio.to_thread(select.select, [sys.stdin], [], [], 1.0)
            if not ready:
                if browser_process is not None and browser_process.poll() is not None:
                    break
                continue
            line = sys.stdin.readline()
            if line.strip().lower() in {"", "q", "quit", "exit"}:
                break

        state_path = profile_path / "storage_state.json"
        try:
            await context.storage_state(path=str(state_path))
        except Exception:
            pass

        print("Closing Crawl4AI login browser...", file=sys.stderr)
        await _cleanup_login_session(
            managed_browser,
            playwright,
            browser,
            prefer_process_termination=True,
        )

        _emit(
            {
                "success": True,
                "profile_name": profile_name,
                "profile_path": str(profile_path),
                "url": args.url,
                "storage_state_path": str(state_path),
            }
        )
        return 0
    except Exception as exc:
        try:
            await _cleanup_login_session(managed_browser, playwright, browser)
        except Exception:
            pass
        return _error("crawl4ai profile login failed", details=str(exc))


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="python -m crawl4ai_adapter")
    subparsers = parser.add_subparsers(dest="command", required=True)

    login = subparsers.add_parser("login")
    login.add_argument("--runtime-dir", required=True)
    login.add_argument("--profile-name", required=True)
    login.add_argument("--profile-path", required=True)
    login.add_argument("--url", required=True)

    return parser


async def _main_async() -> int:
    parser = build_parser()
    args = parser.parse_args()
    if args.command == "login":
        return await _run_login(args)
    return _error(f"unsupported command: {args.command}")


def main() -> int:
    try:
        return asyncio.run(_main_async())
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
