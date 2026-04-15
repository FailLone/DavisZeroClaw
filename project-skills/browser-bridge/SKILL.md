---
name: browser-bridge
description: Read and operate web pages through the local Davis browser bridge. Use when the task needs tab inspection, page snapshots, focused browser reading, or a controlled write action on a trusted site. Always use the localhost browser bridge API instead of browsing external sites directly from the model path.
---

# Browser Bridge

## Safety Rules

- Use only `http_request` against the local Davis browser bridge API.
- Prefer read actions first: `status`, `tabs`, `snapshot`, `evaluate` with `mode=read`.
- Treat browser write actions as sensitive. If the bridge returns `requires_confirmation`, show the proposed action to the user and stop there.
- Do not try to bypass origin policy or confirmation policy.

## Workflow

1. Check browser readiness.
Read [references/browser_bridge_api.md](references/browser_bridge_api.md) and call `GET /browser/status` or `GET /browser/tabs`.

2. Read before you write.
- For page understanding, use `POST /browser/snapshot`.
- For structured extraction, use `POST /browser/evaluate` with `mode=read`.

3. Write only through the bridge.
- Use `POST /browser/action` for clicks and form input.
- Use `POST /browser/open` or `POST /browser/focus` when the user wants navigation or tab switching.

4. Respect confirmation gates.
- `requires_confirmation`: explain the pending action and ask the user.
- `unsupported_surface`: say the current browser surface cannot safely do that action yet.
- `upstream_error`: say the browser bridge is unavailable right now.

## Response Style

- Keep browser summaries short and task-focused.
- Mention the active profile only when it matters.
- For writes, describe the exact target and consequence in one sentence.
