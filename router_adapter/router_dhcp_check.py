"""DHCP-disable script for a specific GPON ONT router admin UI.

Stdout protocol contract (Rust supervisor depends on this):
    The LAST non-empty stdout line MUST be a single JSON object.
    Earlier lines are free-form human-readable logs.

Possible final lines:
    {"status":"ok",    "action":"none"|"disabled", "dhcp_was_enabled":bool, "duration_ms":int}
    {"status":"error", "stage":"<closed-enum>", "reason":"<short>", "duration_ms":int}

Closed `stage` enum: login | navigate | iframe | toggle | apply | unhandled.
The outermost try/except MUST emit `unhandled` for any uncaught exception.

Selectors are firmware-specific. If the router web UI changes after a
firmware update, update the constants below.
"""

from __future__ import annotations

import json
import os
import sys
import time
from typing import Any

from playwright.sync_api import (
    Page,
    Playwright,
    TimeoutError as PlaywrightTimeoutError,
    sync_playwright,
)

# --- Selectors (firmware-specific; update here when UI changes) ---
SEL_LOGIN_PHOTO = "#normalphoto"
SEL_USERNAME = "#txt_normalUsername"
SEL_PASSWORD = "#txt_normalPassword"
SEL_LOGIN_SUBMIT = "#PwdPain1 > div:nth-child(2)"
SEL_MAIN_MENU = "#mainMenu_1"
SEL_THIRD_MENU_DHCP = "#thirdMenu_2"
SEL_IFRAME = "#frameContent"
SEL_DHCP_CHECKBOX = "#dhcpSrvType"
SEL_APPLY = "#btnApply_ex"
SEL_LOGOUT = "#headerLogout"

# --- Timeouts (ms) ---
DEFAULT_TIMEOUT_MS = 5000
NAV_TIMEOUT_MS = 30000
LOGIN_NAV_TIMEOUT_MS = 10000


def emit(payload: dict[str, Any]) -> None:
    """Write the protocol JSON line to stdout. Always the LAST stdout call."""
    sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def log(message: str) -> None:
    """Free-form log line — visible to the Rust supervisor as tracing info."""
    print(message, file=sys.stdout, flush=True)


def main() -> None:
    started = time.monotonic()
    url = os.environ.get("ROUTER_URL", "http://192.168.0.1")
    username = os.environ.get("ROUTER_USERNAME")
    password = os.environ.get("ROUTER_PASSWORD")

    if not username or not password:
        emit(
            {
                "status": "error",
                "stage": "unhandled",
                "reason": "missing ROUTER_USERNAME or ROUTER_PASSWORD env",
                "duration_ms": int((time.monotonic() - started) * 1000),
            }
        )
        sys.exit(1)

    try:
        with sync_playwright() as p:
            outcome = run_check(p, url, username, password)
        outcome["duration_ms"] = int((time.monotonic() - started) * 1000)
        emit(outcome)
        sys.exit(0 if outcome["status"] == "ok" else 1)
    except Exception as exc:  # noqa: BLE001 — top-level safety net
        emit(
            {
                "status": "error",
                "stage": "unhandled",
                "reason": f"{type(exc).__name__}: {exc}",
                "duration_ms": int((time.monotonic() - started) * 1000),
            }
        )
        sys.exit(1)


def run_check(p: Playwright, url: str, username: str, password: str) -> dict[str, Any]:
    log(f"launching chromium for {url}")
    browser = p.chromium.launch(headless=True, args=["--no-sandbox"])
    try:
        context = browser.new_context(viewport={"width": 1280, "height": 800})
        page = context.new_page()

        # --- Navigate ---
        try:
            page.goto(url, wait_until="networkidle", timeout=NAV_TIMEOUT_MS)
            page.wait_for_timeout(2000)
        except PlaywrightTimeoutError as exc:
            return {"status": "error", "stage": "navigate", "reason": f"goto timeout: {exc}"}

        # --- Click photo to reveal login form (skip if not present) ---
        try:
            page.wait_for_selector(SEL_LOGIN_PHOTO, timeout=DEFAULT_TIMEOUT_MS)
            page.click(SEL_LOGIN_PHOTO)
            page.wait_for_timeout(1000)
            log("clicked login photo")
        except PlaywrightTimeoutError:
            log("no login photo (form may already be visible)")

        # --- Login ---
        try:
            page.wait_for_selector(SEL_USERNAME, timeout=DEFAULT_TIMEOUT_MS)
            page.fill(SEL_USERNAME, username)
            page.fill(SEL_PASSWORD, password)
            page.click(SEL_LOGIN_SUBMIT)
            try:
                page.wait_for_load_state("networkidle", timeout=LOGIN_NAV_TIMEOUT_MS)
            except PlaywrightTimeoutError:
                log("no navigation after login click; continuing")
            page.wait_for_timeout(2000)
            log("login submitted")
        except PlaywrightTimeoutError as exc:
            return {
                "status": "error",
                "stage": "login",
                "reason": f"login form selector miss: {exc}",
            }

        # --- Navigate basic config → DHCP ---
        try:
            page.wait_for_selector(SEL_MAIN_MENU, timeout=DEFAULT_TIMEOUT_MS)
            page.click(SEL_MAIN_MENU)
            page.wait_for_timeout(1500)
            page.wait_for_selector(SEL_THIRD_MENU_DHCP, timeout=DEFAULT_TIMEOUT_MS)
            page.click(SEL_THIRD_MENU_DHCP)
            page.wait_for_timeout(1500)
            log("navigated to DHCP page")
        except PlaywrightTimeoutError as exc:
            return {"status": "error", "stage": "navigate", "reason": f"menu selector miss: {exc}"}

        # --- Drop into iframe ---
        try:
            page.wait_for_selector(SEL_IFRAME, timeout=DEFAULT_TIMEOUT_MS)
            frame_handle = page.query_selector(SEL_IFRAME)
            frame = frame_handle.content_frame() if frame_handle else None
            if frame is None:
                return {
                    "status": "error",
                    "stage": "iframe",
                    "reason": "iframe content_frame is None",
                }
        except PlaywrightTimeoutError as exc:
            return {"status": "error", "stage": "iframe", "reason": f"iframe wait timeout: {exc}"}

        # --- Read DHCP checkbox ---
        try:
            frame.wait_for_selector(SEL_DHCP_CHECKBOX, timeout=DEFAULT_TIMEOUT_MS)
            is_enabled = frame.evaluate(
                f"() => {{ const el = document.querySelector('{SEL_DHCP_CHECKBOX}'); return el ? el.checked : false; }}"
            )
        except PlaywrightTimeoutError as exc:
            return {
                "status": "error",
                "stage": "toggle",
                "reason": f"checkbox selector miss: {exc}",
            }

        if not is_enabled:
            log("DHCP already off; logging out")
            try_logout(page)
            return {"status": "ok", "action": "none", "dhcp_was_enabled": False}

        # --- Disable + apply ---
        log("DHCP on; disabling")
        try:
            frame.click(SEL_DHCP_CHECKBOX)
            page.wait_for_timeout(1000)
            frame.wait_for_selector(SEL_APPLY, timeout=DEFAULT_TIMEOUT_MS)
            frame.click(SEL_APPLY)
            page.wait_for_timeout(2000)
        except PlaywrightTimeoutError as exc:
            return {
                "status": "error",
                "stage": "apply",
                "reason": f"apply button selector miss: {exc}",
            }

        try_logout(page)
        return {"status": "ok", "action": "disabled", "dhcp_was_enabled": True}
    finally:
        browser.close()


def try_logout(page: Page) -> None:
    """Best-effort logout. Logged but never fatal."""
    try:
        page.wait_for_selector(SEL_LOGOUT, timeout=DEFAULT_TIMEOUT_MS)
        page.click(SEL_LOGOUT)
        page.wait_for_timeout(1000)
        log("logged out")
    except PlaywrightTimeoutError:
        log("no logout button found (acceptable)")


if __name__ == "__main__":
    main()
