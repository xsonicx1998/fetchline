// Background-worker tests (S13 finish / Milestone 2). Run with:
//   node --test extension/tests/background.test.js
// from app/. No browser is needed: background.js runs in a vm
// context against a fake chrome API. The assertions cover the
// context-menu hand-offs — "Download with Fetchline" and
// "Download all links on this page" — and the last-hand-off /
// pending-count recording the popup reads (Milestone 2).
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const backgroundSrc = fs.readFileSync(path.join(__dirname, "..", "background.js"), "utf8");

const tick = (ms = 10) => new Promise((resolve) => setTimeout(resolve, ms));

function loadBackground({ nativeReply = { ok: true }, tabs = [], tabSendMessage = null } = {}) {
  const storage = {};
  const sentMessages = [];
  const injected = [];
  let nativeHandler = () => nativeReply;
  const listeners = {};
  const chrome = {
    storage: {
      local: {
        get(keys) {
          const out = {};
          for (const k of keys) if (k in storage) out[k] = storage[k];
          return Promise.resolve(out);
        },
        set(obj) {
          Object.assign(storage, obj);
          return Promise.resolve();
        },
      },
    },
    runtime: {
      lastError: null,
      onInstalled: { addListener(fn) { listeners.installed = fn; } },
      onStartup: { addListener(fn) { listeners.startup = fn; } },
      onMessage: { addListener(fn) { listeners.message = fn; } },
      sendNativeMessage(_host, message, cb) {
        sentMessages.push(message);
        const reply = nativeHandler(message);
        if (reply instanceof Error) {
          chrome.runtime.lastError = { message: reply.message };
          cb(null);
        } else {
          chrome.runtime.lastError = null;
          cb(reply);
        }
      },
    },
    contextMenus: {
      create() {},
      onClicked: { addListener(fn) { listeners.menuClicked = fn; } },
    },
    downloads: {
      onCreated: { addListener(fn) { listeners.downloadCreated = fn; } },
      cancel: async () => {},
      erase: async () => {},
    },
    scripting: {
      // Round 72 (A) — the injection fallback: records what was
      // injected into which tab.
      executeScript: async ({ target, files }) => {
        injected.push({ tabId: target.tabId, files });
      },
    },
    tabs: {
      query: async () => tabs,
      // Round 72 (A) — the rescan message to the auto-injected
      // content script. Default: no content script in the tab
      // (fresh install), so the call rejects and the background
      // falls back to injection.
      sendMessage: async (tabId, message) => {
        if (tabSendMessage) return tabSendMessage(tabId, message);
        throw new Error("Could not establish connection");
      },
    },
  };
  const context = vm.createContext({ chrome, console, setTimeout, clearTimeout, Date, URL });
  vm.runInContext(backgroundSrc, context, { filename: "background.js" });
  return {
    storage,
    sentMessages,
    injected,
    listeners,
    setNativeHandler(fn) { nativeHandler = fn; },
  };
}

test("Download all links menu sends download_all with no links and records the hand-off", async () => {
  const env = loadBackground({
    nativeReply: { ok: true, enqueued: 2, pending_count: 2, message: "Queued 2 link(s) from the page" },
  });
  await env.listeners.menuClicked({
    menuItemId: "fetchline-download-all",
    pageUrl: "http://127.0.0.1/page.html",
  });
  await tick(20);
  assert.equal(env.sentMessages.length, 1);
  // JSON comparison: the message was built inside the vm realm, so
  // its Object prototype differs from this realm's (deepStrictEqual
  // rejects it even though the structure is identical).
  assert.equal(
    JSON.stringify(env.sentMessages[0]),
    JSON.stringify({ type: "download_all", page_url: "http://127.0.0.1/page.html", links: [] }),
  );
  assert.equal(env.storage.lastHandoff.ok, true);
  assert.match(env.storage.lastHandoff.message, /Queued 2 link/);
  assert.equal(env.storage.pendingCount, 2);
});

test("Download with Fetchline menu sends the link URL", async () => {
  const env = loadBackground({
    nativeReply: { ok: true, pending_count: 1, message: "Link queued — Fetchline will pick it up" },
  });
  await env.listeners.menuClicked({
    menuItemId: "fetchline-download",
    linkUrl: "http://127.0.0.1/file.zip",
    pageUrl: "http://127.0.0.1/page.html",
  });
  await tick(20);
  assert.equal(
    JSON.stringify(env.sentMessages[0]),
    JSON.stringify({ type: "download_link", url: "http://127.0.0.1/file.zip", referrer: "http://127.0.0.1/page.html" }),
  );
  assert.equal(env.storage.lastHandoff.ok, true);
  assert.equal(env.storage.pendingCount, 1);
});

test("a refused hand-off records a failed last hand-off", async () => {
  const env = loadBackground({
    nativeReply: { ok: false, error: "Browser integration is turned off in Fetchline settings" },
  });
  await env.listeners.menuClicked({
    menuItemId: "fetchline-download",
    linkUrl: "http://127.0.0.1/file.zip",
    pageUrl: "http://127.0.0.1/page.html",
  });
  await tick(20);
  assert.equal(env.storage.lastHandoff.ok, false);
  assert.match(env.storage.lastHandoff.message, /turned off/);
});

test("install refreshes the desktop settings and caches the pending count", async () => {
  const env = loadBackground();
  env.setNativeHandler((message) => {
    if (message.type === "get_settings") {
      return { ok: true, browser_integration: true, excluded_sites: ["ads.example.com"], pending_count: 4 };
    }
    return { ok: true };
  });
  env.listeners.installed();
  await tick(20);
  assert.equal(env.storage.desktopIntegration, true);
  assert.deepEqual(env.storage.desktopExcluded, ["ads.example.com"]);
  assert.equal(env.storage.pendingCount, 4);
});

test("scan_page_for_videos rescans through the auto-injected content script (Round 72 A)", async () => {
  // The detector is auto-injected now, so a scan messages the
  // already-running content script and its summary comes back in
  // the reply — no injection, no storage round-trip. Round 75:
  // the reply is the tab AGGREGATE now (the first sendResponse
  // is not the answer), so the frame's own `videos_found` report
  // is simulated here too — the content script sends one on
  // every rescan, in every frame.
  const env = loadBackground({
    tabs: [{ id: 5, url: "http://127.0.0.1/page.html" }],
    tabSendMessage: async (tabId, message) => {
      assert.equal(tabId, 5);
      assert.equal(message.type, "rescan_videos");
      env.listeners.message(
        { type: "videos_found", pageUrl: "http://127.0.0.1/page.html", total: 2, downloadable: 1 },
        { tab: { id: 5 }, frameId: 0 },
        () => {},
      );
      return { ok: true, total: 2, downloadable: 1 };
    },
  });
  let reply = null;
  env.listeners.message({ type: "scan_page_for_videos" }, {}, (r) => { reply = r; });
  // Round 75: the background waits ~400 ms for the frames to
  // report before answering with the aggregate.
  await tick(600);
  // JSON comparison: the reply was built inside the vm realm, so
  // its Object prototype differs from this realm's.
  assert.equal(JSON.stringify(reply), JSON.stringify({ ok: true, total: 2, downloadable: 1 }));
  assert.deepEqual(env.injected, []);
});

// Round 75 — one frame's scan report, as the content script sends
// it on every rescan (sender.tab + sender.frameId come from the
// runtime).
function reportFrame(env, tabId, frameId, pageUrl, total, downloadable) {
  env.listeners.message(
    { type: "videos_found", pageUrl, total, downloadable },
    { tab: { id: tabId }, frameId },
    () => {},
  );
}

test("scan aggregates across frames: an iframe answering first with 0 does not win (Round 75)", async () => {
  // The Round 73 popup regression: the ad iframe's 0-video reply
  // won the first-response race and the popup said "no videos
  // found" on a page that had videos. Now the reply is the SUM
  // across frames.
  const env = loadBackground({
    tabs: [{ id: 5, url: "http://127.0.0.1/page.html" }],
    tabSendMessage: async () => {
      // The iframe answers FIRST (first-wins) with 0 videos...
      reportFrame(env, 5, 2, "http://127.0.0.1/page.html", 0, 0);
      // ...then the main frame reports its 1 video.
      reportFrame(env, 5, 0, "http://127.0.0.1/page.html", 1, 1);
      return { ok: true, total: 0, downloadable: 0 };
    },
  });
  let reply = null;
  env.listeners.message({ type: "scan_page_for_videos" }, {}, (r) => { reply = r; });
  await tick(600);
  assert.equal(JSON.stringify(reply), JSON.stringify({ ok: true, total: 1, downloadable: 1 }));
});

test("scan sums videos across two frames (Round 75)", async () => {
  const env = loadBackground({
    tabs: [{ id: 5, url: "http://127.0.0.1/page.html" }],
    tabSendMessage: async () => {
      reportFrame(env, 5, 0, "http://127.0.0.1/page.html", 1, 1);
      reportFrame(env, 5, 3, "http://127.0.0.1/page.html", 2, 1);
      return { ok: true, total: 1, downloadable: 1 };
    },
  });
  let reply = null;
  env.listeners.message({ type: "scan_page_for_videos" }, {}, (r) => { reply = r; });
  await tick(600);
  assert.equal(JSON.stringify(reply), JSON.stringify({ ok: true, total: 3, downloadable: 2 }));
});

test("a 0-video frame never overwrites a non-zero aggregate (Round 75)", async () => {
  const env = loadBackground();
  reportFrame(env, 5, 0, "http://example.com/page.html", 2, 1);
  await tick(20);
  assert.equal(env.storage.lastVideoScan.total, 2);
  assert.equal(env.storage.lastVideoScan.downloadable, 1);
  reportFrame(env, 5, 7, "http://example.com/page.html", 0, 0);
  await tick(20);
  assert.equal(env.storage.lastVideoScan.total, 2);
  assert.equal(env.storage.lastVideoScan.downloadable, 1);
});

test("navigation resets a tab's frame scans (Round 75)", async () => {
  const env = loadBackground();
  reportFrame(env, 5, 0, "http://example.com/a.html", 2, 2);
  reportFrame(env, 5, 3, "http://example.com/a.html", 1, 0);
  await tick(20);
  assert.equal(env.storage.lastVideoScan.total, 3);
  // Frame 0 navigates to a new page with no videos: the old
  // frames' scans are cleared, so the aggregate is 0 — not 3.
  reportFrame(env, 5, 0, "http://example.com/b.html", 0, 0);
  await tick(20);
  assert.equal(env.storage.lastVideoScan.total, 0);
  assert.equal(env.storage.lastVideoScan.downloadable, 0);
  assert.equal(env.storage.lastVideoScan.pageUrl, "http://example.com/b.html");
});

test("scan_page_for_videos falls back to injection when the tab has no content script (Round 72 A)", async () => {
  // A page that loaded before the auto-injection landed (fresh
  // install or update) rejects the rescan message — then the
  // background injects, and the reply carries no summary (the
  // injected script reports its own to storage a moment later).
  const env = loadBackground({
    tabs: [{ id: 5, url: "http://127.0.0.1/page.html" }],
  });
  let reply = null;
  env.listeners.message({ type: "scan_page_for_videos" }, {}, (r) => { reply = r; });
  await tick(20);
  assert.equal(JSON.stringify(reply), JSON.stringify({ ok: true }));
  assert.equal(env.injected.length, 1);
  assert.equal(env.injected[0].tabId, 5);
  // JSON comparison: the files array was built inside the vm realm.
  assert.equal(JSON.stringify(env.injected[0].files), JSON.stringify(["video-detect.js", "content.js"]));
});

test("scan_page_for_videos refuses an opted-out site (Round 72 A)", async () => {
  const env = loadBackground({
    tabs: [{ id: 5, url: "http://ads.example.com/page.html" }],
    tabSendMessage: async () => ({ ok: true, total: 1, downloadable: 1 }),
  });
  env.storage.desktopExcluded = ["ads.example.com"];
  let reply = null;
  env.listeners.message({ type: "scan_page_for_videos" }, {}, (r) => { reply = r; });
  await tick(20);
  assert.equal(reply.ok, false);
  assert.match(reply.error, /opted out/);
  assert.deepEqual(env.injected, []);
});

test("video_allowed is no longer answered (Round 73 — the content script reads storage directly)", async () => {
  const env = loadBackground();
  let reply = "unanswered";
  env.listeners.message({ type: "video_allowed", page_url: "http://127.0.0.1/page.html" }, {}, (r) => { reply = r; });
  await tick(20);
  assert.equal(reply, "unanswered");
});

test("Find videos menu rescans the active tab (Round 72 A)", async () => {
  const env = loadBackground({
    tabs: [{ id: 5, url: "http://127.0.0.1/page.html" }],
    tabSendMessage: async () => ({ ok: true, total: 1, downloadable: 1 }),
  });
  await env.listeners.menuClicked({ menuItemId: "fetchline-find-videos" });
  await tick(20);
  assert.deepEqual(env.injected, []);
});
