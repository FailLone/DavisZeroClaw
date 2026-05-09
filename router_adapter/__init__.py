"""Davis router adapter: Playwright-driven LAN router admin automation.

Owned by Davis (separate from crawl4ai_adapter). Spawned by
src/router_supervisor.rs as a one-shot subprocess; emits a single JSON
status line as its final stdout line. See
docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md.
"""
