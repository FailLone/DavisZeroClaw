# Browser Bridge API

This skill talks only to the local Davis browser bridge proxy.

## Endpoints

`GET http://127.0.0.1:3010/browser/status`

Returns worker health plus per-profile readiness.

`GET http://127.0.0.1:3010/browser/profiles`

Returns configured browser profiles and their runtime state.

`GET http://127.0.0.1:3010/browser/tabs?profile=user|managed`

Returns current tabs for the selected profile.

`POST http://127.0.0.1:3010/browser/open`

Body:

```json
{ "profile": "user", "url": "https://example.com", "new_tab": true }
```

`POST http://127.0.0.1:3010/browser/focus`

Body:

```json
{ "profile": "user", "tab_id": "w1:t1" }
```

`POST http://127.0.0.1:3010/browser/snapshot`

Body:

```json
{ "profile": "user", "tab_id": "w1:t1", "format": "text", "selector": "body" }
```

`POST http://127.0.0.1:3010/browser/evaluate`

Body:

```json
{ "profile": "user", "tab_id": "w1:t1", "mode": "read", "script": "JSON.stringify({ title: document.title })" }
```

`POST http://127.0.0.1:3010/browser/action`

Body:

```json
{
  "profile": "managed",
  "tab_id": "managed-1",
  "action": "click",
  "target": { "selector": "button.submit" },
  "payload": {}
}
```

`POST http://127.0.0.1:3010/browser/screenshot`

`POST http://127.0.0.1:3010/browser/wait`

## Important Response Fields

- `status`: `ok`, `requires_confirmation`, `write_blocked`, `unsupported_surface`, `needs_reauth`, or `upstream_error`
- `profile`
- `tab_id`
- `current_url`
- `title`
- `message`
- `issue`
- `issue_type`
- `action_preview`
- `data`
