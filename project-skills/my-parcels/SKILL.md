---
name: my-parcels
description: Check the user's recent parcels, orders, purchases, or product purchase platform from the locally connected Alibaba/Taobao and JD order pages through the Davis localhost proxy. Use when the user asks about recent deliveries, packages in transit, whether anything is arriving today, whether there are pending pickup parcels, where they bought something, whether a product was bought on Taobao/Tmall/JD, or wants to search orders by platform, product name, merchant, or status. Default to searching both Taobao and JD unless the user explicitly narrows the platform. For generic parcel questions such as “帮我查一下最近的快递”, immediately call the local proxy and answer in the same turn without asking clarifying questions or replying with placeholder text.
---

# My Parcels

## Immediate Action Rule

- When this skill applies, you must query the local express proxy before replying.
- For generic parcel questions, answer in one turn. Do not reply with “请稍等”, “我正在查询”, “我来帮你看看”, or any other placeholder-only response.
- Do not ask which platform to check unless the user explicitly requires a platform restriction that changes the query.

## Safety Rules

- Use only `http_request` against the local Davis express proxy.
- Do not browse external shopping or logistics sites directly from the model path.
- Treat parcel results as user-private account data; summarize briefly and avoid repeating unnecessary order details.
- If a source reports `needs_reauth`, tell the user which platform needs to be logged in again with `daviszeroclaw crawl profile login express-ali` or `daviszeroclaw crawl profile login express-jd`.
- For manual CLI debugging, prefer `daviszeroclaw crawl run express-packages --refresh` over ad hoc browser automation.
- Never claim a package, order, or purchase was found or not found unless the local express proxy has been called in the current turn.
- Do not ask which platform to check when the user asks a generic parcel question such as “帮我查最近的快递”, “我最近有什么包裹”, or “我买的东西到了吗”. The proxy already supports querying both sources by default.
- Do not answer with "checking", "please wait", "retrieving", `请稍等`, or `正在查询` as the final response. Call the proxy first, then answer from its result.

## Workflow

1. Start from the package or order list.
Read [references/express_api.md](references/express_api.md) and call `GET /express/packages`.
For generic parcel questions, call it without `source` so both Taobao and JD are searched immediately.

2. Filter only when needed.
- If the user names a platform such as `淘宝`, `天猫`, or `京东`, pass `source=ali` or `source=jd`.
- If the user gives a product hint or status hint, pass `q`.
- If the user explicitly asks to refresh, or asks about `最近` / `刚刚` / `最新`, pass `refresh=true`.
- If the user asks where they bought something, pass the product hint as `q` and search both sources unless they explicitly ask to limit the platform.
- If the user asks "was it Taobao/JD?", still search both sources first when the real platform is uncertain.
- If the user asks a broad question like “帮我查快递”, “最近的包裹”, or “我最近买了什么”, do not ask follow-up questions before querying the proxy.

3. Interpret the response.
- `ok`: summarize the most relevant packages.
- `empty`: explain that no matching package was found.
- `partial`: answer with what is available, then mention which source needs reauth or failed.
- `needs_reauth`: tell the user to rerun the relevant `daviszeroclaw crawl profile login ...` command.
- `upstream_error`: say the parcel pages could not be read for now.

## Response Style

- Prefer the proxy `speech` field when present.
- Mention platform names only when it adds value.
- If there are multiple packages, lead with counts, then the most relevant 1-3 items.
- For "where did I buy X?" questions, lead with the platform and the most recent matching item.
- For generic parcel questions, lead with the overall count and cross-platform summary first, not a request for clarification.
