// Popup tests (S13 finish / Milestone 2). Run with:
//   node --test extension/tests/popup.test.js
// from app/. No browser is needed: popup.js runs in a vm context
// against a fake document and a fake chrome API, and the assertions
// cover what the popup shows — the pending count and the last
// hand-off status — plus the "Send this page" hand-off recording.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const popupSrc = fs.readFileSync(path.join(__dirname, "..", "popup.js"), "utf8");

const tick = (ms = 10) => new Promise((resolve) => setTimeout(resolve, ms));

function fakeElement() {
  return {
    textContent: "",
    className: "",
    checked: false,
    disabled: false,
    hidden: false,
    dataset: {},
    listeners: {},
    addEventListener(type, fn) {
      this.listeners[type] = fn;
    },
  };
}

function loadPopup({ storageData = {}, nativeReply = { ok: true, pending_count: 0 } } = {}) {
  const elements = {};
  const ids = [
    "status", "result", "pendingInfo", "handoffInfo", "catchToggle",
    "siteToggle", "siteLabel", "desktopNote", "sendTab", "findVideos", "openApp",
  ];
  for (const id of ids) elements[id] = fakeElement();
  const storage = { ...storageData };
  const setCalls = [];
  let nativeHandler = () => nativeReply;
  let lastError = null;
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
          setCalls.push(obj);
          return Promise.resolve();
        },
      },
    },
    runtime: {
      get lastError() {
        return lastError;
      },
      sendNativeMessage(_host, _message, cb) {
        const reply = nativeHandler(_message);
        if (reply instanceof Error) {
          lastError = { message: reply.message };
          cb(null);
        } else {
          lastError = null;
          cb(reply);
        }
      },
      sendMessage(_message) {
        return Promise.resolve({ ok: true });
      },
    },
    tabs: {
      query() {
        return Promise.resolve([{ id: 1, url: "https://example.com/page" }]);
      },
    },
  };
  let domContentLoaded = null;
  const document = {
    getElementById(id) {
      return elements[id] || null;
    },
    addEventListener(type, fn) {
      if (type === "DOMContentLoaded") domContentLoaded = fn;
    },
  };
  const context = vm.createContext({
    document,
    chrome,
    console,
    setTimeout,
    clearTimeout,
    Date,
  });
  vm.runInContext(popupSrc, context, { filename: "popup.js" });
  return {
    elements,
    storage,
    setCalls,
    chrome,
    domContentLoaded,
    setNativeHandler(fn) {
      nativeHandler = fn;
    },
  };
}

test("formatPendingCount covers unknown, none, one and many", () => {
  const exposed = loadPopupExposed();
  assert.equal(exposed.formatPendingCount(null), "Waiting links: unknown — check the desktop app.");
  assert.equal(exposed.formatPendingCount(0), "No links waiting in Fetchline.");
  assert.equal(
    exposed.formatPendingCount(1),
    "1 link waiting in Fetchline — it will start downloading shortly.",
  );
  assert.equal(
    exposed.formatPendingCount(3),
    "3 links waiting in Fetchline — they will start downloading shortly.",
  );
});

function loadPopupExposed() {
  // Same loader as loadPopup, but keeps the vm context so the test
  // can reach the FetchlinePopup export popup.js puts on globalThis.
  const elements = {};
  for (const id of ["status", "result", "pendingInfo", "handoffInfo", "catchToggle",
    "siteToggle", "siteLabel", "desktopNote", "sendTab", "findVideos", "openApp"]) {
    elements[id] = fakeElement();
  }
  const storage = {};
  const chrome = {
    storage: {
      local: {
        get: (keys) => Promise.resolve(Object.fromEntries(keys.filter((k) => k in storage).map((k) => [k, storage[k]]))),
        set: (obj) => { Object.assign(storage, obj); return Promise.resolve(); },
      },
    },
    runtime: {
      lastError: null,
      sendNativeMessage: (_m, cb) => cb({ ok: true }),
      sendMessage: () => Promise.resolve({ ok: true }),
    },
    tabs: { query: () => Promise.resolve([{ id: 1, url: "https://example.com/page" }]) },
  };
  const context = vm.createContext({ document: {
    getElementById: (id) => elements[id] || null,
    addEventListener: () => {},
  }, chrome, console, setTimeout, clearTimeout, Date });
  vm.runInContext(popupSrc, context, { filename: "popup.js" });
  return vm.runInContext("FetchlinePopup", context);
}

test("formatHandoff covers nothing sent, success and failure", () => {
  const popup = loadPopupExposed();
  assert.equal(popup.formatHandoff(null), "Nothing sent to Fetchline yet.");
  const ok = popup.formatHandoff({ at: Date.UTC(2026, 10, 15, 12, 0, 0), ok: true, message: "Sent to Fetchline" });
  assert.match(ok, /^Last sent /);
  assert.match(ok, /Sent to Fetchline/);
  const bad = popup.formatHandoff({ at: Date.UTC(2026, 10, 15, 12, 0, 0), ok: false, message: "Fetchline refused the link" });
  assert.match(bad, /^Last send failed /);
  assert.match(bad, /Fetchline refused the link/);
});

test("popup shows the stored pending count and last hand-off on open", async () => {
  const env = loadPopup({
    storageData: {
      pendingCount: 2,
      lastHandoff: { at: Date.now(), ok: true, message: "Sent to Fetchline" },
      catchEnabled: true,
    },
  });
  await env.domContentLoaded();
  await tick();
  assert.match(env.elements.pendingInfo.textContent, /2 links waiting in Fetchline/);
  assert.match(env.elements.handoffInfo.textContent, /^Last sent /);
  assert.match(env.elements.handoffInfo.textContent, /Sent to Fetchline/);
});

test("popup shows the empty state when nothing is waiting or sent", async () => {
  const env = loadPopup({ storageData: { pendingCount: 0, catchEnabled: true } });
  await env.domContentLoaded();
  await tick();
  assert.equal(env.elements.pendingInfo.textContent, "No links waiting in Fetchline.");
  assert.equal(env.elements.handoffInfo.textContent, "Nothing sent to Fetchline yet.");
});

test("Send this page records a successful hand-off and the pending count", async () => {
  const env = loadPopup({ nativeReply: { ok: true, pending_count: 3 } });
  await env.domContentLoaded();
  await tick();
  await env.elements.sendTab.listeners.click();
  await tick(20);
  assert.equal(env.storage.lastHandoff.ok, true);
  assert.equal(env.storage.pendingCount, 3);
  assert.match(env.elements.pendingInfo.textContent, /3 links waiting in Fetchline/);
  assert.match(env.elements.handoffInfo.textContent, /^Last sent /);
});

test("Send this page records a failed hand-off when the host is unreachable", async () => {
  const env = loadPopup();
  env.setNativeHandler(() => new Error("Could not establish connection"));
  await env.domContentLoaded();
  await tick();
  await env.elements.sendTab.listeners.click();
  await tick(20);
  assert.equal(env.storage.lastHandoff.ok, false);
  assert.match(env.storage.lastHandoff.message, /Could not reach Fetchline/);
  assert.match(env.elements.handoffInfo.textContent, /^Last send failed /);
});
