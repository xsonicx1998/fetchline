// Content-script tests (Round 73 + Round 74 dropdown). Run with:
//   node --test extension/tests/content.test.js
// from app/. No browser is needed: content.js (after
// video-detect.js, the same injection order as the manifest)
// runs in a vm context against a fake chrome API and a fake
// DOM, modelled on watcher.test.js. The assertions cover the
// Round 73 video-button changes: per-video variants are
// deduped by URL, a video added after injection gets a button
// via the MutationObserver rescan, an overlay above the video
// still shows the button via elementsFromPoint + rect
// containment, loadedmetadata re-scans without any
// service-worker round-trip, an opted-out / switched-off page
// hides the button, and a storage-read failure defaults to
// ALLOWED. Round 74 adds the approved dropdown: the button is
// a gradient pill, the qualities list drops down anchored
// below it (one row per quality, no sizes), it closes on an
// outside click / Escape / a second button click, and picking
// a quality sends only that variant and shows a toast.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const videoDetectSrc = fs.readFileSync(path.join(__dirname, "..", "video-detect.js"), "utf8");
const contentSrc = fs.readFileSync(path.join(__dirname, "..", "content.js"), "utf8");

const tick = (ms = 20) => new Promise((resolve) => setTimeout(resolve, ms));
const PAGE = "http://127.0.0.1/page.html";
const CLIP = "http://127.0.0.1/clip.mp4";
const RECT = { left: 100, top: 100, right: 500, bottom: 400, width: 400, height: 300 };

function loadContent({ seed = {}, pageUrl = PAGE, storageGetRejects = false, initialVideos = [], initialVideoSpec = null, clock = null, confirmFn = null, sendMessageImpl = null } = {}) {
  const videos = [...initialVideos];
  const storage = { ...seed };
  const sentMessages = [];
  const ports = [];
  const confirmCalls = [];
  const docListeners = {};
  const windowListeners = {};
  const observerInstances = [];
  let elementsFromPointFn = null;
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
    // Setting innerHTML to "" clears the element (the dropdown
    // re-renders this way); a plain property would leave stale
    // list items behind on a second render.
    let inner = "";
    Object.defineProperty(el, "innerHTML", {
      get() {
        return inner;
      },
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
      rect: opts.rect || RECT,
    });
    if (opts.source !== false) {
      video.appendChild(makeElement("source", { src: opts.sourceUrl || url }));
    }
    return video;
  }

  // A video present at injection time (init shows its button
  // synchronously, before the storage read resolves).
  if (initialVideoSpec) {
    videos.push(makeVideo(initialVideoSpec.url, initialVideoSpec));
  }

  const body = makeElement("body");
  const documentElement = makeElement("html");
  const documentFake = {
    body,
    documentElement,
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
    elementsFromPoint(x, y) {
      return elementsFromPointFn ? elementsFromPointFn(x, y) : [];
    },
  };
  const locationFake = { href: pageUrl };
  const windowFake = {
    innerWidth: 1280,
    innerHeight: 800,
    location: locationFake,
    history: {
      pushState() {},
      replaceState() {},
    },
    addEventListener(type, fn) {
      (windowListeners[type] = windowListeners[type] || []).push(fn);
    },
  };
  const chrome = {
    storage: {
      local: {
        get(keys) {
          if (storageGetRejects) return Promise.reject(new Error("storage boom"));
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
      onMessage: {
        addListener(fn) {
          messageListener = fn;
        },
      },
      sendMessage(msg, cb) {
        sentMessages.push(msg);
        if (sendMessageImpl) {
          sendMessageImpl(msg, cb);
          return;
        }
        if (typeof cb === "function") cb({ ok: true });
      },
      // Round 78 — the resolve port to the background: each
      // connect() returns a fake port the test drives by hand
      // (deliver() feeds it host replies).
      connect(opts) {
        const port = {
          name: opts && opts.name,
          messages: [],
          disconnected: false,
          _onMessage: null,
          _onDisconnect: null,
          onMessage: {
            addListener(fn) {
              port._onMessage = fn;
            },
          },
          onDisconnect: {
            addListener(fn) {
              port._onDisconnect = fn;
            },
          },
          postMessage(msg) {
            port.messages.push(msg);
          },
          disconnect() {
            port.disconnected = true;
            if (port._onDisconnect) port._onDisconnect();
          },
        };
        ports.push(port);
        return port;
      },
    },
  };
  class FakeMutationObserver {
    constructor(cb) {
      this.cb = cb;
      this.target = null;
      this.opts = null;
      observerInstances.push(this);
    }
    observe(target, opts) {
      this.target = target;
      this.opts = opts;
    }
    disconnect() {}
    takeRecords() {
      return [];
    }
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
    // Round 78 — an optional fake clock ({ now }) so cache
    // expiry can be tested without waiting out the TTL.
    Date: clock ? { now: () => clock.now } : Date,
    URL,
    confirm(...args) {
      confirmCalls.push(args);
      return confirmFn ? confirmFn(...args) : false;
    },
  });
  // Same injection order as the manifest: video-detect.js first.
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
    ports,
    confirmCalls,
    videos,
    document: documentFake,
    docListeners,
    window: windowFake,
    windowListeners,
    location: locationFake,
    observerInstances,
    messageListener,
    makeElement,
    makeVideo,
    addVideo(video) {
      videos.push(video);
    },
    setElementsFromPoint(fn) {
      elementsFromPointFn = fn;
    },
    fireDocumentEvent(type, event = {}) {
      for (const { fn } of docListeners[type] || []) fn(event);
    },
    fireWindowEvent(type, event = {}) {
      for (const fn of windowListeners[type] || []) fn(event);
    },
    fireElementEvent(el, type, event = {}) {
      for (const fn of el._listeners[type] || []) fn(event);
    },
    deliver(port, msg) {
      if (port._onMessage) port._onMessage(msg);
    },
    fireMutation() {
      for (const obs of observerInstances) obs.cb([], obs);
    },
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
    sentOfType(type) {
      return sentMessages.filter((m) => m && m.type === type);
    },
  };
}

test("a video whose currentSrc equals its <source src> yields exactly 1 variant (no duplicate)", async () => {
  const env = loadContent({});
  const video = env.makeVideo(CLIP, { videoWidth: 320, videoHeight: 176 });
  env.addVideo(video);
  // The video was added after injection, so drive a rescan the
  // way the page would (loadedmetadata), then open the dropdown.
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  assert.equal(env.buttonVisible(), true);
  env.click(env.findButton());
  const dropdown = env.findDropdown();
  assert.ok(dropdown, "dropdown opens on button click");
  assert.equal(dropdown.style.display, "block");
  const header = dropdown.children.find((c) => c.tagName === "H3");
  assert.equal(header.textContent, "Download this video");
  const rows = dropdown.children.filter((c) => c.tagName === "BUTTON");
  assert.equal(rows.length, 1, "one quality row, no duplicate");
  assert.equal(rows[0].children[0].textContent, "320 × 176");
  assert.equal(rows[0].children[1].textContent, "MP4");
  // No sizes anywhere in the dropdown.
  const rowText = rows[0].children.map((c) => c.textContent).join(" ");
  assert.ok(!/\d+(\.\d+)?\s*(KB|MB|GB|bytes)/i.test(rowText), `no size in row: ${rowText}`);
  assert.equal(env.sentOfType("video_allowed").length, 0);
});

test("a video inserted after init() gets a button via the MutationObserver rescan", async () => {
  const env = loadContent({});
  await tick();
  assert.equal(env.buttonVisible(), false);
  assert.equal(env.observerInstances.length, 1);
  // Field checks, not deepEqual: the opts object was created
  // inside the vm realm, so its prototype differs from this
  // realm's Object.prototype and deepStrictEqual rejects it.
  assert.equal(env.observerInstances[0].opts.childList, true);
  assert.equal(env.observerInstances[0].opts.subtree, true);
  const video = env.makeVideo(CLIP, { videoWidth: 320, videoHeight: 176 });
  env.addVideo(video);
  env.fireMutation();
  // The observer rescan is debounced (~200 ms).
  await tick(350);
  assert.equal(env.buttonVisible(), true);
  assert.equal(env.sentOfType("video_allowed").length, 0);
});

test("an overlay div above the video still shows the button (elementsFromPoint + rect)", async () => {
  const env = loadContent({});
  const video = env.makeVideo(CLIP, { videoWidth: 320, videoHeight: 176, rect: RECT });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  assert.equal(env.buttonVisible(), true);
  // Move the pointer away from every video: the button hides
  // after the usual 300 ms delay.
  env.setElementsFromPoint(() => []);
  env.fireDocumentEvent("mousemove", { clientX: 10, clientY: 10, target: env.document.body });
  await tick(350);
  assert.equal(env.buttonVisible(), false);
  // Now hover the video through an overlay: e.target is the
  // overlay, elementsFromPoint is [overlay, video], and the
  // pointer is inside the video rect — the button must show.
  const overlay = env.makeElement("div", { rect: RECT });
  env.setElementsFromPoint(() => [overlay, video]);
  env.fireDocumentEvent("mousemove", { clientX: 200, clientY: 200, target: overlay });
  assert.equal(env.buttonVisible(), true);
});

test("loadedmetadata triggers attachment with no video_allowed message ever sent", async () => {
  const env = loadContent({});
  await tick();
  assert.equal(env.sentOfType("video_allowed").length, 0);
  assert.ok(
    (env.docListeners.loadedmetadata || []).some((l) => l.capture === true),
    "loadedmetadata listener is capturing",
  );
  const video = env.makeVideo(CLIP, { videoWidth: 320, videoHeight: 176 });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  // The scan ran: the popup summary was reported, and the video
  // got its mouseenter listener (attachment, not just a scan).
  assert.ok(env.sentOfType("videos_found").some((m) => m.total >= 1));
  assert.ok((video._listeners.mouseenter || []).length >= 1);
  assert.equal(env.sentOfType("video_allowed").length, 0);
});

test("an excluded site hides the button once the storage read resolves", async () => {
  const env = loadContent({
    seed: { desktopExcluded: ["127.0.0.1"] },
    initialVideoSpec: { url: CLIP, videoWidth: 320, videoHeight: 176 },
  });
  // The button shows synchronously at init, then the storage
  // read (excluded host) hides it again.
  await tick(30);
  assert.equal(env.buttonVisible(), false);
  assert.ok(env.findButton(), "the button element exists but is hidden");
  // A later rescan must not show it again.
  env.fireDocumentEvent("loadedmetadata", { target: env.videos[0] });
  await tick();
  assert.equal(env.buttonVisible(), false);
});

test("desktop integration off hides the button", async () => {
  const env = loadContent({
    seed: { desktopIntegration: false },
    initialVideoSpec: { url: CLIP, videoWidth: 320, videoHeight: 176 },
  });
  await tick(30);
  assert.equal(env.buttonVisible(), false);
  assert.ok(env.findButton(), "the button element exists but is hidden");
});

test("a storage-read failure defaults to ALLOWED", async () => {
  const env = loadContent({
    storageGetRejects: true,
    initialVideos: [],
  });
  const video = env.makeVideo(CLIP, { videoWidth: 320, videoHeight: 176 });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick(30);
  assert.equal(env.buttonVisible(), true);
  assert.equal(env.sentOfType("video_allowed").length, 0);
});

test("the button is a gradient pill and the dropdown anchors below it", async () => {
  const env = loadContent({});
  const video = env.makeVideo(CLIP, { videoWidth: 320, videoHeight: 176 });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  const button = env.findButton();
  assert.ok(button);
  assert.equal(button.style.borderRadius, "999px");
  assert.equal(button.style.background, "linear-gradient(135deg, #8B20FF, #F02DCD 55%, #00C8FF)");
  assert.equal(button.style.padding, "8px 14px");
  env.click(button);
  const dropdown = env.findDropdown();
  assert.ok(dropdown, "dropdown opens on button click");
  assert.equal(dropdown.style.display, "block");
  // Same right edge as the button (video right 500, viewport 1280).
  assert.equal(dropdown.style.right, button.style.right);
  // Below the button: video top 100 + 12 (button offset) + 40 + 6.
  assert.equal(dropdown.style.top, "158px");
  assert.equal(dropdown.style.width, "232px");
  assert.equal(dropdown.style.background, "#0B0F24");
  assert.equal(dropdown.style.border, "1px solid #8B20FF");
});

test("the dropdown closes on an outside click, on Escape, and on a second button click", async () => {
  const env = loadContent({});
  const video = env.makeVideo(CLIP, { videoWidth: 320, videoHeight: 176 });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  env.click(env.findButton());
  assert.equal(env.findDropdown().style.display, "block");
  // A click on the page background (not the button, not the dropdown).
  env.fireDocumentEvent("click", { target: env.document.body });
  assert.equal(env.findDropdown().style.display, "none");
  // Reopen, then Escape.
  env.click(env.findButton());
  assert.equal(env.findDropdown().style.display, "block");
  env.fireDocumentEvent("keydown", { key: "Escape" });
  assert.equal(env.findDropdown().style.display, "none");
  // A second click on the button toggles it closed too.
  env.click(env.findButton());
  assert.equal(env.findDropdown().style.display, "block");
  env.click(env.findButton());
  assert.equal(env.findDropdown().style.display, "none");
});

test("picking a quality sends only that variant and shows a toast", async () => {
  const env = loadContent({});
  const video = env.makeVideo(CLIP, { videoWidth: 320, videoHeight: 176 });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  env.click(env.findButton());
  const dropdown = env.findDropdown();
  const rows = dropdown.children.filter((c) => c.tagName === "BUTTON");
  assert.equal(rows.length, 1);
  env.click(rows[0]);
  const sent = env.sentOfType("video_found");
  assert.equal(sent.length, 1);
  assert.equal(sent[0].variants.length, 1);
  assert.equal(sent[0].variants[0].url, CLIP);
  // The dropdown closes on pick; the toast confirms the send.
  assert.equal(dropdown.style.display, "none");
  const toast = env.document.body.children.find(
    (c) => c.tagName === "DIV" && c !== dropdown,
  );
  assert.ok(toast, "toast exists after picking a quality");
  assert.equal(toast.style.display, "block");
  assert.equal(toast.textContent, "Sent to Sonic Manager — it will appear in the download list.");
});

test("several qualities list once each, with no sizes", async () => {
  const env = loadContent({});
  const video = env.makeVideo("http://127.0.0.1/clip-720p.mp4", {
    videoWidth: 0,
    videoHeight: 0,
    sourceUrl: "http://127.0.0.1/clip-1080p.mp4",
  });
  // A second <source> naming the same file as the currentSrc:
  // dedupe must drop it.
  video.appendChild(env.makeElement("source", { src: "http://127.0.0.1/clip-720p.mp4" }));
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  env.click(env.findButton());
  const dropdown = env.findDropdown();
  const rows = dropdown.children.filter((c) => c.tagName === "BUTTON");
  assert.equal(rows.length, 2, "720p + 1080p, the duplicate 720p dropped");
  const labels = rows.map((r) => r.children[0].textContent);
  assert.deepEqual(labels.sort(), ["1080p HD", "720p HD"]);
  for (const row of rows) {
    assert.equal(row.children[1].textContent, "MP4");
    const text = row.children.map((c) => c.textContent).join(" ");
    assert.ok(!/\d+(\.\d+)?\s*(KB|MB|GB|bytes)/i.test(text), `no size in row: ${text}`);
  }
});

// -- Round 78 (yt-dlp) resolve flow --------------------------------------

const YT_PAGE = "https://www.youtube.com/watch?v=abc123";
const YT_BLOB = "blob:https://www.youtube.com/9d8f7a6b-1c2d-3e4f-5a6b-7c8d9e0f1a2b";

function resolvedReply(requestId, overrides = {}) {
  return {
    type: "resolved",
    ok: true,
    request_id: requestId,
    title: "Big Buck Bunny",
    is_live: false,
    degraded: false,
    warning: null,
    formats: [
      { format_id: "137", label: "1080p HD", height: 1080, ext: "mp4", has_audio: false, needs_merge: true, filesize: 123456, url: "https://cdn.example.com/v1080.mp4", audio_url: "https://cdn.example.com/a140.m4a", audio_format_id: "140", headers: { "User-Agent": "x" }, fragmented: false },
      { format_id: "136", label: "720p HD", height: 720, ext: "mp4", has_audio: false, needs_merge: true, filesize: 65432, url: "https://cdn.example.com/v720.mp4", audio_url: "https://cdn.example.com/a140.m4a", audio_format_id: "140", headers: {}, fragmented: false },
      { format_id: "18", label: "360p", height: 360, ext: "mp4", has_audio: true, needs_merge: false, filesize: 11111, url: "https://cdn.example.com/v360.mp4", audio_url: null, audio_format_id: null, headers: {}, fragmented: false },
      { format_id: "hls", label: "720p", height: 720, ext: "mp4", has_audio: true, needs_merge: false, filesize: null, url: "https://cdn.example.com/master.m3u8", audio_url: null, audio_format_id: null, headers: {}, fragmented: true },
      { format_id: "160", label: "144p", height: 144, ext: "mp4", has_audio: false, needs_merge: true, filesize: 100, url: "https://cdn.example.com/v144.mp4", audio_url: null, audio_format_id: null, headers: {}, fragmented: false },
    ],
    ...overrides,
  };
}

// A platform video (blob: src) clicked through to a resolved
// list; returns the env, video and the port the resolve ran on.
async function resolveVideo(env, video, replyOverrides) {
  env.click(env.findButton());
  // Round 79 — the click now reads stored consent first (an
  // async storage read), so the port opens a tick later.
  await tick();
  assert.equal(env.ports.length, 1, "one resolve port opened on click");
  const port = env.ports[0];
  assert.equal(port.name, "sonic-resolve");
  const request = port.messages[0];
  assert.equal(request.type, "resolve_video");
  assert.equal(request.page_url, YT_PAGE);
  // No-cookies-first when no consent is stored: the first
  // resolve never carries cookies. Round 79: a stored grant
  // sends cookies on the first resolve instead — see the
  // stored-consent test below.
  // JSON comparison: the array was built inside the vm realm,
  // so its Object prototype differs from this realm's.
  assert.equal(request.with_cookies, false);
  assert.equal(JSON.stringify(request.cookies), JSON.stringify([]));
  env.deliver(port, resolvedReply(request.request_id, replyOverrides));
  await tick();
  return port;
}

function qualityRows(env) {
  const dropdown = env.findDropdown();
  return dropdown.children.filter((c) => c.tagName === "BUTTON" && c._quality === true);
}

function dropdownText(env) {
  return env.findDropdown().children.map((c) => c.textContent).join(" ");
}

test("a blob: video on YouTube gets a button; the same video off-platform does not", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  assert.equal(env.buttonVisible(), true, "blob: src on a platform still shows the button");

  const plain = loadContent({ pageUrl: PAGE });
  const plainVideo = plain.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  plain.addVideo(plainVideo);
  plain.fireDocumentEvent("loadedmetadata", { target: plainVideo });
  await tick();
  assert.equal(plain.buttonVisible(), false, "blob: src off-platform: no direct source, no button");
});

test("an empty-src video on YouTube gets a button; off-platform it does not", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo("", { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  assert.equal(env.buttonVisible(), true, "empty src on a platform still shows the button");

  const plain = loadContent({ pageUrl: PAGE });
  const plainVideo = plain.makeVideo("", { videoWidth: 1280, videoHeight: 720, source: false });
  plain.addVideo(plainVideo);
  plain.fireDocumentEvent("loadedmetadata", { target: plainVideo });
  await tick();
  assert.equal(plain.buttonVisible(), false);
});

test("clicking the button resolves lazily: Checking… state, heartbeat, then qualities", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  assert.equal(env.ports.length, 0, "nothing resolves on page load or hover");
  env.click(env.findButton());
  await tick();
  const port = env.ports[0];
  assert.equal(env.findButton().disabled, true, "repeat clicks disabled while resolving");
  assert.match(dropdownText(env), /Checking… 0 s/);
  // No direct rows while resolving — only the Cancel button.
  assert.equal(qualityRows(env).length, 0);
  env.deliver(port, { type: "resolve_progress", ok: true, elapsed_secs: 5, request_id: port.messages[0].request_id });
  assert.match(dropdownText(env), /Checking… 5 s/);
  env.deliver(port, resolvedReply(port.messages[0].request_id));
  await tick();
  assert.equal(env.findButton().disabled, false);
  const rows = qualityRows(env);
  assert.equal(rows.length, 4, "1080p + 720p + 360p + stream; the 144p row is dropped");
  assert.deepEqual(
    rows.map((r) => r.children[0].textContent),
    ["1080p HD (video only — merged in app)", "720p HD (video only — merged in app)", "360p", "720p (stream)"],
  );
  for (const row of rows) {
    const text = row.children.map((c) => c.textContent).join(" ");
    assert.ok(!/\d+(\.\d+)?\s*(KB|MB|GB|bytes)/i.test(text), `no size in row: ${text}`);
  }
});

test("Cancel closes the resolve port and the dropdown", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  env.click(env.findButton());
  await tick();
  const port = env.ports[0];
  const cancel = env.findDropdown().children.find((c) => c.tagName === "BUTTON" && c._quality !== true);
  assert.ok(cancel, "Cancel button present while resolving");
  assert.equal(cancel.textContent, "Cancel");
  env.click(cancel);
  assert.equal(port.disconnected, true, "the port closes, killing the host's yt-dlp");
  assert.deepEqual(
    port.messages.map((m) => m.type),
    ["resolve_video", "cancel_resolve"],
  );
  assert.equal(env.findDropdown().style.display, "none");
});

test("picking a resolved quality sends one video_found variant with the resolve's cookies", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  await resolveVideo(env, video);
  const rows = qualityRows(env);
  env.click(rows[0]);
  const sent = env.sentOfType("video_found");
  assert.equal(sent.length, 1);
  assert.equal(sent[0].variants.length, 1, "exactly one variant");
  const v = sent[0].variants[0];
  assert.equal(v.url, "https://cdn.example.com/v1080.mp4");
  assert.equal(v.quality, "1080p");
  assert.equal(v.format, "MP4");
  assert.equal(v.live, false);
  assert.equal(v.drm, false);
  assert.equal(v.resolved, true);
  assert.equal(v.filename, "Big Buck Bunny.mp4");
  assert.equal(v.audio_url, "https://cdn.example.com/a140.m4a");
  assert.equal(v.source_url, YT_PAGE);
  assert.equal(v.format_id, "137");
  assert.equal(v.fragmented, false);
  assert.equal(JSON.stringify(sent[0].cookies), JSON.stringify([]), "no cookies were used for this resolve");
  assert.equal(env.findDropdown().style.display, "none");
  const toast = env.document.body.children.find((c) => c.tagName === "DIV" && c !== env.findDropdown());
  assert.ok(toast);
  assert.equal(toast.textContent, "Sent to Sonic Manager — it will appear in the download list.");
});

test("a fragmented pick sends the manifest URL with fragmented:true", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  await resolveVideo(env, video);
  const rows = qualityRows(env);
  env.click(rows[3]);
  const v = env.sentOfType("video_found")[0].variants[0];
  assert.equal(v.url, "https://cdn.example.com/master.m3u8");
  assert.equal(v.fragmented, true);
});

test("a degraded reply shows the warning and the formats that do exist", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  await resolveVideo(env, video, {
    degraded: true,
    warning: "YouTube is limiting higher qualities for this video right now",
    formats: [
      { format_id: "18", label: "360p", height: 360, ext: "mp4", has_audio: true, needs_merge: false, filesize: 11111, url: "https://cdn.example.com/v360.mp4", audio_url: null, audio_format_id: null, headers: {}, fragmented: false },
    ],
  });
  assert.match(dropdownText(env), /YouTube is limiting higher qualities for this video right now/);
  const rows = qualityRows(env);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].children[0].textContent, "360p");
});

test("a degraded reply with no server warning shows the default warning text", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  await resolveVideo(env, video, { degraded: true, warning: null });
  assert.match(dropdownText(env), /YouTube is limiting higher qualities for this video right now/);
});

test("a live resolve shows it can't be downloaded, with no qualities", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  await resolveVideo(env, video, { is_live: true });
  assert.match(dropdownText(env), /Live streams can't be downloaded/);
  assert.equal(qualityRows(env).length, 0);
});

test("a second click reuses the cached qualities — no second resolve", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  await resolveVideo(env, video);
  // Close the dropdown, then click the button again.
  env.fireDocumentEvent("click", { target: env.document.body });
  assert.equal(env.findDropdown().style.display, "none");
  env.click(env.findButton());
  assert.equal(env.ports.length, 1, "cached: no second resolve port");
  assert.equal(qualityRows(env).length, 4, "cached qualities render immediately");
});

test("an expired cache re-resolves before sending the pick", async () => {
  const clock = { now: 1_000_000 };
  const env = loadContent({ pageUrl: YT_PAGE, clock });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  await resolveVideo(env, video);
  const rows = qualityRows(env);
  // Past the TTL: the pick must re-resolve first.
  clock.now += 3 * 60 * 1000 + 1;
  env.click(rows[0]);
  assert.equal(env.ports.length, 2, "stale cache: a fresh resolve runs before the pick");
  assert.match(dropdownText(env), /Checking…/);
  assert.equal(env.sentOfType("video_found").length, 0, "nothing sent until the fresh resolve answers");
  const fresh = env.ports[1];
  env.deliver(fresh, resolvedReply(fresh.messages[0].request_id));
  await tick();
  const sent = env.sentOfType("video_found");
  assert.equal(sent.length, 1);
  assert.equal(sent[0].variants[0].url, "https://cdn.example.com/v1080.mp4");
});

test("SPA navigation clears the cache: the next video resolves again", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  await resolveVideo(env, video);
  assert.equal(qualityRows(env).length, 4);
  // YouTube navigates with pushState, no reload.
  env.location.href = "https://www.youtube.com/watch?v=zzz999";
  env.window.history.pushState({}, "", "https://www.youtube.com/watch?v=zzz999");
  env.click(env.findButton());
  await tick();
  assert.equal(env.ports.length, 2, "after navigation the old qualities are gone — a fresh resolve runs");
  assert.match(dropdownText(env), /Checking…/);
});

test("SPA navigation via popstate also clears the cache", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  await resolveVideo(env, video);
  env.location.href = "https://www.youtube.com/watch?v=zzz999";
  env.fireWindowEvent("popstate", {});
  env.click(env.findButton());
  await tick();
  assert.equal(env.ports.length, 2, "popstate cleared the cache too");
});

test("needs_login: grant asks once, stores consent, retries with cookies", async () => {
  const cookies = [{ name: "session", value: "abc" }];
  const env = loadContent({
    pageUrl: YT_PAGE,
    confirmFn: () => true,
    sendMessageImpl: (msg, cb) => {
      if (msg.type === "cookies_for") cb({ ok: true, cookies });
      else if (typeof cb === "function") cb({ ok: true });
    },
  });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  env.click(env.findButton());
  await tick();
  const port = env.ports[0];
  env.deliver(port, {
    type: "error",
    ok: false,
    request_id: port.messages[0].request_id,
    error: "This video needs login.",
    needs_login: true,
    error_kind: "login_required",
  });
  await tick(30);
  assert.equal(env.confirmCalls.length, 1, "asked once");
  assert.match(env.confirmCalls[0][0], /needs login/i);
  assert.equal(env.storage["ytdlp_cookie_consent:www.youtube.com"], true, "consent stored per hostname");
  assert.equal(env.ports.length, 2, "retried after the grant");
  const retry = env.ports[1].messages[0];
  assert.equal(retry.with_cookies, true);
  assert.deepEqual(retry.cookies, cookies, "the retry carries the page's cookies");
  // The retry resolves: picking a quality sends those same cookies.
  env.deliver(env.ports[1], resolvedReply(retry.request_id));
  await tick();
  env.click(qualityRows(env)[0]);
  assert.deepEqual(env.sentOfType("video_found")[0].cookies, cookies);
});

test("needs_login: a stored grant sends cookies on the FIRST resolve — no cookieless failure first (Round 79)", async () => {
  const cookies = [{ name: "session", value: "abc" }];
  const env = loadContent({
    pageUrl: YT_PAGE,
    seed: { "ytdlp_cookie_consent:www.youtube.com": true },
    confirmFn: () => true,
    sendMessageImpl: (msg, cb) => {
      if (msg.type === "cookies_for") cb({ ok: true, cookies });
      else if (typeof cb === "function") cb({ ok: true });
    },
  });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  env.click(env.findButton());
  await tick();
  assert.equal(env.ports.length, 1, "one resolve — the first already carries cookies");
  const first = env.ports[0].messages[0];
  assert.equal(first.with_cookies, true, "stored consent: the first resolve is cookied");
  assert.deepEqual(first.cookies, cookies, "the first resolve carries the page's cookies");
  assert.equal(env.confirmCalls.length, 0, "stored consent grants silently, never asked");
  // The cookied attempt ALSO fails with needs_login: the
  // cookies didn't help — show the error, do not loop or ask
  // again.
  env.deliver(env.ports[0], {
    type: "error",
    ok: false,
    request_id: first.request_id,
    error: "This video needs login.",
    needs_login: true,
    error_kind: "login_required",
  });
  await tick(30);
  assert.equal(env.confirmCalls.length, 0, "no second consent ask after a cookied failure");
  assert.equal(env.ports.length, 1, "no retry: the cookies already failed");
  assert.match(dropdownText(env), /needs login/);
});

test("needs_login: a denial shows the error and is not asked again this session", async () => {
  const env = loadContent({ pageUrl: YT_PAGE, confirmFn: () => false });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  const loginError = (requestId) => ({
    type: "error",
    ok: false,
    request_id: requestId,
    error: "This video needs login.",
    needs_login: true,
    error_kind: "login_required",
  });
  env.click(env.findButton());
  await tick();
  env.deliver(env.ports[0], loginError(env.ports[0].messages[0].request_id));
  await tick(30);
  assert.equal(env.confirmCalls.length, 1);
  assert.match(dropdownText(env), /needs login/);
  assert.equal(env.storage["ytdlp_cookie_consent:www.youtube.com"], undefined, "a denial is never stored");
  assert.equal(env.ports.length, 1, "no retry after a denial");
  // Click again (close, then reopen): the error comes back, but
  // the user is not asked a second time in this session.
  env.click(env.findButton());
  env.click(env.findButton());
  await tick();
  assert.equal(env.ports.length, 2);
  env.deliver(env.ports[1], loginError(env.ports[1].messages[0].request_id));
  await tick(30);
  assert.equal(env.confirmCalls.length, 1, "still asked only once");
  assert.match(dropdownText(env), /needs login/);
  // Round 79 — "Try again" after the denial does NOT repeat
  // the identical cookieless request and does NOT ask again:
  // the denial stands for this session, so the error is shown
  // again with no new resolve.
  const retry = env.findDropdown().children.find((c) => c.tagName === "BUTTON" && c.textContent === "Try again");
  assert.ok(retry, "a retry button is offered");
  env.click(retry);
  await tick(30);
  assert.equal(env.confirmCalls.length, 1, "still asked only once");
  assert.equal(env.ports.length, 2, "no new resolve after a denied retry");
  assert.match(dropdownText(env), /needs login/);
});

test("a plain resolve error shows the host's message, with a retry", async () => {
  const env = loadContent({ pageUrl: YT_PAGE });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  env.click(env.findButton());
  await tick();
  const port = env.ports[0];
  env.deliver(port, {
    type: "error",
    ok: false,
    request_id: port.messages[0].request_id,
    error: "This video is not available in your country.",
    needs_login: false,
    error_kind: "geo_blocked",
  });
  await tick();
  assert.match(dropdownText(env), /not available in your country/);
  assert.equal(qualityRows(env).length, 0);
  const retry = env.findDropdown().children.find((c) => c.tagName === "BUTTON" && c.textContent === "Try again");
  assert.ok(retry, "a retry button is offered");
  env.click(retry);
  await tick();
  assert.equal(env.ports.length, 2, "retry opens a fresh resolve");
  // Round 79 — the retry repeats the failed attempt's cookies:
  // this one was cookieless, so the retry is cookieless too.
  const retryRequest = env.ports[1].messages[0];
  assert.equal(retryRequest.with_cookies, false);
  assert.equal(JSON.stringify(retryRequest.cookies), JSON.stringify([]));
});

// -- Round 79 (stored-consent first resolve / "Try again") ----------

test("Try again after a cookied attempt fails retries WITH the same cookies", async () => {
  const cookies = [{ name: "session", value: "abc" }];
  const env = loadContent({
    pageUrl: YT_PAGE,
    seed: { "ytdlp_cookie_consent:www.youtube.com": true },
    sendMessageImpl: (msg, cb) => {
      if (msg.type === "cookies_for") cb({ ok: true, cookies });
      else if (typeof cb === "function") cb({ ok: true });
    },
  });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  env.click(env.findButton());
  await tick();
  assert.equal(env.ports[0].messages[0].with_cookies, true, "stored grant: the first resolve is already cookied");
  env.deliver(env.ports[0], {
    type: "error",
    ok: false,
    request_id: env.ports[0].messages[0].request_id,
    error: "Something went wrong on the host.",
    needs_login: false,
    error_kind: "unknown",
  });
  await tick();
  assert.match(dropdownText(env), /Something went wrong/);
  const retry = env.findDropdown().children.find((c) => c.tagName === "BUTTON" && c.textContent === "Try again");
  assert.ok(retry, "a retry button is offered");
  env.click(retry);
  await tick();
  assert.equal(env.ports.length, 2, "retry opens a fresh resolve");
  const retryRequest = env.ports[1].messages[0];
  assert.equal(retryRequest.with_cookies, true, "the retry keeps the failed attempt's cookies");
  assert.deepEqual(retryRequest.cookies, cookies);
});

test("Try again after a needs_login error retries with cookies once a grant is stored", async () => {
  const cookies = [{ name: "session", value: "abc" }];
  const env = loadContent({
    pageUrl: YT_PAGE,
    confirmFn: () => false,
    sendMessageImpl: (msg, cb) => {
      if (msg.type === "cookies_for") cb({ ok: true, cookies });
      else if (typeof cb === "function") cb({ ok: true });
    },
  });
  const video = env.makeVideo(YT_BLOB, { videoWidth: 1280, videoHeight: 720, source: false });
  env.addVideo(video);
  env.fireDocumentEvent("loadedmetadata", { target: video });
  await tick();
  env.click(env.findButton());
  await tick();
  env.deliver(env.ports[0], {
    type: "error",
    ok: false,
    request_id: env.ports[0].messages[0].request_id,
    error: "This video needs login.",
    needs_login: true,
    error_kind: "login_required",
  });
  await tick(30);
  assert.equal(env.confirmCalls.length, 1, "asked once, denied");
  assert.match(dropdownText(env), /needs login/);
  // Another tab stores a grant for this site after the denial.
  env.storage["ytdlp_cookie_consent:www.youtube.com"] = true;
  const retry = env.findDropdown().children.find((c) => c.tagName === "BUTTON" && c.textContent === "Try again");
  assert.ok(retry, "a retry button is offered");
  env.click(retry);
  await tick(30);
  assert.equal(env.confirmCalls.length, 1, "the stored grant answers silently — never asked again");
  assert.equal(env.ports.length, 2, "retry runs with cookies instead of repeating the cookieless request");
  const retryRequest = env.ports[1].messages[0];
  assert.equal(retryRequest.with_cookies, true);
  assert.deepEqual(retryRequest.cookies, cookies);
});
