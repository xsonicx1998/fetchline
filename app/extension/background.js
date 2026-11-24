// Fetchline browser extension — background service worker (MV3).
//
// What it does, and nothing more:
//  * Adds a "Download with Fetchline" item to the right-click menu on
//    links and on image / video / audio elements (the menu API hands
//    us the URL, so no content script is needed for those).
//  * Auto-injects the video detector into every http(s) page
//    (manifest.json `content_scripts`, Round 72): the floating
//    Download button appears next to any video without a click.
//    The detector reads the integration preference and the
//    excluded-sites list directly from chrome.storage.local
//    (Round 73) — no service-worker round-trip on its critical
//    path, so the button can scan the moment it is injected.
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

// Round 78 (yt-dlp) — resolving a platform video takes seconds
// (the host spawns yt-dlp and waits for it), so a resolve uses
// its own connectNative port with a 30–35 s bound — NEVER the
// 15 s one-shot hand-over bound above, which would abort slow
// resolves. Heartbeats arrive on the port about every 5 s.
const RESOLVE_TIMEOUT_MS = 32000;
const RESOLVE_PORT_NAME = "sonic-resolve";
const EXTRACTOR_ARGS_KEY = "ytdlp_extractor_args";

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
  relayEnabled: "relayEnabled",       // Round 88 — relay master switch, cached from get_settings (default ON)
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

// Round 75 — per-tab video scans, aggregated across frames.
// `chrome.tabs.sendMessage` without a frameId broadcasts to every
// frame and the FIRST response wins, so a rescan used to report an
// ad iframe's "0 videos" while the page had videos; and every
// frame's `videos_found` overwrote the stored scan, so the last
// writer won. Now each frame's scan is kept keyed by tab + frame,
// and the popup shows the SUM across frames. A 0-video frame can
// never overwrite a non-zero aggregate — it only contributes 0.
// In-memory only: the worker rebuilds it from the frames'
// reports on start, and the stored `lastVideoScan` always holds
// the aggregate (not one frame's numbers) for the popup fallback.
const tabVideoScans = new Map(); // tabId -> { pageUrl, frames: Map<frameId, {total, downloadable, at}> }

function recordFrameScan(tabId, frameId, scan) {
  if (tabId == null || frameId == null) return;
  let tab = tabVideoScans.get(tabId);
  if (!tab) {
    tab = { pageUrl: scan.pageUrl || null, frames: new Map() };
    tabVideoScans.set(tabId, tab);
  }
  // Frame 0 is the page itself: a new page URL there means the tab
  // navigated — the old frames' scans belong to the old page.
  if (frameId === 0 && scan.pageUrl) {
    if (tab.pageUrl && scan.pageUrl !== tab.pageUrl) tab.frames.clear();
    tab.pageUrl = scan.pageUrl;
  }
  tab.frames.set(frameId, {
    total: scan.total || 0,
    downloadable: scan.downloadable || 0,
    at: Date.now(),
  });
}

function tabVideoAggregate(tabId) {
  const tab = tabVideoScans.get(tabId);
  if (!tab || tab.frames.size === 0) return null;
  let total = 0;
  let downloadable = 0;
  for (const frame of tab.frames.values()) {
    total += frame.total;
    downloadable += frame.downloadable;
  }
  return { pageUrl: tab.pageUrl, total, downloadable };
}

// A closed tab's scans are forgotten, so a later tab reusing the
// id never inherits stale frames. Guarded: the test fake's
// chrome.tabs has no onRemoved.
if (chrome.tabs && chrome.tabs.onRemoved && chrome.tabs.onRemoved.addListener) {
  chrome.tabs.onRemoved.addListener((tabId) => {
    tabVideoScans.delete(tabId);
    // Round 88 — a closed tab's relay state goes with it, so a
    // later tab reusing the id never inherits its videos.
    relayClearTab(tabId);
    relayPageContext.delete(tabId);
  });
}

// -- Round 88 — relay (browser capture, relay-plan.md §4) ------------
//
// Observe-only webRequest capture of inline media, manifests and
// files the other four capture paths miss. Passive media /
// manifests are BUFFERED per tab and only reach the host when the
// user clicks one in the video button or the popup — never an
// auto-popping dialog. Only an explicit file capture (kind
// "file") goes to the host immediately. Nothing here is ever
// logged with a URL, a message body or a cookie value.

const RELAY_MEDIA_MIN_BYTES = 256 * 1024; // media under this is an ad / UI sound, not a download
const RELAY_MAYBE_MAX_BYTES = 64 * 1024; // a "maybe" manifest answer is small and text-ish
const RELAY_SENT_HEADERS_TTL_MS = 2 * 60 * 1000;
const RELAY_SENT_HEADERS_CAP = 512;
const RELAY_SEEN_TTL_MS = 2 * 60 * 1000;

// The only request headers ever remembered or forwarded —
// `Cookie` is NEVER in this set (Revision 1: cookies ride the
// validated `cookies` array only, read live via cookiesFor).
const RELAY_HEADER_CANONICAL = {
  "user-agent": "User-Agent",
  referer: "Referer",
  origin: "Origin",
  "accept-language": "Accept-Language",
  "sec-fetch-site": "Sec-Fetch-Site",
  "sec-fetch-mode": "Sec-Fetch-Mode",
  "sec-fetch-dest": "Sec-Fetch-Dest",
  "sec-fetch-user": "Sec-Fetch-User",
  "sec-ch-ua": "Sec-CH-UA",
  "sec-ch-ua-mobile": "Sec-CH-UA-Mobile",
  "sec-ch-ua-platform": "Sec-CH-UA-Platform",
};

const RELAY_MEDIA_EXTENSIONS = [
  "mp4", "mkv", "webm", "mov", "avi",
  "mp3", "flac", "wav", "aac", "ogg", "m4v", "m4a",
];
// Segment pieces, never files: a .ts / .m4s is one fragment of a
// stream the player already fetched, not a download.
const RELAY_SEGMENT_EXTENSIONS = new Set(["ts", "m4s", "m4v", "m2ts"]);
// Passive media/manifests are suppressed on these page hosts —
// the video button + yt-dlp resolver owns those videos (§4.3).
// Matched on the TAB's host / initiator, never the media URL's
// host (a tiktokcdn.com stream on a TikTok page is suppressed;
// the same CDN on another page is not).
const RELAY_YTDLP_HOSTS = [
  "youtube.com", "youtu.be", "tiktok.com",
  "instagram.com", "facebook.com", "fb.watch",
];

const RELAY_WEBREQUEST_TYPES = [
  "main_frame", "sub_frame", "media", "xmlhttprequest", "other", "object",
];
const RELAY_WEBREQUEST_URLS = ["http://*/*", "https://*/*"];

// requestId -> { headers, at }: the browser's exact request
// headers, keyed by requestId (Mister's keying — URL-keying
// breaks when the same URL is fetched twice with different
// headers). No Cookie is ever stored here.
const relaySentHeaders = new Map();
// requestId -> first URL before any redirect (Mister's map):
// the relay message carries url (first, for renewal) and
// finalUrl (where the bytes are).
const relayRedirectFirst = new Map();
// url -> at: every URL handed over by ANY path, 2-minute TTL.
const relaySeen = new Map();
// tabId -> Set(folder): folders of manifests captured in this
// tab — a URL under one is a piece of that stream, not a file.
// Scoped by tabId: the same CDN folder in two tabs must not
// block each other.
const relayManifestFolders = new Map();
// tabId -> Map(url -> relay_capture item): passive inline
// media/manifests waiting for the user to click one.
const relayBuffer = new Map();
// tabId -> { pageUrl, title }: the content.js page mirror
// (Chrome webRequest gives initiator only — no documentUrl /
// originUrl — so the page URL and title come from here).
const relayPageContext = new Map();

function relaySessionApi() {
  try {
    return chrome.storage && chrome.storage.session ? chrome.storage.session : null;
  } catch {
    return null;
  }
}

function relayBufferKey(tabId) {
  return `relayBuffer:${tabId}`;
}

function relayPageKey(tabId) {
  return `relayPage:${tabId}`;
}

function relayMirrorBuffer(tabId) {
  const api = relaySessionApi();
  if (!api) return;
  const items = [...(relayBuffer.get(tabId) || new Map()).values()];
  // Fire-and-forget: the in-memory buffer is the source of
  // truth; the session mirror only survives a worker eviction
  // (and is what the popup reads).
  try {
    const p = api.set({ [relayBufferKey(tabId)]: items });
    if (p && typeof p.catch === "function") p.catch(() => {});
  } catch {
    // Session storage unavailable: the in-memory buffer stands.
  }
}

function relayMirrorPage(tabId, pageUrl, title) {
  relayPageContext.set(tabId, { pageUrl, title: title || "" });
  const api = relaySessionApi();
  if (!api) return;
  try {
    const p = api.set({ [relayPageKey(tabId)]: { pageUrl, title: title || "" } });
    if (p && typeof p.catch === "function") p.catch(() => {});
  } catch {
    // Session storage unavailable: the in-memory mirror stands.
  }
}

function relayNotePageUrl(tabId, url) {
  const prev = relayPageContext.get(tabId);
  relayMirrorPage(tabId, url, prev ? prev.title : "");
}

function relayFolderOf(url) {
  try {
    const u = new URL(url);
    const path = u.pathname;
    const slash = path.lastIndexOf("/");
    return `${u.origin}${slash > 0 ? path.slice(0, slash) : ""}`;
  } catch {
    return "";
  }
}

function relayNoteManifestFolder(tabId, url) {
  const folder = relayFolderOf(url);
  if (!folder) return;
  let set = relayManifestFolders.get(tabId);
  if (!set) {
    set = new Set();
    relayManifestFolders.set(tabId, set);
  }
  set.add(folder);
}

function relayUnderManifestFolder(tabId, url) {
  const set = relayManifestFolders.get(tabId);
  if (!set || set.size === 0) return false;
  const folder = relayFolderOf(url);
  if (!folder) return false;
  for (const known of set) {
    if (folder === known || folder.startsWith(`${known}/`)) return true;
  }
  return false;
}

function relayNoteSeen(url) {
  if (url) relaySeen.set(url, Date.now());
}

function relayWasSeen(url) {
  if (!url) return false;
  const at = relaySeen.get(url);
  if (!at) return false;
  if (Date.now() - at > RELAY_SEEN_TTL_MS) {
    relaySeen.delete(url);
    return false;
  }
  return true;
}

// One entry per (tabId, URL); a later response for the same URL
// only fills in a missing size — it never creates a second row.
function relayBufferAdd(tabId, item) {
  let tab = relayBuffer.get(tabId);
  if (!tab) {
    tab = new Map();
    relayBuffer.set(tabId, tab);
  }
  const prev = tab.get(item.finalUrl || item.url);
  if (prev && (prev.content_length == null || prev.content_length === 0) && item.content_length) {
    prev.content_length = item.content_length;
  }
  if (!prev) tab.set(item.finalUrl || item.url, item);
  relayMirrorBuffer(tabId);
}

function relayBufferItems(tabId, surfacedOnly) {
  const tab = relayBuffer.get(tabId);
  if (!tab) return [];
  const items = [...tab.values()];
  // The video button / popup surface media + manifests only: a
  // "maybe" is recorded, never auto-offered (§4.2).
  if (surfacedOnly) return items.filter((i) => i.kind === "media" || i.kind === "manifest");
  return items;
}

// Round 88.1 — restore the per-tab buffer after a service
// worker restart. The in-memory relayBuffer is the source of
// truth while the worker lives, but Chrome evicts an idle MV3
// worker after ~30 s and the restart wipes it — and the
// session mirror (written on every mutation) was never read
// back. The video button still showed the rows (content.js
// holds its own copy from before the eviction), but the click
// then failed with "That video is no longer available on this
// page" and nothing ever reached the host — exactly the Round
// 88 VPS retest failure (RDP is slow, so minutes pass between
// the dropdown opening and the click). Restoring is lazy, per
// tab, on the two read paths (relay_get_buffered and
// relay_send_buffered); the popup reads the session mirror
// directly and needs no restore. Only a tab with NO in-memory
// map is restored — a live worker's map is authoritative, and
// a consumed tab's mirror is empty anyway.
async function relayRestoreBuffer(tabId) {
  if (tabId == null || relayBuffer.has(tabId)) return;
  const api = relaySessionApi();
  if (!api) return;
  let stored = null;
  try {
    stored = await api.get(relayBufferKey(tabId));
  } catch {
    return;
  }
  const items =
    stored && Array.isArray(stored[relayBufferKey(tabId)])
      ? stored[relayBufferKey(tabId)]
      : [];
  if (items.length === 0) return;
  const tab = new Map();
  for (const item of items) {
    if (!item || typeof item !== "object") continue;
    const key = item.finalUrl || item.url;
    if (!key || tab.has(key)) continue;
    tab.set(key, item);
  }
  if (tab.size > 0) relayBuffer.set(tabId, tab);
}

function relayBufferRemove(tabId, url) {
  const tab = relayBuffer.get(tabId);
  if (!tab) return;
  tab.delete(url);
  relayMirrorBuffer(tabId);
}

function relayClearTab(tabId) {
  // Round 88.3 — a cleared tab's URLs must also leave the
  // seen-set: the buffer is gone, so a later response for the
  // same URL is a re-capture, not a duplicate. Before this,
  // the seen entry survived the clear, and the dedupe in
  // handleRelayHeaders (plus the 206 start>0 rule in
  // relayClassify) dropped every later fetch for two minutes
  // — a same-tab navigation to a media URL lost its relay row
  // with no way back, exactly the Round 88.2 live failure.
  // (A send is different: relayBufferRemove consumes the item
  // WITHOUT forgetting it, so a handed-off URL is never
  // re-captured — that path is untouched here.)
  const tab = relayBuffer.get(tabId);
  if (tab) {
    for (const item of tab.values()) {
      if (!item) continue;
      if (item.finalUrl) relaySeen.delete(item.finalUrl);
      if (item.url) relaySeen.delete(item.url);
    }
  }
  relayBuffer.delete(tabId);
  relayManifestFolders.delete(tabId);
  const api = relaySessionApi();
  if (!api) return;
  try {
    const p = api.set({ [relayBufferKey(tabId)]: [] });
    if (p && typeof p.catch === "function") p.catch(() => {});
  } catch {
    // Session storage unavailable: the in-memory clear stands.
  }
}

function relayRememberSentHeaders(details) {
  if (!details || details.requestId == null) return;
  const headers = {};
  for (const h of details.requestHeaders || []) {
    const canonical = RELAY_HEADER_CANONICAL[String(h.name || "").toLowerCase()];
    if (canonical) headers[canonical] = h.value;
  }
  // 2-minute TTL + 512 cap: sweep the expired, then drop the
  // oldest while over the cap (Map order is insertion order).
  const now = Date.now();
  for (const [id, entry] of relaySentHeaders) {
    if (now - entry.at > RELAY_SENT_HEADERS_TTL_MS) relaySentHeaders.delete(id);
  }
  relaySentHeaders.delete(details.requestId);
  relaySentHeaders.set(details.requestId, { headers, at: now });
  while (relaySentHeaders.size > RELAY_SENT_HEADERS_CAP) {
    const oldest = relaySentHeaders.keys().next().value;
    relaySentHeaders.delete(oldest);
  }
}

function relayRememberRedirect(details) {
  if (!details || details.requestId == null || !details.url) return;
  // The FIRST hop wins: a later hop must not overwrite it.
  if (!relayRedirectFirst.has(details.requestId)) {
    relayRedirectFirst.set(details.requestId, details.url);
  }
}

// Both maps are keyed by requestId, so one forget covers both
// (Jumani note: the redirect map needs the same keying and the
// same cleanup as SENT_HEADERS).
function relayForgetRequest(requestId) {
  if (requestId == null) return;
  relaySentHeaders.delete(requestId);
  relayRedirectFirst.delete(requestId);
}

function relayUserAgentFallback() {
  try {
    if (typeof navigator !== "undefined" && navigator.userAgent) return navigator.userAgent;
  } catch {
    // No navigator in this context (a Node test without one).
  }
  return "";
}

function relayTopLevelSiteOf(pageUrl) {
  try {
    return new URL(pageUrl).origin;
  } catch {
    return null;
  }
}

function relayResponseHeaders(details) {
  const out = {};
  for (const h of details.responseHeaders || []) {
    out[String(h.name || "").toLowerCase()] = h.value;
  }
  return out;
}

// "bytes 0-1023/238000000" -> { start, end, total }; "bytes
// */123" and "bytes 0-99/*" have no full total (total null).
function relayParseContentRange(value) {
  const m = /^bytes (\d+)-(\d+)\/(\d+|\*)$/.exec(String(value || "").trim());
  if (!m) return null;
  return { start: Number(m[1]), end: Number(m[2]), total: m[3] === "*" ? null : Number(m[3]) };
}

const RELAY_CONTENT_TYPE_EXT = {
  "video/mp4": "mp4",
  "video/webm": "webm",
  "video/x-matroska": "mkv",
  "video/quicktime": "mov",
  "video/x-msvideo": "avi",
  "audio/mpeg": "mp3",
  "audio/flac": "flac",
  "audio/wav": "wav",
  "audio/ogg": "ogg",
  "application/ogg": "ogg",
  "application/vnd.apple.mpegurl": "m3u8",
  "application/x-mpegurl": "m3u8",
  "application/dash+xml": "mpd",
  "application/zip": "zip",
  "application/pdf": "pdf",
};

function relayFilename(disposition, url, pageTitle, contentType) {
  // filename*= (UTF-8, RFC 5987) first, then filename=.
  const starred = /filename\*\s*=\s*UTF-8''([^;]+)/i.exec(disposition || "");
  if (starred) {
    try {
      return decodeURIComponent(starred[1].trim());
    } catch {
      return starred[1].trim();
    }
  }
  const plain = /filename\s*=\s*"([^"]+)"|filename\s*=\s*([^;]+)/i.exec(disposition || "");
  if (plain) {
    const name = (plain[1] || plain[2] || "").trim();
    if (name) return name;
  }
  try {
    const base = decodeURIComponent(new URL(url).pathname.split("/").pop() || "");
    if (base) return base;
  } catch {
    // A URL that will not parse has no filename in it.
  }
  const ext = RELAY_CONTENT_TYPE_EXT[contentType] || "";
  const title = String(pageTitle || "").trim();
  if (title) return ext ? `${title}.${ext}` : title;
  return ext ? `download.${ext}` : "download";
}

function relayIsYtdlpPageHost(host) {
  if (!host) return false;
  return RELAY_YTDLP_HOSTS.some((d) => host === d || host.endsWith(`.${d}`));
}

// The capture decision point (§4.2–4.3), in the plan's exact
// early-bail order. Returns a relay_capture item, or null when
// the response is not a capture. Pure except for the seen-set,
// the tab-scoped manifest folders and the page mirror.
function relayClassify(details, page) {
  const url = details.url || "";
  const tabId = details.tabId;
  const type = details.type || "";
  const headers = relayResponseHeaders(details);
  const contentType = (headers["content-type"] || "").split(";")[0].trim().toLowerCase();
  const ext = extensionOf(url);
  const disposition = headers["content-disposition"] || "";
  const isAttachment = /^\s*attachment/i.test(disposition);

  // 1. No page to attribute it to (service-worker fetches).
  if (typeof tabId !== "number" || tabId < 0) return null;
  // 2. blob: / data: / ws: are not replayable.
  if (!/^https?:\/\//i.test(url)) return null;
  // 3. Only a successful response can be a download.
  const status = details.statusCode || 0;
  if (status < 200 || status >= 300) return null;
  // 4. An attachment on a navigation is the download watcher's
  // job (chrome.downloads.onCreated) — never double-capture it.
  if ((type === "main_frame" || type === "sub_frame") && isAttachment) return null;
  // PDF exclusion (user decision, 2026-11-23 — supersedes the
  // plan as written for PDFs): an INLINE PDF opened in a tab
  // to read must NOT pop the Add URL dialog — the most common
  // IDM annoyance. A PDF with attachment is already skipped
  // by step 4 (the watcher owns it); an explicit PDF download
  // (Chrome's PDF button, right-click) still goes through the
  // watcher / context menu, untouched by this rule.
  const isPdf = contentType === "application/pdf" || ext === "pdf";
  if (type === "main_frame" && isPdf && !isAttachment) return null;
  // 5. Range pieces: ?range= / ?bytestart= / ?byteend= URL
  // patterns are chunk fetches, never whole files.
  if (/[?&](range|bytestart|byteend)=/i.test(url)) return null;
  // googlevideo outright (YouTube's own handler).
  const host = hostOf(url);
  if (host === "googlevideo.com" || (host && host.endsWith(".googlevideo.com"))) return null;

  // The capture-time size: the Content-Range total when there
  // is one (including the accepted first 206), else
  // Content-Length.
  const rangeInfo = headers["content-range"]
    ? relayParseContentRange(headers["content-range"])
    : null;
  let contentLength = null;
  if (rangeInfo && rangeInfo.total != null) contentLength = rangeInfo.total;
  else if (headers["content-length"]) {
    const n = Number(headers["content-length"]);
    if (Number.isFinite(n)) contentLength = n;
  }
  // 5 (continued). A 206 is a whole file ONLY on its first
  // fetch: a full Content-Range total >= 256 KiB, especially
  // start === 0 (an HTML5 <video> player's Range: bytes=0-
  // gets a 206 back — skipping every 206 would drop 90%+ of
  // direct MP4/WebM streams). A 206 with no full total, a
  // total under 256 KiB, or a later seek (start > 0) for an
  // already-seen URL is a chunk, not a file.
  if (status === 206) {
    if (!rangeInfo || rangeInfo.total == null) return null;
    if (rangeInfo.total < RELAY_MEDIA_MIN_BYTES) return null;
    if (rangeInfo.start > 0 && relayWasSeen(url)) return null;
    contentLength = rangeInfo.total;
  }

  // 6. Segments: a segment extension / type, or a URL inside
  // an already-captured manifest's folder IN THE SAME TAB.
  // A .m3u8/.mpd URL itself is never suppressed this way — a
  // master and its child variant live in the same folder.
  const isManifestUrl = ext === "m3u8" || ext === "mpd";
  if (!isManifestUrl) {
    if (RELAY_SEGMENT_EXTENSIONS.has(ext) || contentType === "video/mp2t") return null;
    if (relayUnderManifestFolder(tabId, url)) return null;
  }

  // 7. Classify by content-type first, extension second.
  let kind = null;
  if (
    contentType === "application/vnd.apple.mpegurl" ||
    contentType === "application/x-mpegurl" ||
    ext === "m3u8"
  ) {
    kind = "manifest";
  } else if (contentType === "application/dash+xml" || ext === "mpd") {
    kind = "manifest";
  } else if (isAttachment && type !== "main_frame" && type !== "sub_frame") {
    // An attachment on a non-navigation type is an explicit
    // file capture — the only kind that auto-opens the dialog.
    kind = "file";
  } else if (
    contentType.startsWith("video/") ||
    contentType.startsWith("audio/") ||
    contentType === "application/ogg" ||
    (RELAY_MEDIA_EXTENSIONS.includes(ext) &&
      (!contentType || contentType === "application/octet-stream"))
  ) {
    kind = "media";
  } else if (DEFAULT_CATCH_EXTENSIONS.includes(ext)) {
    kind = "file";
  } else if (
    (type === "xmlhttprequest" || type === "other") &&
    (!contentType || contentType === "text/plain") &&
    (contentLength == null || contentLength < RELAY_MAYBE_MAX_BYTES)
  ) {
    // "Maybe" manifest (Mister's class): a small text/untyped
    // answer that might name a manifest — recorded, never
    // auto-offered.
    kind = "maybe";
  } else {
    return null;
  }

  // Media under 256 KiB is an ad, a UI sound or a page chime.
  // Files (attachment / catch-list) have no size minimum, and
  // manifests are always captured.
  if (kind === "media" && contentLength != null && contentLength < RELAY_MEDIA_MIN_BYTES) {
    return null;
  }

  // yt-dlp hosts: passive media/manifests are suppressed
  // entirely (the video button + resolver owns those videos).
  // An explicit file capture on such a page still works.
  const pageHost = hostOf((page && page.pageUrl) || details.initiator || "");
  if ((kind === "media" || kind === "manifest") && relayIsYtdlpPageHost(pageHost)) {
    return null;
  }

  const firstUrl = relayRedirectFirst.get(details.requestId) || url;
  const sent = relaySentHeaders.get(details.requestId);
  const headersOut = sent ? { ...sent.headers } : {};
  // Cold service worker: onSendHeaders ran before the eviction
  // and the map is empty — fall back to navigator.userAgent
  // for UA and the page URL for Referer (cookies still come
  // live from cookiesFor below; only the exact request headers
  // are lost).
  if (!headersOut["User-Agent"]) {
    const ua = relayUserAgentFallback();
    if (ua) headersOut["User-Agent"] = ua;
  }
  const pageUrl = (page && page.pageUrl) || details.initiator || "";
  if (!headersOut["Referer"] && pageUrl) headersOut["Referer"] = pageUrl;

  return {
    type: "relay_capture",
    url: firstUrl,
    finalUrl: url,
    page_url: pageUrl,
    referrer: headersOut["Referer"] || pageUrl || "",
    content_type: contentType,
    content_length: contentLength,
    filename: relayFilename(disposition, url, page && page.title, contentType),
    kind,
    headers: headersOut,
    cookies: [],
  };
}

// The onHeadersReceived handler: gate, classify, dedupe, then
// hand over (file) or buffer (media / manifest / maybe).
async function handleRelayHeaders(details) {
  try {
    if (!details || typeof details.tabId !== "number" || details.tabId < 0) return;
    const state = await loadState();
    // Relay gating (§6): the relay switch AND the desktop
    // master switch must both be on, and the site must not be
    // excluded. The browser_integration master switch still
    // gates everything — integration off means no relay at all.
    if (state[STORE_KEYS.relayEnabled] === false) return;
    if (state[STORE_KEYS.desktopIntegration] === false) return;
    const page = relayPageContext.get(details.tabId) || null;
    const pageUrl = (page && page.pageUrl) || details.initiator || "";
    if (isExcluded(hostOf(pageUrl) || hostOf(details.url || ""), state)) return;
    // A fresh HTML page load retires the old page's buffered
    // items — old-page videos must not linger in the video
    // button / popup after navigation.
    const respHeaders = relayResponseHeaders(details);
    if (
      details.type === "main_frame" &&
      (respHeaders["content-type"] || "").startsWith("text/html")
    ) {
      relayClearTab(details.tabId);
      relayNotePageUrl(details.tabId, details.url || "");
      return;
    }
    const item = relayClassify(details, page);
    if (!item) return;
    // Dedupe: a URL already handed over by ANY path is never
    // re-captured. A later response for a buffered URL only
    // fills in a missing size.
    if (relayWasSeen(item.finalUrl)) {
      const tab = relayBuffer.get(details.tabId);
      const prev = tab && tab.get(item.finalUrl);
      if (prev && (prev.content_length == null || prev.content_length === 0) && item.content_length) {
        prev.content_length = item.content_length;
        relayMirrorBuffer(details.tabId);
      }
      return;
    }
    // Cookies are read live against finalUrl (after a redirect
    // the cookies that matter are the destination's), with the
    // top-level site so partitioned (CHIPS) cookies are not
    // missed. Counts only in logs — never values.
    const topLevelSite = pageUrl ? relayTopLevelSiteOf(pageUrl) : null;
    item.cookies = await cookiesFor(item.finalUrl, topLevelSite);
    relayNoteSeen(item.finalUrl);
    relayNoteSeen(item.url);
    if (item.kind === "manifest") relayNoteManifestFolder(details.tabId, item.finalUrl);
    if (item.kind === "file") {
      // The ONLY path that auto-opens the Add URL dialog.
      await handToFetchline(item);
      return;
    }
    // Passive inline media / manifest / maybe: buffered per
    // tab, surfaced in the video button + popup. No message
    // goes to the host and no dialog opens here.
    relayBufferAdd(details.tabId, item);
  } catch {
    // A relay failure must never break the page: observe-only
    // means a missed capture, not a broken browse.
  }
}

// Send a buffered item to the host when the user clicks it in
// the video button or the popup. Cookies are re-read live at
// click time — the buffered copy may be stale by now.
async function relaySendBuffered(tabId, url) {
  const tab = relayBuffer.get(tabId);
  const item =
    tab &&
    [...tab.values()].find((i) => i.url === url || i.finalUrl === url);
  if (!item) {
    throw new Error("That video is no longer available on this page.");
  }
  const message = { ...item, headers: { ...item.headers } };
  const topLevelSite = message.page_url ? relayTopLevelSiteOf(message.page_url) : null;
  message.cookies = await cookiesFor(message.finalUrl, topLevelSite);
  const reply = await handToFetchline(message);
  relayBufferRemove(tabId, item.finalUrl || item.url);
  return reply;
}

// Observe-only listeners, registered synchronously at the top
// level (a service worker must register them before it can be
// evicted). Guarded: contexts without webRequest (older test
// fakes) simply run without the relay.
if (chrome.webRequest && chrome.webRequest.onSendHeaders) {
  chrome.webRequest.onSendHeaders.addListener(
    (details) => relayRememberSentHeaders(details),
    { urls: RELAY_WEBREQUEST_URLS, types: RELAY_WEBREQUEST_TYPES },
    ["requestHeaders", "extraHeaders"],
  );
}
if (chrome.webRequest && chrome.webRequest.onBeforeRedirect) {
  chrome.webRequest.onBeforeRedirect.addListener(
    (details) => relayRememberRedirect(details),
    { urls: RELAY_WEBREQUEST_URLS, types: RELAY_WEBREQUEST_TYPES },
  );
}
if (chrome.webRequest && chrome.webRequest.onHeadersReceived) {
  chrome.webRequest.onHeadersReceived.addListener(
    (details) => {
      void handleRelayHeaders(details);
    },
    { urls: RELAY_WEBREQUEST_URLS, types: RELAY_WEBREQUEST_TYPES },
    ["responseHeaders"],
  );
}
if (chrome.webRequest && chrome.webRequest.onCompleted) {
  chrome.webRequest.onCompleted.addListener((details) => relayForgetRequest(details.requestId));
}
if (chrome.webRequest && chrome.webRequest.onErrorOccurred) {
  chrome.webRequest.onErrorOccurred.addListener((details) => relayForgetRequest(details.requestId));
}

async function loadState() {
  const stored = await chrome.storage.local.get([
    STORE_KEYS.catchEnabled,
    STORE_KEYS.excludedSites,
    STORE_KEYS.desktopIntegration,
    STORE_KEYS.desktopExcluded,
    STORE_KEYS.pendingCount,
    STORE_KEYS.lastHandoff,
    STORE_KEYS.relayEnabled,
  ]);
  return {
    [STORE_KEYS.catchEnabled]: stored[STORE_KEYS.catchEnabled] !== false, // default ON
    [STORE_KEYS.excludedSites]: stored[STORE_KEYS.excludedSites] || [],
    [STORE_KEYS.desktopIntegration]: stored[STORE_KEYS.desktopIntegration] ?? null,
    [STORE_KEYS.desktopExcluded]: stored[STORE_KEYS.desktopExcluded] || [],
    [STORE_KEYS.pendingCount]: stored[STORE_KEYS.pendingCount] ?? null,
    [STORE_KEYS.lastHandoff]: stored[STORE_KEYS.lastHandoff] || null,
    // Round 88 — the relay master switch, default ON (the
    // desktop default is ON; the engine side adds the setting).
    [STORE_KEYS.relayEnabled]: stored[STORE_KEYS.relayEnabled] !== false,
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

// Round 71 (A4) — a cheap liveness probe for the download
// watcher: true only when the host answers a ping. The watcher
// calls this BEFORE pausing any browser download, so a dead host
// never costs the user a paused-then-resumed download (or a
// swallowed click). A 3 s bound, not 15 s: the host answers in
// milliseconds when healthy, and a download must not wait on a
// dead one. Failures are silent here — the popup shows host
// status from its own ping; the watcher simply lets the browser
// download proceed natively when the host is down.
function pingHost() {
  return new Promise((resolve) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      resolve(false);
    }, 3000);
    const done = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(value);
    };
    try {
      chrome.runtime.sendNativeMessage(HOST_NAME, { type: "ping" }, (reply) => {
        if (chrome.runtime.lastError) {
          done(false);
          return;
        }
        done(!!(reply && reply.ok));
      });
    } catch {
      done(false);
    }
  });
}

// Cookies for one hand-off (S13 finish): the cookies the browser
// holds for the exact URL being handed over. The `Cookie`
// header needs name/value only — but the temp Netscape file
// yt-dlp reads (`--cookies`) needs each cookie's OWN domain
// and path, so those ride along too: a cookie bound to a CDN
// host keeps that host (yt-dlp only sends a cookie to a host
// its domain matches — rewriting every cookie to the page
// host made CDN cookies match nothing), and a cookie with a
// non-root path keeps that path. The host attaches them to
// that download's probe and segment requests, for that host
// only; it never logs them, and the desktop app keeps them
// in memory only (a restart drops them) and wipes them from
// the pending row when the drain decides the link. No cookies
// are ever read for a URL that is not being handed over, and
// a page the host expands itself (Download All) carries none
// — the host cannot use them there. Guarded: a context
// without chrome.cookies simply sends no cookies.
// Round 86 — cookie pipeline checkpoint: log only the cookie
// COUNT and the host, NEVER values. The R85 retest could not
// tell whether the CDN GET lacked cookies, because no stage
// logged even their presence. `url` may be empty (a message
// with no URL at all); the count still logs.
// Round 87 — `cookiesFor` keeps each cookie's domain and
// path (name/value/domain/path), not name/value only: the
// old strip is exactly what lost the CDN scope (see
// background.js:275,908 in the R86 head — the host rebuilt
// every cookie as `.{page host}` + `/`).
function logCookieCount(url, count) {
  try {
    const host = url ? new URL(url).host : "(no url)";
    console.log(`Sonic Manager: cookies for ${host}: ${count} cookie(s)`);
  } catch {
    // URL parsing must never break the hand-off.
  }
}

async function cookiesFor(url, topLevelSite) {
  try {
    if (!chrome.cookies || typeof chrome.cookies.getAll !== "function") return [];
    // Round 88 — query BOTH the unpartitioned jar AND (when
    // the top-level site is known) the partitioned one.
    // Passing partitionKey alone returns ONLY partitioned
    // (CHIPS) cookies and omits the unpartitioned ones, so a
    // single partitioned query would LOSE the normal session
    // cookie. The two lists are merged and deduped by
    // name+domain+path.
    const unpartitioned = await chrome.cookies.getAll({ url });
    let partitioned = [];
    if (topLevelSite) {
      try {
        partitioned = await chrome.cookies.getAll({ url, partitionKey: { topLevelSite } });
      } catch {
        // Partitioned read unsupported or refused: the
        // unpartitioned list above still stands.
        partitioned = [];
      }
    }
    const seen = new Set();
    const out = [];
    for (const c of [...(unpartitioned || []), ...(partitioned || [])]) {
      const key = `${c.name}\0${c.domain}\0${c.path}`;
      if (seen.has(key)) continue;
      seen.add(key);
      // Round 87 — keep each cookie's own domain and path
      // (the host writes them into the Netscape file verbatim;
      // dropping them here is what lost the CDN scope).
      // Name/value/domain/path only — no expiry, no flags, no
      // values in logs.
      out.push({ name: c.name, value: c.value, domain: c.domain, path: c.path });
    }
    return out;
  } catch (err) {
    // Round 86 — the old bare `catch` swallowed the failure
    // and returned [], so a refill blocked by missing host
    // permission (the R85 TikTok case: no host_permissions for
    // the v16 CDN host) looked identical to "no cookies for
    // this URL". Host + message only — never cookie values.
    try {
      const host = new URL(url).host;
      console.warn(
        `Sonic Manager: cookiesFor failed for ${host}: ${err && err.message ? err.message : err}`,
      );
    } catch {
      // URL parsing must never break the hand-off.
    }
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
      // Round 88.3 — only a field the host actually carries
      // updates the cache. A reply that omits a setting must
      // not reset it to a default: an older host sends no
      // relay_enabled, and clobbering the cache with a default
      // would erase a value the user set. (Absent still means
      // ON at read time — loadState defaults relayEnabled to
      // true when nothing is stored.)
      const update = {};
      if (typeof reply.browser_integration === "boolean") {
        update[STORE_KEYS.desktopIntegration] = reply.browser_integration;
      }
      if (Array.isArray(reply.excluded_sites)) {
        update[STORE_KEYS.desktopExcluded] = reply.excluded_sites;
      }
      // Round 88 — the relay switch rides the same reply.
      if (typeof reply.relay_enabled === "boolean") {
        update[STORE_KEYS.relayEnabled] = reply.relay_enabled;
      }
      if (typeof reply.pending_count === "number") {
        update[STORE_KEYS.pendingCount] = reply.pending_count;
      }
      if (Object.keys(update).length > 0) {
        await chrome.storage.local.set(update);
      }
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
  // the old no-cookie message. Round 78: a message that already
  // carries a `cookies` field (a resolved quality pick — "only
  // the cookies used for that resolve, if any", possibly []) is
  // passed through untouched, never re-filled from the media URL.
  // Round 85 — REVISED: an EMPTY array is not a cookie set, it
  // is the absence of one. A public TikTok video's first resolve
  // is cookieless (no consent grant yet), so its pick carries
  // `cookies: []` — and the old `!("cookies" in message)` check
  // then blocked the refill from the media URL, so the pending
  // row got no cookies and the engine GET sent no Cookie at all:
  // TikTok's CDN answers that with 403. An empty array is now
  // refilled from the media URL like a direct pick (the browser's
  // own cookie store domain-matches `.tiktok.com` cookies to the
  // v16 CDN host); a NON-empty array still passes through
  // untouched.
  let cookieUrl = null;
  if (message.type === "download_link" && message.url) cookieUrl = message.url;
  const hasCookies =
    Array.isArray(message.cookies) && message.cookies.length > 0;
  if (
    message.type === "video_found" &&
    message.variants &&
    message.variants[0] &&
    message.variants[0].url &&
    !hasCookies
  ) {
    cookieUrl = message.variants[0].url;
  }
  // Round 88 — a relay_capture is refilled from its finalUrl
  // (where the bytes are), with the page's top-level site so
  // partitioned (CHIPS) cookies are included. A non-empty
  // cookies array passes through untouched, as for video_found.
  let relayTopLevelSite = null;
  if (
    message.type === "relay_capture" &&
    (message.finalUrl || message.url) &&
    !hasCookies
  ) {
    cookieUrl = message.finalUrl || message.url;
    if (message.page_url) relayTopLevelSite = relayTopLevelSiteOf(message.page_url);
  }
  if (cookieUrl) {
    const cookies = await cookiesFor(cookieUrl, relayTopLevelSite);
    if (cookies.length > 0) message.cookies = cookies;
    // Round 86 — checkpoint the refill (count + host only).
    logCookieCount(cookieUrl, cookies.length);
  } else if (hasCookies) {
    // A non-empty cookie set passes through untouched (Round
    // 78): log its count at the same checkpoint.
    const u =
      message.url ||
      (message.variants && message.variants[0] && message.variants[0].url) ||
      "";
    logCookieCount(u, message.cookies.length);
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
  // Round 88 — every URL handed over by ANY path joins the
  // relay seen-set, so the relay never re-captures it (a
  // video-button pick, a link click and a watcher hand-off
  // all suppress a later relay response for the same URL).
  relayNoteSeen(message.url);
  if (message.finalUrl) relayNoteSeen(message.finalUrl);
  if (Array.isArray(message.variants)) {
    for (const v of message.variants) if (v && v.url) relayNoteSeen(v.url);
  }
  return reply;
}

// Round 78 — one yt-dlp resolve over a long-lived native
// port. The content script asks (lazily, on button click only);
// the host answers with resolve_progress heartbeats, then one
// `resolved` or `error` reply. Closing the port kills the
// host's yt-dlp, so Cancel is just a disconnect. `handlers`
// are onProgress / onResolved / onError; cancel() is silent —
// a user-cancelled resolve is not an error to surface.
function startNativeResolve(request, handlers) {
  let settled = false;
  let cancelled = false;
  let port = null;
  const timer = setTimeout(() => {
    finishError({
      type: "error",
      ok: false,
      request_id: request.request_id,
      error: "Sonic Manager took too long to fetch this video's qualities.",
      needs_login: false,
      error_kind: "timeout",
    });
  }, RESOLVE_TIMEOUT_MS);
  function finish(fn, value) {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    try {
      if (port) port.disconnect();
    } catch {
      // Already gone: nothing to close.
    }
    if (!cancelled && typeof fn === "function") fn(value);
  }
  function finishError(value) {
    finish(handlers.onError, value);
  }
  try {
    port = chrome.runtime.connectNative(HOST_NAME);
  } catch (err) {
    finishError({
      type: "error",
      ok: false,
      request_id: request.request_id,
      error: `Could not reach Sonic Manager: ${err && err.message ? err.message : err}`,
      needs_login: false,
      error_kind: "spawn_failed",
    });
    return { cancel() {} };
  }
  port.onMessage.addListener((msg) => {
    if (!msg) return;
    // Round 79 — a stale pre-R78 host answers an unknown
    // `resolve_video` message with an `error` reply that
    // carries NO request_id, so the guard below used to drop it
    // and the resolve hung until the 32 s timeout fired. An
    // id-less error can only belong to this port's one in-flight
    // request, so it is delivered as that request's error (a
    // stale-host diagnostic) instead of being dropped. A
    // present-but-different request_id is still ignored — the
    // cross-talk guard stands.
    if (msg.type === "error" && (msg.request_id === undefined || msg.request_id === null)) {
      finishError(msg);
      return;
    }
    if (msg.request_id !== request.request_id) return;
    if (msg.type === "resolve_progress") {
      if (typeof handlers.onProgress === "function") handlers.onProgress(msg);
      return;
    }
    if (msg.type === "resolved") {
      finish(handlers.onResolved, msg);
      return;
    }
    if (msg.type === "error") {
      finishError(msg);
    }
  });
  // onDisconnect means the host died mid-resolve — surface it
  // as an error, not a silent hang.
  if (port.onDisconnect && typeof port.onDisconnect.addListener === "function") {
    port.onDisconnect.addListener(() => {
      finishError({
        type: "error",
        ok: false,
        request_id: request.request_id,
        error: "Sonic Manager stopped answering while fetching this video's qualities.",
        needs_login: false,
        error_kind: "spawn_failed",
      });
    });
  }
  try {
    port.postMessage(request);
  } catch (err) {
    finishError({
      type: "error",
      ok: false,
      request_id: request.request_id,
      error: `Could not reach Sonic Manager: ${err && err.message ? err.message : err}`,
      needs_login: false,
      error_kind: "spawn_failed",
    });
  }
  return {
    cancel() {
      if (settled) return;
      cancelled = true;
      settled = true;
      clearTimeout(timer);
      try {
        if (port) port.disconnect();
      } catch {
        // Already gone.
      }
    },
  };
}

// Round 78 — the content script's resolve port. One native port
// per resolve_video message; a second message on the same port
// (the needs-login retry) opens a fresh native port. The first
// resolve carries cookies only when the content script has a
// stored consent grant for the site (Round 79); otherwise it is
// cookieless (with_cookies false, cookies []), and only the
// retry after a needs_login error carries cookies, with the
// cookies the content script fetched via `cookies_for` below.
// The hidden extractor-args override is read from storage and
// passed through only when the user stored one — no
// player_client default is ever set in v1.
if (chrome.runtime.onConnect && chrome.runtime.onConnect.addListener) {
  chrome.runtime.onConnect.addListener((port) => {
    if (!port || port.name !== RESOLVE_PORT_NAME) return;
    let current = null;
    const forward = (msg) => {
      try {
        port.postMessage(msg);
      } catch {
        // Content side went away: the native port is cancelled
        // below via onDisconnect.
      }
    };
    port.onMessage.addListener(async (msg) => {
      if (!msg) return;
      if (msg.type === "cancel_resolve") {
        if (current) current.cancel();
        current = null;
        return;
      }
      if (msg.type !== "resolve_video") return;
      const request = {
        type: "resolve_video",
        page_url: msg.page_url,
        request_id: msg.request_id,
        with_cookies: msg.with_cookies === true,
        cookies: Array.isArray(msg.cookies) ? msg.cookies : [],
      };
      try {
        const stored = await chrome.storage.local.get([EXTRACTOR_ARGS_KEY]);
        const override =
          stored && typeof stored[EXTRACTOR_ARGS_KEY] === "string"
            ? stored[EXTRACTOR_ARGS_KEY].trim()
            : "";
        if (override) request.extractor_args = override;
      } catch {
        // Storage unreadable: resolve without the override.
      }
      current = startNativeResolve(request, {
        onProgress: (m) => forward(m),
        onResolved: (m) => {
          current = null;
          forward(m);
        },
        onError: (m) => {
          current = null;
          forward(m);
        },
      });
    });
    if (port.onDisconnect && typeof port.onDisconnect.addListener === "function") {
      port.onDisconnect.addListener(() => {
        if (current) current.cancel();
        current = null;
      });
    }
  });
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
      // Round 72 (A) — the detector is auto-injected now, so ask
      // it to re-scan (a page that loaded before the injection
      // landed gets the injection as a fallback inside
      // `scanActiveTab`).
      await scanActiveTab();
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
    // Round 72 (B) — the scheme gate is on the FILE's URL, never
    // the page's: an http(s) download link on a file:// page is
    // caught normally (the page scheme is irrelevant here). A
    // file:// download itself is skipped on purpose — the file is
    // already on disk, and the app cannot fetch it: the engine
    // probe rejects every non-http(s)/ftp scheme, so handing it
    // over would only create a Failed row. Chrome downloads the
    // local file natively instead.
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
    // Round 71 (A4) — PING FIRST: never pause the browser
    // download until the native host has answered a ping (3 s).
    // Pausing first and handing over second means a dead host
    // costs a round-trip while the user's download sits paused,
    // and a crash between pause and resume swallows the click
    // (the Round 70 retest's auto-capture failure: the download
    // vanished with no dialog and no Chrome download). With the
    // ping first, a dead host means the browser download simply
    // proceeds natively — nothing is ever paused, nothing is
    // ever lost. The ping is cheap (~ms when the host is alive).
    const hostUp = await pingHost();
    if (!hostUp) return;
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
    // Round 73 (Group 4) — already-finished hardening: a small
    // download can finish while the hand-off is in flight, and
    // cancel() on a completed download throws in real Chrome.
    // Ask for the download's CURRENT state first: if it is
    // complete, erase the browser's finished copy from the list
    // and do NOT call cancel; if it is still in progress, cancel
    // then erase as before. If the state lookup itself is
    // unavailable or rejects, fall back to the old behavior (try
    // cancel; erase only if it was cancelled). A cancel failure
    // on an in-progress download still leaves the browser copy
    // alone (BUG-049) — it is never erased uncancelled.
    let currentState = null;
    let searchOk = false;
    try {
      if (chrome.downloads && typeof chrome.downloads.search === "function") {
        const results = await chrome.downloads.search({ id: item.id });
        const found =
          (results || []).find((d) => d && d.id === item.id) || (results || [])[0];
        if (found && typeof found.state === "string") {
          currentState = found.state;
          searchOk = true;
        }
      }
    } catch {
      // Search unavailable/rejected: fall through to try cancel.
    }
    if (searchOk && currentState === "complete") {
      await chrome.downloads.erase({ id: item.id }).catch(() => {});
      return;
    }
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

// Video grabber (S08): inject the detector into the ACTIVE tab.
// Round 72 (A) — this is now only the FALLBACK: the detector is
// auto-injected into every http(s) page (manifest.json
// `content_scripts`), so a scan normally just messages the
// already-running content script (`scanActiveTab` below). A page
// that loaded before the auto-injection landed (fresh install
// or update) has no content script yet — then this injects it
// (the `__fetchlineVideoInstalled` guard makes it a no-op when
// one is already there). Excluded sites and a switched-off
// desktop flag refuse the scan here, before anything is
// injected, exactly as before.
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

// Round 72 (A) — scan the active tab: prefer the content script
// that auto-injection already runs there (it re-scans and replies
// with the summary), and inject only when the tab has no content
// script yet (the `tabs.sendMessage` rejects with "Could not
// establish connection"). The checks are the same as
// `injectVideoDetector`'s — integration on, site not opted out —
// so the menu and the popup cannot scan a page the user opted out
// of. Returns the scan summary when the content script answered
// one, or `{}` when the fallback injection ran (the injected
// script reports its own summary to storage a moment later).
async function scanActiveTab() {
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
  try {
    // Round 75 — the rescan broadcast reaches every frame, and
    // each frame reports its own `videos_found`. The FIRST
    // sendResponse is NOT the answer (an ad iframe with 0 videos
    // used to win it), so the reply here is ignored: wait ~400 ms
    // for the frames to report, then return the tab aggregate —
    // the SUM across frames. A rejection still means "no content
    // script here" and falls back to injection.
    let answered;
    try {
      await chrome.tabs.sendMessage(tab.id, { type: "rescan_videos" });
      answered = true;
    } catch {
      answered = false;
    }
    if (!answered) {
      await injectVideoDetector();
      return {};
    }
    await new Promise((resolve) => setTimeout(resolve, 400));
    const aggregate = tabVideoAggregate(tab.id);
    return aggregate ? { total: aggregate.total, downloadable: aggregate.downloadable } : {};
  } catch {
    await injectVideoDetector();
    return {};
  }
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
  // Round 73 — the `video_allowed` gate is gone: the content
  // script reads the integration preference and the excluded
  // sites directly from chrome.storage.local, so nothing here
  // answers that message type any more.
  // Round 78 — the content script cannot read chrome.cookies
  // itself, so it asks here for a page's cookies when (and only
  // when) the user granted the needs-login retry for that site.
  // The reply is the scoped pairs from `cookiesFor`
  // (name/value/domain/path), so the host's Netscape file
  // keeps each cookie's own scope.
  if (message && message.type === "cookies_for") {
    cookiesFor(message.url || "")
      .then((cookies) => sendResponse({ ok: true, cookies }))
      .catch(() => sendResponse({ ok: true, cookies: [] }));
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
      ...(message.cookies !== undefined ? { cookies: message.cookies } : {}),
    })
      .then((reply) => sendResponse({ ok: true, reply }))
      .catch((err) => sendResponse({ ok: false, error: String(err) }));
    return true;
  }
  // The content script's scan summary, aggregated per tab across
  // frames for the popup to read (Round 75). Each frame reports
  // its own numbers; the stored scan always holds the SUM, so a
  // 0-video iframe can never overwrite a page that has videos.
  if (message && message.type === "videos_found") {
    const tabId = _sender && _sender.tab && _sender.tab.id;
    const frameId = _sender && typeof _sender.frameId === "number" ? _sender.frameId : null;
    if (tabId != null && frameId != null) {
      // Which frame answered, for the diagnosis (counts only —
      // never a URL or a message body).
      if (typeof console !== "undefined" && console.debug) {
        console.debug(
          `[sonic] videos_found tab=${tabId} frame=${frameId} total=${message.total || 0} downloadable=${message.downloadable || 0}`,
        );
      }
      recordFrameScan(tabId, frameId, message);
      const aggregate = tabVideoAggregate(tabId);
      if (aggregate) {
        void chrome.storage.local.set({
          lastVideoScan: {
            pageUrl: aggregate.pageUrl,
            total: aggregate.total,
            downloadable: aggregate.downloadable,
            at: Date.now(),
          },
        });
      }
    } else {
      // No tab/frame on the sender (an older content script):
      // keep the old single-scan write rather than dropping it.
      void chrome.storage.local.set({
        lastVideoScan: {
          pageUrl: message.pageUrl,
          total: message.total || 0,
          downloadable: message.downloadable || 0,
          at: Date.now(),
        },
      });
    }
    return false;
  }
  // Video grabber (S08): scan the active tab — the same helper
  // the right-click menu uses now that the popup has no button
  // for it. Kept as a message so the popup (and a test) can ask
  // for a fresh scan. The reply carries the summary when the
  // content script answered one, so the popup can show this
  // page's result without waiting on storage.
  if (message && message.type === "scan_page_for_videos") {
    scanActiveTab()
      .then((result) => sendResponse({ ok: true, ...(result || {}) }))
      .catch((err) => sendResponse({ ok: false, error: String(err) }));
    return true;
  }
  // Round 88 — the page-context mirror (Jumani note):
  // content.js CANNOT read its own tabId and has no default
  // access to chrome.storage.session, so it sends { pageUrl,
  // title } here and the worker files it under sender.tab.id.
  // A changed page URL is a top-level navigation: the old
  // page's buffered relay items and manifest folders are
  // cleared, so old-page videos don't linger.
  if (message && message.type === "page_context") {
    const tabId = _sender && _sender.tab && _sender.tab.id;
    if (tabId != null && message.pageUrl) {
      const prev = relayPageContext.get(tabId);
      if (prev && prev.pageUrl && prev.pageUrl !== message.pageUrl) {
        relayClearTab(tabId);
      }
      relayMirrorPage(tabId, message.pageUrl, message.title || "");
    }
    return false;
  }
  // Round 88 — the video button / popup ask for this tab's
  // buffered relay items. Only media + manifests are
  // surfaced; a "maybe" is recorded, never offered. Round
  // 88.1 — restore from the session mirror first: a worker
  // restart wipes the in-memory buffer, and the items must
  // still be listed.
  if (message && message.type === "relay_get_buffered") {
    const tabId =
      message.tabId != null
        ? message.tabId
        : _sender && _sender.tab && _sender.tab.id;
    (async () => {
      if (tabId != null) await relayRestoreBuffer(tabId);
      const items = tabId != null ? relayBufferItems(tabId, true) : [];
      sendResponse({ ok: true, items });
    })();
    return true;
  }
  // Round 88 — the user clicked a buffered relay item: send
  // its relay_capture to the host (cookies re-read live at
  // click time) and drop it from the buffer. The background
  // worker is the only path to the host. Round 88.1 — restore
  // from the session mirror first, or a click after a worker
  // restart fails with "That video is no longer available"
  // although the button still shows the row.
  if (message && message.type === "relay_send_buffered") {
    const tabId =
      message.tabId != null
        ? message.tabId
        : _sender && _sender.tab && _sender.tab.id;
    (async () => {
      if (tabId != null) await relayRestoreBuffer(tabId);
      return relaySendBuffered(tabId, message.url || message.finalUrl || "");
    })()
      .then((reply) => sendResponse({ ok: true, reply }))
      .catch((err) => sendResponse({ ok: false, error: String(err) }));
    return true;
  }
  return false;
});
