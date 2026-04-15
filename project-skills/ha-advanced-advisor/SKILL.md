---
name: ha-advanced-advisor
description: Advise on advanced Home Assistant architecture and power-user patterns such as automation layering, scripts, scenes, custom sentences, and KNX-friendly naming and exposure. Use when the user asks how to structure higher-level HA logic beyond basic naming cleanup, or when `ha-config-advisor` reveals deeper architectural opportunities.
---

# HA Advanced Advisor

## Safety Rules

- This skill is advisory only.
- Do not modify Home Assistant.
- Do not recommend rewriting everything at once; prioritize the smallest architectural step that improves reliability.

## Workflow

1. Read the current HA advisor report.
Call `GET http://127.0.0.1:3010/advisor/config-report`.

2. Focus on architecture-level opportunities.
Cover only the relevant subset of:
- automation layering and naming
- when to introduce `script` / `scene`
- when to encode phrases through custom sentences
- how to separate HA-native logic from Davis/LLM logic
- KNX-facing naming and exposure strategy

3. Propose a staged roadmap.
Prefer:
- immediate cleanup
- next-level abstractions
- later advanced integration patterns

## Response Style

- Be strategic, not exhaustive.
- Tie every recommendation back to an observed issue or likely failure mode.
- Prefer small, durable patterns over clever one-offs.
