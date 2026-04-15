---
name: ha-reconciliation-advisor
description: Review likely Home Assistant entity replacements after gateway refreshes or sync churn. Use when entities become unavailable, new similarly named entities appear, or when the user asks whether old and new HA devices should be mapped together. This skill is advisory only and should produce migration suggestions rather than modifying aliases or groups automatically.
---

# HA Reconciliation Advisor

## Safety Rules

- Treat this skill as read-only.
- Do not modify Home Assistant, `control_aliases.json`, or runtime state.
- Do not claim two entities are definitely the same device unless the evidence is unusually strong.
- Prefer "likely replacement" / "needs review" wording over absolute statements.

## Workflow

1. Read the candidate report.
Call `GET http://127.0.0.1:3010/advisor/replacement-candidates`.

2. Read the broader advisor report when needed.
Call `GET http://127.0.0.1:3010/advisor/config-report`.

3. Review the top candidates.
For each important candidate, cover:
- old unavailable entity name
- possible replacement entity name
- domain and area evidence
- why it looks like a replacement vs a duplicate exposure
- whether alias/group migration should be considered now or only after manual confirmation

4. Recommend the next 3-5 concrete checks or migrations.
Prefer:
- "migrate alias/group after confirmation"
- "keep under review because it may be a duplicate exposure"
- "fix area assignment first"

## Response Style

- Start with the strongest candidates.
- Separate `high_confidence` from `needs_review`.
- Keep the advice practical and reversible.
