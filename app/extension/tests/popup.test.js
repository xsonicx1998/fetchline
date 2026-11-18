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

function loadPopup({ nativeReply = { ok: true } } = {}) {
  const elements = {};
  for (const id of ["status", "statusText"]) elements[id] = fakeElement();
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
});

test("popup shows Not connected when the host is unreachable", async () => {
  const env = loadPopup();
  env.setNativeHandler(() => new Error("Could not establish connection"));
  await env.domContentLoaded();
  await tick();
  assert.equal(env.elements.statusText.textContent, "Not connected");
  assert.match(env.elements.status.className, /status-bad/);
});

test("popup shows Not connected when the host answers with an error", async () => {
  const env = loadPopup({ nativeReply: { ok: false, error: "nope" } });
  await env.domContentLoaded();
  await tick();
  assert.equal(env.elements.statusText.textContent, "Not connected");
  assert.match(env.elements.status.className, /status-bad/);
});
