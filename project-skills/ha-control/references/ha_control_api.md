# HA Control Proxy API

Use the Home Assistant endpoints on the Davis local proxy for control requests.

## Resolve a Target

`GET http://127.0.0.1:3010/resolve-control-target`

Query parameters:

- `query` (required): user-facing target phrase such as `书房灯带`
- `action` (required): one of `turn_on`, `turn_off`, `toggle`, `set_brightness`, `query_state`

Important response fields:

- `status`
- `reason`
- `resolution_type`
- `resolved_targets`
- `candidate_count`
- `second_best_gap`
- `matched_by`
- `confidence`
- `best_guess_used`
- `candidates`
- `suggestions`

## Execute a Control Request

`POST http://127.0.0.1:3010/execute-control`

JSON body:

```json
{
  "raw_text": "打开书房灯带",
  "query": "书房灯带",
  "action": "turn_on"
}
```

Optional fields:

- `targets`: explicit `entity_id` list
- `service_data`: for example `{ "brightness_pct": 60 }`

Important response fields:

- `status`: `success`, `partial_success`, or `failed`
- `reason`: stable machine-readable failure code such as `resolution_ambiguous`, `missing_credentials`, `ha_auth_failed`, or `ha_unreachable`
- `resolution`
- `executed_services`
- `targets`
- `speech`
- `advisor_suggestion`

Behavior notes:

- When `status` is `failed` and `reason` is `resolution_ambiguous`, treat the response as a clarification request. Do not guess a target, and do not re-run control with the same unresolved query.
- Use the returned `resolution.candidates` and top-level candidate metadata to surface the available options in the agent layer.
- Keep the final user-facing wording in the agent or skill layer, not in the proxy contract.
- The proxy `speech` field is already optimized for Shortcut / Siri brevity, for example `书房灯带已关闭。`, `书房灯带已打开，亮度51%。`, or `书房灯带亮度已调到55%。`.
- For live state queries, prefer the proxy `speech` plus `targets[*].brightness_pct` instead of reading raw Home Assistant brightness values directly.
