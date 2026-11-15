// Download-watcher tests (S13 finish). Run with:
//   node --test extension/tests/watcher.test.js
// from app/. No browser is needed: background.js runs in a vm
// context against a fake chrome API, and the download watcher
// (chrome.downloads.onCreated) is driven with a simulated stream
// of download events, the way a real browser would fire them:
// a downloadable link, an excluded site, integration off, a file
// type outside the catch list, and the same URL twice. The
// assertions cover the whole decision: cancel the browser
// download, erase it, hand the URL to the host (with that URL's
// cookies, when the browser has any), and record the hand-off —
// or leave the browser download completely alone.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const backgroundSrc = fs.readFileSync(path.join(__dirname, "..", "background.js"), "utf8");

const tick = (ms = 20) => new Promise((resolve) => setTimeout(resolve, ms));

// Test-only dummy cookie values, never real ones.
const DUMMY_COOKIES = [{ name: "session", value: "test-dummy-value" }];

function loadBackground({ seed = {}, cookiesForUrl = () => [] } = {}) {
  const storage = { ...seed };
  const sentMessages = [];
  const cancelled = [];
  const erased = [];
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
        chrome.runtime.lastError = null;
        cb({ ok: true, pending_count: sentMessages.length, message: "Link queued — Fetchline will pick it up" });
      },
    },
    contextMenus: {
      create() {},
      onClicked: { addListener(fn) { listeners.menuClicked = fn; } },
    },
    downloads: {
      onCreated: { addListener(fn) { listeners.downloadCreated = fn; } },
      cancel: async ({ id }) => { cancelled.push(id); },
      erase: async ({ id }) => { erased.push(id); },
    },
    cookies: {
      getAll: async ({ url }) => cookiesForUrl(url),
    },
    scripting: {},
    tabs: { query: async () => [] },
  };
  const context = vm.createContext({ chrome, console, setTimeout, clearTimeout, Date, URL });
  vm.runInContext(backgroundSrc, context, { filename: "background.js" });
  return {
    storage,
    sentMessages,
    cancelled,
    erased,
    listeners,
    async fireDownload(item) {
      await listeners.downloadCreated(item);
      await tick();
    },
  };
}

const zipDownload = (id, url = "http://127.0.0.1/file.zip") => ({
  id,
  url,
  filename: "file.zip",
  state: "in_progress",
  referrer: "http://127.0.0.1/page.html",
});

test("watcher cancels a downloadable link and hands it to Fetchline", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(zipDownload(1));
  assert.deepEqual(env.cancelled, [1]);
  assert.deepEqual(env.erased, [1]);
  assert.equal(env.sentMessages.length, 1);
  assert.equal(
    JSON.stringify(env.sentMessages[0]),
    JSON.stringify({
      type: "download_link",
      url: "http://127.0.0.1/file.zip",
      referrer: "http://127.0.0.1/page.html",
    }),
  );
  assert.equal(env.storage.lastHandoff.ok, true);
});

test("watcher attaches the target URL's cookies to the hand-off", async () => {
  const env = loadBackground({
    seed: { desktopIntegration: true },
    cookiesForUrl: (url) => (url === "http://127.0.0.1/file.zip" ? DUMMY_COOKIES : []),
  });
  await env.fireDownload(zipDownload(1));
  assert.equal(env.sentMessages.length, 1);
  assert.equal(
    JSON.stringify(env.sentMessages[0].cookies),
    JSON.stringify(DUMMY_COOKIES),
  );
  // Only name/value pairs travel — no domain, path, expiry or flags.
  assert.deepEqual(Object.keys(env.sentMessages[0].cookies[0]).sort(), ["name", "value"]);
});

test("watcher sends no cookies field when the browser has none for the URL", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(zipDownload(1));
  assert.equal(env.sentMessages.length, 1);
  assert.ok(!("cookies" in env.sentMessages[0]));
});

test("watcher reads the type from a full local download path", async () => {
  // Chrome reports the absolute local path as `filename`, and the
  // URL need not carry the extension at all (BUG-023: the old code
  // fed the filename to a URL parser and skipped everything).
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload({
    id: 1,
    url: "https://127.0.0.1/download?id=7",
    filename: "/home/user/Downloads/report.pdf",
    state: "in_progress",
    referrer: "https://127.0.0.1/page.html",
  });
  assert.deepEqual(env.cancelled, [1]);
  assert.equal(env.sentMessages.length, 1);
  assert.equal(env.sentMessages[0].url, "https://127.0.0.1/download?id=7");
});

test("watcher leaves an excluded site's download alone", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true, desktopExcluded: ["127.0.0.1"] } });
  await env.fireDownload(zipDownload(1));
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 0);
  assert.equal(env.storage.lastHandoff, undefined);
});

test("watcher leaves downloads alone when desktop integration is off", async () => {
  const env = loadBackground({ seed: { desktopIntegration: false } });
  await env.fireDownload(zipDownload(1));
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 0);
});

test("watcher ignores a file type outside the catch list", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload({
    id: 1,
    url: "http://127.0.0.1/notes.txt",
    filename: "notes.txt",
    state: "in_progress",
    referrer: "http://127.0.0.1/page.html",
  });
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 0);
});

test("the same URL twice is handed over twice (the drain de-duplicates by path)", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(zipDownload(1));
  await env.fireDownload(zipDownload(2));
  assert.deepEqual(env.cancelled, [1, 2]);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.sentMessages[0].url, "http://127.0.0.1/file.zip");
  assert.equal(env.sentMessages[1].url, "http://127.0.0.1/file.zip");
});

test("watcher ignores non-http(s) downloads and downloads that already finished", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload({
    id: 1,
    url: "ftp://127.0.0.1/file.zip",
    filename: "file.zip",
    state: "in_progress",
  });
  await env.fireDownload({
    id: 2,
    url: "http://127.0.0.1/file.zip",
    filename: "file.zip",
    state: "complete",
  });
  assert.deepEqual(env.cancelled, []);
  assert.equal(env.sentMessages.length, 0);
});
