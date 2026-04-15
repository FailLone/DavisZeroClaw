# HA Audit Proxy API

This skill talks only to the local Davis HA audit proxy.
It is intended for historical audit queries, not present-tense live status checks.

## Endpoint

`GET http://127.0.0.1:3010/resolve-entity`

Query parameters:

- `entity_id` (required, may be a full entity ID, a suffix such as `main_bedroom_on_off`, or a friendly name)

Important response fields:

- `status`: `ok`, `not_found`, `ambiguous`, or `config_issue`
- `resolved_entity_id`
- `matched_by`
- `friendly_name`
- `related_entity_ids`
- `suggestions`
- `issue`

`GET http://127.0.0.1:3010/audit`

Query parameters:

- `entity_id` (required, may be a full entity ID, a suffix such as `main_bedroom_on_off`, or a friendly name)
- `window_minutes` (optional, default `60`)
- `start` (optional, ISO 8601)
- `end` (optional, ISO 8601)

Use either `window_minutes` or an explicit `start`/`end` pair.

## Example Request

```text
http://127.0.0.1:3010/audit?entity_id=<entity_id>&window_minutes=60
```

## Result Types

### `evidence`

Returned when the proxy found relevant logbook and/or history data.

Important fields:

- `resolved_entity_id`
- `matched_by`
- `findings`
- `actor`
- `source`
- `confidence`
- `counts`
- `entities`

### `config_issue`

Returned when the audit could not run due to an environment or Home Assistant issue.

Important fields:

- `issue`

Expected `issue.issue_type` values:

- `missing_credentials`
- `ha_unreachable`
- `ha_auth_failed`
- `recorder_not_enabled`
- `entity_not_found`
- `entity_ambiguous`
- `bad_request`

### `no_evidence`

Returned when the proxy successfully queried logbook and history, but did not find enough evidence in the requested window.

Important fields:

- `queried_sources`
- `missing_evidence_types`
- `possible_reasons`
- `current_state`
- `confidence`

## Required Tool Usage

Use `http_request` with a simple `GET`.
Do not call Home Assistant write tools while using this skill.
