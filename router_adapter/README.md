# router_adapter

Davis Playwright adapter that drives the LAN router admin page to keep DHCP disabled. Mirrors `crawl4ai_adapter/` in shape; not related to crawling. See `docs/superpowers/specs/2026-05-09-router-dhcp-worker-design.md` for the design.

## Setup (canonical)

```bash
daviszeroclaw router-dhcp install
```

This creates `.runtime/davis/router-adapter-venv/`, installs `playwright` + `python-dotenv` into it, and runs `playwright install chromium` with `PLAYWRIGHT_BROWSERS_PATH=.runtime/davis/playwright-browsers/` set so Chromium is shared with the crawl4ai adapter.

## Manual setup (debugging)

```bash
python3 -m venv .runtime/davis/router-adapter-venv
.runtime/davis/router-adapter-venv/bin/pip install -e router_adapter
PLAYWRIGHT_BROWSERS_PATH=.runtime/davis/playwright-browsers \
  .runtime/davis/router-adapter-venv/bin/python -m playwright install chromium
```

## Standalone run

```bash
ROUTER_URL=http://192.168.0.1 \
ROUTER_USERNAME=admin \
ROUTER_PASSWORD='your_password' \
PLAYWRIGHT_BROWSERS_PATH=.runtime/davis/playwright-browsers \
.runtime/davis/router-adapter-venv/bin/python -m router_adapter
```

The LAST stdout line is JSON; everything before is free-form logging.

## Updating selectors

Selectors are firmware-specific. They live as constants at the top of `router_dhcp_check.py`:

| Constant | Purpose |
|---|---|
| `SEL_LOGIN_PHOTO` | First-page image to click before login form appears |
| `SEL_USERNAME`, `SEL_PASSWORD` | Login form text inputs |
| `SEL_LOGIN_SUBMIT` | Submit (it's a `<div>` styled as a button) |
| `SEL_MAIN_MENU`, `SEL_THIRD_MENU_DHCP` | Sidebar nav into DHCP settings |
| `SEL_IFRAME` | The settings iframe |
| `SEL_DHCP_CHECKBOX` | The toggle to read/click |
| `SEL_APPLY` | "Apply" button after toggling |
| `SEL_LOGOUT` | Header logout button |

To find the right selectors after a firmware change: open the router page in Chrome DevTools, use the inspector to read `id` / `class` / `name` attributes for each step, and update the constants. Run `daviszeroclaw router-dhcp run-once` after each change to validate.
