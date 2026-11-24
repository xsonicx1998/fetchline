// Relay end-to-end tests (Round 88.2). Run with:
//   node --test extension/tests/relay-e2e.test.js
// from app/. No browser is needed, but unlike relay.test.js
// (which calls the background's message listener directly)
// and content.test.js (which fakes sendMessage), this file
// wires the REAL content.js + video-detect.js to the REAL
// background.js through chrome.runtime.sendMessage, and drives
// the REAL fixture server's responses through webRequest —
// the full live sequence: capture -> dropdown -> click ->
// host. The 88.1 worker-restart test proved the buffer
// restore in isolation; the 88.1 VPS retest still failed
// live, so this file reproduces the exact live flow,
// including the click the retester actually made (a DIRECT
// row for Big Buck Bunny / the fixture clip) as well as the
// relay row.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";
import {
  startFixtureServer,
  RELAY_CLIP_SIZE,
  RELAY_VIDEO_PATH,
  RELAY_CLIP_PATH,
} from "../../../scripts/e2e/fixture-server.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const backgroundSrc = fs.readFileSync(path.join(__dirname, "..", "background.js"), "utf8");
const videoDetectSrc = fs.readFileSync(path.join(__dirname, "..", "video-detect.js"), "utf8");
const contentSrc = fs.readFileSync(path.join(__dirname, "..", "content.js"), "utf8");

const tick = (ms = 20) => new Promise((resolve) => setTimeout(resolve, ms));
const TAB = 5;

// Round 88.4 — the webRequest fake validates addListener the way
// Chrome 157's generated bindings do: the filter argument is
// REQUIRED (a bare addListener(callback) throws "No matching
// signature"), and extraInfoSpec items must be in the event's
// allowed set (web_request.json). See relay.test.js — the old
// lenient fake let background.js's two filter-less relay
// registrations pass the suite while killing the real service
// worker at startup (registration failed, status 15).
const WEBREQUEST_EXTRA_INFO = {
  onSendHeaders: new Set(["requestHeaders", "extraHeaders"]),
  onBeforeRedirect: new Set(["responseHeaders", "extraHeaders"]),
  onHeadersReceived: new Set([
    "blocking",
    "responseHeaders",
    "extraHeaders",
    "securityInfo",
    "securityInfoRawDer",
  ]),
  onCompleted: new Set(["responseHeaders", "extraHeaders", "securityInfo", "securityInfoRawDer"]),
  onErrorOccurred: new Set(["extraHeaders"]),
};
const WEBREQUEST_STORE_KEYS = {
  onSendHeaders: "sendHeaders",
  onBeforeRedirect: "beforeRedirect",
  onHeadersReceived: "headersReceived",
  onCompleted: "completed",
  onErrorOccurred: "error",
};

function strictWebRequestFake(store) {
  const fake = {};
  for (const [event, allowed] of Object.entries(WEBREQUEST_EXTRA_INFO)) {
    fake[event] = {
      addListener(callback, filter, extraInfoSpec) {
        if (typeof callback !== "function") throw new TypeError("No matching signature.");
        if (
          !filter ||
          typeof filter !== "object" ||
          !Array.isArray(filter.urls) ||
          filter.urls.some((u) => typeof u !== "string")
        ) {
          throw new TypeError("No matching signature.");
        }
        if (
          filter.types !== undefined &&
          (!Array.isArray(filter.types) || filter.types.some((t) => typeof t !== "string"))
        ) {
          throw new TypeError("No matching signature.");
        }
        if (
          extraInfoSpec !== undefined &&
          (!Array.isArray(extraInfoSpec) || extraInfoSpec.some((o) => !allowed.has(o)))
        ) {
          throw new TypeError("No matching signature.");
        }
        store[WEBREQUEST_STORE_KEYS[event]] = callback;
      },
    };
  }
  return fake;
}

// --- background harness (modelled on relay.test.js) -----------------------
function loadBackground({
  nativeReply = { ok: true },
  sessionStore: sharedSession,
  storage: sharedStorage,
} = {}) {
  const storage = sharedStorage || {};
  const sessionStore = sharedSession || {};
  const sentMessages = [];
  let nativeHandler = () => nativeReply;
  const listeners = {};
  const webRequest = {};
  const tabRemoved = [];
  let nowMs = 1_700_000_000_000;
  const fakeDate = { now: () => nowMs };
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
      session: {
        get(keys) {
          const list = Array.isArray(keys) ? keys : [keys];
          const out = {};
          for (const k of list) if (k in sessionStore) out[k] = sessionStore[k];
          return Promise.resolve(out);
        },
        set(obj) {
          Object.assign(sessionStore, obj);
          return Promise.resolve();
        },
      },
    },
    cookies: {
      async getAll() {
        return [];
      },
    },
    runtime: {
      lastError: null,
      onInstalled: { addListener(fn) { listeners.installed = fn; } },
      onStartup: { addListener(fn) { listeners.startup = fn; } },
      onMessage: { addListener(fn) { listeners.message = fn; } },
      onConnect: { addListener(fn) { listeners.connect = fn; } },
      sendNativeMessage(_host, message, cb) {
        sentMessages.push(JSON.parse(JSON.stringify(message)));
        const reply = nativeHandler(message);
        if (reply instanceof Error) {
          chrome.runtime.lastError = { message: reply.message };
          cb(null);
        } else {
          chrome.runtime.lastError = null;
          cb(reply);
        }
      },
      connectNative(_host) {
        const port = {
          messages: [],
          disconnected: false,
          _onMessage: null,
          _onDisconnect: null,
          onMessage: { addListener(fn) { port._onMessage = fn; } },
          onDisconnect: { addListener(fn) { port._onDisconnect = fn; } },
          postMessage(msg) { port.messages.push(msg); },
          disconnect() {
            port.disconnected = true;
            if (port._onDisconnect) port._onDisconnect();
          },
        };
        return port;
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
    scripting: { executeScript: async () => {} },
    tabs: {
      query: async () => [],
      sendMessage: async () => {
        throw new Error("Could not establish connection");
      },
      onRemoved: { addListener(fn) { tabRemoved.push(fn); } },
    },
    webRequest: strictWebRequestFake(webRequest),
  };
  const context = vm.createContext({
    chrome,
    console,
    setTimeout,
    clearTimeout,
    Date: fakeDate,
    URL,
    navigator: { userAgent: "FakeBrowser/1.0" },
  });
  vm.runInContext(backgroundSrc, context, { filename: "background.js" });
  return {
    storage,
    sessionStore,
    sentMessages,
    listeners,
    webRequest,
    setNativeHandler(fn) { nativeHandler = fn; },
    advance(ms) { nowMs += ms; },
    sendHeaders(details) { webRequest.sendHeaders(details); },
    headersReceived(details) { webRequest.headersReceived(details); },
    async buffered(tabId) {
      let reply = null;
      listeners.message(
        { type: "relay_get_buffered", tabId },
        {},
        (r) => { reply = r; },
      );
      await tick();
      return reply && Array.isArray(reply.items)
        ? JSON.parse(JSON.stringify(reply.items))
        : [];
    },
  };
}

function headersReceived({
  tabId = TAB,
  url,
  type = "media",
  status = 200,
  headers = {},
  requestId = "r1",
  initiator = "",
} = {}) {
  return {
    tabId,
    url,
    type,
    statusCode: status,
    requestId,
    initiator,
    responseHeaders: Object.entries(headers).map(([name, value]) => ({ name, value })),
  };
}

// --- content harness (modelled on content.test.js) ------------------------
function loadContent({ pageUrl, initialVideoSpec, wire }) {
  const videos = [];
  const storage = {};
  const sentMessages = [];
  const docListeners = {};
  const windowListeners = {};
  const observerInstances = [];
  let messageListener = null;

  function makeElement(tag, attrs = {}) {
    const el = {
      tagName: tag.toUpperCase(),
      children: [],
      style: {},
      textContent: attrs.textContent || "",
      videoWidth: attrs.videoWidth || 0,
      videoHeight: attrs.videoHeight || 0,
      duration: attrs.duration ?? NaN,
      mediaKeys: attrs.mediaKeys || null,
      currentSrc: attrs.currentSrc || "",
      _attrs: { ...attrs },
      _listeners: {},
      _rect: attrs.rect || null,
      parentNode: null,
      getAttribute(name) {
        const v = this._attrs[name];
        return v === undefined ? null : v;
      },
      setAttribute(name, value) {
        this._attrs[name] = value;
      },
      addEventListener(type, fn) {
        (this._listeners[type] = this._listeners[type] || []).push(fn);
      },
      removeEventListener(type, fn) {
        this._listeners[type] = (this._listeners[type] || []).filter((f) => f !== fn);
      },
      appendChild(child) {
        this.children.push(child);
        if (child) child.parentNode = this;
        return child;
      },
      prepend(child) {
        this.children.unshift(child);
        if (child) child.parentNode = this;
        return child;
      },
      querySelectorAll(sel) {
        if (sel === "source") return this.children.filter((c) => c.tagName === "SOURCE");
        return [];
      },
      getBoundingClientRect() {
        return this._rect || { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 };
      },
    };
    let inner = "";
    Object.defineProperty(el, "innerHTML", {
      get() { return inner; },
      set(v) {
        inner = String(v);
        if (v === "") el.children = [];
      },
      configurable: true,
    });
    return el;
  }

  function makeVideo(url, opts = {}) {
    const video = makeElement("video", {
      currentSrc: url,
      videoWidth: opts.videoWidth || 0,
      videoHeight: opts.videoHeight || 0,
      duration: opts.duration ?? 120,
      rect: opts.rect || { left: 100, top: 100, right: 500, bottom: 400, width: 400, height: 300 },
    });
    if (opts.source !== false) {
      video.appendChild(makeElement("source", { src: opts.sourceUrl || url }));
    }
    return video;
  }

  if (initialVideoSpec) videos.push(makeVideo(initialVideoSpec.url, initialVideoSpec));

  const body = makeElement("body");
  const documentFake = {
    body,
    documentElement: makeElement("html"),
    addEventListener(type, fn, capture) {
      (docListeners[type] = docListeners[type] || []).push({ fn, capture: capture === true });
    },
    querySelectorAll(sel) {
      if (sel === "video" || sel === "video, audio") return [...videos];
      if (sel === "a[href]") return [];
      return [];
    },
    contains(node) {
      return videos.includes(node);
    },
    createElement(tag) {
      return makeElement(tag);
    },
    createTextNode(text) {
      return { nodeType: 3, textContent: text };
    },
    elementsFromPoint() {
      return [];
    },
  };
  const locationFake = { href: pageUrl };
  const windowFake = {
    innerWidth: 1280,
    innerHeight: 800,
    location: locationFake,
    history: { pushState() {}, replaceState() {} },
    addEventListener(type, fn) {
      (windowListeners[type] = windowListeners[type] || []).push(fn);
    },
  };
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
      onMessage: { addListener(fn) { messageListener = fn; } },
      sendMessage(msg, cb) {
        sentMessages.push(msg);
        // The live wire: straight into the real background's
        // onMessage listener, with the sender this tab would
        // have (content.js sends no tabId of its own).
        wire(msg, { tab: { id: TAB } }, cb);
      },
      connect(opts) {
        const port = {
          name: opts && opts.name,
          messages: [],
          disconnected: false,
          _onMessage: null,
          _onDisconnect: null,
          onMessage: { addListener(fn) { port._onMessage = fn; } },
          onDisconnect: { addListener(fn) { port._onDisconnect = fn; } },
          postMessage(msg) { port.messages.push(msg); },
          disconnect() {
            port.disconnected = true;
            if (port._onDisconnect) port._onDisconnect();
          },
        };
        return port;
      },
    },
  };
  class FakeMutationObserver {
    constructor(cb) {
      this.cb = cb;
      observerInstances.push(this);
    }
    observe() {}
    disconnect() {}
    takeRecords() { return []; }
  }

  const context = vm.createContext({
    chrome,
    window: windowFake,
    document: documentFake,
    location: locationFake,
    MutationObserver: FakeMutationObserver,
    console,
    setTimeout,
    clearTimeout,
    Date,
    URL,
    confirm() { return false; },
  });
  vm.runInContext(videoDetectSrc, context, { filename: "video-detect.js" });
  vm.runInContext(
    "globalThis.dedupeVariants = globalThis.FetchlineVideoDetect.dedupeVariants; " +
      "globalThis.scanLiveMedia = globalThis.FetchlineVideoDetect.scanLiveMedia; " +
      "globalThis.isSupportedPlatform = globalThis.FetchlineVideoDetect.isSupportedPlatform; " +
      "globalThis.normalizePageUrl = globalThis.FetchlineVideoDetect.normalizePageUrl; " +
      "globalThis.degradedWarningText = globalThis.FetchlineVideoDetect.degradedWarningText; " +
      "globalThis.filterResolvableFormats = globalThis.FetchlineVideoDetect.filterResolvableFormats; " +
      "globalThis.qualityRowLabel = globalThis.FetchlineVideoDetect.qualityRowLabel; " +
      "globalThis.buildResolvedVariant = globalThis.FetchlineVideoDetect.buildResolvedVariant; " +
      "globalThis.createResolveCache = globalThis.FetchlineVideoDetect.createResolveCache;",
    context,
    { filename: "video-detect-globals.js" },
  );
  vm.runInContext(contentSrc, context, { filename: "content.js" });

  return {
    storage,
    sentMessages,
    videos,
    document: documentFake,
    messageListener,
    findButton() {
      return body.children.find((c) => c.tagName === "BUTTON");
    },
    buttonVisible() {
      const b = body.children.find((c) => c.tagName === "BUTTON");
      return !!b && b.style.display !== "none";
    },
    findDropdown() {
      return body.children.find((c) => c.tagName === "DIV");
    },
    click(el) {
      for (const fn of el._listeners.click || []) fn({});
    },
    // Round 88.2 — simulate a page left open across an
    // extension reload: Chrome's sendMessage throws
    // "Extension context invalidated" synchronously from the
    // stale content script. A click must render that failure
    // in the dropdown, not die silently.
    breakSendMessage() {
      chrome.runtime.sendMessage = () => {
        throw new Error("Extension context invalidated.");
      };
    },
    sentOfType(type) {
      return sentMessages.filter((m) => m && m.type === type);
    },
  };
}

// Fetch the clip from the real fixture server the way the
// player's first fetch does, and return the real response
// headers the relay classifier sees.
async function realClipHeaders(baseUrl) {
  const res = await fetch(`${baseUrl}${RELAY_CLIP_PATH}`, {
    headers: { Range: "bytes=0-" },
  });
  assert.equal(res.status, 206, "fixture serves the first fetch as 206");
  const headers = {};
  for (const [k, v] of res.headers.entries()) headers[k] = v;
  // The body is ~1 MiB; drain it so the server can close cleanly.
  await res.arrayBuffer();
  return headers;
}

// The live wire between content.js and background.js: the
// background's onMessage listener, called with this tab as
// the sender — exactly what Chrome delivers.
function wireTo(bg) {
  return (msg, sender, cb) => {
    let responded = false;
    const sendResponse = (reply) => {
      responded = true;
      if (typeof cb === "function") cb(reply);
    };
    const ret = bg.listeners.message(msg, sender, sendResponse);
    // A handler that returns false never responds; the caller
    // (page_context, videos_found) sends those fire-and-forget.
    void ret;
    void responded;
  };
}

// --- tests -----------------------------------------------------------------

test("fixture clip: the dropdown shows a direct row AND a relay row; the relay row click sends relay_capture", async () => {
  const { server, baseUrl } = await startFixtureServer();
  try {
    const clipUrl = `${baseUrl}${RELAY_CLIP_PATH}`;
    const pageUrl = `${baseUrl}${RELAY_VIDEO_PATH}`;
    const clipHeaders = await realClipHeaders(baseUrl);
    assert.equal(clipHeaders["content-range"], `bytes 0-${RELAY_CLIP_SIZE - 1}/${RELAY_CLIP_SIZE}`);

    const bg = loadBackground();
    const content = loadContent({
      pageUrl,
      initialVideoSpec: { url: clipUrl },
      wire: wireTo(bg),
    });
    await tick(50);
    // The page load: main_frame HTML clears the tab, then the
    // player's first fetch (Range bytes=0-) is a 206.
    bg.headersReceived(headersReceived({
      url: pageUrl,
      type: "main_frame",
      status: 200,
      headers: { "Content-Type": "text/html; charset=utf-8" },
      requestId: "r0",
      initiator: "",
    }));
    bg.sendHeaders({
      requestId: "r1",
      url: clipUrl,
      headers: {
        Range: "bytes=0-",
        "User-Agent": "FakeBrowser/1.0",
        Referer: pageUrl,
      },
    });
    bg.headersReceived(headersReceived({
      url: clipUrl,
      type: "media",
      status: 206,
      headers: clipHeaders,
      requestId: "r1",
      initiator: pageUrl,
    }));
    await tick();

    // The relay buffered the media (the capture itself).
    const buffered = await bg.buffered(TAB);
    assert.equal(buffered.length, 1, "the 206 media is buffered");
    assert.equal(buffered[0].kind, "media");
    assert.equal(buffered[0].finalUrl, clipUrl);

    // The dropdown: one direct row (no dimensions yet, so
    // "Unknown quality") plus the relay section below it.
    assert.equal(content.buttonVisible(), true, "the video gets a Download button");
    content.click(content.findButton());
    const dropdown = content.findDropdown();
    assert.ok(dropdown, "dropdown opens on button click");
    await tick(50);
    const rows = dropdown.children.filter((c) => c.tagName === "BUTTON");
    const directRow = rows.find((r) => r.children[1] && r.children[1].textContent === "MP4");
    assert.ok(directRow, "a direct MP4 row is listed");
    assert.equal(directRow.children[0].textContent, "Unknown quality");
    const relayRow = rows.find((r) => r.children[1] && r.children[1].textContent === "Relay");
    assert.ok(relayRow, "a relay row with the Relay mark is listed");
    assert.match(relayRow.children[0].textContent, /relay-clip\.mp4/);
    const header = dropdown.children.find(
      (c) => c.tagName === "P" && /Caught from the browser/.test(c.textContent || ""),
    );
    assert.ok(header, "the relay section has its header");

    // Clicking the relay row sends relay_capture — and only
    // then (nothing went to the host while it was playing).
    assert.equal(bg.sentMessages.filter((m) => m.type === "relay_capture").length, 0);
    content.click(relayRow);
    await tick(50);
    const captures = bg.sentMessages.filter((m) => m.type === "relay_capture");
    assert.equal(captures.length, 1, "one relay_capture reaches the host");
    assert.equal(captures[0].finalUrl, clipUrl);
    assert.equal(captures[0].url, clipUrl);
    // The item leaves the buffer once sent.
    assert.equal((await bg.buffered(TAB)).length, 0, "the sent item leaves the buffer");
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test("fixture clip: the DIRECT row click (the one the VPS retester made) sends video_found", async () => {
  const { server, baseUrl } = await startFixtureServer();
  try {
    const clipUrl = `${baseUrl}${RELAY_CLIP_PATH}`;
    const pageUrl = `${baseUrl}${RELAY_VIDEO_PATH}`;
    const clipHeaders = await realClipHeaders(baseUrl);

    const bg = loadBackground();
    const content = loadContent({
      pageUrl,
      initialVideoSpec: { url: clipUrl },
      wire: wireTo(bg),
    });
    await tick(50);
    bg.headersReceived(headersReceived({
      url: pageUrl,
      type: "main_frame",
      status: 200,
      headers: { "Content-Type": "text/html; charset=utf-8" },
      requestId: "r0",
      initiator: "",
    }));
    bg.headersReceived(headersReceived({
      url: clipUrl,
      type: "media",
      status: 206,
      headers: clipHeaders,
      requestId: "r1",
      initiator: pageUrl,
    }));
    await tick();

    content.click(content.findButton());
    const dropdown = content.findDropdown();
    await tick(50);
    const rows = dropdown.children.filter((c) => c.tagName === "BUTTON");
    const directRow = rows.find((r) => r.children[1] && r.children[1].textContent === "MP4");
    assert.ok(directRow, "a direct MP4 row is listed");
    content.click(directRow);
    await tick(50);
    const found = bg.sentMessages.filter((m) => m.type === "video_found");
    assert.equal(found.length, 1, "one video_found reaches the host");
    assert.equal(found[0].variants[0].url, clipUrl);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test("fixture clip: the relay row click still sends after a worker restart between capture and click", async () => {
  const { server, baseUrl } = await startFixtureServer();
  try {
    const clipUrl = `${baseUrl}${RELAY_CLIP_PATH}`;
    const pageUrl = `${baseUrl}${RELAY_VIDEO_PATH}`;
    const clipHeaders = await realClipHeaders(baseUrl);

    let bg = loadBackground();
    // The wire is re-pointed at the fresh worker after the
    // restart — content.js keeps its own (old) background
    // reference only through this closure.
    let currentBg = bg;
    const content = loadContent({
      pageUrl,
      initialVideoSpec: { url: clipUrl },
      wire: (msg, sender, cb) => wireTo(currentBg)(msg, sender, cb),
    });
    await tick(50);
    bg.headersReceived(headersReceived({
      url: pageUrl,
      type: "main_frame",
      status: 200,
      headers: { "Content-Type": "text/html; charset=utf-8" },
      requestId: "r0",
      initiator: "",
    }));
    bg.headersReceived(headersReceived({
      url: clipUrl,
      type: "media",
      status: 206,
      headers: clipHeaders,
      requestId: "r1",
      initiator: pageUrl,
    }));
    await tick();
    assert.equal((await bg.buffered(TAB)).length, 1, "buffered before the restart");

    // Evict the worker: a fresh vm context, same session store.
    bg = loadBackground({ sessionStore: bg.sessionStore, storage: bg.storage });
    currentBg = bg;

    content.click(content.findButton());
    const dropdown = content.findDropdown();
    await tick(50);
    const rows = dropdown.children.filter((c) => c.tagName === "BUTTON");
    const relayRow = rows.find((r) => r.children[1] && r.children[1].textContent === "Relay");
    assert.ok(relayRow, "the relay row is still listed after the restart");
    content.click(relayRow);
    await tick(50);
    const captures = bg.sentMessages.filter((m) => m.type === "relay_capture");
    assert.equal(captures.length, 1, "one relay_capture reaches the host after the restart");
    assert.equal(captures[0].finalUrl, clipUrl);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test("Bunny-like direct navigation: the mp4 page buffers, and the relay row click sends relay_capture", async () => {
  const { server, baseUrl } = await startFixtureServer();
  try {
    const clipUrl = `${baseUrl}${RELAY_CLIP_PATH}`;
    const clipHeaders = await realClipHeaders(baseUrl);

    const bg = loadBackground();
    // Direct navigation to the mp4 itself: the page IS the
    // video, with real dimensions once loaded.
    const content = loadContent({
      pageUrl: clipUrl,
      initialVideoSpec: { url: clipUrl, videoWidth: 853, videoHeight: 480 },
      wire: wireTo(bg),
    });
    await tick(50);
    // The main_frame response is the mp4 (not HTML), so no
    // tab clear — the 206 is captured straight away.
    bg.headersReceived(headersReceived({
      url: clipUrl,
      type: "main_frame",
      status: 206,
      headers: clipHeaders,
      requestId: "r1",
      initiator: "",
    }));
    await tick();
    assert.equal((await bg.buffered(TAB)).length, 1, "the direct-navigation mp4 is buffered");

    content.click(content.findButton());
    const dropdown = content.findDropdown();
    await tick(50);
    const rows = dropdown.children.filter((c) => c.tagName === "BUTTON");
    const directRow = rows.find((r) => r.children[1] && r.children[1].textContent === "MP4");
    assert.ok(directRow, "a direct MP4 row is listed");
    assert.equal(directRow.children[0].textContent, "853 × 480");
    const relayRow = rows.find((r) => r.children[1] && r.children[1].textContent === "Relay");
    assert.ok(relayRow, "a relay row with the Relay mark is listed");
    content.click(relayRow);
    await tick(50);
    const captures = bg.sentMessages.filter((m) => m.type === "relay_capture");
    assert.equal(captures.length, 1, "one relay_capture reaches the host");
    assert.equal(captures[0].finalUrl, clipUrl);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test("same-tab navigation to the mp4 keeps the relay row in the dropdown", async () => {
  // Round 88.3 — the Round 88.2 live failure, end to end: the
  // tab first shows an HTML page, then navigates to the mp4
  // itself. The mp4's 206 is buffered, the media document's
  // page_context clears the buffer for the new page, and the
  // player's next fetch must re-capture — otherwise the
  // dropdown shows only the direct row, exactly as the VPS
  // retest saw.
  const { server, baseUrl } = await startFixtureServer();
  try {
    const clipUrl = `${baseUrl}${RELAY_CLIP_PATH}`;
    const pageUrl = `${baseUrl}${RELAY_VIDEO_PATH}`;
    const clipHeaders = await realClipHeaders(baseUrl);

    const bg = loadBackground();
    const content = loadContent({
      pageUrl,
      initialVideoSpec: { url: clipUrl },
      wire: wireTo(bg),
    });
    await tick(50);
    // The first page: an HTML video page (its own page_context
    // arrives from content.js on load).
    bg.headersReceived(headersReceived({
      url: pageUrl,
      type: "main_frame",
      status: 200,
      headers: { "Content-Type": "text/html; charset=utf-8" },
      requestId: "r0",
      initiator: "",
    }));
    await tick();
    // The tab navigates to the mp4 itself: the main_frame 206
    // is captured straight away.
    bg.headersReceived(headersReceived({
      url: clipUrl,
      type: "main_frame",
      status: 206,
      headers: clipHeaders,
      requestId: "r1",
      initiator: "",
    }));
    await tick();
    assert.equal((await bg.buffered(TAB)).length, 1, "the direct-navigation mp4 is buffered");
    // The media document's content script mirrors the new page
    // URL — a new top-level page, so the buffer is cleared.
    bg.listeners.message(
      { type: "page_context", pageUrl: clipUrl, title: "" },
      { tab: { id: TAB } },
      () => {},
    );
    await tick();
    assert.equal((await bg.buffered(TAB)).length, 0, "the navigation clears the old buffer");
    // The player keeps fetching (a seek): the re-capture must
    // land in the buffer.
    bg.headersReceived(headersReceived({
      url: clipUrl,
      type: "media",
      status: 206,
      headers: {
        "Content-Type": "video/mp4",
        "Content-Range": `bytes 262144-${RELAY_CLIP_SIZE - 1}/${RELAY_CLIP_SIZE}`,
        "Content-Length": String(RELAY_CLIP_SIZE - 262144),
      },
      requestId: "r2",
      initiator: "",
    }));
    await tick();
    assert.equal((await bg.buffered(TAB)).length, 1, "the seek re-captures the cleared item");

    // The dropdown shows the relay row below the direct row.
    content.click(content.findButton());
    const dropdown = content.findDropdown();
    await tick(50);
    const rows = dropdown.children.filter((c) => c.tagName === "BUTTON");
    const relayRow = rows.find((r) => r.children[1] && r.children[1].textContent === "Relay");
    assert.ok(relayRow, "a relay row with the Relay mark is listed after the navigation");
    assert.match(relayRow.children[0].textContent, /relay-clip\.mp4/);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test("same-tab navigation: the relay row click after the seek re-capture sends relay_capture", async () => {
  // Round 88.5 — the Round 88.4 live failure: the row re-appears
  // after the same-tab navigation (88.3 fixed that), but clicking
  // it only showed the "Sent to Sonic Manager" toast — no Add URL
  // dialog, no download. This test drives the click itself and
  // asserts the capture reaches the host.
  const { server, baseUrl } = await startFixtureServer();
  try {
    const clipUrl = `${baseUrl}${RELAY_CLIP_PATH}`;
    const pageUrl = `${baseUrl}${RELAY_VIDEO_PATH}`;
    const clipHeaders = await realClipHeaders(baseUrl);

    const bg = loadBackground();
    const content = loadContent({
      pageUrl,
      initialVideoSpec: { url: clipUrl },
      wire: wireTo(bg),
    });
    await tick(50);
    bg.headersReceived(headersReceived({
      url: pageUrl,
      type: "main_frame",
      status: 200,
      headers: { "Content-Type": "text/html; charset=utf-8" },
      requestId: "r0",
      initiator: "",
    }));
    await tick();
    bg.headersReceived(headersReceived({
      url: clipUrl,
      type: "main_frame",
      status: 206,
      headers: clipHeaders,
      requestId: "r1",
      initiator: "",
    }));
    await tick();
    assert.equal((await bg.buffered(TAB)).length, 1, "the direct-navigation mp4 is buffered");
    bg.listeners.message(
      { type: "page_context", pageUrl: clipUrl, title: "" },
      { tab: { id: TAB } },
      () => {},
    );
    await tick();
    assert.equal((await bg.buffered(TAB)).length, 0, "the navigation clears the old buffer");
    bg.headersReceived(headersReceived({
      url: clipUrl,
      type: "media",
      status: 206,
      headers: {
        "Content-Type": "video/mp4",
        "Content-Range": `bytes 262144-${RELAY_CLIP_SIZE - 1}/${RELAY_CLIP_SIZE}`,
        "Content-Length": String(RELAY_CLIP_SIZE - 262144),
      },
      requestId: "r2",
      initiator: "",
    }));
    await tick();
    assert.equal((await bg.buffered(TAB)).length, 1, "the seek re-captures the cleared item");

    content.click(content.findButton());
    const dropdown = content.findDropdown();
    await tick(50);
    const rows = dropdown.children.filter((c) => c.tagName === "BUTTON");
    const relayRow = rows.find((r) => r.children[1] && r.children[1].textContent === "Relay");
    assert.ok(relayRow, "a relay row with the Relay mark is listed after the navigation");
    content.click(relayRow);
    await tick(50);
    const captures = bg.sentMessages.filter((m) => m.type === "relay_capture");
    assert.equal(captures.length, 1, "one relay_capture reaches the host after the same-tab click");
    assert.equal(captures[0].finalUrl, clipUrl);
    assert.equal((await bg.buffered(TAB)).length, 0, "the sent item leaves the buffer");
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test("a relay row click from a stale content script (extension reloaded) renders the failure in the dropdown", async () => {
  const { server, baseUrl } = await startFixtureServer();
  try {
    const clipUrl = `${baseUrl}${RELAY_CLIP_PATH}`;
    const pageUrl = `${baseUrl}${RELAY_VIDEO_PATH}`;
    const clipHeaders = await realClipHeaders(baseUrl);

    const bg = loadBackground();
    const content = loadContent({
      pageUrl,
      initialVideoSpec: { url: clipUrl },
      wire: wireTo(bg),
    });
    await tick(50);
    bg.headersReceived(headersReceived({
      url: pageUrl,
      type: "main_frame",
      status: 200,
      headers: { "Content-Type": "text/html; charset=utf-8" },
      requestId: "r0",
      initiator: "",
    }));
    bg.headersReceived(headersReceived({
      url: clipUrl,
      type: "media",
      status: 206,
      headers: clipHeaders,
      requestId: "r1",
      initiator: pageUrl,
    }));
    await tick();

    content.click(content.findButton());
    const dropdown = content.findDropdown();
    await tick(50);
    const rows = dropdown.children.filter((c) => c.tagName === "BUTTON");
    const relayRow = rows.find((r) => r.children[1] && r.children[1].textContent === "Relay");
    assert.ok(relayRow, "a relay row is listed");
    // The page was left open across an extension reload: the
    // content script's context is invalidated, so its
    // sendMessage throws synchronously. Before the 88.2 fix
    // the throw escaped the click handler — the dropdown
    // stayed closed, the row stayed disabled with "Sending…",
    // and nothing on screen explained the dead click.
    content.breakSendMessage();
    assert.doesNotThrow(
      () => content.click(relayRow),
      "the click handler must not let the send throw escape",
    );
    await tick(50);
    const captures = bg.sentMessages.filter((m) => m.type === "relay_capture");
    assert.equal(captures.length, 0, "nothing was sent from the invalidated context");
    assert.equal(dropdown.style.display, "block", "the dropdown re-opens with the failure");
    const note = dropdown.children.find((c) => c.tagName === "P");
    assert.ok(note, "the failure note is rendered in the dropdown");
    assert.match(
      note.textContent,
      /Extension context invalidated/,
      "the note names the real failure",
    );
    const retry = dropdown.children.find(
      (c) => c.tagName === "BUTTON" && c.textContent === "Try again",
    );
    assert.ok(retry, "the dropdown offers a retry");
    assert.equal(relayRow.disabled, false, "the row is re-enabled after the failure");
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

