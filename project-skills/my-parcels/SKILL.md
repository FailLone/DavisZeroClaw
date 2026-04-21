---
name: my-parcels
description: Check the user's recent parcels, orders, purchases, or product purchase platform from the locally connected Alibaba/Taobao and JD order pages through the Davis localhost proxy. Use when the user asks about recent deliveries, packages in transit, whether anything is arriving today, whether there are pending pickup parcels, where they bought something, whether a product was bought on Taobao/Tmall/JD, or wants to search orders by platform, product name, merchant, or status. Do not ask the user for a tracking number by default.
---

# My Parcels

## Safety Rules

- Use only `http_request` against the local Davis express proxy.
- Do not browse external shopping or logistics sites directly from the model path.
- Treat parcel results as user-private account data; summarize briefly and avoid repeating unnecessary order details.
- If a source reports `needs_reauth`, tell the user which platform needs to be logged in again.
- Never claim a package, order, or purchase was found or not found unless the local express proxy has been called in the current turn.
- Do not answer with "checking", "please wait", or "retrieving" as a final response. Call the proxy first, then answer from its result.

## Workflow

1. Start from the package or order list.
Read [references/express_api.md](references/express_api.md) and call `GET /express/packages`.

2. Filter only when needed.
- If the user names a platform such as `淘宝`, `天猫`, or `京东`, pass `source=ali` or `source=jd`.
- If the user gives a product hint or status hint, pass `q`.
- If the user explicitly asks to refresh, pass `refresh=true`.
- If the user asks where they bought something, pass the product hint as `q` and search both sources unless they explicitly ask to limit the platform.
- If the user asks "was it Taobao/JD?", still search both sources first when the real platform is uncertain.

3. Interpret the response.
- `ok`: summarize the most relevant packages.
- `empty`: explain that no matching package was found.
- `partial`: answer with what is available, then mention which source needs reauth or failed.
- `needs_reauth`: tell the user to rerun the platform login helper.
- `upstream_error`: say the parcel pages could not be read for now.

## Response Style

- Prefer the proxy `speech` field when present.
- Mention platform names only when it adds value.
- If there are multiple packages, lead with counts, then the most relevant 1-3 items.
- For "where did I buy X?" questions, lead with the platform and the most recent matching item.
