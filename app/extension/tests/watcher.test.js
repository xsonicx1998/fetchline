// Download-watcher tests (S13 finish). Run with:
//   node --test extension/tests/watcher.test.js
// from app/. No browser is needed: background.js runs in a vm
// context against a fake chrome API, and the download watcher
// (chrome.downloads.onCreated) is driven with a simulated stream
// of download events, the way a real browser would fire them:
// a downloadable link, an excluded site, integration off, a file
// type outside the catch list, and the same URL twice. The
// assertions cover the whole decision: pause the browser
// download, hand the URL to the host (with that URL's
// cookies, when the browser has any), cancel + erase the browser
// copy ONLY once the app confirms receipt — or resume the browser
// download when the hand-off fails, so the file is never
// swallowed (Round 69) — or leave the browser download
// completely alone. BUG-049: the
// fake's cancel enforces Chrome's real signature (a bare download
// id — cancel({ id }) throws "No matching signature" in real
// Chrome), and a cancel failure (download already finished) must
// still hand the URL over instead of dropping it silently.
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

function loadBackground({ seed = {}, cookiesForUrl = () => [], cancelThrows = false, hostError = null, failHandover = false, completeDuringPing = false, searchThrows = false } = {}) {
  const storage = { ...seed };
  const sentMessages = [];
  const cancelled = [];
  const erased = [];
  const paused = [];
  const resumed = [];
  const listeners = {};
  // Round 73 (Group 4) — simulated per-download state for
  // chrome.downloads.search: fireDownload seeds it from the
  // item's state (default in_progress); tests can flip it via
  // setDownloadState, and completeDuringPing flips every tracked
  // download to "complete" while the ping is in flight, so the
  // already-finished race can be simulated.
  const downloadStates = new Map();
  const setDownloadState = (id, state) => { downloadStates.set(id, state); };
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
        // Round 73 (Group 4) — completeDuringPing: the download
        // finishes while the ping is in flight. Flip the tracked
        // state before resolving so the watcher's post-handover
        // search sees "complete".
        if (completeDuringPing && message.type === "ping") {
          for (const id of downloadStates.keys()) downloadStates.set(id, "complete");
        }
        // Round 71 (A4) — `failHandover` fails only the hand-off,
        // not the ping: the watcher pings first, so a host that
        // answers the ping but fails the hand-off must still
        // resume the browser download (the Round 69 safety rule).
        // `hostError` fails everything, ping included.
        if (hostError || (failHandover && message.type !== "ping")) {
          chrome.runtime.lastError = { message: hostError || "Could not establish connection" };
          cb(null);
          return;
        }
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
      // BUG-049: chrome.downloads.cancel takes the bare download id,
      // cancel(downloadId) — the old code passed { id } and real
      // Chrome threw "No matching signature" on every download while
      // this fake happily destructured the object. The fake now
      // enforces the real signature, so a wrong call fails here.
      // Round 69: pause/resume take the same bare id.
      pause: async (id) => {
        if (typeof id !== "number") throw new Error("No matching signature.");
        paused.push(id);
      },
      resume: async (id) => {
        if (typeof id !== "number") throw new Error("No matching signature.");
        resumed.push(id);
      },
      cancel: async (id) => {
        if (typeof id !== "number") throw new Error("No matching signature.");
        if (cancelThrows) throw new Error("Download not found");
        cancelled.push(id);
      },
      erase: async ({ id }) => { erased.push(id); },
      // Round 73 (Group 4) — chrome.downloads.search: returns the
      // simulated current state for the id (default in_progress
      // when the test never seeded/flipped it).
      search: async ({ id } = {}) => {
        if (searchThrows) throw new Error("search failed");
        return [{ id, state: downloadStates.get(id) || "in_progress" }];
      },
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
    paused,
    resumed,
    listeners,
    downloadStates,
    setDownloadState,
    async fireDownload(item) {
      downloadStates.set(item.id, item.state || "in_progress");
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
  // Round 69: the browser download is paused while the hand-off
  // runs, and cancelled only once the app confirms receipt.
  assert.deepEqual(env.paused, [1]);
  assert.deepEqual(env.resumed, []);
  assert.deepEqual(env.cancelled, [1]);
  assert.deepEqual(env.erased, [1]);
  // Round 71 (A4) — the watcher pings first: message 0 is the
  // ping, message 1 is the hand-off.
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.sentMessages[0].type, "ping");
  assert.equal(
    JSON.stringify(env.sentMessages[1]),
    JSON.stringify({
      type: "download_link",
      url: "http://127.0.0.1/file.zip",
      referrer: "http://127.0.0.1/page.html",
    }),
  );
  assert.equal(env.storage.lastHandoff.ok, true);
});

test("watcher resumes the browser download when the hand-off fails", async () => {
  // Round 69 (a) — the safety rule: NEVER cancel a browser
  // download until the app confirms receipt. The old order
  // cancelled first, so a failed hand-off silently swallowed the
  // file (Round 68 retest). Now the paused browser download is
  // resumed and proceeds normally in Chrome. Round 71: the ping
  // succeeds here (failHandover fails only the hand-off), so the
  // pause-then-resume path is what is under test.
  const env = loadBackground({
    seed: { desktopIntegration: true },
    failHandover: true,
  });
  await env.fireDownload(zipDownload(1));
  assert.deepEqual(env.paused, [1]);
  assert.deepEqual(env.resumed, [1]);
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.storage.lastHandoff.ok, false);
});

test("watcher never pauses when the host does not answer the ping (Round 71)", async () => {
  // Round 71 (A4) — ping first: a dead host means the browser
  // download is never paused at all, so there is nothing to
  // resume and nothing to swallow — the file simply downloads
  // in Chrome. Only the ping itself was sent.
  const env = loadBackground({
    seed: { desktopIntegration: true },
    hostError: "Could not establish connection",
  });
  await env.fireDownload(zipDownload(1));
  assert.deepEqual(env.paused, []);
  assert.deepEqual(env.resumed, []);
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 1);
  assert.equal(env.sentMessages[0].type, "ping");
  // Round 88.8 — the ping refusal is recorded so the popup can
  // say why (the silent killer in the live dlproxy case).
  assert.equal(env.storage.lastHandoff.ok, false);
  assert.match(env.storage.lastHandoff.message, /did not answer the ping within 3 seconds/);
});

test("watcher attaches the target URL's cookies to the hand-off", async () => {
  const env = loadBackground({
    seed: { desktopIntegration: true },
    cookiesForUrl: (url) => (url === "http://127.0.0.1/file.zip" ? DUMMY_COOKIES : []),
  });
  await env.fireDownload(zipDownload(1));
  // Round 71 (A4) — message 0 is the ping, message 1 the hand-off.
  assert.equal(env.sentMessages.length, 2);
  assert.equal(
    JSON.stringify(env.sentMessages[1].cookies),
    JSON.stringify(DUMMY_COOKIES),
  );
  // Round 87 — domain/path now travel with each cookie (the
  // scoped writer needs them); values are never logged.
  assert.deepEqual(Object.keys(env.sentMessages[1].cookies[0]).sort(), ["domain", "name", "path", "value"]);
});

test("watcher sends no cookies field when the browser has none for the URL", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(zipDownload(1));
  assert.equal(env.sentMessages.length, 2);
  assert.ok(!("cookies" in env.sentMessages[1]));
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
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.sentMessages[1].url, "https://127.0.0.1/download?id=7");
});

test("watcher leaves an excluded site's download alone", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true, desktopExcluded: ["127.0.0.1"] } });
  await env.fireDownload(zipDownload(1));
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 0);
  // Round 88.8 — the refusal is recorded so the popup can say why.
  assert.equal(env.storage.lastHandoff.ok, false);
  assert.match(env.storage.lastHandoff.message, /opted out: 127\.0\.0\.1/);
});

test("watcher leaves downloads alone when desktop integration is off", async () => {
  const env = loadBackground({ seed: { desktopIntegration: false } });
  await env.fireDownload(zipDownload(1));
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 0);
  // Round 88.8 — the refusal is recorded so the popup can say why.
  assert.equal(env.storage.lastHandoff.ok, false);
  assert.match(env.storage.lastHandoff.message, /Browser integration is turned off/);
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
  // Round 88.8 — the refusal is recorded so the popup can say why.
  assert.equal(env.storage.lastHandoff.ok, false);
  assert.match(env.storage.lastHandoff.message, /Not a downloadable file type \(txt\)/);
});

test("the same URL twice is handed over twice (the drain de-duplicates by path)", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(zipDownload(1));
  await env.fireDownload(zipDownload(2));
  assert.deepEqual(env.cancelled, [1, 2]);
  // Round 71 (A4) — each download sends a ping then a hand-off.
  assert.equal(env.sentMessages.length, 4);
  assert.equal(env.sentMessages[1].url, "http://127.0.0.1/file.zip");
  assert.equal(env.sentMessages[3].url, "http://127.0.0.1/file.zip");
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

test("watcher calls cancel with the bare download id, not a query object (BUG-049)", async () => {
  // Real Chrome's cancel(downloadId) throws "No matching signature"
  // for cancel({ id }) — and the old fake destructured the object,
  // so the suite passed while every real download sailed past the
  // watcher. The fake now throws on a non-number, so this test
  // fails if the call ever regresses to the object form.
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(zipDownload(7));
  assert.deepEqual(env.cancelled, [7]);
  assert.equal(env.sentMessages.length, 2);
});

test("watcher still hands over when cancel fails because the download already finished (BUG-049)", async () => {
  // A small file can complete before this async handler runs, so
  // cancel rejects. The URL must still reach Sonic Manager instead
  // of vanishing silently — and the browser's finished copy is
  // left alone (nothing is erased when nothing was cancelled).
  const env = loadBackground({ seed: { desktopIntegration: true }, cancelThrows: true });
  await env.fireDownload(zipDownload(1));
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.sentMessages[1].url, "http://127.0.0.1/file.zip");
  assert.equal(env.storage.lastHandoff.ok, true);
});

test("watcher skips a file:// download: the file is already local (Round 72 B)", async () => {
  // A download whose URL is file:// is a local file Chrome is
  // already fetching from disk. The engine probe rejects every
  // non-http(s)/ftp scheme, so handing it over would only create
  // a Failed row — the watcher leaves it to download natively.
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload({
    id: 7,
    url: "file:///C:/Users/test/Downloads/clip.mp4",
    filename: "clip.mp4",
    state: "in_progress",
    referrer: "file:///C:/page.html",
  });
  assert.deepEqual(env.paused, []);
  assert.deepEqual(env.cancelled, []);
  assert.equal(env.sentMessages.length, 0);
});

test("watcher catches an http(s) download link on a file:// page (Round 72 B)", async () => {
  // The Round 71 retest's file:// link clicks produced nothing,
  // which looked like a page-scheme gap — but the watcher keys
  // on the FILE's URL, never the page's. A link to an http(s)
  // file on a local HTML page is caught exactly like one on a
  // web page (the retest clicks most plausibly hit a moment when
  // the fixture server was down).
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload({
    id: 8,
    url: "http://127.0.0.1:62282/slow-a.mp4",
    filename: "slow-a.mp4",
    state: "in_progress",
    referrer: "file:///C:/videos.html",
  });
  assert.deepEqual(env.paused, [8]);
  assert.deepEqual(env.cancelled, [8]);
  assert.deepEqual(env.erased, [8]);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.sentMessages[1].url, "http://127.0.0.1:62282/slow-a.mp4");
});

test("watcher erases without cancelling when the download completes during pingHost() (Round 73 Group 4)", async () => {
  // The download finishes while the ping is in flight, so by the
  // time the hand-off is confirmed the browser copy is complete:
  // cancel() on a completed download throws in real Chrome, so
  // the watcher must erase the finished copy from the list and
  // NEVER call cancel. The hand-off itself still succeeds and
  // nothing is resumed.
  const env = loadBackground({ seed: { desktopIntegration: true }, completeDuringPing: true });
  await env.fireDownload(zipDownload(1));
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, [1]);
  assert.deepEqual(env.resumed, []);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.sentMessages[1].url, "http://127.0.0.1/file.zip");
  assert.equal(env.storage.lastHandoff.ok, true);
});

test("watcher catches a download with an empty filename by falling back to the URL (Round 73 Group 4)", async () => {
  // Chrome can fire onCreated before the filename is known
  // (filename ""), so the watcher falls back to the URL's
  // extension (extensionOfFilename("") || extensionOf(url)) —
  // no onDeterminingFilename listener is needed.
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload({
    id: 1,
    url: "http://127.0.0.1/slow-a.mp4",
    filename: "",
    state: "in_progress",
    referrer: "http://127.0.0.1/page.html",
  });
  assert.deepEqual(env.paused, [1]);
  assert.deepEqual(env.cancelled, [1]);
  assert.deepEqual(env.erased, [1]);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.sentMessages[1].url, "http://127.0.0.1/slow-a.mp4");
  assert.equal(env.storage.lastHandoff.ok, true);
});

test("watcher falls back to try-cancel when downloads.search rejects (Round 73 Group 4)", async () => {
  // If the state lookup itself fails, the watcher must behave
  // exactly as before: try cancel, erase only if cancelled.
  const env = loadBackground({ seed: { desktopIntegration: true }, searchThrows: true });
  await env.fireDownload(zipDownload(1));
  assert.deepEqual(env.cancelled, [1]);
  assert.deepEqual(env.erased, [1]);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.storage.lastHandoff.ok, true);
});

// Round 88.8 — MIME fallback: an opaque proxy link carries no
// extension in the URL or the filename, so the watcher must use
// the download's MIME type instead. The live case is a RAR from
// https://tunnel5.dlproxy.uk/download/<token>?sig=<token>.
const opaqueDownload = (id, mime) => ({
  id,
  url: "https://tunnel5.dlproxy.uk/download/nGw5A-kxTG7-bukYw_8crDs9rYodE8GaaTGSu1gvA?sig=VgA7APPUYnL3RQLg",
  filename: "download",
  mime,
  state: "in_progress",
  referrer: "https://tunnel5.dlproxy.uk/page.html",
});

test("watcher still catches a .rar by filename with no extension in the URL (Round 88.8)", async () => {
  // Existing behavior pinned: the filename alone decides when it
  // carries a catch-list extension, whatever the URL looks like.
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload({
    id: 1,
    url: "https://tunnel5.dlproxy.uk/download/nGw5A-kxTG7-bukYw_8crDs9rYodE8GaaTGSu1gvA?sig=VgA7APPUYnL3RQLg",
    filename: "archive.rar",
    state: "in_progress",
    referrer: "https://tunnel5.dlproxy.uk/page.html",
  });
  assert.deepEqual(env.paused, [1]);
  assert.deepEqual(env.cancelled, [1]);
  assert.deepEqual(env.erased, [1]);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(
    env.sentMessages[1].url,
    "https://tunnel5.dlproxy.uk/download/nGw5A-kxTG7-bukYw_8crDs9rYodE8GaaTGSu1gvA?sig=VgA7APPUYnL3RQLg",
  );
  assert.equal(env.storage.lastHandoff.ok, true);
});

test("watcher catches an extensionless download when the MIME type is a RAR archive (Round 88.8)", async () => {
  // The live defect: neither the URL nor the filename carries
  // .rar, so the extension check alone skips a real file. The
  // server's MIME type is the signal that catches it.
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(opaqueDownload(1, "application/x-rar-compressed"));
  assert.deepEqual(env.paused, [1]);
  assert.deepEqual(env.cancelled, [1]);
  assert.deepEqual(env.erased, [1]);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(
    env.sentMessages[1].url,
    "https://tunnel5.dlproxy.uk/download/nGw5A-kxTG7-bukYw_8crDs9rYodE8GaaTGSu1gvA?sig=VgA7APPUYnL3RQLg",
  );
  assert.equal(env.storage.lastHandoff.ok, true);
});

test("watcher catches an extensionless download served as application/octet-stream (Round 88.8)", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(opaqueDownload(1, "application/octet-stream"));
  assert.deepEqual(env.paused, [1]);
  assert.deepEqual(env.cancelled, [1]);
  assert.deepEqual(env.erased, [1]);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.storage.lastHandoff.ok, true);
});

test("watcher catches an extensionless download served as video/mp4 (Round 88.8)", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(opaqueDownload(1, "video/mp4"));
  assert.deepEqual(env.paused, [1]);
  assert.deepEqual(env.cancelled, [1]);
  assert.deepEqual(env.erased, [1]);
  assert.equal(env.sentMessages.length, 2);
  assert.equal(env.storage.lastHandoff.ok, true);
});

test("watcher does not catch an extensionless download served as text/html (Round 88.8)", async () => {
  // An HTML page must never open an Add URL dialog.
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(opaqueDownload(1, "text/html"));
  assert.deepEqual(env.paused, []);
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 0);
});

test("watcher does not catch an extensionless download served as application/json (Round 88.8)", async () => {
  // The f.txt spam lesson (Rounds 88.5/88.6): a JSON attachment
  // is never a file, whatever the URL looks like.
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(opaqueDownload(1, "application/json"));
  assert.deepEqual(env.paused, []);
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 0);
});

test("watcher does not catch an extensionless download served as text/plain (Round 88.8)", async () => {
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(opaqueDownload(1, "text/plain"));
  assert.deepEqual(env.paused, []);
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 0);
});

test("watcher still skips an extensionless download with no MIME type (Round 88.8)", async () => {
  // No extension anywhere and no MIME signal: the old behavior
  // is preserved — nothing to classify the download with.
  const env = loadBackground({ seed: { desktopIntegration: true } });
  await env.fireDownload(opaqueDownload(1, undefined));
  assert.deepEqual(env.paused, []);
  assert.deepEqual(env.cancelled, []);
  assert.deepEqual(env.erased, []);
  assert.equal(env.sentMessages.length, 0);
});
