# My Parcels SOP

## Steps

1. **Read parcel data** — Call the local Davis express proxy at `GET http://127.0.0.1:3010/express/packages`. Omit `source` for generic parcel questions so Taobao and JD are both queried. Add `refresh=true` for `最近`, `刚刚`, or `最新`.
   - tools: http_request

2. **Handle login state** — If the response status is `needs_reauth` or `partial`, identify which source needs reauthentication and tell the operator to run `daviszeroclaw crawl profile login express-ali` or `daviszeroclaw crawl profile login express-jd`.
   - tools: http_request

3. **Summarize the result** — Answer with the overall count first, then the most relevant 1-3 parcels or the requested platform/product match. Do not reply with placeholder text before the proxy response is available.
