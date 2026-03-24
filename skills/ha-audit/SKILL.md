---
name: ha-audit
description: Investigate Home Assistant history in a strict read-only way. Use when the user asks about a past time window such as last night, earlier today, between specific times, who changed an entity, why it toggled before, whether it was turned on or off in the past, or to audit a Home Assistant entity without performing any write operation. Do not use this skill for present-tense live status questions such as whether a light is on now. This skill uses the local Davis HA audit proxy through http_request and distinguishes between evidence found, Home Assistant configuration issues, and no evidence found.
---

# HA Audit

## Safety Rules

- Treat all audit requests as read-only by default.
- Never call any Home Assistant tool during an audit request. Do not use `homeassistant__*` tools at all.
- Use only `http_request` against the local Davis audit proxy.
- Do not fabricate actors, causes, or timelines. If evidence is weak, say so.
- Use this skill only for historical questions. If the user is asking about the current state right now, do not use this skill.

## Workflow

1. Confirm that the request is historical.
Use this skill only when the user is asking about the past: a previous time window, a prior change, or an audit question.
If the request is about the current live state, answer it through the normal non-audit path instead of this skill.

2. Identify the target `entity_id`.
Pass the user's raw entity hint to the audit proxy. The proxy can resolve exact entity IDs, suffixes such as `main_bedroom_on_off`, and friendly names such as `主卧空调`.
If the user is mainly asking "what is the real entity_id", call the proxy's resolver endpoint first.
Ask one short clarification question only if the proxy reports ambiguity.

3. Determine the audit window.
Default to the last 60 minutes unless the user specifies another window.

4. Call the local audit proxy.
Read [references/ha_audit_api.md](references/ha_audit_api.md) for the endpoint, query parameters, and response fields.

5. Interpret the result.
- `evidence`: synthesize your own concise answer from `resolved_entity_id`, `findings`, `actor`, `source`, `confidence`, and the per-entity timelines.
- `config_issue`: explain the blocker using `issue.issue_type`, `issue.issue_category`, `issue.recommended_actions`, and `issue.suggestions`.
- `no_evidence`: state that logbook and history were queried but no sufficient evidence was found in the requested window, then explain the gap using `missing_evidence_types` and `possible_reasons`.

## Response Style

- Keep the answer concise and factual.
- Separate "what we know" from "what we cannot prove".
- Treat the proxy as a data source, not as the final narrator. Write the user-facing answer in natural language.
- If the proxy returns `config_issue`, prefer actionable guidance over speculation.
- If the proxy returns `no_evidence`, do not present it as a configuration failure unless the issue payload explicitly indicates one.
