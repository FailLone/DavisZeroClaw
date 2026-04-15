import http from "node:http";
import { spawn, execFile as execFileCallback } from "node:child_process";
import { promisify } from "node:util";
import { mkdir } from "node:fs/promises";
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";

const execFile = promisify(execFileCallback);

const port = Number(process.env.DAVIS_BROWSER_BRIDGE_PORT || "3011");
const defaultProfile = process.env.DAVIS_BROWSER_DEFAULT_PROFILE || "user";
const profiles = JSON.parse(process.env.DAVIS_BROWSER_PROFILES_JSON || "[]");
const remoteDebuggingUrl =
  process.env.DAVIS_BROWSER_REMOTE_DEBUGGING_URL || "http://127.0.0.1:9222";
const allowAppleScriptFallback =
  process.env.DAVIS_BROWSER_ALLOW_APPLESCRIPT_FALLBACK !== "false";
const screenshotsDir =
  process.env.DAVIS_BROWSER_SCREENSHOTS_DIR ||
  path.join(process.cwd(), ".runtime", "davis", "browser-screenshots");
const profilesDir =
  process.env.DAVIS_BROWSER_PROFILES_DIR ||
  path.join(process.cwd(), ".runtime", "davis", "browser-profiles");

const managedPageIds = new WeakMap();
let nextManagedTabId = 1;
let playwrightModulePromise = null;
let managedContextPromise = null;

async function runOsascript(script, args = []) {
  return await new Promise((resolve, reject) => {
    const child = spawn("osascript", ["-", ...args], {
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) {
        resolve({ stdout, stderr });
      } else {
        reject(new Error(stderr.trim() || `osascript exited with ${code}`));
      }
    });
    child.stdin.end(script);
  });
}

function isoNow() {
  return new Date().toISOString();
}

function jsonResponse(res, payload, statusCode = 200) {
  res.writeHead(statusCode, { "content-type": "application/json; charset=utf-8" });
  res.end(JSON.stringify(payload));
}

function badRequest(message) {
  return actionResponse({
    status: "upstream_error",
    issue_type: "bad_request",
    message,
  });
}

function actionResponse({
  status = "ok",
  profile = null,
  tab_id = null,
  current_url = null,
  title = null,
  message = null,
  issue_type = null,
  action_preview = null,
  data = null,
}) {
  return {
    status,
    checked_at: isoNow(),
    profile,
    tab_id,
    current_url,
    title,
    message,
    issue_type,
    action_preview,
    data,
  };
}

async function readJsonBody(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  if (chunks.length === 0) return {};
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

function resolveProfile(requested) {
  const candidate = typeof requested === "string" && requested.trim() ? requested.trim() : defaultProfile;
  return profiles.find((profile) => profile.name === candidate)?.name || defaultProfile;
}

async function loadPlaywright() {
  if (!playwrightModulePromise) {
    playwrightModulePromise = import("playwright").catch(() => null);
  }
  return playwrightModulePromise;
}

async function isRemoteDebuggingReachable() {
  try {
    const response = await fetch(`${remoteDebuggingUrl}/json/version`);
    return response.ok;
  } catch {
    return false;
  }
}

async function listChromeTabs() {
  const script = `
set text item delimiters to ""
set lineSep to character id 10
set fieldSep to character id 31
set outLines to {}
tell application "Google Chrome"
  set frontWindowIndex to -1
  try
    set frontWindowIndex to index of front window
  end try
  repeat with wIndex from 1 to count of windows
    set w to window wIndex
    set activeIndex to active tab index of w
    repeat with tIndex from 1 to count of tabs of w
      set t to tab tIndex of w
      set isActive to "false"
      if (index of w = frontWindowIndex) and tIndex = activeIndex then
        set isActive to "true"
      end if
      set safeURL to my sanitizeText(URL of t)
      set safeTitle to my sanitizeText(title of t)
      set end of outLines to ("w" & wIndex & ":t" & tIndex & fieldSep & isActive & fieldSep & safeURL & fieldSep & safeTitle)
    end repeat
  end repeat
end tell
set AppleScript's text item delimiters to lineSep
set joinedText to outLines as text
set AppleScript's text item delimiters to ""
return joinedText

on sanitizeText(inputText)
  set outputText to inputText as text
  set outputText to my replaceText(outputText, character id 10, " ")
  set outputText to my replaceText(outputText, character id 13, " ")
  set outputText to my replaceText(outputText, character id 31, " ")
  return outputText
end sanitizeText

on replaceText(sourceText, searchText, replaceText)
  set AppleScript's text item delimiters to searchText
  set itemsList to every text item of sourceText
  set AppleScript's text item delimiters to replaceText
  set joinedText to itemsList as text
  set AppleScript's text item delimiters to ""
  return joinedText
end replaceText
`;
  const { stdout } = await execFile("osascript", ["-e", script]);
  const fieldSep = String.fromCharCode(31);
  return stdout
    .split(/\n+/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [tabId, active, currentUrl, title] = line.split(fieldSep);
      return {
        tab_id: tabId,
        profile: "user",
        active: active === "true",
        writable: false,
        current_url: currentUrl || null,
        title: title || null,
      };
    });
}

async function openChromeUrl(url, newTab = false) {
  if (newTab) {
    await execFile("open", ["-a", "Google Chrome", url]);
    return;
  }
  const script = `
on run argv
  set targetUrl to item 1 of argv
  tell application "Google Chrome"
    if (count of windows) = 0 then
      make new window
    end if
    set URL of active tab of front window to targetUrl
    activate
  end tell
end run
`;
  await runOsascript(script, [url]);
}

async function focusChromeTab(tabId) {
  const match = /^w(\d+):t(\d+)$/.exec(tabId || "");
  if (!match) {
    throw new Error("invalid tab id");
  }
  const [_, windowIndex, tabIndex] = match;
  const script = `
on run argv
  set targetWindowIndex to (item 1 of argv) as integer
  set targetTabIndex to (item 2 of argv) as integer
  tell application "Google Chrome"
    set index of window targetWindowIndex to 1
    set active tab index of front window to targetTabIndex
    activate
  end tell
end run
`;
  await runOsascript(script, [windowIndex, tabIndex]);
}

async function executeChromeJavascript(tabId, jsSource) {
  const match = /^w(\d+):t(\d+)$/.exec(tabId || "");
  if (!match) {
    throw new Error("invalid tab id");
  }
  const [_, windowIndex, tabIndex] = match;
  const script = `
on run argv
  set targetWindowIndex to (item 1 of argv) as integer
  set targetTabIndex to (item 2 of argv) as integer
  set jsSource to item 3 of argv
  tell application "Google Chrome"
    set resultJson to execute tab targetTabIndex of window targetWindowIndex javascript jsSource
    return resultJson
  end tell
end run
`;
  const { stdout } = await runOsascript(script, [windowIndex, tabIndex, jsSource]);
  return stdout.trim();
}

function snapshotScript(format, selector) {
  const selectorArg = selector ? JSON.stringify(selector) : "null";
  return `JSON.stringify((() => {
    const selector = ${selectorArg};
    const root = selector ? document.querySelector(selector) : document.body;
    if (!root) {
      return { found: false, format: ${JSON.stringify(format || "text")}, content: null };
    }
    const mode = ${JSON.stringify(format || "text")};
    if (mode === "html") {
      return { found: true, format: mode, content: root.outerHTML || root.innerHTML || "" };
    }
    if (mode === "interactive") {
      const nodes = Array.from(root.querySelectorAll('a,button,input,textarea,select')).slice(0, 200).map((node, index) => ({
        index: index + 1,
        tag: (node.tagName || '').toLowerCase(),
        text: (node.innerText || node.value || node.getAttribute('aria-label') || '').trim(),
        selector_hint: node.id ? '#' + node.id : (node.name ? '[name=\"' + node.name + '\"]' : null)
      }));
      return { found: true, format: mode, content: nodes };
    }
    return { found: true, format: mode, content: root.innerText || "" };
  })())`;
}

function safeJsonParse(raw) {
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

async function getManagedContext() {
  if (!managedContextPromise) {
    managedContextPromise = (async () => {
      const playwright = await loadPlaywright();
      if (!playwright?.chromium) {
        throw new Error("playwright is not installed");
      }
      const userDataDir = path.join(profilesDir, "managed");
      await mkdir(userDataDir, { recursive: true });
      const context = await playwright.chromium.launchPersistentContext(userDataDir, {
        headless: true,
      });
      if (context.pages().length === 0) {
        await context.newPage();
      }
      return context;
    })();
  }
  return managedContextPromise;
}

function assignManagedTabId(page) {
  if (!managedPageIds.has(page)) {
    managedPageIds.set(page, `managed-${nextManagedTabId++}`);
  }
  return managedPageIds.get(page);
}

async function listManagedTabs() {
  const context = await getManagedContext();
  const pages = context.pages();
  return Promise.all(
    pages.map(async (page, index) => ({
      tab_id: assignManagedTabId(page),
      profile: "managed",
      active: index === pages.length - 1,
      writable: true,
      current_url: page.url() || null,
      title: await page.title().catch(() => null),
    })),
  );
}

async function resolveManagedPage(tabId) {
  const context = await getManagedContext();
  const pages = context.pages();
  const selected =
    (tabId && pages.find((page) => assignManagedTabId(page) === tabId)) || pages[pages.length - 1];
  if (!selected) {
    throw new Error("managed page not found");
  }
  return selected;
}

async function statusPayload() {
  const remoteDebuggingReachable = await isRemoteDebuggingReachable();
  const playwright = await loadPlaywright();
  const profilesPayload = [
    {
      profile: "user",
      mode: "existing_session",
      browser: "chrome",
      status:
        remoteDebuggingReachable || allowAppleScriptFallback ? "ok" : "needs_reauth",
      writable: false,
      fallback_in_use: !remoteDebuggingReachable && allowAppleScriptFallback,
      message: remoteDebuggingReachable
        ? "Chrome remote debugging reachable"
        : allowAppleScriptFallback
          ? "Chrome remote debugging unavailable, using AppleScript read-only fallback"
          : "Chrome remote debugging unavailable",
    },
    {
      profile: "managed",
      mode: "managed",
      browser: "chromium",
      status: playwright?.chromium ? "ok" : "unsupported_surface",
      writable: Boolean(playwright?.chromium),
      fallback_in_use: false,
      message: playwright?.chromium
        ? "Playwright managed browser available"
        : "Playwright not installed; managed browser unavailable",
    },
  ];
  return {
    status: profilesPayload.some((profile) => profile.status === "ok") ? "ok" : "upstream_error",
    checked_at: isoNow(),
    worker_available: true,
    worker_url: `http://127.0.0.1:${port}`,
    profiles: profilesPayload,
    message: "browser worker ready",
  };
}

async function handleStatus(req, res) {
  jsonResponse(res, await statusPayload());
}

async function handleProfiles(req, res) {
  const status = await statusPayload();
  jsonResponse(res, {
    status: status.status,
    checked_at: status.checked_at,
    default_profile: defaultProfile,
    profiles: status.profiles,
  });
}

async function handleTabs(req, res, url) {
  const profile = resolveProfile(url.searchParams.get("profile"));
  if (profile === "user") {
    try {
      const tabs = await listChromeTabs();
      jsonResponse(res, {
        status: "ok",
        checked_at: isoNow(),
        profile,
        tabs,
        message: "read tabs from Google Chrome",
      });
    } catch (error) {
      jsonResponse(res, {
        status: "upstream_error",
        checked_at: isoNow(),
        profile,
        tabs: [],
        message: String(error.message || error),
      });
    }
    return;
  }
  try {
    const tabs = await listManagedTabs();
    jsonResponse(res, {
      status: "ok",
      checked_at: isoNow(),
      profile,
      tabs,
      message: "read tabs from managed browser",
    });
  } catch (error) {
    jsonResponse(res, {
      status: "unsupported_surface",
      checked_at: isoNow(),
      profile,
      tabs: [],
      message: String(error.message || error),
      issue_type: "unsupported_surface",
    });
  }
}

async function handleOpen(req, res, body) {
  const profile = resolveProfile(body.profile);
  const url = String(body.url || "").trim();
  if (!url) {
    return jsonResponse(res, badRequest("url is required"), 400);
  }
  if (profile === "user") {
    try {
      await openChromeUrl(url, Boolean(body.new_tab));
      return jsonResponse(
        res,
        actionResponse({
          profile,
          current_url: url,
          message: "opened url in Google Chrome",
        }),
      );
    } catch (error) {
      return jsonResponse(
        res,
        actionResponse({
          status: "upstream_error",
          profile,
          current_url: url,
          message: String(error.message || error),
          issue_type: "browser_bridge_unavailable",
        }),
      );
    }
  }
  try {
    const context = await getManagedContext();
    const page = body.new_tab ? await context.newPage() : await resolveManagedPage();
    await page.goto(url);
    return jsonResponse(
      res,
      actionResponse({
        profile,
        tab_id: assignManagedTabId(page),
        current_url: page.url(),
        title: await page.title().catch(() => null),
        message: "opened url in managed browser",
      }),
    );
  } catch (error) {
    return jsonResponse(
      res,
      actionResponse({
        status: "unsupported_surface",
        profile,
        current_url: url,
        message: String(error.message || error),
        issue_type: "unsupported_surface",
      }),
    );
  }
}

async function handleFocus(req, res, body) {
  const profile = resolveProfile(body.profile);
  const tabId = String(body.tab_id || "").trim();
  if (!tabId) {
    return jsonResponse(res, badRequest("tab_id is required"), 400);
  }
  if (profile === "user") {
    try {
      await focusChromeTab(tabId);
      return jsonResponse(
        res,
        actionResponse({ profile, tab_id: tabId, message: "focused Chrome tab" }),
      );
    } catch (error) {
      return jsonResponse(
        res,
        actionResponse({
          status: "upstream_error",
          profile,
          tab_id: tabId,
          message: String(error.message || error),
          issue_type: "browser_bridge_unavailable",
        }),
      );
    }
  }
  try {
    const page = await resolveManagedPage(tabId);
    await page.bringToFront();
    return jsonResponse(
      res,
      actionResponse({
        profile,
        tab_id: assignManagedTabId(page),
        current_url: page.url(),
        title: await page.title().catch(() => null),
        message: "focused managed browser tab",
      }),
    );
  } catch (error) {
    return jsonResponse(
      res,
      actionResponse({
        status: "unsupported_surface",
        profile,
        tab_id: tabId,
        message: String(error.message || error),
        issue_type: "unsupported_surface",
      }),
    );
  }
}

async function handleSnapshot(req, res, body) {
  const profile = resolveProfile(body.profile);
  const tabId = body.tab_id ? String(body.tab_id) : null;
  const format = body.format || "text";
  const selector = body.selector || null;
  if (profile === "user") {
    try {
      const tabs = await listChromeTabs();
      const activeTab = tabId ? tabs.find((tab) => tab.tab_id === tabId) : tabs.find((tab) => tab.active);
      if (!activeTab) {
        throw new Error("Chrome tab not found");
      }
      const raw = await executeChromeJavascript(activeTab.tab_id, snapshotScript(format, selector));
      return jsonResponse(
        res,
        actionResponse({
          profile,
          tab_id: activeTab.tab_id,
          current_url: activeTab.current_url,
          title: activeTab.title,
          message: "snapshot captured from Google Chrome",
          data: safeJsonParse(raw),
        }),
      );
    } catch (error) {
      return jsonResponse(
        res,
        actionResponse({
          status: "upstream_error",
          profile,
          tab_id: tabId,
          message: String(error.message || error),
          issue_type: "browser_bridge_unavailable",
        }),
      );
    }
  }
  try {
    const page = await resolveManagedPage(tabId);
    const data = await page.evaluate(
      ({ mode, selector }) => {
        const root = selector ? document.querySelector(selector) : document.body;
        if (!root) {
          return { found: false, format: mode, content: null };
        }
        if (mode === "html") {
          return { found: true, format: mode, content: root.outerHTML || root.innerHTML || "" };
        }
        if (mode === "interactive") {
          return {
            found: true,
            format: mode,
            content: Array.from(
              root.querySelectorAll("a,button,input,textarea,select"),
            )
              .slice(0, 200)
              .map((node, index) => ({
                index: index + 1,
                tag: (node.tagName || "").toLowerCase(),
                text: (node.innerText || node.value || node.getAttribute("aria-label") || "").trim(),
              })),
          };
        }
        return { found: true, format: mode, content: root.innerText || "" };
      },
      { mode: format, selector },
    );
    return jsonResponse(
      res,
      actionResponse({
        profile,
        tab_id: assignManagedTabId(page),
        current_url: page.url(),
        title: await page.title().catch(() => null),
        message: "snapshot captured from managed browser",
        data,
      }),
    );
  } catch (error) {
    return jsonResponse(
      res,
      actionResponse({
        status: "unsupported_surface",
        profile,
        tab_id: tabId,
        message: String(error.message || error),
        issue_type: "unsupported_surface",
      }),
    );
  }
}

async function handleEvaluate(req, res, body) {
  const profile = resolveProfile(body.profile);
  const tabId = body.tab_id ? String(body.tab_id) : null;
  const script = String(body.script || "").trim();
  if (!script) {
    return jsonResponse(res, badRequest("script is required"), 400);
  }
  if (profile === "user") {
    try {
      const tabs = await listChromeTabs();
      const activeTab = tabId ? tabs.find((tab) => tab.tab_id === tabId) : tabs.find((tab) => tab.active);
      if (!activeTab) {
        throw new Error("Chrome tab not found");
      }
      const raw = await executeChromeJavascript(activeTab.tab_id, script);
      return jsonResponse(
        res,
        actionResponse({
          profile,
          tab_id: activeTab.tab_id,
          current_url: activeTab.current_url,
          title: activeTab.title,
          message: "executed javascript in Google Chrome",
          data: safeJsonParse(raw),
        }),
      );
    } catch (error) {
      return jsonResponse(
        res,
        actionResponse({
          status: "upstream_error",
          profile,
          tab_id: tabId,
          message: String(error.message || error),
          issue_type: "browser_bridge_unavailable",
        }),
      );
    }
  }
  try {
    const page = await resolveManagedPage(tabId);
    const data = await page.evaluate(script);
    return jsonResponse(
      res,
      actionResponse({
        profile,
        tab_id: assignManagedTabId(page),
        current_url: page.url(),
        title: await page.title().catch(() => null),
        message: "executed javascript in managed browser",
        data,
      }),
    );
  } catch (error) {
    return jsonResponse(
      res,
      actionResponse({
        status: "unsupported_surface",
        profile,
        tab_id: tabId,
        message: String(error.message || error),
        issue_type: "unsupported_surface",
      }),
    );
  }
}

async function handleAction(req, res, body) {
  const profile = resolveProfile(body.profile);
  const tabId = body.tab_id ? String(body.tab_id) : null;
  const action = String(body.action || "").trim();
  const target = body.target || {};
  const payload = body.payload || {};
  if (!action) {
    return jsonResponse(res, badRequest("action is required"), 400);
  }
  if (profile === "user") {
    return jsonResponse(
      res,
      actionResponse({
        status: "unsupported_surface",
        profile,
        tab_id: tabId,
        message: "user Chrome profile currently supports read-only fallback only",
        issue_type: "unsupported_surface",
      }),
    );
  }
  try {
    const page = await resolveManagedPage(tabId);
    const selector = target.selector || null;
    const text = target.text || null;
    if (action === "click") {
      if (selector) {
        await page.click(selector);
      } else if (text) {
        await page.getByText(text).first().click();
      } else {
        throw new Error("click requires selector or text");
      }
    } else if (action === "type" || action === "fill") {
      const value = String(payload.value ?? payload.text ?? "");
      if (!selector) {
        throw new Error(`${action} requires target.selector`);
      }
      if (action === "type") {
        await page.locator(selector).first().type(value);
      } else {
        await page.locator(selector).first().fill(value);
      }
    } else if (action === "press") {
      const key = String(payload.key || "");
      if (!key) {
        throw new Error("press requires payload.key");
      }
      await page.keyboard.press(key);
    } else if (action === "select") {
      const value = String(payload.value || "");
      if (!selector || !value) {
        throw new Error("select requires target.selector and payload.value");
      }
      await page.selectOption(selector, value);
    } else if (action === "scroll") {
      const y = Number(payload.y ?? 600);
      await page.evaluate((deltaY) => window.scrollBy(0, deltaY), y);
    } else {
      throw new Error(`unsupported action: ${action}`);
    }
    return jsonResponse(
      res,
      actionResponse({
        profile,
        tab_id: assignManagedTabId(page),
        current_url: page.url(),
        title: await page.title().catch(() => null),
        message: `executed ${action} in managed browser`,
      }),
    );
  } catch (error) {
    return jsonResponse(
      res,
      actionResponse({
        status: "unsupported_surface",
        profile,
        tab_id: tabId,
        message: String(error.message || error),
        issue_type: "unsupported_surface",
      }),
    );
  }
}

async function handleScreenshot(req, res, body) {
  const profile = resolveProfile(body.profile);
  if (profile === "user") {
    return jsonResponse(
      res,
      actionResponse({
        status: "unsupported_surface",
        profile,
        tab_id: body.tab_id || null,
        message: "screenshots are only supported for managed browser right now",
        issue_type: "unsupported_surface",
      }),
    );
  }
  try {
    await mkdir(screenshotsDir, { recursive: true });
    const page = await resolveManagedPage(body.tab_id ? String(body.tab_id) : null);
    const screenshotPath = path.join(
      screenshotsDir,
      `${Date.now()}-${crypto.randomUUID()}.png`,
    );
    await page.screenshot({ path: screenshotPath, fullPage: Boolean(body.full_page) });
    return jsonResponse(
      res,
      actionResponse({
        profile,
        tab_id: assignManagedTabId(page),
        current_url: page.url(),
        title: await page.title().catch(() => null),
        message: "captured managed browser screenshot",
        data: { screenshot_path: screenshotPath },
      }),
    );
  } catch (error) {
    return jsonResponse(
      res,
      actionResponse({
        status: "unsupported_surface",
        profile,
        tab_id: body.tab_id || null,
        message: String(error.message || error),
        issue_type: "unsupported_surface",
      }),
    );
  }
}

async function handleWait(req, res, body) {
  const profile = resolveProfile(body.profile);
  if (profile === "user") {
    return jsonResponse(
      res,
      actionResponse({
        status: "unsupported_surface",
        profile,
        tab_id: body.tab_id || null,
        message: "wait is only supported for managed browser right now",
        issue_type: "unsupported_surface",
      }),
    );
  }
  try {
    const page = await resolveManagedPage(body.tab_id ? String(body.tab_id) : null);
    const timeout = Number(body.timeout_ms || 10000);
    if (body.selector) {
      await page.waitForSelector(String(body.selector), { timeout });
    } else if (body.text) {
      await page.getByText(String(body.text)).first().waitFor({ timeout });
    } else if (body.url_pattern) {
      await page.waitForURL(new RegExp(String(body.url_pattern)), { timeout });
    } else {
      await page.waitForLoadState("networkidle", { timeout });
    }
    return jsonResponse(
      res,
      actionResponse({
        profile,
        tab_id: assignManagedTabId(page),
        current_url: page.url(),
        title: await page.title().catch(() => null),
        message: "wait condition satisfied in managed browser",
      }),
    );
  } catch (error) {
    return jsonResponse(
      res,
      actionResponse({
        status: "unsupported_surface",
        profile,
        tab_id: body.tab_id || null,
        message: String(error.message || error),
        issue_type: "unsupported_surface",
      }),
    );
  }
}

const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url || "/", `http://127.0.0.1:${port}`);
    if (req.method === "GET" && url.pathname === "/status") return handleStatus(req, res);
    if (req.method === "GET" && url.pathname === "/profiles") return handleProfiles(req, res);
    if (req.method === "GET" && url.pathname === "/tabs") return handleTabs(req, res, url);

    const body =
      req.method === "POST" ? await readJsonBody(req).catch(() => ({ __bad_json__: true })) : {};
    if (body.__bad_json__) return jsonResponse(res, badRequest("invalid JSON body"), 400);

    if (req.method === "POST" && url.pathname === "/open") return handleOpen(req, res, body);
    if (req.method === "POST" && url.pathname === "/focus") return handleFocus(req, res, body);
    if (req.method === "POST" && url.pathname === "/snapshot") return handleSnapshot(req, res, body);
    if (req.method === "POST" && url.pathname === "/evaluate") return handleEvaluate(req, res, body);
    if (req.method === "POST" && url.pathname === "/action") return handleAction(req, res, body);
    if (req.method === "POST" && url.pathname === "/screenshot") return handleScreenshot(req, res, body);
    if (req.method === "POST" && url.pathname === "/wait") return handleWait(req, res, body);

    jsonResponse(
      res,
      {
        status: "upstream_error",
        checked_at: isoNow(),
        message: `unsupported route: ${req.method} ${url.pathname}`,
      },
      404,
    );
  } catch (error) {
    jsonResponse(
      res,
      {
        status: "upstream_error",
        checked_at: isoNow(),
        message: String(error.message || error),
      },
      500,
    );
  }
});

server.listen(port, "127.0.0.1", async () => {
  try {
    await mkdir(screenshotsDir, { recursive: true });
    await mkdir(profilesDir, { recursive: true });
  } catch {}
  process.stdout.write(`browser worker listening on http://127.0.0.1:${port}\n`);
});

process.on("SIGTERM", async () => {
  server.close(() => process.exit(0));
});
