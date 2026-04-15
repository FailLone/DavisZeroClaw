---
name: ha-config-advisor
description: Diagnose Home Assistant configuration quality for voice and agent control. Use when the user asks to整理、优化、体检 Home Assistant naming, aliases, areas, groups, Assist exposure, or custom sentences, or when repeated control failures suggest the configuration is not agent-friendly. This skill is read-only and should output a concrete remediation report rather than modifying HA.
---

# HA Config Advisor

## Safety Rules

- Treat this skill as read-only.
- Do not write to Home Assistant.
- Do not pretend to have changed aliases, groups, or exposure settings.
- Base advice on the proxy report and on real repeated failures when available.

## Workflow

1. Fetch the latest configuration report.
Call `GET http://127.0.0.1:3010/advisor/config-report`.

2. Fetch recent failure history when relevant.
Call `GET http://127.0.0.1:3010/advisor/failure-summary`.

3. Summarize the highest-value fixes first.
Cover:
- duplicate friendly names
- cross-domain name conflicts
- missing room semantics
- repeated ambiguous control failures, especially `resolution_ambiguous`
- suggested aliases
- suggested groups
- read-only migration suggestions for aliases and group members
- suggested Assist exposure targets
- suggested custom sentences

4. Recommend the next 3-5 concrete fixes.
Prefer precise recommendations over long generic advice.
If repeated failures include `resolution_ambiguous`, explain that the configuration is leaving the proxy with multiple plausible targets and point the user toward alias, area, group, or naming cleanup rather than generic retry advice.
If `suggestions.migration_suggestions` is present, treat it as a review checklist. Do not claim the JSON snippet has been applied; tell the user it is a proposed `control_aliases.json` change to review.

## Response Style

- Write a concise remediation report.
- Start with the highest-impact problems.
- Separate “detected now” from “recommended next”.
- Keep the tone practical and non-judgmental.
