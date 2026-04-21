# Express Proxy API

This skill talks only to the local Davis express proxy.

When a source requires reauthentication, the corresponding login helpers are:

- `daviszeroclaw crawl profile login express-ali`
- `daviszeroclaw crawl profile login express-jd`

Useful CLI helpers:

- `daviszeroclaw crawl source list`
- `daviszeroclaw crawl run express-packages --refresh`

## Endpoints

`GET http://127.0.0.1:3010/express/auth-status`

Returns per-source login status for:

- `ali`
- `jd`

`GET http://127.0.0.1:3010/express/packages`

Default behavior for generic parcel questions:

- Omit `source` to search both Taobao and JD immediately.
- Use `refresh=true` when the user asks about `最近`, `刚刚`, `最新`, or explicitly requests a refresh.
- Do not ask a platform clarification question before calling this endpoint unless the user explicitly requires platform restriction.

Query parameters:

- `source` (optional): `ali`, `jd`, or omitted for both
- `q` (optional): free-text filter for title, status, carrier, shop name, or masked tracking text
- `refresh` (optional): `true` / `1` to bypass cache

`GET http://127.0.0.1:3010/express/search`

Alias of `express/packages`, using `q` or `query`.

Use this endpoint for purchase-platform questions such as "where did I buy contact lenses?" with:

`GET http://127.0.0.1:3010/express/search?q=隐形眼镜`

## Important Response Fields

- `status`: `ok`, `empty`, `partial`, `needs_reauth`, or `upstream_error`
- `speech`
- `package_count`
- `packages`
- `sources`

Each package can include:

- `source`
- `merchant`
- `title`
- `shop_name`
- `status`
- `latest_update`
- `latest_time`
- `carrier`
- `tracking_no_masked`
- `pickup_code_masked`
- `eta_text`
- `detail_url`
