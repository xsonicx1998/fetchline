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

function loadContent({ seed = {}, pageUrl = PAGE, storageGetRejects = false, initialVideos = [], initialVideoSpec = null } = {}) {
  const videos = [...initialVideos];
  const storage = { ...seed };
  const sentMessages = [];
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
        if (typeof cb === "function") cb({ ok: true });
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
    Date,
    URL,
  });
  // Same injection order as the manifest: video-detect.js first.
  vm.runInContext(videoDetectSrc, context, { filename: "video-detect.js" });
  vm.runInContext(
    "globalThis.dedupeVariants = globalThis.FetchlineVideoDetect.dedupeVariants; " +
      "globalThis.scanLiveMedia = globalThis.FetchlineVideoDetect.scanLiveMedia;",
    context,
    { filename: "video-detect-globals.js" },
  );
  vm.runInContext(contentSrc, context, { filename: "content.js" });

  return {
    storage,
    sentMessages,
    videos,
    document: documentFake,
    docListeners,
    window: windowFake,
    windowListeners,
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
    fireElementEvent(el, type, event = {}) {
      for (const fn of el._listeners[type] || []) fn(event);
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
