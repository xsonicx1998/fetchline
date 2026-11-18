// Fetchline browser extension — background service worker (MV3).
//
// What it does, and nothing more:
//  * Adds a "Download with Fetchline" item to the right-click menu on
//    links and on image / video / audio elements (the menu API hands
//    us the URL, so no content script is injected into any page).
//  * Watches downloads the browser starts. When catching is on, the
//    site is not opted out and the file type is in the catch list,
//    it pauses the browser download, hands the URL to the desktop
//    app, and cancels the browser copy ONLY once the app confirms
//    receipt. If the hand-off fails the browser download is resumed
//    and proceeds normally in Chrome — a failed hand-off never
//    swallows the file (Round 69). Page loads are never blocked.
//  * Records every hand-off in chrome.storage.local for the popup
//    to show (Milestone 2): the last hand-off's outcome and time,
//    and the pending count the host reports — never a URL.
//  * Talks to the desktop app over Native Messaging
//    (chrome.runtime.sendNativeMessage, host "com.fetchline.host").
//
// What it deliberately does NOT do: it never reads page content,
// and it never logs a URL or a message body — failures are counted,
// not printed. Since the S13 finish it DOES send cookies, but only
// the cookies for the exact URL being handed over (see cookiesFor
// below): a login-only link needs its session cookie on the probe
// and on the download itself, and sending anything else would be
// pointless — the host attaches them to that download's requests
// only, for that host only, and never logs them.

const HOST_NAME = "com.fetchline.host";

// File types the download watcher hands to Fetchline. A page that
// only links to one of these still needs a click or a right-click;
// the watcher only fires for downloads the browser itself started.
// Keep this list conservative: small web assets (images on a page,
// stylesheets, scripts) are not downloads and never reach us.
const DEFAULT_CATCH_EXTENSIONS = [
  "zip", "rar", "7z", "tar", "gz",
  "exe", "msi", "dmg", "iso", "apk",
  "pdf", "epub",
  "mp4", "mkv", "avi", "mov", "webm",
  "mp3", "flac", "wav", "aac", "ogg",
  "doc", "docx", "xls", "xlsx", "ppt", "pptx",
];

const STORE_KEYS = {
  catchEnabled: "catchEnabled",       // extension catch toggle, default true
  excludedSites: "excludedSites",     // extension-local per-site opt-outs
  desktopIntegration: "desktopIntegration", // last known desktop flag (null = unknown)
  desktopExcluded: "desktopExcluded", // last known desktop excluded-sites list
  pendingCount: "pendingCount",       // links waiting in pending_links (Milestone 2)
  lastHandoff: "lastHandoff",         // {at, ok, message} of the last hand-off (Milestone 2)
};

function hostOf(url) {
  try {
    return new URL(url).hostname.toLowerCase();
  } catch {
    return null;
  }
}

function extensionOf(url) {
  try {
    const path = new URL(url).pathname;
    const dot = path.lastIndexOf(".");
    if (dot === -1) return "";
    return path.slice(dot + 1).toLowerCase();
  } catch {
    return "";
  }
}

// The extension of a download's local filename ("file.zip", or a
// full path like "/home/user/Downloads/file.zip" — Chrome's
// downloads API reports the absolute local path). This must NOT go
// through extensionOf: a bare filename is not a URL, and
// `new URL("file.zip")` throws, so extensionOf returns "" and the
// watcher would skip every download that already has a filename
// (BUG-023: the watcher was effectively dead — see watcher.test.js).
function extensionOfFilename(filename) {
  if (!filename) return "";
  const base = filename.split(/[\\/]/).pop() || "";
  const dot = base.lastIndexOf(".");
  if (dot === -1) return "";
  return base.slice(dot + 1).toLowerCase();
}

function isExcluded(host, state) {
  if (!host) return false;
  const lists = [state[STORE_KEYS.excludedSites] || [], state[STORE_KEYS.desktopExcluded] || []];
  return lists.some((list) =>
    list.some((entry) => {
      const e = String(entry).toLowerCase();
      return host === e || host.endsWith(`.${e}`);
    }),
  );
}

async function loadState() {
  const stored = await chrome.storage.local.get([
    STORE_KEYS.catchEnabled,
    STORE_KEYS.excludedSites,
    STORE_KEYS.desktopIntegration,
    STORE_KEYS.desktopExcluded,
    STORE_KEYS.pendingCount,
    STORE_KEYS.lastHandoff,
  ]);
  return {
    [STORE_KEYS.catchEnabled]: stored[STORE_KEYS.catchEnabled] !== false, // default ON
    [STORE_KEYS.excludedSites]: stored[STORE_KEYS.excludedSites] || [],
    [STORE_KEYS.desktopIntegration]: stored[STORE_KEYS.desktopIntegration] ?? null,
    [STORE_KEYS.desktopExcluded]: stored[STORE_KEYS.desktopExcluded] || [],
    [STORE_KEYS.pendingCount]: stored[STORE_KEYS.pendingCount] ?? null,
    [STORE_KEYS.lastHandoff]: stored[STORE_KEYS.lastHandoff] || null,
  };
}

// Record the last hand-off for the popup to show (Milestone 2):
// whether it worked, when, and what the host said. Only the outcome
// is stored — never a URL or a message body.
async function recordHandoff(ok, message, pendingCount) {
  const update = {
    [STORE_KEYS.lastHandoff]: { at: Date.now(), ok, message: message || "" },
  };
  if (typeof pendingCount === "number") {
    update[STORE_KEYS.pendingCount] = pendingCount;
  }
  await chrome.storage.local.set(update).catch(() => {});
}

function sendToHost(message) {
  return new Promise((resolve, reject) => {
    let settled = false;
    // Round 69 (a): the host is a one-shot process that answers
    // in milliseconds when healthy; a reply that never comes
    // (crashed host, wedged DB open) must fail loudly instead of
    // hanging the caller forever — the download watcher pauses
    // the browser download while it waits, so an unbounded wait
    // would stall the file. A late reply after the timeout is a
    // harmless no-op (the promise already settled).
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      reject(new Error("Sonic Manager did not answer within 15 seconds"));
    }, 15000);
    const done = (fn, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      fn(value);
    };
    try {
      chrome.runtime.sendNativeMessage(HOST_NAME, message, (reply) => {
        if (chrome.runtime.lastError) {
          done(reject, new Error(chrome.runtime.lastError.message));
          return;
        }
        done(resolve, reply || null);
      });
    } catch (err) {
      done(reject, err);
    }
  });
}

// Cookies for one hand-off (S13 finish): the cookies the browser
// holds for the exact URL being handed over, as name/value pairs
// only — no domain, path, expiry or flags, because the Cookie
// header needs nothing else and the host must never learn more
// than it needs. The host attaches them to that download's probe
// and segment requests, for that host only; it never logs them,
// and the desktop app keeps them in memory only (a restart drops
// them) and wipes them from the pending row when the drain decides
// the link. No cookies are ever read for a URL that is not being
// handed over, and a page the host expands itself (Download All)
// carries none — the host cannot use them there. Guarded: a
// context without chrome.cookies simply sends no cookies.
async function cookiesFor(url) {
  try {
    if (!chrome.cookies || typeof chrome.cookies.getAll !== "function") return [];
    const all = await chrome.cookies.getAll({ url });
    return (all || []).map((c) => ({ name: c.name, value: c.value }));
  } catch {
    return [];
  }
}

// Ask the desktop app for its browser-integration flag and excluded
// sites, and cache them. The desktop setting is the direction of
// truth: when it is off, this extension must not catch anything.
// A missing host (desktop app not installed / not running) is a
// normal state, not an error to surface — catching simply stays
// governed by the extension's own toggle until the host answers.
async function refreshDesktopSettings() {
  try {
    const reply = await sendToHost({ type: "get_settings" });
    if (reply && reply.ok) {
      const update = {
        [STORE_KEYS.desktopIntegration]: reply.browser_integration !== false,
        [STORE_KEYS.desktopExcluded]: reply.excluded_sites || [],
      };
      if (typeof reply.pending_count === "number") {
        update[STORE_KEYS.pendingCount] = reply.pending_count;
      }
      await chrome.storage.local.set(update);
    }
  } catch {
    // Host unreachable: keep the last cached values.
  }
}

async function handToFetchline(message) {
  // Attach the target URL's cookies, if the browser has any: the
  // download_link URL, or the chosen video variant's URL. A
  // download_all carries no cookies — the host expands the page
  // itself (see cookiesFor). Only a non-empty list is attached, so
  // a hand-off with no cookies for its URL is byte-identical to
  // the old no-cookie message.
  let cookieUrl = null;
  if (message.type === "download_link" && message.url) cookieUrl = message.url;
  if (
    message.type === "video_found" &&
    message.variants &&
    message.variants[0] &&
    message.variants[0].url
  ) {
    cookieUrl = message.variants[0].url;
  }
  if (cookieUrl) {
    const cookies = await cookiesFor(cookieUrl);
    if (cookies.length > 0) message.cookies = cookies;
  }
  let reply;
  try {
    reply = await sendToHost(message);
  } catch (err) {
    await recordHandoff(false, `Could not reach Sonic Manager: ${err.message || err}`);
    throw err;
  }
  if (reply && reply.ok === false) {
    const error = reply.error || "Sonic Manager refused the link";
    await recordHandoff(false, error);
    throw new Error(error);
  }
  await recordHandoff(
    true,
    (reply && reply.message) || "Sent to Sonic Manager",
    reply && typeof reply.pending_count === "number" ? reply.pending_count : undefined,
  );
  return reply;
}

// -- context menus -------------------------------------------------------

chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.create({
    id: "fetchline-download",
    title: "Download with Sonic Manager",
    contexts: ["link", "image", "video", "audio"],
  });
  chrome.contextMenus.create({
    id: "fetchline-download-all",
    title: "Download all links on this page with Sonic Manager",
    contexts: ["page"],
  });
  // Round 68 — the popup v2 has no "Find videos" button any
  // more, so the video scan moved here: right-click anywhere on
  // a page and the floating Download button appears on its
  // videos.
  chrome.contextMenus.create({
    id: "fetchline-find-videos",
    title: "Find videos on this page",
    contexts: ["page"],
  });
  void refreshDesktopSettings();
});

chrome.runtime.onStartup.addListener(() => {
  void refreshDesktopSettings();
});

chrome.contextMenus.onClicked.addListener(async (info) => {
  try {
    if (info.menuItemId === "fetchline-find-videos") {
      await injectVideoDetector();
      return;
    }
    if (info.menuItemId === "fetchline-download-all") {
      // No content script, so the extension cannot list the page's
      // links itself. The host expands the page: it fetches the page
      // and enqueues every downloadable link it finds.
      await handToFetchline({ type: "download_all", page_url: info.pageUrl, links: [] });
      return;
    }
    if (info.menuItemId === "fetchline-download") {
      // linkUrl for links; srcUrl for media elements.
      const url = info.linkUrl || info.srcUrl;
      if (!url) return;
      await handToFetchline({ type: "download_link", url, referrer: info.pageUrl || null });
    }
  } catch (err) {
    // Round 69 — the old empty catch swallowed injection failures
    // (no permission, tab gone, scripting error), so "Find videos
    // on this page" could do nothing with no trace at all — the
    // Round 68 retest's video failure. Record it for the popup,
    // which shows the last scan outcome. The user gets no toast
    // from a service worker, and the link stays where it was (the
    // browser download was never started for a menu click).
    await chrome.storage.local
      .set({
        lastVideoScan: {
          error: err && err.message ? err.message : String(err),
          at: Date.now(),
        },
      })
      .catch(() => {});
  }
});

// -- download interception -----------------------------------------------

chrome.downloads.onCreated.addListener(async (item) => {
  try {
    if (!item || !item.url) return;
    if (!item.url.startsWith("http://") && !item.url.startsWith("https://")) return;
    if (item.state && item.state !== "in_progress") return;
    const state = await loadState();
    if (!state[STORE_KEYS.catchEnabled]) return;
    // Desktop setting wins: off means do not catch, whatever the
    // extension toggle says. Unknown (host never answered) means
    // the extension toggle decides on its own.
    if (state[STORE_KEYS.desktopIntegration] === false) return;
    const host = hostOf(item.url);
    if (isExcluded(host, state)) return;
    // The filename's own suffix first (Chrome reports the local
    // path, which may be the only place the type appears — a
    // download URL need not end in .zip); fall back to the URL.
    const ext = extensionOfFilename(item.filename) || extensionOf(item.url);
    if (!DEFAULT_CATCH_EXTENSIONS.includes(ext)) return;
    // Round 69 (a) — NEVER cancel the browser download until the
    // app confirms receipt. The old order cancelled first and
    // handed over second, so a failed hand-off silently swallowed
    // the file (neither in the app nor in Chrome — the Round 68
    // retest's auto-capture failure). The new order:
    //   1. pause the browser download (best-effort: a tiny file
    //      may already be finished, and then there is nothing to
    //      pause);
    //   2. hand the URL to the desktop app and WAIT for its reply;
    //   3. only on a confirmed receipt, cancel (and erase) the
    //      browser copy;
    //   4. on ANY hand-off failure, resume the browser download
    //      so it proceeds normally in Chrome — the file is never
    //      lost, it just downloads in the browser instead.
    // BUG-049 (Round 50): chrome.downloads.cancel takes the bare
    // download id — cancel(downloadId) — NOT a query object.
    // The old call cancel({ id }) threw "No matching signature" in
    // real Chrome on EVERY download, the empty catch below swallowed
    // it, and the browser download sailed past the watcher uncaught
    // with no trace (the Node fake destructured an object, so the
    // unit suite never saw it).
    let paused = false;
    try {
      await chrome.downloads.pause(item.id);
      paused = true;
    } catch {
      // Already finished (or otherwise unpausable): the browser
      // copy stays where it is; hand over anyway so the link
      // still lands in Sonic Manager instead of vanishing.
    }
    let handedOver = false;
    try {
      await handToFetchline({
        type: "download_link",
        url: item.url,
        referrer: item.referrer || null,
      });
      handedOver = true;
    } catch {
      // The hand-off failed and the browser download is still
      // there (paused, or finished): resume it so Chrome
      // finishes the file normally. `handToFetchline` already
      // recorded the failure for the popup.
      if (paused) {
        await chrome.downloads.resume(item.id).catch(() => {});
      }
      return;
    }
    if (!handedOver) return;
    let cancelled = false;
    try {
      await chrome.downloads.cancel(item.id);
      cancelled = true;
    } catch {
      // Already finished while the hand-off was in flight: the
      // browser's finished copy stays where it is (only a
      // cancelled download is erased from the browser's list).
    }
    if (cancelled) {
      await chrome.downloads.erase({ id: item.id }).catch(() => {});
    }
  } catch {
    // Unexpected watcher failure before the hand-off: the browser
    // download was never cancelled by us, so it proceeds normally.
  }
});

// Video grabber (S08): inject the detector into the ACTIVE tab
// only, and only because the user asked — the right-click menu's
// "Find videos on this page" item, or the old popup button's
// `scan_page_for_videos` message. activeTab grants temporary
// access to that one tab; there is no content_scripts entry, no
// host permission and no <all_urls>, so no page is ever scanned
// unprompted. Excluded sites and a switched-off desktop flag
// refuse the scan here, before anything is injected.
async function injectVideoDetector() {
  const state = await loadState();
  if (state[STORE_KEYS.desktopIntegration] === false) {
    throw new Error("Browser integration is turned off in Sonic Manager settings");
  }
  const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
  const tab = tabs[0];
  if (!tab || !tab.id || !tab.url || !tab.url.startsWith("http")) {
    throw new Error("This page has no web address to scan.");
  }
  if (isExcluded(hostOf(tab.url), state)) {
    throw new Error("This site is opted out — videos here are not scanned.");
  }
  await chrome.scripting.executeScript({
    target: { tabId: tab.id },
    files: ["video-detect.js", "content.js"],
  });
}

// Let the popup ask for a fresh connection check on demand.
chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message && message.type === "refresh_desktop_settings") {
    refreshDesktopSettings().then(() => sendResponse({ ok: true }));
    return true;
  }
  if (message && message.type === "ping_host") {
    sendToHost({ type: "ping" })
      .then((reply) => sendResponse({ ok: !!(reply && reply.ok !== false), reply }))
      .catch((err) => sendResponse({ ok: false, error: String(err) }));
    return true;
  }
  // Video grabber (S08): the content script reports one chosen
  // variant, and this worker is the only path to the host — the
  // content script never talks to Native Messaging itself.
  if (message && message.type === "video_found") {
    handToFetchline({
      type: "video_found",
      page_url: message.page_url,
      variants: message.variants || [],
    })
      .then((reply) => sendResponse({ ok: true, reply }))
      .catch((err) => sendResponse({ ok: false, error: String(err) }));
    return true;
  }
  // The content script's scan summary, cached for the popup to read.
  if (message && message.type === "videos_found") {
    void chrome.storage.local.set({
      lastVideoScan: {
        pageUrl: message.pageUrl,
        total: message.total || 0,
        downloadable: message.downloadable || 0,
        at: Date.now(),
      },
    });
    return false;
  }
  // Video grabber (S08): inject the detector — the same helper
  // the right-click menu uses now that the popup has no button
  // for it. Kept as a message so an older popup (or a test) can
  // still ask for a scan.
  if (message && message.type === "scan_page_for_videos") {
    injectVideoDetector()
      .then(() => sendResponse({ ok: true }))
      .catch((err) => sendResponse({ ok: false, error: String(err) }));
    return true;
  }
  return false;
});
