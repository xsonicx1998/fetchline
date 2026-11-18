// Popup tests (Round 68, popup v2). Run with:
//   node --test extension/tests/popup.test.js
// from app/. No browser is needed: popup.js runs in a vm context
// against a fake document and a fake chrome API, and the
// assertions cover what the popup shows — the connection pill
// (Connected / Not connected) for a reachable, unreachable and
// error-answering desktop app. The v2 popup has no toggles or
// buttons: catching is governed by the desktop app, and videos
// are found from the right-click menu.
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
    listeners: {},
    addEventListener(type, fn) {
      this.listeners[type] = fn;
    },
  };
}

function loadPopup({ nativeReply = { ok: true }, lastVideoScan = null } = {}) {
  const elements = {};
  for (const id of ["status", "statusText", "statusDetail", "scanLine"]) elements[id] = fakeElement();
  let nativeHandler = () => nativeReply;
  let lastError = null;
  const chrome = {
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
    },
    storage: {
      local: {
        async get(_key) {
          return lastVideoScan ? { lastVideoScan } : {};
        },
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
    domContentLoaded,
    setNativeHandler(fn) {
      nativeHandler = fn;
    },
  };
}

test("popup shows Connected when the desktop app answers the ping", async () => {
  const env = loadPopup({ nativeReply: { ok: true } });
  await env.domContentLoaded();
  await tick();
  assert.equal(env.elements.statusText.textContent, "Connected");
  assert.match(env.elements.status.className, /status-ok/);
  assert.equal(env.elements.statusDetail.textContent, "");
});

test("popup shows Not connected when the host is unreachable", async () => {
  const env = loadPopup();
  env.setNativeHandler(() => new Error("Could not establish connection"));
  await env.domContentLoaded();
  await tick();
  assert.equal(env.elements.statusText.textContent, "Not connected");
  assert.match(env.elements.status.className, /status-bad/);
  // Round 69 — the host's own words, not a bare "Not connected".
  assert.equal(env.elements.statusDetail.textContent, "Could not establish connection");
});

test("popup shows Not connected when the host answers with an error", async () => {
  const env = loadPopup({ nativeReply: { ok: false, error: "nope" } });
  await env.domContentLoaded();
  await tick();
  assert.equal(env.elements.statusText.textContent, "Not connected");
  assert.match(env.elements.status.className, /status-bad/);
  assert.equal(env.elements.statusDetail.textContent, "nope");
});

test("popup shows the last video-scan outcome", async () => {
  // Round 69 — the scan line surfaces what "Find videos on this
  // page" found (or why it failed), instead of silent nothing.
  const none = loadPopup({ lastVideoScan: { total: 0, downloadable: 0, at: Date.now() } });
  await none.domContentLoaded();
  await tick();
  assert.match(none.elements.scanLine.textContent, /no videos found/);

  const found = loadPopup({ lastVideoScan: { total: 2, downloadable: 1, at: Date.now() } });
  await found.domContentLoaded();
  await tick();
  assert.match(found.elements.scanLine.textContent, /1 downloadable video/);

  const failed = loadPopup({ lastVideoScan: { error: "no web address to scan", at: Date.now() } });
  await failed.domContentLoaded();
  await tick();
  assert.match(failed.elements.scanLine.textContent, /Video scan failed: no web address to scan/);
});
