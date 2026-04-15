---
name: ha-control
description: Control Home Assistant entities in the present tense through the local Davis HA proxy. Use when the user wants to turn devices on or off, toggle them, adjust brightness, or control a room-level light group such as “书房的灯”. Prefer this skill over direct `homeassistant__HassTurnOn` / `homeassistant__HassTurnOff` calls so Davis can resolve fuzzy Chinese names into explicit entity IDs and attach configuration advice after repeated failures or ambiguous matches.
---

# HA Control

## Safety Rules

- Use this skill only for present-tense control requests.
- Use `query_state` only when the user is explicitly asking about the current live state of a specific Home Assistant entity or room.
- Do not call `homeassistant__HassTurnOn` or `homeassistant__HassTurnOff` directly.
- Use only `http_request` against the local Davis HA proxy. Do not invent a custom control interface name.
- Prefer the local Davis HA proxy over raw Home Assistant write tools.
- If the user is asking about the past, use `ha-audit` instead.
- If the user is only asking for architecture advice, use `ha-config-advisor` or `ha-advanced-advisor` instead.
- Do not turn general conversation, troubleshooting questions, or historical investigation into device control.

## Routing Hints

Prefer `ha-control` for requests such as:

- “打开书房灯带”
- “把父母间吊灯关掉”
- “把书房的灯打开”
- “书房灯带现在开着吗”

Do not use `ha-control` for requests such as:

- “昨晚是谁关的父母间吊灯”
- “之前书房灯带为什么反复开关”
- “昨天 10 点到 11 点谁动过客厅灯”
- “为什么 Home Assistant 里这个实体总掉线”

## Workflow

1. Extract the intended action.
Map the request to one of:
- `turn_on`
- `turn_off`
- `toggle`
- `set_brightness`
- `query_state`

2. Resolve the target phrase.
Use the user’s natural-language device phrase as `query`.
If the user uses a pronoun such as “它”, infer the target from the current thread context first.

3. Call the local control endpoint.
Use `http_request`, and only these endpoints:

- `GET http://127.0.0.1:3010/resolve-control-target?query=...&action=...`
- `POST http://127.0.0.1:3010/execute-control`

Read [references/ha_control_api.md](references/ha_control_api.md) for the exact request and response shapes.

For execution, send JSON like:

```json
{
  "raw_text": "打开书房灯带",
  "query": "书房灯带",
  "action": "turn_on"
}
```

Execution rules:

- Resolve first, then execute.
- For `POST /execute-control`, set `Content-Type: application/json`.
- Send a plain JSON object body. Do not send form data. Do not wrap the body inside another envelope such as `input`, `payload`, or `request`.
- Put `query`, `action`, and optional `service_data` in the JSON body, not in the URL query string.

4. Use the proxy result as the source of truth.
- If `status` is `success`, tell the user the action completed.
- If `status` is `partial_success`, explain what succeeded and what did not.
- If `status` is `failed`, inspect `reason` before deciding how to respond.
- If `reason` is `resolution_ambiguous`, do not guess and do not retry the same query blindly. Use the returned `candidates` metadata to ask the user to choose a target, and preserve the candidate list in the conversation context.
- If `reason` indicates a configuration or resolution issue, explain that the proxy could not resolve the target and rely on the structured fields instead of inventing a device name.
- If the proxy returns `advisor_suggestion`, append a short recommendation to run `ha-config-advisor`.

## Response Style

- Keep the answer short and action-oriented.
- Prefer the proxy’s `speech` field when present.
- For `query_state`, keep the spoken answer to one short sentence per entity.
- Do not claim a device changed state unless the proxy says the action succeeded.
