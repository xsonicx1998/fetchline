// Popup tests (Round 68, popup v2). Run with:
//   node --test extension/tests/popup.test.js
// from app/. No browser is needed: popup.js runs in a vm context
// against a fake document and a fake chrome API, and the
// assertions cover what the popup shows — the connection pill
// (Connected / Not connected) for a reachable, unreachable and
// error-answering desktop app. The v2 popup has no toggles or
// buttons: catching is governed by the desktop app, and videos
// are found automatically on every page (Round 72) or from the
// right-click menu.
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

function loadPopup({ nativeReply = { ok: true }, lastVideoScan = null, scanReply = null, tabUrl = "http://127.0.0.1/page.html" } = {}) {
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
      // Round 72 (A) — the fresh scan the popup triggers on open.
      // Default (null): no background answers, so the popup falls
      // back to the stored scan — the old behaviour. Tests pass an
      // explicit reply (or an Error to throw) to cover the fresh
      // paths.
      sendMessage(_message) {
        if (scanReply instanceof Error) return Promise.reject(scanReply);
        if (scanReply === null) return Promise.reject(new Error("Could not establish connection"));
        return Promise.resolve(scanReply);
      },
    },
    tabs: {
      query: async () => (tabUrl ? [{ url: tabUrl }] : []),
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
    URL,
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

test("popup shows Connected when the ping says the app is alive (Round 75)", async () => {
  const env = loadPopup({ nativeReply: { ok: true, type: "pong", app_alive: true } });
  await env.domContentLoaded();
  await tick();
  assert.equal(env.elements.statusText.textContent, "Connected");
  assert.match(env.elements.status.className, /status-ok/);
});

test("popup shows the app is closed when the ping says app_alive is false (Round 75)", async () => {
  // The host is standalone and answers even with fetchline.exe
  // closed — the old popup said "Connected" forever. Now the
  // ping reply's app_alive decides.
  const env = loadPopup({ nativeReply: { ok: true, type: "pong", app_alive: false } });
  await env.domContentLoaded();
  await tick();
  assert.equal(env.elements.statusText.textContent, "Sonic Manager is closed - open it to catch downloads");
  assert.match(env.elements.status.className, /status-bad/);
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

test("popup shows the fresh scan for the current page, not a stale stored one (Round 72 A)", async () => {
  // The Round 71 retest's popup said "1 downloadable video
  // found" on a page with no video — a stored scan from an
  // earlier page, presented as this page's. A fresh scan wins
  // over the stored one, and the line says "This page".
  const env = loadPopup({
    scanReply: { ok: true, total: 0, downloadable: 0 },
    lastVideoScan: { pageUrl: "http://other.example.com/x", total: 5, downloadable: 5, at: Date.now() },
    tabUrl: "http://127.0.0.1/page.html",
  });
  await env.domContentLoaded();
  await tick();
  assert.match(env.elements.scanLine.textContent, /This page: no videos found/);

  const found = loadPopup({
    scanReply: { ok: true, total: 2, downloadable: 1 },
    lastVideoScan: { pageUrl: "http://other.example.com/x", total: 5, downloadable: 5, at: Date.now() },
    tabUrl: "http://127.0.0.1/page.html",
  });
  await found.domContentLoaded();
  await tick();
  assert.match(found.elements.scanLine.textContent, /This page: 1 downloadable video/);
});

test("popup labels a stored scan with the page it was for (Round 72 A)", async () => {
  // No fresh scan (the background does not answer): the stored
  // scan still shows, but labelled with its page's host, so it
  // can never pose as the current page's.
  const env = loadPopup({
    lastVideoScan: { pageUrl: "http://other.example.com/x", total: 2, downloadable: 1, at: Date.now() },
    tabUrl: "http://127.0.0.1/page.html",
  });
  await env.domContentLoaded();
  await tick();
  assert.match(env.elements.scanLine.textContent, /Last scan \(other\.example\.com\): 1 downloadable video/);
});

test("popup shows a fresh scan failure only when nothing is stored (Round 72 A)", async () => {
  const failed = loadPopup({
    scanReply: { ok: false, error: "This site is opted out — videos here are not scanned." },
  });
  await failed.domContentLoaded();
  await tick();
  assert.match(failed.elements.scanLine.textContent, /Video scan failed: This site is opted out/);

  // The same failure with a stored scan: the stored line wins.
  const stored = loadPopup({
    scanReply: { ok: false, error: "This site is opted out — videos here are not scanned." },
    lastVideoScan: { pageUrl: "http://other.example.com/x", total: 1, downloadable: 1, at: Date.now() },
    tabUrl: "http://127.0.0.1/page.html",
  });
  await stored.domContentLoaded();
  await tick();
  assert.match(stored.elements.scanLine.textContent, /Last scan \(other\.example\.com\): 1 downloadable video/);
});

test("popup reads storage after the injection-fallback scan (Round 72 A)", async () => {
  // The fallback scan returns { ok: true } with no numbers (the
  // injected script reports to storage a moment later), so the
  // popup waits, then reads the stored scan — and recognises it
  // as this page's because the page matches.
  const env = loadPopup({
    scanReply: { ok: true },
    lastVideoScan: { pageUrl: "http://127.0.0.1/page.html", total: 1, downloadable: 1, at: Date.now() },
    tabUrl: "http://127.0.0.1/page.html",
  });
  await env.domContentLoaded();
  await tick(400);
  assert.match(env.elements.scanLine.textContent, /This page: 1 downloadable video/);
});
