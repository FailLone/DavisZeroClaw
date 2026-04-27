# Cloudflare Tunnel External Access — Design Spec

**Date:** 2026-04-27
**Status:** Approved

## Problem

The Davis Shortcut bridge binds on `0.0.0.0:3012`, making it LAN-reachable. When the user's iPhone leaves the home Wi-Fi network (cellular), `build_shortcut`'s auto-detected LAN IP is unreachable. There is no path for the Shortcut to POST to Davis from the internet.

## Solution

Integrate Cloudflare Tunnel as an optional external-access layer. A persistent `cloudflared` process on the Mac creates an outbound tunnel to Cloudflare's edge, exposing port 3012 at a stable public hostname. Port 3012 already has `X-Webhook-Secret` validation — the security layer is in place.

## Scope

- Davis manages the **tunnel config file** and **launchd plist** for `cloudflared`
- Davis does **not** install `cloudflared`, run `cloudflared login`, `tunnel create`, or `tunnel route dns` — those remain manual user steps (same pattern as zeroclaw: `require_command` + brew hint)
- Three new CLI commands under `daviszeroclaw service`: `tunnel-install`, `tunnel-uninstall`, `tunnel-status`
- `service status` gains a tunnel health line

## Out of Scope

- Cloudflare API automation (login, tunnel creation, DNS routing)
- Downloading or managing the `cloudflared` binary
- TLS termination changes (Cloudflare handles TLS; Davis receives plain HTTP on 127.0.0.1:3012)
- Any changes to port 3010 or zeroclaw ports

---

## §1 Configuration

### `local.toml` — new `[tunnel]` section

```toml
[tunnel]
# Cloudflare Tunnel ID (from: cloudflared tunnel create davis-shortcut)
tunnel_id = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"

# Public hostname routed to this tunnel (must be a Cloudflare-managed domain)
hostname = "davis.yourdomain.com"
```

Both fields are optional at parse time; `tunnel-install` validates their presence at runtime.

`local.example.toml` gets a commented-out `[tunnel]` block documenting the two fields.

### `src/app_config.rs`

```rust
#[derive(Deserialize, Default)]
pub struct TunnelConfig {
    pub tunnel_id: Option<String>,
    pub hostname: Option<String>,
}

// Added to LocalConfig:
pub tunnel: Option<TunnelConfig>,
```

### No `[webhook] external_url`

`shortcut install --url "https://<hostname>/shortcut"` already accepts an explicit URL override. The hostname lives in `[tunnel]`; users reference that section in the install docs. No duplicate field.

---

## §2 CLI Commands

All three commands are added to `src/cli/service.rs`. No new file needed (projected final size ~850 lines; split to `src/cli/tunnel.rs` only if it exceeds 800).

### `daviszeroclaw service tunnel-install`

1. `require_command("cloudflared")` — bail: `"cloudflared not found. Install it first: brew install cloudflare/cloudflare/cloudflared"`
2. Read `local.toml [tunnel]` — bail if `tunnel_id` or `hostname` is missing: `"[tunnel] tunnel_id and hostname are required in local.toml. See local.example.toml."`
3. Verify `~/.cloudflared/<tunnel_id>.json` exists — bail: `"Tunnel credentials not found at ~/.cloudflared/<id>.json. Run: cloudflared tunnel create <name>"`
4. Write `~/.cloudflared/davis-shortcut.yml`:
   ```yaml
   tunnel: <tunnel_id>
   credentials-file: /Users/<user>/.cloudflared/<tunnel_id>.json

   ingress:
     - hostname: <hostname>
       service: http://127.0.0.1:3012
     - service: http_status:404
   ```
5. Write `~/Library/LaunchAgents/com.daviszeroclaw.tunnel.plist` via `render_tunnel_launchd_plist`
6. `launchctl bootstrap user/<uid> <plist>`
7. Poll `https://<hostname>/health` for up to 10s; print `tunnel online` or `tunnel started (health check timed out — may need a few seconds to propagate)`

### `daviszeroclaw service tunnel-uninstall`

1. `launchctl bootout user/<uid> <plist>` (ignore failure — may already be stopped)
2. Remove `~/Library/LaunchAgents/com.daviszeroclaw.tunnel.plist`
3. Remove `~/.cloudflared/davis-shortcut.yml`

### `daviszeroclaw service tunnel-status`

- Plist absent → no output (silent, consistent with proxy behavior when not installed)
- Plist present:
  - Check launchctl state
  - GET `https://<hostname>/health`, measure latency
  - Print one of:
    ```
    - tunnel: running → davis.yourdomain.com reachable (latency: 218ms)
    - tunnel: running → davis.yourdomain.com unreachable (timeout)
    - tunnel: stopped
    ```

### `daviszeroclaw service status` (existing command)

Appends the tunnel status line after the existing proxy line. Reuses `tunnel_status` logic.

---

## §3 launchd Plist

New function `render_tunnel_launchd_plist(spec)` in `service.rs`, parallel to `render_proxy_launchd_plist`:

```xml
<key>Label</key><string>com.daviszeroclaw.tunnel</string>
<key>ProgramArguments</key>
<array>
  <string>/path/to/cloudflared</string>
  <string>tunnel</string>
  <string>--config</string>
  <string>/Users/<user>/.cloudflared/davis-shortcut.yml</string>
  <string>run</string>
</array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
```

Helper `tunnel_service_plist_path()` returns `~/Library/LaunchAgents/com.daviszeroclaw.tunnel.plist`.

---

## §4 Testing

New unit tests in `src/cli/tests.rs`:

| Test | Asserts |
|---|---|
| `tunnel_install_missing_cloudflared` | `require_command` fail → correct error message with brew hint |
| `tunnel_install_missing_config` | `[tunnel]` fields absent → bail with config hint |
| `tunnel_install_missing_credentials` | credentials JSON absent → bail with `cloudflared tunnel create` hint |
| `render_tunnel_launchd_plist` | plist XML contains label, binary path, config path, KeepAlive |
| `tunnel_status_no_plist` | plist absent → no output (silent) |

`launchctl` and network calls are not unit-tested (consistent with existing service test strategy).

---

## User Setup Flow (post-implementation)

```bash
# 1. Install cloudflared
brew install cloudflare/cloudflare/cloudflared

# 2. Authorize & create tunnel (manual, once)
cloudflared login
cloudflared tunnel create davis-shortcut
cloudflared tunnel route dns davis-shortcut davis.yourdomain.com

# 3. Add to local.toml
[tunnel]
tunnel_id = "<uuid from step 2>"
hostname = "davis.yourdomain.com"

# 4. Install tunnel service
daviszeroclaw service tunnel-install

# 5. Rebuild Shortcut with external URL
daviszeroclaw shortcut install --url "https://davis.yourdomain.com/shortcut"
```
