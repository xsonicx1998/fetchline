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

function loadBackground({ nativeReply = { ok: true } } = {}) {
  const storage = {};
  const sentMessages = [];
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
    scripting: {},
    tabs: { query: async () => [] },
  };
  const context = vm.createContext({ chrome, console, setTimeout, clearTimeout, Date });
  vm.runInContext(backgroundSrc, context, { filename: "background.js" });
  return {
    storage,
    sentMessages,
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
