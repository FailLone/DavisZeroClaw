# MemPalace MCP Tool Patterns

Use MemPalace through the live MCP tools exposed by the `mempalace` server. Do not hard-code request JSON from this file; always follow the current MCP tool schema and descriptions visible at runtime.

## Schema Rule

- Treat the MCP server as the source of truth for tool names, arguments, and result shapes.
- Use this file only to choose intent: status, search, KG, diary, drawer, correction, or boundary handling.
- If a tool schema differs from the names below, follow the live schema and the Memory Protocol returned by `mempalace_status`.

## Availability And Wake-Up

- Use `mempalace_status` before the first memory-dependent answer in a session, or when tool availability is uncertain.
- Follow the returned Memory Protocol. In particular: do not guess remembered facts; query first.
- Use reconnect only after external CLI changes or transient backend failures.

## Reading Personal Memory

- Use semantic search for natural-language recall: preferences, prior decisions, "do you remember", and durable personal context.
- Use KG query for stable entity/relation facts, such as user preferences or Davis behavior rules.
- Use diary read for continuity questions such as "where did we leave off" or "what happened last time".
- Fetch a full drawer only after search returns a relevant drawer identifier or equivalent result reference.

If no relevant memory is found, say no stored memory was found. Do not infer from vague memory.

## Writing Personal Memory

- Use diary writes for continuity summaries, task milestones, and session-level notes.
- Use KG writes for stable reusable facts about the user, Davis behavior, or long-running preferences.
- Use drawers for durable notes that need more context than a KG fact.
- Check duplicates before filing long text when the tool is available.

Write dates, names, URLs, commands, caveats, and the user's wording when those details matter.

## Corrections And Forgetting

When a user corrects memory:

1. Search or query the old memory.
2. Invalidate or supersede the outdated structured fact when the tool is available.
3. Add the corrected fact with a date and enough context.
4. Make clear which version is current.

Update drawers only when an existing drawer should be edited in place. Delete drawers only when the user explicitly wants removal.

## Live Tool Boundary

Do not use memory as the source of truth for live data. Query the right tool first:

- Home state: Home Assistant / HA MCP.
- Browser state or website content: browser bridge.
- Orders and packages: express/order tools.
- Logs and runtime status: Davis local proxy or local files.
- Current web facts: web search when appropriate.

After live verification, store only a reusable conclusion if it will help later.

## Never Store

- Secrets, raw credentials, API keys, passwords, tokens, or one-time codes.
- Raw chat filler, throwaway brainstorms, and temporary work buffers.
- Large raw logs, browser dumps, order tables, or message transcripts.
- Unverified web claims as facts; store sourced notes with uncertainty only when durable.

If the user asks to operate MemPalace itself, switch to the vendor `mempalace` skill.
