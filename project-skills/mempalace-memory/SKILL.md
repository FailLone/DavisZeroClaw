---
name: mempalace-memory
description: Decide when Davis should use MemPalace as authoritative personal long-term memory instead of ZeroClaw built-in conversational memory. Use for remembered user preferences, personal facts, prior decisions, ongoing life/admin context, "do you remember" questions, "remember this" requests, corrections, forgetting, superseding facts, diary-style continuity, and answers that depend on earlier conversations. Do not use for MemPalace setup, project mining, direct search, status, repair, CLI help, or one-off tasks whose answer is already in the current context.
---

# MemPalace Memory Policy

Use this skill when Davis needs personal durable memory for day-to-day agent runtime behavior.
DavisZeroClaw is not a project to mine by default; it is the agent runtime using MemPalace.
MemPalace is the user's long-term memory layer, not the default store for raw logs, secrets, or project files.

For MemPalace maintenance operations such as setup, mine, status, repair, CLI help, or direct manual search, use the vendor `mempalace` skill instead.

## Use MemPalace When

- The user asks whether Davis remembers something.
- The user asks about prior preferences, personal facts, previous decisions, recurring tasks, or long-running context.
- The user asks Davis to remember, correct, forget, supersede, or preserve something.
- The answer depends on earlier conversations rather than only the current chat.
- A stable claim about the user or Davis behavior would otherwise be guessed.
- An external tool finds a useful conclusion that should be reusable later.

## Do Not Use MemPalace When

- The answer is fully contained in the current message or current open context.
- The user is asking for transient brainstorming, drafting, translation, or one-off coding help.
- A live source of truth is required first, such as Home Assistant state, browser state, orders, logs, or current web data.
- The content is a secret, API key, password, token, one-time code, or raw credential.
- The memory would be vague chat filler with no future value.
- The user asks to operate MemPalace itself; use the vendor `mempalace` skill instead.

## Runtime Protocol

1. Load the protocol.
When doing memory work and MemPalace status has not been checked in this session, call `mempalace__mempalace_status` first and follow the returned Memory Protocol.

2. Read before answering.
Before answering from remembered facts, use MemPalace MCP search, KG, or diary tools. If nothing relevant is found, say no stored memory was found.

3. Verify live facts elsewhere.
For live or tool-owned facts, query the appropriate tool first. Store only the durable conclusion if it will help later.

4. Write deliberately.
Only write durable memory for stable preferences, confirmed facts, important decisions, corrections, and session milestones. Preserve dates, names, URLs, commands, caveats, and the user's wording when relevant.

5. Correct instead of piling up contradictions.
When the user corrects a stored fact, search for the old fact first, invalidate or supersede it when possible, then add the corrected fact and mark which version is current.

## Placement

Choose stable, reusable locations when storing drawers or structured memory.

- User preferences, personal facts, and habits: `wing_user`.
- Davis behavior rules and memory policy: `wing_davis`.
- Durable smart-home knowledge: `wing_home`.
- Shopping or order conclusions: `wing_shopping`.
- Learning goals and research interests: `wing_learning`.
- User-defined long-running projects: `wing_projects`.

Use short topic rooms such as `memory-system`, `contact-lenses`, or `agent-learning`. Reuse the same room name across wings when topics connect. If unsure, prefer a broad stable room over inventing a narrow one.

## Boundary With ZeroClaw Memory

ZeroClaw built-in memory is short-term conversational context. MemPalace is the authoritative durable memory for cross-session personal recall.

## Tool Guidance

For MCP tool selection and Davis-specific boundaries, read [references/mempalace_tools.md](references/mempalace_tools.md). Follow the live MCP schema at runtime; do not rely on hard-coded JSON examples.
