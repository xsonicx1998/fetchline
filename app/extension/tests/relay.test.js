// Relay tests (Round 88). Run with:
//   node --test extension/tests/relay.test.js
// from app/. No browser is needed: background.js runs in a vm
// context against a fake chrome API with webRequest listeners,
// chrome.storage.session and partitioned cookies, modelled on
// background.test.js. The assertions cover the classifier matrix
// (plan §10), sent-headers keying / TTL / cap / cleanup,
// dedupe (seen-set, tab-scoped manifest folder, master+variant
// .m3u8, googlevideo, yt-dlp host suppression), the cold
// service-worker fallback, the relay_capture message shape,
// the PDF exclusion (user decision 2026-11-23), and the
// per-tab buffer + click-to-send flow.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const backgroundSrc = fs.readFileSync(path.join(__dirname, "..", "background.js"), "utf8");

const tick = (ms = 20) => new Promise((resolve) => setTimeout(resolve, ms));
const PAGE = "https://example.com/watch/123";
const CLIP = "https://cdn.example.com/v/movie.mp4";

function loadBackground({
  nativeReply = { ok: true },
  cookies = [],
  partitionedCookies = [],
  cookiesError = null,
  testConsole = console,
  // Round 88.1 — a shared session store (and local store)
  // lets a test simulate a service-worker restart: a second
  // load is a fresh vm context with empty in-memory maps, but
  // the SAME chrome.storage.session — exactly what survives
  // a real worker eviction. Defaults keep every existing test
  // on a fresh store.
  sessionStore: sharedSession,
  storage: sharedStorage,
} = {}) {
  const storage = sharedStorage || {};
  const sessionStore = sharedSession || {};
  const sentMessages = [];
  const nativePorts = [];
  let nativeHandler = () => nativeReply;
  const listeners = {};
  const webRequest = {};
  const tabRemoved = [];
  // A controllable clock: the relay's TTLs (sent-headers 2 min,
  // seen-set 2 min) are tested by moving time, not waiting.
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
      async getAll({ url, partitionKey }) {
        if (cookiesError) throw cookiesError;
        // A partitioned query returns ONLY partitioned
        // (CHIPS) cookies; an unpartitioned query returns only
        // the normal jar — the two lists are separate sets.
        const list = partitionKey ? partitionedCookies : cookies;
        return list
          .filter((c) => !c.url || c.url === url)
          .map((c) => ({
            name: c.name,
            value: c.value,
            domain: c.domain ?? "example.com",
            path: c.path ?? "/",
          }));
      },
    },
    runtime: {
      lastError: null,
      onInstalled: { addListener(fn) { listeners.installed = fn; } },
      onStartup: { addListener(fn) { listeners.startup = fn; } },
      onMessage: { addListener(fn) { listeners.message = fn; } },
      onConnect: { addListener(fn) { listeners.connect = fn; } },
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
        nativePorts.push(port);
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
      onRemoved: {
        addListener(fn) { tabRemoved.push(fn); },
      },
    },
    webRequest: {
      onSendHeaders: { addListener(fn) { webRequest.sendHeaders = fn; } },
      onBeforeRedirect: { addListener(fn) { webRequest.beforeRedirect = fn; } },
      onHeadersReceived: { addListener(fn) { webRequest.headersReceived = fn; } },
      onCompleted: { addListener(fn) { webRequest.completed = fn; } },
      onErrorOccurred: { addListener(fn) { webRequest.error = fn; } },
    },
  };
  const context = vm.createContext({
    chrome,
    console: testConsole,
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
    // Fire a webRequest event the way Chrome would.
    sendHeaders(details) { webRequest.sendHeaders(details); },
    redirect(details) { webRequest.beforeRedirect(details); },
    headersReceived(details) { webRequest.headersReceived(details); },
    completed(details) { webRequest.completed(details); },
    errorOccurred(details) { webRequest.error(details); },
    removeTab(tabId) { for (const fn of tabRemoved) fn(tabId); },
    async setPageContext(tabId, pageUrl, title = "") {
      listeners.message(
        { type: "page_context", pageUrl, title },
        { tab: { id: tabId } },
        () => {},
      );
      await tick();
    },
    async buffered(tabId) {
      let reply = null;
      listeners.message(
        { type: "relay_get_buffered", tabId },
        {},
        (r) => { reply = r; },
      );
      await tick();
      // JSON round-trip: the reply was built inside the vm
      // realm, so its Object/Array prototypes differ from
      // this realm's (deepStrictEqual rejects them even though
      // the structure is identical — see background.test.js).
      return reply && Array.isArray(reply.items)
        ? JSON.parse(JSON.stringify(reply.items))
        : [];
    },
    async sendBuffered(tabId, url) {
      let reply = null;
      listeners.message(
        { type: "relay_send_buffered", tabId, url },
        {},
        (r) => { reply = r; },
      );
      await tick();
      return reply;
    },
  };
}

// Response headers as Chrome reports them: an array of
// { name, value } with real-world casing.
function headersReceived({
  tabId = 5,
  url = CLIP,
  type = "media",
  status = 200,
  headers = {},
  requestId = "r1",
  initiator = "https://example.com",
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

function sendHeaders({ requestId = "r1", url = CLIP, headers = {} } = {}) {
  return {
    requestId,
    url,
    requestHeaders: Object.entries(headers).map(([name, value]) => ({ name, value })),
  };
}

const MP4_HEADERS = { "Content-Type": "video/mp4", "Content-Length": "238000000" };

// -- classifier matrix ----------------------------------------------------

test("HLS manifest by content-type is buffered, never sent to the host", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/hls/master.m3u8",
    type: "xmlhttprequest",
    headers: { "Content-Type": "application/vnd.apple.mpegurl" },
  }));
  await tick();
  const items = await env.buffered(5);
  assert.equal(items.length, 1);
  assert.equal(items[0].kind, "manifest");
  assert.equal(items[0].finalUrl, "https://cdn.example.com/hls/master.m3u8");
  assert.equal(env.sentMessages.length, 0, "a passive manifest sends nothing to the host");
});

test("HLS by .m3u8 extension and DASH by type and by .mpd are manifests", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/hls/master.m3u8",
    type: "xmlhttprequest",
    headers: { "Content-Type": "text/plain" },
  }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/dash/manifest",
    type: "xmlhttprequest",
    headers: { "Content-Type": "application/dash+xml" },
    requestId: "r2",
  }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/dash/manifest.mpd",
    type: "xmlhttprequest",
    headers: { "Content-Type": "text/plain" },
    requestId: "r3",
  }));
  await tick();
  const items = await env.buffered(5);
  assert.equal(items.length, 3);
  assert.deepEqual(items.map((i) => i.kind), ["manifest", "manifest", "manifest"]);
});

test("mp4 by video/mp4 and by extension with octet-stream are buffered media", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/v/other.mp4",
    headers: { "Content-Type": "application/octet-stream", "Content-Length": "5000000" },
    requestId: "r2",
  }));
  await tick();
  const items = await env.buffered(5);
  assert.equal(items.length, 2);
  assert.deepEqual(items.map((i) => i.kind), ["media", "media"]);
  assert.equal(env.sentMessages.length, 0);
});

test("tiny media under 256 KiB is skipped", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    headers: { "Content-Type": "video/mp4", "Content-Length": "1000" },
  }));
  await tick();
  assert.deepEqual(await env.buffered(5), []);
});

test("HTML page, non-2xx, tabId -1 and blob: URLs are skipped", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    type: "main_frame",
    headers: { "Content-Type": "text/html; charset=utf-8" },
  }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/v/err.mp4",
    status: 404,
    headers: MP4_HEADERS,
    requestId: "r2",
  }));
  env.headersReceived(headersReceived({
    tabId: -1,
    url: "https://cdn.example.com/v/sw.mp4",
    headers: MP4_HEADERS,
    requestId: "r3",
  }));
  env.headersReceived(headersReceived({
    url: "blob:https://example.com/abc",
    headers: MP4_HEADERS,
    requestId: "r4",
  }));
  await tick();
  assert.deepEqual(await env.buffered(5), []);
  assert.equal(env.sentMessages.length, 0);
});

test("attachment on a navigation is skipped (the watcher owns it)", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    type: "main_frame",
    url: "https://cdn.example.com/file.zip",
    headers: {
      "Content-Type": "application/zip",
      "Content-Disposition": 'attachment; filename="file.zip"',
    },
  }));
  env.headersReceived(headersReceived({
    type: "sub_frame",
    url: "https://cdn.example.com/file.zip",
    headers: {
      "Content-Type": "application/zip",
      "Content-Disposition": 'attachment; filename="file.zip"',
    },
    requestId: "r2",
  }));
  await tick();
  assert.deepEqual(await env.buffered(5), []);
  assert.equal(env.sentMessages.length, 0);
});

test("attachment on a non-navigation type and a catch-list extension go to the host immediately", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    type: "xmlhttprequest",
    url: "https://cdn.example.com/file.zip",
    headers: {
      "Content-Type": "application/zip",
      "Content-Disposition": 'attachment; filename="file.zip"',
    },
  }));
  env.headersReceived(headersReceived({
    type: "other",
    url: "https://cdn.example.com/archive.rar",
    headers: { "Content-Type": "application/octet-stream", "Content-Length": "12345" },
    requestId: "r2",
  }));
  await tick();
  // Both are explicit file captures: immediate relay_capture.
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.sentMessages[0].type, "relay_capture");
  assert.equal(env.sentMessages[0].kind, "file");
  assert.equal(env.sentMessages[0].filename, "file.zip");
  assert.equal(env.sentMessages[1].kind, "file");
  assert.equal(env.sentMessages[1].filename, "archive.rar");
  assert.deepEqual(await env.buffered(5), [], "file captures are not buffered");
});

test("googlevideo.com/videoplayback is skipped outright", async () => {
  const env = loadBackground();
  await env.setPageContext(5, "https://www.youtube.com/watch?v=abc", "Video");
  env.headersReceived(headersReceived({
    url: "https://rr1---sn-abc.googlevideo.com/videoplayback?expire=1",
    headers: MP4_HEADERS,
  }));
  await tick();
  assert.deepEqual(await env.buffered(5), []);
});

// -- 206 / range rules ------------------------------------------------------

test("206 first fetch (start=0, full total >= 256 KiB) is captured with total as size", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    status: 206,
    headers: {
      "Content-Type": "video/mp4",
      "Content-Range": "bytes 0-1048575/238000000",
      "Content-Length": "1048576",
    },
  }));
  await tick();
  const items = await env.buffered(5);
  assert.equal(items.length, 1, "the first 206 is the whole file, not a chunk");
  assert.equal(items[0].kind, "media");
  assert.equal(items[0].content_length, 238000000);
});

test("a subsequent 206 seek (start>0) for an already-seen URL is skipped", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    status: 206,
    headers: {
      "Content-Type": "video/mp4",
      "Content-Range": "bytes 0-1048575/238000000",
      "Content-Length": "1048576",
    },
  }));
  await tick();
  assert.equal((await env.buffered(5)).length, 1);
  // The player seeks: same URL, start > 0 — a chunk, not a file.
  env.headersReceived(headersReceived({
    status: 206,
    requestId: "r2",
    headers: {
      "Content-Type": "video/mp4",
      "Content-Range": "bytes 1048576-2097151/238000000",
      "Content-Length": "1048576",
    },
  }));
  await tick();
  assert.equal((await env.buffered(5)).length, 1, "the seek must not create a second entry");
});

test("206 with no full total (/*) and 206 with a total under 256 KiB are skipped", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    status: 206,
    headers: { "Content-Type": "video/mp4", "Content-Range": "bytes 0-999/*" },
  }));
  env.headersReceived(headersReceived({
    status: 206,
    requestId: "r2",
    headers: { "Content-Type": "video/mp4", "Content-Range": "bytes 0-999/100000" },
  }));
  await tick();
  assert.deepEqual(await env.buffered(5), []);
});

test("?range= and ?bytestart= URL patterns are skipped", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/v/movie.mp4?range=0-1048575",
    headers: MP4_HEADERS,
  }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/v/other.mp4?bytestart=1048576",
    headers: { "Content-Type": "video/mp4", "Content-Length": "5000000" },
    requestId: "r2",
  }));
  await tick();
  assert.deepEqual(await env.buffered(5), []);
});

// -- segments + manifest folder (tab-scoped) ---------------------------------

test("segments are skipped and a URL under a captured manifest's folder is a piece, not a file", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/hls/master.m3u8",
    type: "xmlhttprequest",
    headers: { "Content-Type": "application/vnd.apple.mpegurl" },
  }));
  await tick();
  assert.equal((await env.buffered(5)).length, 1);
  // A .ts segment in the manifest's folder, and an mp4 in the
  // same folder (a piece of that stream, not a separate file).
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/hls/seg0.ts",
    headers: { "Content-Type": "video/mp2t", "Content-Length": "500000" },
    requestId: "r2",
  }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/hls/clip.mp4",
    headers: MP4_HEADERS,
    requestId: "r3",
  }));
  await tick();
  assert.equal((await env.buffered(5)).length, 1, "segments and folder pieces stay suppressed");
});

test("master + variant .m3u8: variant captured, segments suppressed, folder rule is tab-scoped", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  await env.setPageContext(6, PAGE, "Watch");
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/hls/master.m3u8",
    type: "xmlhttprequest",
    headers: { "Content-Type": "application/vnd.apple.mpegurl" },
  }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/hls/720p/variant.m3u8",
    type: "xmlhttprequest",
    headers: { "Content-Type": "application/vnd.apple.mpegurl" },
    requestId: "r2",
  }));
  await tick();
  // A .m3u8 is itself a manifest and is captured even inside a
  // captured manifest's folder (master vs child variant).
  assert.equal((await env.buffered(5)).length, 2);
  // The same CDN folder in ANOTHER tab does not block: the
  // folder rule is scoped by tabId.
  env.headersReceived(headersReceived({
    tabId: 6,
    url: "https://cdn.example.com/hls/other.mp4",
    headers: MP4_HEADERS,
    requestId: "r3",
  }));
  await tick();
  assert.equal((await env.buffered(6)).length, 1, "tab 6 is not blocked by tab 5's manifest folder");
});

// -- dedupe -----------------------------------------------------------------

test("dedupe: the same URL twice yields one buffered entry", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  env.headersReceived(headersReceived({ headers: MP4_HEADERS, requestId: "r2" }));
  await tick();
  assert.equal((await env.buffered(5)).length, 1);
});

test("dedupe: a URL handed over by the video button is never re-captured by the relay", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  let reply = null;
  env.listeners.message(
    {
      type: "video_found",
      page_url: PAGE,
      variants: [{ url: CLIP }],
    },
    {},
    (r) => { reply = r; },
  );
  await tick();
  assert.equal(reply.ok, true);
  env.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  assert.deepEqual(await env.buffered(5), [], "the video-button hand-off owns this URL now");
});

// -- sent headers -------------------------------------------------------------

test("sent headers are keyed by requestId, never by URL", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.sendHeaders(sendHeaders({
    requestId: "r-a",
    headers: { "User-Agent": "UA-A/1.0", Referer: PAGE, Cookie: "session=secret" },
  }));
  env.sendHeaders(sendHeaders({
    requestId: "r-b",
    url: CLIP,
    headers: { "User-Agent": "UA-B/2.0", Referer: PAGE },
  }));
  env.headersReceived(headersReceived({ requestId: "r-a", headers: MP4_HEADERS }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/v/other.mp4",
    requestId: "r-b",
    headers: { "Content-Type": "video/mp4", "Content-Length": "5000000" },
  }));
  await tick();
  const items = await env.buffered(5);
  assert.equal(items.length, 2);
  // Same URL, different requestIds: each item keeps its own UA.
  assert.equal(items[0].headers["User-Agent"], "UA-A/1.0");
  assert.equal(items[1].headers["User-Agent"], "UA-B/2.0");
  // A raw Cookie header is NEVER stored or forwarded.
  assert.equal("Cookie" in items[0].headers, false);
  assert.equal("cookie" in items[0].headers, false);
});

test("sent headers expire after the 2-minute TTL (cold-worker fallback takes over)", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.sendHeaders(sendHeaders({
    requestId: "r-old",
    headers: { "User-Agent": "UA-Old/1.0", Referer: PAGE },
  }));
  env.advance(121 * 1000);
  // Any new insert sweeps the expired entry.
  env.sendHeaders(sendHeaders({
    requestId: "r-new",
    url: "https://cdn.example.com/v/other.mp4",
    headers: { "User-Agent": "UA-New/1.0" },
  }));
  env.headersReceived(headersReceived({ requestId: "r-old", headers: MP4_HEADERS }));
  await tick();
  const items = await env.buffered(5);
  assert.equal(items.length, 1);
  assert.equal(
    items[0].headers["User-Agent"],
    "FakeBrowser/1.0",
    "the expired entry is gone, so the fallback UA is used",
  );
  assert.equal(items[0].headers["Referer"], PAGE, "the fallback Referer is the page URL");
});

test("sent headers are capped at 512 entries (oldest evicted first)", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.sendHeaders(sendHeaders({
    requestId: "r-first",
    headers: { "User-Agent": "UA-First/1.0" },
  }));
  for (let i = 0; i < 512; i++) {
    env.sendHeaders(sendHeaders({
      requestId: `r-${i}`,
      url: `https://cdn.example.com/v/f${i}.mp4`,
      headers: { "User-Agent": `UA-${i}/1.0` },
    }));
  }
  env.headersReceived(headersReceived({ requestId: "r-first", headers: MP4_HEADERS }));
  await tick();
  const items = await env.buffered(5);
  assert.equal(items.length, 1);
  assert.equal(
    items[0].headers["User-Agent"],
    "FakeBrowser/1.0",
    "the first entry was evicted by the cap",
  );
});

test("sent headers are cleaned up on onCompleted AND onErrorOccurred", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.sendHeaders(sendHeaders({
    requestId: "r-done",
    headers: { "User-Agent": "UA-Done/1.0" },
  }));
  env.sendHeaders(sendHeaders({
    requestId: "r-err",
    url: "https://cdn.example.com/v/other.mp4",
    headers: { "User-Agent": "UA-Err/1.0" },
  }));
  env.completed({ requestId: "r-done" });
  env.errorOccurred({ requestId: "r-err" });
  env.headersReceived(headersReceived({ requestId: "r-done", headers: MP4_HEADERS }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/v/other.mp4",
    requestId: "r-err",
    headers: { "Content-Type": "video/mp4", "Content-Length": "5000000" },
  }));
  await tick();
  const items = await env.buffered(5);
  assert.equal(items.length, 2);
  assert.deepEqual(
    items.map((i) => i.headers["User-Agent"]),
    ["FakeBrowser/1.0", "FakeBrowser/1.0"],
    "both completed and errored requests lost their stored headers",
  );
});

// -- redirects ------------------------------------------------------------------

test("onBeforeRedirect on media/xmlhttprequest: url is the FIRST url, finalUrl the last", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.redirect({
    requestId: "rx",
    url: "https://example.com/play",
    type: "media",
  });
  env.redirect({
    requestId: "rx",
    url: "https://cdn.example.com/signed.mp4?sig=1",
    type: "xmlhttprequest",
  });
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/signed.mp4?sig=1",
    requestId: "rx",
    headers: MP4_HEADERS,
  }));
  await tick();
  const items = await env.buffered(5);
  assert.equal(items.length, 1);
  assert.equal(items[0].url, "https://example.com/play", "url is the first URL (for renewal)");
  assert.equal(items[0].finalUrl, "https://cdn.example.com/signed.mp4?sig=1", "finalUrl is where the bytes are");
});

// -- cold worker fallback ---------------------------------------------------------

test("cold service worker: empty SENT_HEADERS falls back to navigator UA + Referer + live cookies", async () => {
  const env = loadBackground({
    cookies: [{ name: "session", value: "abc", url: CLIP }],
  });
  await env.setPageContext(5, PAGE, "Watch");
  // No onSendHeaders fired at all (the worker was evicted).
  env.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  const items = await env.buffered(5);
  assert.equal(items.length, 1);
  assert.equal(items[0].headers["User-Agent"], "FakeBrowser/1.0");
  assert.equal(items[0].headers["Referer"], PAGE);
  assert.equal(JSON.stringify(items[0].cookies), JSON.stringify([
    { name: "session", value: "abc", domain: "example.com", path: "/" },
  ]));
});

// -- yt-dlp host suppression ------------------------------------------------------

test("passive media is suppressed on TikTok/Instagram/Facebook pages, explicit files still work", async () => {
  const env = loadBackground();
  await env.setPageContext(5, "https://www.tiktok.com/@u/video/123", "TikTok video");
  env.headersReceived(headersReceived({
    url: "https://v16.tiktokcdn.com/v.mp4",
    headers: MP4_HEADERS,
    initiator: "https://www.tiktok.com",
  }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/hls/master.m3u8",
    type: "xmlhttprequest",
    headers: { "Content-Type": "application/vnd.apple.mpegurl" },
    requestId: "r2",
    initiator: "https://www.tiktok.com",
  }));
  // An explicit file on the same page still goes through.
  env.headersReceived(headersReceived({
    type: "other",
    url: "https://cdn.example.com/file.zip",
    headers: { "Content-Type": "application/zip" },
    requestId: "r3",
    initiator: "https://www.tiktok.com",
  }));
  await tick();
  assert.deepEqual(await env.buffered(5), [], "passive media + manifests are suppressed on yt-dlp pages");
  assert.equal(env.sentMessages.length, 1, "the explicit file still goes to the host");
  assert.equal(env.sentMessages[0].kind, "file");

  const ig = loadBackground();
  await ig.setPageContext(5, "https://www.instagram.com/reel/abc/", "Reel");
  ig.headersReceived(headersReceived({
    url: "https://scontent.cdninstagram.com/v.mp4",
    headers: MP4_HEADERS,
    initiator: "https://www.instagram.com",
  }));
  await tick();
  assert.deepEqual(await ig.buffered(5), [], "Instagram passive media is suppressed too");
});

// -- message shape + cookies ----------------------------------------------------

test("relay_capture shape: no Cookie key in headers, cookies array shape, finalUrl vs url", async () => {
  const env = loadBackground({
    cookies: [{ name: "session", value: "abc", domain: ".example.com", path: "/" }],
    partitionedCookies: [{ name: "chips", value: "xyz", domain: ".example.com", path: "/" }],
  });
  await env.setPageContext(5, PAGE, "Watch");
  env.sendHeaders(sendHeaders({
    headers: {
      "User-Agent": "UA/1.0",
      Referer: PAGE,
      Cookie: "session=abc",
      "Accept-Language": "en-US",
    },
  }));
  env.headersReceived(headersReceived({
    type: "other",
    url: "https://cdn.example.com/file.zip",
    headers: { "Content-Type": "application/zip", "Content-Disposition": 'attachment; filename="file.zip"' },
  }));
  await tick();
  assert.equal(env.sentMessages.length, 1);
  const msg = env.sentMessages[0];
  assert.equal(msg.type, "relay_capture");
  assert.equal(msg.url, "https://cdn.example.com/file.zip");
  assert.equal(msg.finalUrl, "https://cdn.example.com/file.zip");
  assert.equal(msg.page_url, PAGE);
  assert.equal(msg.kind, "file");
  assert.equal(msg.content_type, "application/zip");
  assert.equal(msg.filename, "file.zip");
  // Headers: the allow-list only, Cookie never present.
  assert.equal(msg.headers["User-Agent"], "UA/1.0");
  assert.equal(msg.headers["Accept-Language"], "en-US");
  assert.equal("Cookie" in msg.headers, false);
  assert.equal(JSON.stringify(Object.keys(msg.headers).sort()), JSON.stringify(["Accept-Language", "Referer", "User-Agent"]));
  // Cookies: BOTH the unpartitioned jar AND the partitioned
  // (CHIPS) list, merged and deduped, name/value/domain/path.
  assert.equal(JSON.stringify(msg.cookies), JSON.stringify([
    { name: "session", value: "abc", domain: ".example.com", path: "/" },
    { name: "chips", value: "xyz", domain: ".example.com", path: "/" },
  ]));
});

test("partitioned + unpartitioned cookies with the same scope dedupe to one entry", async () => {
  const env = loadBackground({
    cookies: [{ name: "session", value: "abc", domain: ".example.com", path: "/" }],
    partitionedCookies: [{ name: "session", value: "abc", domain: ".example.com", path: "/" }],
  });
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    type: "other",
    url: "https://cdn.example.com/file.zip",
    headers: { "Content-Type": "application/zip" },
  }));
  await tick();
  assert.equal(env.sentMessages.length, 1);
  assert.equal(env.sentMessages[0].cookies.length, 1, "the same cookie in both jars is one entry");
});

test("no cookie value is ever logged", async () => {
  const lines = [];
  const testConsole = {
    log: (...args) => { lines.push(args.join(" ")); },
    warn: (...args) => { lines.push(args.join(" ")); },
    error: (...args) => { lines.push(args.join(" ")); },
  };
  const env = loadBackground({
    cookies: [{ name: "session", value: "secret-value-123", url: CLIP }],
    testConsole,
  });
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  assert.ok(
    !lines.some((l) => l.includes("secret-value-123")),
    `no log line may carry a cookie value, got: ${lines.join(" | ")}`,
  );
});

// -- PDF exclusion (user decision 2026-11-23) -----------------------------------

test("inline PDF on main_frame is skipped; PDF with attachment on main_frame is skipped (watcher owns it)", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    type: "main_frame",
    url: "https://example.com/doc.pdf",
    headers: { "Content-Type": "application/pdf", "Content-Length": "500000" },
  }));
  env.headersReceived(headersReceived({
    type: "main_frame",
    url: "https://example.com/doc2.pdf",
    headers: {
      "Content-Type": "application/pdf",
      "Content-Disposition": 'attachment; filename="doc2.pdf"',
    },
    requestId: "r2",
  }));
  await tick();
  assert.deepEqual(await env.buffered(5), [], "reading a PDF must not pop the Add URL dialog");
  assert.equal(env.sentMessages.length, 0);
});

test("a non-PDF main_frame inline file is still captured", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  // A big file opened directly in a tab (main_frame non-HTML).
  env.headersReceived(headersReceived({
    type: "main_frame",
    url: "https://example.com/big.zip",
    headers: { "Content-Type": "application/zip", "Content-Length": "1000000" },
  }));
  await tick();
  assert.equal(env.sentMessages.length, 1);
  assert.equal(env.sentMessages[0].type, "relay_capture");
  assert.equal(env.sentMessages[0].kind, "file");
});

// -- gating, buffer lifecycle, click-to-send ------------------------------------

test("relay gating: relay_enabled off, integration off, or an excluded site captures nothing", async () => {
  const off = loadBackground();
  off.storage.relayEnabled = false;
  await off.setPageContext(5, PAGE, "Watch");
  off.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  assert.deepEqual(await off.buffered(5), [], "relay_enabled off means no relay");

  const noIntegration = loadBackground();
  noIntegration.storage.desktopIntegration = false;
  await noIntegration.setPageContext(5, PAGE, "Watch");
  noIntegration.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  assert.deepEqual(await noIntegration.buffered(5), [], "integration off means no relay");

  const excluded = loadBackground();
  excluded.storage.desktopExcluded = ["example.com"];
  await excluded.setPageContext(5, PAGE, "Watch");
  excluded.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  assert.deepEqual(await excluded.buffered(5), [], "an excluded site is never relayed");
});

test("a 'maybe' manifest is recorded in the buffer but never surfaced", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    type: "xmlhttprequest",
    url: "https://cdn.example.com/api/stream-info",
    headers: { "Content-Type": "text/plain", "Content-Length": "100" },
  }));
  await tick();
  // The surfaced list (video button / popup) stays empty.
  assert.deepEqual(await env.buffered(5), []);
  // But the session mirror recorded it as kind "maybe".
  const stored = env.sessionStore["relayBuffer:5"] || [];
  assert.equal(stored.length, 1);
  assert.equal(stored[0].kind, "maybe");
});

test("page_context mirror: page_url + title come from content.js; navigation clears the buffer", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "My Video Title");
  env.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  let items = await env.buffered(5);
  assert.equal(items.length, 1);
  assert.equal(items[0].page_url, PAGE);
  // A navigation to a new top-level page retires the old items.
  await env.setPageContext(5, "https://example.com/watch/456", "Next Video");
  assert.deepEqual(await env.buffered(5), [], "old-page videos must not linger after navigation");
  // The mirror updated: a new capture uses the new page URL.
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/v/other.mp4",
    headers: { "Content-Type": "video/mp4", "Content-Length": "5000000" },
    requestId: "r2",
  }));
  await tick();
  items = await env.buffered(5);
  assert.equal(items.length, 1);
  assert.equal(items[0].page_url, "https://example.com/watch/456");
});

test("closing a tab clears its buffered relay items and manifest folders", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/hls/master.m3u8",
    type: "xmlhttprequest",
    headers: { "Content-Type": "application/vnd.apple.mpegurl" },
  }));
  env.headersReceived(headersReceived({
    url: "https://cdn.example.com/v/movie.mp4",
    headers: MP4_HEADERS,
    requestId: "r2",
  }));
  await tick();
  assert.equal((await env.buffered(5)).length, 2);
  env.removeTab(5);
  await tick();
  assert.deepEqual(await env.buffered(5), []);
  assert.equal((env.sessionStore["relayBuffer:5"] || []).length, 0);
});

test("clicking a buffered item sends relay_capture with live cookies and drops it from the buffer", async () => {
  const env = loadBackground({
    cookies: [{ name: "session", value: "live-abc", url: CLIP }],
  });
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  assert.equal((await env.buffered(5)).length, 1);
  const reply = await env.sendBuffered(5, CLIP);
  assert.equal(reply.ok, true);
  assert.equal(env.sentMessages.length, 1);
  const msg = env.sentMessages[0];
  assert.equal(msg.type, "relay_capture");
  assert.equal(msg.kind, "media");
  assert.equal(msg.finalUrl, CLIP);
  // Cookies re-read live at click time.
  assert.equal(JSON.stringify(msg.cookies), JSON.stringify([
    { name: "session", value: "live-abc", domain: "example.com", path: "/" },
  ]));
  assert.deepEqual(await env.buffered(5), [], "the sent item leaves the buffer");
});

test("sending a buffered item that is gone fails instead of sending a stale capture", async () => {
  const env = loadBackground();
  const reply = await env.sendBuffered(5, CLIP);
  assert.equal(reply.ok, false);
  assert.match(reply.error, /no longer available/);
  assert.equal(env.sentMessages.length, 0);
});

test("a fresh HTML page load clears the old page's buffered items", async () => {
  const env = loadBackground();
  await env.setPageContext(5, PAGE, "Watch");
  env.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  assert.equal((await env.buffered(5)).length, 1);
  // The tab navigates: a main_frame HTML response arrives.
  env.headersReceived(headersReceived({
    type: "main_frame",
    url: "https://example.com/other.html",
    headers: { "Content-Type": "text/html" },
    requestId: "r2",
  }));
  await tick();
  assert.deepEqual(await env.buffered(5), []);
});

test("worker restart: buffered items survive in chrome.storage.session and can still be sent (Round 88.1)", async () => {
  // The Round 88 VPS retest failure: the video button showed
  // the buffered rows (content.js held its own copy), but the
  // click sent nothing — the service worker had been evicted
  // in the minutes between (RDP is slow; Chrome evicts an idle
  // MV3 worker after ~30 s), the in-memory relayBuffer was
  // wiped, and the session mirror was never read back, so
  // relaySendBuffered threw "That video is no longer
  // available" and the host never saw the capture.
  const sessionStore = {};
  const env1 = loadBackground({ sessionStore });
  await env1.setPageContext(5, PAGE, "Watch");
  env1.headersReceived(headersReceived({ headers: MP4_HEADERS }));
  await tick();
  assert.equal((await env1.buffered(5)).length, 1);
  // The mirror is what survives the restart.
  assert.equal((sessionStore["relayBuffer:5"] || []).length, 1);
  // Simulate the restart: a fresh vm context (empty in-memory
  // maps) sharing the SAME session store.
  const env2 = loadBackground({ sessionStore });
  assert.equal(
    (await env2.buffered(5)).length,
    1,
    "the buffer is restored from session storage after a restart",
  );
  const reply = await env2.sendBuffered(5, CLIP);
  assert.equal(reply.ok, true, `the click must send after a restart: ${JSON.stringify(reply)}`);
  assert.equal(env2.sentMessages.length, 1);
  assert.equal(env2.sentMessages[0].type, "relay_capture");
  assert.equal(env2.sentMessages[0].finalUrl, CLIP);
});
