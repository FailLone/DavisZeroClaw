use crate::{
    build_issue, crawl4ai_crawl, isoformat, normalize_text, now_utc, Crawl4aiConfig,
    Crawl4aiPageRequest, Crawl4aiProfileLocks, ExpressAuthStatusResponse, ExpressPackage,
    ExpressPackagesResponse, ExpressSourceSnapshot, ExpressSourceStatus, RuntimePaths,
};
use futures::future::join_all;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

const EXPRESS_CACHE_TTL_SECS: i64 = 600;
// If this grows beyond ~3 entries, swap the join_all fan-out in
// express_auth_status / express_packages for buffer_unordered(N) to cap
// concurrent Chromium attaches against the crawl4ai adapter.
const EXPRESS_SOURCES: [&str; 2] = ["ali", "jd"];
const EXPRESS_PAYLOAD_ATTR: &str = "data-davis-express-payload=\"";

pub async fn express_auth_status(
    paths: RuntimePaths,
    crawl4ai_config: Crawl4aiConfig,
    profile_locks: Crawl4aiProfileLocks,
) -> ExpressAuthStatusResponse {
    // Fan out per-source fetches concurrently. Each future acquires its own
    // per-profile lock *inside* the async block so contention is scoped to
    // same-profile calls; different sources proceed in parallel.
    let source_futures = EXPRESS_SOURCES.iter().map(|source| {
        let paths = paths.clone();
        let cfg = crawl4ai_config.clone();
        let locks = profile_locks.clone();
        async move {
            let lock = acquire_profile_lock(&locks, &express_profile_name(source)).await;
            fetch_source_status(&paths, &cfg, lock, source).await
        }
    });
    let sources: Vec<_> = join_all(source_futures).await;
    ExpressAuthStatusResponse {
        status: aggregate_status_from_statuses(&sources),
        checked_at: isoformat(now_utc()),
        sources,
    }
}

pub async fn express_packages(
    paths: RuntimePaths,
    crawl4ai_config: Crawl4aiConfig,
    profile_locks: Crawl4aiProfileLocks,
    source: Option<String>,
    query: Option<String>,
    force_refresh: bool,
) -> ExpressPackagesResponse {
    // Same fan-out pattern as express_auth_status: per-profile lock acquired
    // inside each future so distinct sources don't serialize on the HashMap
    // mutex for the full crawl duration.
    let snapshot_futures = select_sources(source.as_deref())
        .into_iter()
        .map(|selected_source| {
            let paths = paths.clone();
            let cfg = crawl4ai_config.clone();
            let locks = profile_locks.clone();
            async move {
                let lock =
                    acquire_profile_lock(&locks, &express_profile_name(selected_source)).await;
                load_or_fetch_source(&paths, &cfg, lock, selected_source, force_refresh).await
            }
        });
    let snapshots: Vec<_> = join_all(snapshot_futures).await;

    let normalized_query = query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_text);
    let mut packages = snapshots
        .iter()
        .flat_map(|snapshot| snapshot.packages.clone())
        .collect::<Vec<_>>();
    if let Some(ref needle) = normalized_query {
        packages.retain(|package| package_matches_query(package, needle));
    }
    packages.sort_by(|left, right| {
        right
            .latest_time
            .cmp(&left.latest_time)
            .then_with(|| left.source.cmp(&right.source))
    });

    let status = aggregate_status(&snapshots, packages.len());
    let speech = build_speech(&status, &packages, query.as_deref());
    ExpressPackagesResponse {
        status,
        checked_at: isoformat(now_utc()),
        source: source
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        query: query
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        refreshed: force_refresh,
        package_count: packages.len(),
        packages,
        sources: snapshots
            .into_iter()
            .map(|snapshot| snapshot.source_status)
            .collect(),
        speech,
    }
}

fn select_sources(source: Option<&str>) -> Vec<&'static str> {
    match source.map(str::trim).filter(|value| !value.is_empty()) {
        Some("ali") => vec!["ali"],
        Some("jd") => vec!["jd"],
        _ => EXPRESS_SOURCES.to_vec(),
    }
}

async fn load_or_fetch_source(
    paths: &RuntimePaths,
    crawl4ai_config: &Crawl4aiConfig,
    profile_lock: Arc<Mutex<()>>,
    source: &str,
    force_refresh: bool,
) -> ExpressSourceSnapshot {
    if !force_refresh {
        if let Some(snapshot) = read_cache(paths.express_cache_path(source)) {
            if cache_is_fresh(&snapshot.source_status.checked_at) {
                let mut cached = snapshot;
                cached.source_status.cached = true;
                return cached;
            }
        }
    }
    let snapshot = fetch_source_snapshot(paths, crawl4ai_config, profile_lock, source).await;
    let _ = write_cache(paths.express_cache_path(source), &snapshot);
    snapshot
}

async fn fetch_source_status(
    paths: &RuntimePaths,
    crawl4ai_config: &Crawl4aiConfig,
    profile_lock: Arc<Mutex<()>>,
    source: &str,
) -> ExpressSourceStatus {
    match crawl_source_payload(
        paths,
        crawl4ai_config,
        profile_lock,
        source,
        auth_script(source),
    )
    .await
    {
        Ok(payload) => parse_source_status(source, &payload, None, None),
        Err(message) => {
            source_error_snapshot(source, "upstream_error", "crawl4ai_unavailable", message)
                .source_status
        }
    }
}

async fn fetch_source_snapshot(
    paths: &RuntimePaths,
    crawl4ai_config: &Crawl4aiConfig,
    profile_lock: Arc<Mutex<()>>,
    source: &str,
) -> ExpressSourceSnapshot {
    match crawl_source_payload(
        paths,
        crawl4ai_config,
        profile_lock,
        source,
        packages_script(source),
    )
    .await
    {
        Ok(payload) => {
            parse_snapshot_payload(source, &payload, None, None).unwrap_or_else(|message| {
                source_error_snapshot(source, "upstream_error", "site_changed", message)
            })
        }
        Err(message) => {
            source_error_snapshot(source, "upstream_error", "crawl4ai_unavailable", message)
        }
    }
}

fn source_order_url(source: &str) -> &'static str {
    match source {
        "ali" => "https://buyertrade.taobao.com/trade/itemlist/list_bought_items.htm",
        "jd" => "https://order.jd.com/center/list.action",
        _ => "",
    }
}

fn source_login_message(source: &str) -> String {
    match source {
        "ali" => "请先在 Crawl4AI 托管 profile 中登录淘宝订单页".to_string(),
        "jd" => "请先在 Crawl4AI 托管 profile 中登录京东订单页".to_string(),
        _ => "请先在 Crawl4AI 托管 profile 中登录对应站点页面".to_string(),
    }
}

fn auth_script(source: &str) -> String {
    wrap_payload_script(match source {
        "ali" => "(() => { const href = location.href; const title = document.title || ''; const body = document.body ? document.body.innerText : ''; const loggedIn = href.includes('buyertrade.taobao.com') && (body.includes('订单号') || body.includes('已买到的宝贝') || body.includes('查看物流') || body.includes('订单详情')); return { source: 'ali', status: loggedIn ? 'ok' : 'needs_reauth', checked_at: new Date().toISOString(), logged_in: loggedIn, package_count: 0, current_url: href, title, message: loggedIn ? '淘宝订单页登录状态正常' : '需要重新登录淘宝订单页', issue_type: loggedIn ? null : 'auth_required', packages: [] }; })()",
        "jd" => "(() => { const href = location.href; const title = document.title || ''; const body = document.body ? document.body.innerText : ''; const loggedIn = href.includes('order.jd.com') && (body.includes('我的订单') || body.includes('订单号') || body.includes('查看物流') || body.includes('订单详情')); return { source: 'jd', status: loggedIn ? 'ok' : 'needs_reauth', checked_at: new Date().toISOString(), logged_in: loggedIn, package_count: 0, current_url: href, title, message: loggedIn ? '京东订单页登录状态正常' : '需要重新登录京东订单页', issue_type: loggedIn ? null : 'auth_required', packages: [] }; })()",
        _ => "({ status: 'upstream_error', issue_type: 'site_changed' })",
    })
}

fn packages_script(source: &str) -> String {
    wrap_payload_script(match source {
        "ali" => "(() => { const href = location.href; const title = document.title || ''; const body = document.body ? document.body.innerText : ''; const loggedIn = href.includes('buyertrade.taobao.com') && (body.includes('订单号') || body.includes('已买到的宝贝') || body.includes('查看物流') || body.includes('订单详情')); if (!loggedIn) { return { source: 'ali', status: 'needs_reauth', checked_at: new Date().toISOString(), logged_in: false, package_count: 0, current_url: href, title, message: '需要重新登录淘宝订单页', issue_type: 'auth_required', packages: [] }; } const cards = Array.from(document.querySelectorAll('.trade-container-shopOrderContainer, .trade-bought-list-order-container')); const splitLines = (text) => text.split(/\\n+/).map(line => line.trim()).filter(Boolean); const statusWords = ['待收货','待发货','已完成','已签收','退款中','交易成功','待取件','派送中','卖家已发货','买家已付款']; const carrierWords = ['顺丰','中通','圆通','韵达','申通','极兔','邮政','德邦','菜鸟','京东快递','EMS']; const firstMatch = (lines, words) => lines.find(line => words.some(word => line.includes(word))) || null; const longest = (values) => values.filter(Boolean).sort((a, b) => b.length - a.length)[0] || null; const packages = cards.map((card, index) => { const text = (card.innerText || '').trim(); const lines = splitLines(text); const links = Array.from(card.querySelectorAll('a[href]')).map(link => ({ text: (link.innerText || '').trim(), href: link.href })).filter(item => item.text && !item.text.includes('订单详情') && !item.text.includes('查看物流')); const detailLink = Array.from(card.querySelectorAll('a[href]')).find(link => { const hrefValue = link.href || ''; return hrefValue.includes('detail') || hrefValue.includes('trade') || hrefValue.includes('logistics'); }); return { id: card.id || ('ali-' + String(index + 1)), source: 'ali', merchant: text.includes('天猫') ? 'tmall' : 'taobao', title: longest(links.map(item => item.text)) || null, shop_name: lines.find(line => line.includes('旗舰店') || line.includes('专营店') || line.includes('企业店') || line.includes('运营中心') || line.includes('代购')) || null, status: firstMatch(lines, statusWords), latest_update: lines.find(line => /(物流|派件|签收|取件|驿站|运输|发货|退款中)/.test(line)) || null, latest_time: lines.find(line => /\\d{4}-\\d{2}-\\d{2}|\\d{2}:\\d{2}/.test(line)) || null, carrier: firstMatch(lines, carrierWords), tracking_no_masked: lines.find(line => /(订单号|运单|物流单号|快递单号|单号)/.test(line)) || null, pickup_code_masked: lines.find(line => /(取件码|提货码)/.test(line)) || null, eta_text: lines.find(line => /(预计|送达|到达|发货承诺)/.test(line)) || null, detail_url: detailLink ? detailLink.href : null, raw_source_meta: { text_excerpt: lines.slice(0, 12) } }; }).filter(item => item.title || item.status || item.latest_update); return { source: 'ali', status: packages.length ? 'ok' : 'empty', checked_at: new Date().toISOString(), logged_in: true, package_count: packages.length, current_url: href, title, message: packages.length ? '已读取淘宝订单列表' : '淘宝订单页已登录，但暂未提取到包裹卡片', packages }; })()",
        "jd" => "(() => { const href = location.href; const title = document.title || ''; const body = document.body ? document.body.innerText : ''; const loggedIn = href.includes('order.jd.com') && (body.includes('我的订单') || body.includes('订单号') || body.includes('查看物流') || body.includes('订单详情')); if (!loggedIn) { return { source: 'jd', status: 'needs_reauth', checked_at: new Date().toISOString(), logged_in: false, package_count: 0, current_url: href, title, message: '需要重新登录京东订单页', issue_type: 'auth_required', packages: [] }; } const cards = Array.from(document.querySelectorAll('tbody[id^=\"tb-\"]')); const statusWords = ['待收货','已签收','待取件','配送中','已发货','已完成','待付款','订单回收站']; const carrierWords = ['京东快递','京东物流','顺丰','中通','圆通','韵达','申通','极兔','邮政','德邦','EMS']; const splitLines = (text) => text.split(/\\n+/).map(line => line.trim()).filter(Boolean); const firstMatch = (lines, words) => lines.find(line => words.some(word => line.includes(word))) || null; const longest = (values) => values.filter(Boolean).sort((a, b) => b.length - a.length)[0] || null; const packages = cards.map((card, index) => { const text = (card.innerText || '').trim(); const lines = splitLines(text); const links = Array.from(card.querySelectorAll('a[href]')).map(link => ({ text: (link.innerText || '').trim(), href: link.href })).filter(item => item.text && !item.text.includes('订单详情') && !item.text.includes('查看物流')); const detailLink = Array.from(card.querySelectorAll('a[href]')).find(link => { const hrefValue = link.href || ''; return hrefValue.includes('detail') || hrefValue.includes('track') || hrefValue.includes('order'); }); return { id: card.id || ('jd-' + String(index + 1)), source: 'jd', merchant: 'jd', title: longest(links.map(item => item.text)) || null, shop_name: lines.find(line => line.includes('旗舰店') || line.includes('专营店') || line.includes('京东自营') || line.includes('京东大药房')) || null, status: firstMatch(lines, statusWords), latest_update: lines.find(line => /(配送|签收|揽收|运输|取件|驿站)/.test(line)) || null, latest_time: lines.find(line => /\\d{4}-\\d{2}-\\d{2}|\\d{2}:\\d{2}/.test(line)) || null, carrier: firstMatch(lines, carrierWords), tracking_no_masked: lines.find(line => /(订单号|运单|物流单号|快递单号)/.test(line)) || null, pickup_code_masked: lines.find(line => /(取件码|提货码)/.test(line)) || null, eta_text: lines.find(line => /(预计|送达|到达)/.test(line)) || null, detail_url: detailLink ? detailLink.href : null, raw_source_meta: { text_excerpt: lines.slice(0, 12) } }; }).filter(item => item.title || item.status || item.latest_update); return { source: 'jd', status: packages.length ? 'ok' : 'empty', checked_at: new Date().toISOString(), logged_in: true, package_count: packages.length, current_url: href, title, message: packages.length ? '已读取京东订单列表' : '京东订单页已登录，但暂未提取到包裹卡片', packages }; })()",
        _ => "({ status: 'upstream_error', issue_type: 'site_changed' })",
    })
}

async fn crawl_source_payload(
    paths: &RuntimePaths,
    crawl4ai_config: &Crawl4aiConfig,
    profile_lock: Arc<Mutex<()>>,
    source: &str,
    script: String,
) -> Result<Value, String> {
    // Serialize concurrent fetches against the same Chromium user_data_dir;
    // two attaches on one profile race on `SingletonLock` and the second
    // fails opaquely. Guard is released when this function returns.
    let _guard = profile_lock.lock().await;
    let response = crawl4ai_crawl(
        paths,
        crawl4ai_config,
        Crawl4aiPageRequest {
            profile_name: express_profile_name(source),
            url: source_order_url(source).to_string(),
            wait_for: Some(source_wait_for(source).to_string()),
            js_code: Some(script),
        },
    )
    .await?;
    if !response.success {
        return Err(response.error_message.unwrap_or_else(|| {
            format!(
                "{}。请先运行 `daviszeroclaw crawl profile login express-{source}` 完成登录。",
                source_login_message(source)
            )
        }));
    }
    extract_payload_value(&response).map_err(|message| {
        format!(
            "failed to parse crawl4ai payload for {source}: {message}. 请确认 `daviszeroclaw crawl profile login express-{source}` 已完成并且订单页结构未变化。"
        )
    })
}

async fn acquire_profile_lock(
    profile_locks: &Crawl4aiProfileLocks,
    profile: &str,
) -> Arc<Mutex<()>> {
    let mut map = profile_locks.lock().await;
    map.entry(profile.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn express_profile_name(source: &str) -> String {
    format!("express-{source}")
}

fn source_wait_for(source: &str) -> &'static str {
    match source {
        "ali" => "js:() => !!document.body && (document.querySelector('.trade-container-shopOrderContainer, .trade-bought-list-order-container') || document.body.innerText.includes('已买到的宝贝') || document.body.innerText.includes('登录'))",
        "jd" => "js:() => !!document.body && (document.querySelector('tbody[id^=\"tb-\"]') || document.body.innerText.includes('我的订单') || document.body.innerText.includes('登录'))",
        _ => "css:body",
    }
}

fn wrap_payload_script(payload_expression: &str) -> String {
    format!(
        "(function() {{ const payload = {payload_expression}; const root = document.body || document.documentElement; if (!root) {{ return payload; }} let marker = root.querySelector('[data-davis-express-payload]'); if (!marker) {{ marker = document.createElement('div'); marker.hidden = true; root.appendChild(marker); }} marker.setAttribute('data-davis-express-payload', encodeURIComponent(JSON.stringify(payload))); marker.textContent = 'davis-express-payload'; return payload; }})();"
    )
}

fn extract_payload_value(response: &crate::Crawl4aiPageResult) -> Result<Value, String> {
    if let Some(value) = extract_js_execution_payload(&response.raw) {
        return Ok(value);
    }
    for html in [response.html.as_deref(), response.cleaned_html.as_deref()]
        .into_iter()
        .flatten()
    {
        if let Ok(value) = extract_payload_from_html(html) {
            return Ok(value);
        }
    }
    Err("express payload marker not found in crawl4ai html".to_string())
}

fn extract_payload_from_html(html: &str) -> Result<Value, String> {
    let start = html
        .find(EXPRESS_PAYLOAD_ATTR)
        .ok_or_else(|| "express payload marker not found in crawl4ai html".to_string())?
        + EXPRESS_PAYLOAD_ATTR.len();
    let rest = &html[start..];
    let end = rest
        .find('"')
        .ok_or_else(|| "express payload marker was truncated".to_string())?;
    let encoded = &rest[..end];
    let decoded = urlencoding::decode(encoded)
        .map_err(|err| format!("decode express payload from crawl4ai html: {err}"))?;
    serde_json::from_str::<Value>(&decoded)
        .map_err(|err| format!("parse express payload from crawl4ai html: {err}"))
}

fn extract_js_execution_payload(raw: &Value) -> Option<Value> {
    let direct = raw.get("js_execution_result");
    let nested = direct.and_then(|value| value.get("result"));
    let value = nested.or(direct)?;
    let normalized = normalize_script_value(value);
    if normalized.get("status").is_some() || normalized.get("source").is_some() {
        Some(normalized)
    } else {
        None
    }
}

fn parse_source_status(
    source: &str,
    data: &Value,
    current_url: Option<String>,
    title: Option<String>,
) -> ExpressSourceStatus {
    let checked_at = isoformat(now_utc());
    let value = normalize_script_value(data);
    let issue_type = value
        .get("issue_type")
        .and_then(Value::as_str)
        .map(str::to_string);
    ExpressSourceStatus {
        source: source.to_string(),
        status: value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("upstream_error")
            .to_string(),
        checked_at: value
            .get("checked_at")
            .and_then(Value::as_str)
            .unwrap_or(&checked_at)
            .to_string(),
        logged_in: value
            .get("logged_in")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        package_count: value
            .get("package_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        cached: false,
        current_url: value
            .get("current_url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(current_url),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(title),
        message: value
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
        issue: issue_type.map(|kind| build_issue(&kind, &format!("express:{source}"), Vec::new())),
    }
}

fn parse_snapshot_payload(
    source: &str,
    data: &Value,
    current_url: Option<String>,
    title: Option<String>,
) -> Result<ExpressSourceSnapshot, String> {
    let checked_at = isoformat(now_utc());
    let value = normalize_script_value(data);
    let packages = value
        .get("packages")
        .cloned()
        .map(serde_json::from_value::<Vec<ExpressPackage>>)
        .transpose()
        .map_err(|err| format!("invalid express packages payload: {err}"))?
        .unwrap_or_default();
    let issue_type = value
        .get("issue_type")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(ExpressSourceSnapshot {
        source_status: ExpressSourceStatus {
            source: source.to_string(),
            status: value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("upstream_error")
                .to_string(),
            checked_at: value
                .get("checked_at")
                .and_then(Value::as_str)
                .unwrap_or(&checked_at)
                .to_string(),
            logged_in: value
                .get("logged_in")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            package_count: value
                .get("package_count")
                .and_then(Value::as_u64)
                .map(|count| count as usize)
                .unwrap_or(packages.len()),
            cached: false,
            current_url: value
                .get("current_url")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(current_url),
            title: value
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(title),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string),
            issue: issue_type
                .map(|kind| build_issue(&kind, &format!("express:{source}"), Vec::new())),
        },
        packages,
    })
}

fn normalize_script_value(data: &Value) -> Value {
    match data {
        Value::String(raw) => {
            serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.clone()))
        }
        other => other.clone(),
    }
}

fn cache_is_fresh(value: &str) -> bool {
    crate::parse_time(value)
        .map(|checked_at| (now_utc() - checked_at).num_seconds() <= EXPRESS_CACHE_TTL_SECS)
        .unwrap_or(false)
}

fn read_cache(path: impl AsRef<Path>) -> Option<ExpressSourceSnapshot> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_cache(path: impl AsRef<Path>, snapshot: &ExpressSourceSnapshot) -> std::io::Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_vec_pretty(snapshot)
        .map_err(|err| std::io::Error::other(format!("serialize express cache: {err}")))?;
    fs::write(path, raw)
}

fn source_error_snapshot(
    source: &str,
    status: &str,
    issue_type: &str,
    message: String,
) -> ExpressSourceSnapshot {
    ExpressSourceSnapshot {
        source_status: ExpressSourceStatus {
            source: source.to_string(),
            status: status.to_string(),
            checked_at: isoformat(now_utc()),
            logged_in: false,
            package_count: 0,
            cached: false,
            current_url: None,
            title: None,
            message: Some(message),
            issue: Some(build_issue(
                issue_type,
                &format!("express:{source}"),
                Vec::new(),
            )),
        },
        packages: Vec::new(),
    }
}

fn package_matches_query(package: &ExpressPackage, query: &str) -> bool {
    let haystack = [
        package.source.as_str(),
        package.merchant.as_deref().unwrap_or_default(),
        package.title.as_deref().unwrap_or_default(),
        package.shop_name.as_deref().unwrap_or_default(),
        package.status.as_deref().unwrap_or_default(),
        package.latest_update.as_deref().unwrap_or_default(),
        package.carrier.as_deref().unwrap_or_default(),
        package.tracking_no_masked.as_deref().unwrap_or_default(),
        package.pickup_code_masked.as_deref().unwrap_or_default(),
    ]
    .join(" ");
    normalize_text(&haystack).contains(query)
}

fn aggregate_status(sources: &[ExpressSourceSnapshot], package_count: usize) -> String {
    if sources.is_empty() {
        return "upstream_error".to_string();
    }
    let ok_like_count = sources
        .iter()
        .filter(|snapshot| matches!(snapshot.source_status.status.as_str(), "ok" | "empty"))
        .count();
    let needs_reauth_count = sources
        .iter()
        .filter(|snapshot| snapshot.source_status.status == "needs_reauth")
        .count();
    let upstream_error_count = sources
        .iter()
        .filter(|snapshot| snapshot.source_status.status == "upstream_error")
        .count();

    if package_count > 0 {
        if ok_like_count == sources.len() {
            return "ok".to_string();
        }
        return "partial".to_string();
    }
    if ok_like_count == sources.len() {
        return "empty".to_string();
    }
    if needs_reauth_count == sources.len() {
        return "needs_reauth".to_string();
    }
    if upstream_error_count == sources.len() {
        return "upstream_error".to_string();
    }
    "partial".to_string()
}

fn aggregate_status_from_statuses(sources: &[ExpressSourceStatus]) -> String {
    if sources.is_empty() {
        return "upstream_error".to_string();
    }
    let ok_like_count = sources
        .iter()
        .filter(|source| matches!(source.status.as_str(), "ok" | "empty"))
        .count();
    let needs_reauth_count = sources
        .iter()
        .filter(|source| source.status == "needs_reauth")
        .count();
    let upstream_error_count = sources
        .iter()
        .filter(|source| source.status == "upstream_error")
        .count();

    if ok_like_count == sources.len() {
        return "ok".to_string();
    }
    if needs_reauth_count == sources.len() {
        return "needs_reauth".to_string();
    }
    if upstream_error_count == sources.len() {
        return "upstream_error".to_string();
    }
    "partial".to_string()
}

fn build_speech(status: &str, packages: &[ExpressPackage], query: Option<&str>) -> Option<String> {
    match status {
        "needs_reauth" => Some("淘宝或京东的登录状态已失效，请重新登录后再试。".to_string()),
        "upstream_error" => Some("快递页面暂时无法读取，请稍后再试。".to_string()),
        _ => {
            if packages.is_empty() {
                return Some(
                    match query.map(str::trim).filter(|value| !value.is_empty()) {
                        Some(q) => format!("没有找到和“{q}”相关的快递记录。"),
                        None => "最近没有读到快递记录。".to_string(),
                    },
                );
            }

            let transit_count = packages
                .iter()
                .filter(|package| {
                    package
                        .status
                        .as_deref()
                        .map(is_transit_status)
                        .unwrap_or(false)
                })
                .count();
            let pickup_count = packages
                .iter()
                .filter(|package| {
                    package
                        .status
                        .as_deref()
                        .map(is_pickup_status)
                        .unwrap_or(false)
                })
                .count();
            let signed_count = packages
                .iter()
                .filter(|package| {
                    package
                        .status
                        .as_deref()
                        .map(is_signed_status)
                        .unwrap_or(false)
                })
                .count();
            Some(format!(
                "共找到 {} 个包裹，其中 {} 个在途，{} 个待取件，{} 个已签收。",
                packages.len(),
                transit_count,
                pickup_count,
                signed_count
            ))
        }
    }
}

fn is_transit_status(value: &str) -> bool {
    ["运输", "在途", "派件", "揽收", "出库", "发货"]
        .iter()
        .any(|needle| value.contains(needle))
}

fn is_pickup_status(value: &str) -> bool {
    ["待取", "驿站", "取件码", "投柜"]
        .iter()
        .any(|needle| value.contains(needle))
}

fn is_signed_status(value: &str) -> bool {
    ["签收", "已收货", "完成"]
        .iter()
        .any(|needle| value.contains(needle))
}
