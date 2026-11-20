// Fetchline video detection (S08 / F05) — the shared pure logic.
//
// This file runs in two places, so it uses no browser-only APIs
// except URL (available in both Node and the page):
//  * the content script (content.js) calls `scanLiveMedia` against
//    the live DOM, where it can read real videoWidth/videoHeight
//    and the playback state (live / DRM);
//  * the Node tests (tests/video-detect.test.js) call
//    `extractMediaFromHtml` over the fixtures in tests/fixtures/.
// The Rust engine mirrors the same rules in engine/src/video.rs —
// the fixture tests on both sides assert the same counts, so the
// two detectors cannot drift apart silently.
//
// Quality is NEVER invented: it comes from the element's real
// dimensions, a resolution label in the URL or file name
// (720p / 1080p / 4K), or — when neither exists — the literal
// string "Unknown quality".
//
// Only openly downloadable streams are reported. A DRM-protected
// element is reported with drm: true so the panel can show the
// honest "not downloadable" state; it is never queued. A live
// stream is reported with live: true; its .m3u8 URL is offered
// as-is (a playlist file, not a merged video).

const KNOWN_MEDIA_EXTENSIONS = [
  "mp4", "webm", "flv", "mp3", "m3u8", "mpd",
  "mkv", "mov", "avi", "m4v", "m4a", "wav", "ogg", "flac", "aac",
];

const UNKNOWN_QUALITY = "Unknown quality";

function extensionOfUrl(url) {
  try {
    const path = new URL(url).pathname;
    const last = path.split("/").pop() || "";
    const dot = last.lastIndexOf(".");
    if (dot === -1) return "";
    return last.slice(dot + 1).toLowerCase();
  } catch {
    return "";
  }
}

function isMediaUrl(url) {
  return KNOWN_MEDIA_EXTENSIONS.includes(extensionOfUrl(url));
}

function formatOfUrl(url) {
  const ext = extensionOfUrl(url);
  return KNOWN_MEDIA_EXTENSIONS.includes(ext) ? ext.toUpperCase() : null;
}

function qualityFromDimensions(width, height) {
  if (!width && !height) return null;
  if (!width) return `${height}p`;
  if (!height) return `${width} wide`;
  return `${width} × ${height}`;
}

// A resolution label bounded by non-alphanumerics, so "720p"
// matches in "clip-720p.mp4" but not inside "17209.mp4".
function qualityFromLabel(text) {
  const lower = String(text || "").toLowerCase();
  for (const label of ["2160p", "1440p", "1080p", "720p", "480p", "360p", "240p"]) {
    const at = lower.search(new RegExp(`(^|[^a-z0-9])${label}([^a-z0-9]|$)`));
    if (at !== -1) return label;
  }
  if (lower.search(/(^|[^a-z0-9])4k([^a-z0-9]|$)/) !== -1) return "4K";
  if (lower.search(/(^|[^a-z0-9])8k([^a-z0-9]|$)/) !== -1) return "8K";
  return null;
}

function qualityFor(url, width, height) {
  return (
    qualityFromDimensions(width || 0, height || 0) ||
    qualityFromLabel(url) ||
    UNKNOWN_QUALITY
  );
}

function dedupeVariants(variants) {
  const seen = new Set();
  return variants.filter((v) => {
    if (!v.url || seen.has(v.url)) return false;
    seen.add(v.url);
    return true;
  });
}

// HLS decision (Milestone 3, 2026-11-15 — see engine/src/video.rs):
// playlists are offered as-is, never merged into one video file.
// The note below is the honest label the panel shows for them, and
// parseM3u8 is the shared playlist reader: a master playlist lists
// its real variants (resolution/bandwidth from the playlist), a
// media playlist counts its segments and says whether it is live
// (no #EXT-X-ENDLIST) or encrypted (#EXT-X-KEY with a method other
// than NONE). Returns null when the text is not a playlist at all.
// Mirrors the Rust parse_m3u8 in engine/src/video.rs — the Node
// tests assert the same playlists on both sides.
function playlistNote(format, live) {
  if (format === "MPD") {
    return "DASH manifest — saved as-is; its segments are not merged into one video file.";
  }
  if (live) {
    return "Live stream — the playlist file is saved as-is; it is not merged into one video file.";
  }
  return "Playlist file — saved as-is; it is not merged into one video file.";
}

function parseM3u8(text) {
  if (typeof text !== "string") return null;
  const lines = text.split(/\r?\n/).map((l) => l.trim()).filter((l) => l !== "");
  if (lines.length === 0 || lines[0] !== "#EXTM3U") return null;
  const variants = [];
  let segments = 0;
  let durationSecs = 0;
  let ended = false;
  let encrypted = false;
  let pending = null;
  for (const line of lines.slice(1)) {
    if (line.startsWith("#EXT-X-STREAM-INF:")) {
      let resolution = null;
      let bandwidth = null;
      for (const attr of line.slice("#EXT-X-STREAM-INF:".length).split(",")) {
        const eq = attr.indexOf("=");
        if (eq === -1) continue;
        const key = attr.slice(0, eq).trim();
        const value = attr.slice(eq + 1).trim().replace(/^"|"$/g, "");
        if (key.toUpperCase() === "RESOLUTION") {
          const m = value.match(/^(\d+)x(\d+)$/);
          if (m) resolution = [Number(m[1]), Number(m[2])];
        } else if (key.toUpperCase() === "BANDWIDTH") {
          const n = Number(value);
          if (Number.isFinite(n)) bandwidth = n;
        }
      }
      pending = { resolution, bandwidth };
      continue;
    }
    if (line.startsWith("#EXT-X-KEY:")) {
      // Strip the tag name first: the first attribute is
      // "METHOD=...", not "#EXT-X-KEY:METHOD=...".
      const method = line.slice("#EXT-X-KEY:".length).split(",").map((a) => a.split("=")).find(
        ([k]) => k && k.trim().toUpperCase() === "METHOD",
      );
      const value = method ? method[1].trim().replace(/^"|"$/g, "").toUpperCase() : "NONE";
      if (value !== "NONE") encrypted = true;
      pending = null;
      continue;
    }
    if (line === "#EXT-X-ENDLIST") {
      ended = true;
      pending = null;
      continue;
    }
    if (line.startsWith("#EXTINF:")) {
      const num = Number(line.slice("#EXTINF:".length).split(",")[0].trim());
      if (Number.isFinite(num)) {
        segments += 1;
        durationSecs += num;
      }
      pending = null;
      continue;
    }
    if (line.startsWith("#")) {
      pending = null;
      continue;
    }
    if (pending) {
      variants.push({ uri: line, resolution: pending.resolution, bandwidth: pending.bandwidth });
      pending = null;
    }
  }
  const isMaster = variants.length > 0;
  return {
    isMaster,
    variants,
    segments,
    durationSecs,
    live: !isMaster && !ended,
    encrypted,
  };
}

// Scan a page's HTML for media: <video src>, <audio src>,
// <source src> and directly linked media files. Relative links
// are resolved against the page URL; fragments are dropped;
// non-http(s), javascript:/mailto:/data: links are skipped.
// Live/DRM state cannot be seen in static HTML: a fixture marks
// them with data-live / data-drm attributes, which this scan reads
// (the live DOM scan reads the real playback state instead).
function extractMediaFromHtml(html, pageUrl) {
  const out = [];
  let base;
  try {
    base = new URL(pageUrl);
  } catch {
    return out;
  }
  // Tag scan: <video …>, <audio …>, <source …>, <a …> with src/href.
  const tagRe = /<(video|audio|source|a)\b[^>]*?(?:src|href)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))[^>]*>/gi;
  let m;
  while ((m = tagRe.exec(html)) !== null) {
    const tag = m[1].toLowerCase();
    const raw = (m[2] ?? m[3] ?? m[4] ?? "").trim();
    if (!raw || raw.startsWith("#")) continue;
    const lower = raw.toLowerCase();
    if (
      lower.startsWith("javascript:") ||
      lower.startsWith("mailto:") ||
      lower.startsWith("tel:") ||
      lower.startsWith("data:")
    ) {
      continue;
    }
    let resolved;
    try {
      resolved = new URL(raw, base);
    } catch {
      continue;
    }
    if (resolved.protocol !== "http:" && resolved.protocol !== "https:") continue;
    resolved.hash = "";
    const url = resolved.toString();
    if (!isMediaUrl(url)) continue;
    const wholeTag = m[0];
    const live = /data-live\s*=\s*["']true["']/i.test(wholeTag);
    const drm = /data-drm\s*=\s*["']true["']/i.test(wholeTag);
    const format = formatOfUrl(url);
    const isPlaylist = format === "M3U8" || format === "MPD";
    out.push({
      url,
      format,
      quality: qualityFor(url, 0, 0),
      live: live || extensionOfUrl(url) === "m3u8",
      drm,
      note: isPlaylist ? playlistNote(format, live || extensionOfUrl(url) === "m3u8") : null,
    });
  }
  return dedupeVariants(out);
}

// Scan the live DOM of a page: every <video> / <audio> element and
// its <source> children, plus directly linked media files. The
// element's real dimensions give the quality; duration === Infinity
// (or an .m3u8 source) marks a live stream; a set mediaKeys object
// marks a DRM-protected element. `doc` is `document` in the page.
function scanLiveMedia(doc, pageUrl) {
  const found = [];
  const push = (url, width, height, live, drm) => {
    if (!url || !isMediaUrl(url)) return;
    let absolute;
    try {
      absolute = new URL(url, pageUrl).toString();
    } catch {
      return;
    }
    found.push({
      url: absolute,
      format: formatOfUrl(absolute),
      quality: qualityFor(absolute, width || 0, height || 0),
      live: !!live || extensionOfUrl(absolute) === "m3u8",
      drm: !!drm,
      note:
        formatOfUrl(absolute) === "M3U8" || formatOfUrl(absolute) === "MPD"
          ? playlistNote(formatOfUrl(absolute), !!live || extensionOfUrl(absolute) === "m3u8")
          : null,
    });
  };
  doc.querySelectorAll("video, audio").forEach((el) => {
    const isVideo = el.tagName.toLowerCase() === "video";
    const width = isVideo ? el.videoWidth || 0 : 0;
    const height = isVideo ? el.videoHeight || 0 : 0;
    const live = isVideo && el.duration === Infinity;
    const drm = !!el.mediaKeys;
    const src = el.currentSrc || el.getAttribute("src") || "";
    if (src) push(src, width, height, live, drm);
    // Each <source> is its own variant: its quality comes from its
    // own URL label, NOT from the parent element's dimensions —
    // those describe whichever source is currently playing, and
    // lending them to every source would invent qualities.
    el.querySelectorAll("source").forEach((s) => {
      push(s.getAttribute("src") || "", 0, 0, live, drm);
    });
  });
  // Directly linked media files the page points at.
  doc.querySelectorAll('a[href]').forEach((a) => {
    push(a.getAttribute("href") || "", 0, 0, false, false);
  });
  return dedupeVariants(found);
}

// Round 78 (yt-dlp) — supported platforms. The <video> on
// these pages is a blob:/MediaSource handle, never the file, so
// the direct scan above can never supply qualities there — the
// content script shows the Download button anyway and resolves
// qualities through the host's yt-dlp on click. Subdomains
// (www/mobile/m) count; lookalikes (notyoutube.com) do not —
// the match is exact-or-dot-suffix, never a substring.
const SUPPORTED_PLATFORM_DOMAINS = [
  { domain: "youtube.com", name: "YouTube" },
  { domain: "youtu.be", name: "YouTube" },
  { domain: "facebook.com", name: "Facebook" },
  { domain: "fb.watch", name: "Facebook" },
  { domain: "instagram.com", name: "Instagram" },
  { domain: "tiktok.com", name: "TikTok" },
];

function platformOfUrl(url) {
  let host = "";
  try {
    host = new URL(url).hostname.toLowerCase();
  } catch {
    return null;
  }
  return (
    SUPPORTED_PLATFORM_DOMAINS.find(
      ({ domain }) => host === domain || host.endsWith(`.${domain}`),
    ) || null
  );
}

function isSupportedPlatform(url) {
  return platformOfUrl(url) !== null;
}

function platformName(url) {
  const hit = platformOfUrl(url);
  return hit ? hit.name : null;
}

// On a supported platform the button attaches to a <video> no
// matter what its src is — blob:, empty, or anything else —
// because the src is never the downloadable file there. Off a
// platform this is always false: the button stays gated on a
// direct downloadable source, exactly as before.
function shouldShowPlatformButton(pageUrl) {
  return isSupportedPlatform(pageUrl);
}

// Cache key: the page URL without its fragment (a #t=… jump is
// the same video). The query stays — on YouTube the video id
// lives in ?v=, so dropping it would mix two videos' qualities.
function normalizePageUrl(url) {
  try {
    const u = new URL(url);
    u.hash = "";
    return u.toString();
  } catch {
    return String(url || "");
  }
}

// Resolved format lists live a few minutes in the content
// script's memory (per page, never persisted — the URLs are
// signed and go stale). `now` is injectable so tests can move
// time without waiting out the TTL.
const RESOLVE_CACHE_TTL_MS = 3 * 60 * 1000;

function createResolveCache(ttlMs = RESOLVE_CACHE_TTL_MS, now = () => Date.now()) {
  const entries = new Map();
  const keyOf = (url) => normalizePageUrl(url);
  return {
    ttlMs,
    // The resolved entry ({ resolved, cookies }) or null when
    // missing or older than the TTL.
    get(url) {
      const entry = entries.get(keyOf(url));
      if (!entry) return null;
      if (now() - entry.at > ttlMs) {
        entries.delete(keyOf(url));
        return null;
      }
      return entry.value;
    },
    // The raw entry even when stale, so a quality pick can tell
    // "expired, re-resolve first" apart from "never resolved".
    getEntry(url) {
      return entries.get(keyOf(url)) || null;
    },
    isStale(url) {
      const entry = entries.get(keyOf(url));
      return !entry || now() - entry.at > ttlMs;
    },
    set(url, value) {
      entries.set(keyOf(url), { value, at: now() });
    },
    clear() {
      entries.clear();
    },
    size() {
      return entries.size;
    },
  };
}

// Degraded warning: the server's own text when it has one (it
// names the cause), else the honest default — never invented
// detail, and null when nothing is degraded.
const DEGRADED_WARNING_DEFAULT =
  "YouTube is limiting higher qualities for this video right now";

function degradedWarningText(resolved) {
  if (!resolved || !resolved.degraded) return null;
  const warning = typeof resolved.warning === "string" ? resolved.warning.trim() : "";
  return warning || DEGRADED_WARNING_DEFAULT;
}

// Only 360p and up are offered: a 240p/144p row would be a
// quality nobody picks from this list. A format with no height
// reported is kept — hiding a quality we cannot judge would
// invent a limit the resolver never stated.
function filterResolvableFormats(formats) {
  return (formats || []).filter((f) => {
    if (!f) return false;
    if (typeof f.height === "number") return f.height >= 360;
    return true;
  });
}

// One dropdown row's label: the resolver's label plus the honest
// note for a video-only row (needs an ffmpeg merge in the app)
// or a manifest row (a stream, downloaded by yt-dlp itself).
// Sizes are never shown — nothing here knows one.
function qualityRowLabel(format) {
  const label = (format && format.label) || "Unknown quality";
  if (format && format.fragmented) return `${label} (stream)`;
  if (format && format.needs_merge) return `${label} (video only — merged in app)`;
  return label;
}

// A filename safe to hand to Windows: illegal characters and
// control characters stripped, whitespace collapsed, "video"
// when nothing survives.
function sanitizeTitleFilename(title, ext) {
  const clean = String(title || "")
    .replace(/[\\/:*?"<>|\x00-\x1f]/g, "")
    .replace(/\s+/g, " ")
    .trim();
  const base = clean || "video";
  const suffix = ext ? `.${String(ext).toLowerCase().replace(/^\./, "")}` : "";
  return `${base}${suffix}`;
}

// The single variant sent to the host when the user picks a
// resolved quality — the existing video_found shape plus the
// Round 78 fields (serde-defaulted on the Rust side). Quality
// comes from the height ("1080p"), never from a label guess. A
// fragmented pick carries the manifest URL, or the page URL
// when the resolver gave no manifest URL at all.
function buildResolvedVariant(format, resolved, pageUrl) {
  const ext = String((format && format.ext) || "mp4").toLowerCase();
  const height = format && typeof format.height === "number" ? format.height : null;
  const quality = height ? `${height}p` : (format && format.label) || "Unknown quality";
  const url =
    format && format.fragmented
      ? format.url || pageUrl
      : (format && format.url) || pageUrl;
  return {
    url,
    quality,
    format: ext.toUpperCase(),
    live: !!(resolved && resolved.is_live),
    drm: false,
    resolved: true,
    filename: sanitizeTitleFilename(resolved && resolved.title, ext),
    // Round 81 — headers is a JSON-encoded STRING, not the raw
    // map: the engine's VideoVariantInput takes Option<String>
    // (parsed by headers_from_json), and sending the map made
    // the host refuse the whole video_found message with
    // "invalid type: map, expected a string" — a picked quality
    // silently created no download.
    headers: JSON.stringify((format && format.headers) || {}),
    audio_url: (format && format.audio_url) || null,
    source_url: pageUrl,
    format_id: (format && format.format_id) || null,
    fragmented: !!(format && format.fragmented),
    // Round 84 — the resolved format's size (filesize, falling
    // back to filesize_approx in the host's quality row): the
    // engine stores it on the pending row so the Add URL dialog
    // renders the pre-resolved size and confirm creates the
    // download without probing the stream URL. A format with no
    // size sends null — the download still proceeds.
    filesize:
      format && typeof format.filesize === "number" ? format.filesize : null,
  };
}

// Round 81 — the URL a resolve should run for: a video page,
// never a feed. A TikTok /foryou page HAS videos but is not a
// video page — yt-dlp answers "not a supported video page"
// for it — so the resolve must run for the playing video's own
// page instead. Returns the video-page URL for an anchor href,
// or null when the href is not a video page (a feed, a profile,
// a channel). Relative hrefs resolve against the current page.
function videoPageUrlIfVideoPage(href) {
  let u;
  try {
    u = new URL(href, typeof location !== "undefined" ? location.href : "https://example.invalid");
  } catch {
    return null;
  }
  const host = (u.hostname || "").toLowerCase();
  if (/(^|\.)tiktok\.com$/.test(host)) {
    // Round 83 — the user segment may be empty: a feed video
    // whose author handle was not found builds
    // /@/video/<id>, and that form must still count as a
    // video page (yt-dlp accepts it; see the retest check).
    if (/^\/@[^/]*\/video\/\d+\/?$/.test(u.pathname)) return u.toString();
    return null;
  }
  if (/(^|\.)youtube\.com$/.test(host) || /(^|\.)youtu\.be$/.test(host)) {
    if (u.pathname === "/watch" && u.searchParams.get("v")) return u.toString();
    if (/^\/(shorts|embed)\/[^/]+/.test(u.pathname)) return u.toString();
    return null;
  }
  return null;
}

// Round 83 — TikTok video ids are snowflakes: the high 32
// bits are a unix timestamp in seconds. A 17–20 digit number
// is only a video id when that timestamp falls between
// 2016-01-01 and tomorrow — this rejects the junk ids the
// live feed carries (e.g. 093800000000000001, whose high bits
// decode to 1970). The dump-proven id 7692373771000352030
// decodes to 2026-10-03. BigInt keeps the full 64-bit id
// exact; a plain Number would lose the low bits.
function isTiktokSnowflakeId(id) {
  if (typeof id !== "string" || !/^\d{17,20}$/.test(id)) return false;
  let ts;
  try {
    ts = Number(BigInt(id) >> 32n);
  } catch {
    return false;
  }
  const min = 1451606400; // 2016-01-01T00:00:00Z
  const max = Math.floor(Date.now() / 1000) + 86400; // tomorrow
  return ts >= min && ts <= max;
}

// The video id inside a TikTok /@user/video/<id> URL, or null
// for any other URL (a YouTube watch URL has no such id).
function tiktokVideoIdFromUrl(url) {
  const m = /\/video\/(\d{17,20})(?:[/?#]|$)/.exec(String(url || ""));
  return m ? m[1] : null;
}

// Round 83 — a TikTok video id from ONE element's OWN
// attributes: the xgplayer wrapper id (xgwrapper-<index>-<id>),
// or a data-vid / data-aweme-id / data-video-id attribute.
// Ancestors' own attributes ONLY — never outerHTML text: a
// high ancestor's outerHTML also carries neighbour slides'
// ids (the dump's i=7+ levels prove it), and a descendant
// search would pick the wrong video.
function tiktokIdFromAttributes(el) {
  if (!el) return null;
  const candidates = [];
  const idAttr =
    (typeof el.id === "string" && el.id) ||
    (typeof el.getAttribute === "function" && el.getAttribute("id")) ||
    "";
  const wrapper = /xgwrapper-\d+-(\d{17,20})/.exec(idAttr);
  if (wrapper) candidates.push(wrapper[1]);
  if (typeof el.getAttribute === "function") {
    for (const name of ["data-vid", "data-aweme-id", "data-video-id"]) {
      const v = el.getAttribute(name);
      if (typeof v === "string" && /^\d{17,20}$/.test(v)) candidates.push(v);
    }
  }
  if (el.dataset) {
    for (const key of ["vid", "awemeId", "videoId"]) {
      const v = el.dataset[key];
      if (typeof v === "string" && /^\d{17,20}$/.test(v)) candidates.push(v);
    }
  }
  for (const c of candidates) {
    if (isTiktokSnowflakeId(c)) return c;
  }
  return null;
}

// Round 83 — a TikTok video id from the video element's own
// src / poster URL (an item_id= or video_id= query segment).
// The live feed's playing video has a blob: src, so this
// usually finds nothing — it is a fallback, not the path.
function tiktokIdFromVideoSrc(video) {
  if (!video) return null;
  const urls = [];
  if (typeof video.currentSrc === "string") urls.push(video.currentSrc);
  if (typeof video.getAttribute === "function") {
    urls.push(video.getAttribute("src") || "");
    urls.push(video.getAttribute("poster") || "");
  }
  if (typeof video.poster === "string") urls.push(video.poster);
  if (typeof video.querySelectorAll === "function") {
    try {
      for (const s of video.querySelectorAll("source") || []) {
        if (s && typeof s.getAttribute === "function") {
          urls.push(s.getAttribute("src") || "");
        }
      }
    } catch {
      // A source list that throws is no id source.
    }
  }
  for (const u of urls) {
    const m = /(?:item_id|video_id)=(\d{17,20})/.exec(String(u || ""));
    if (m && isTiktokSnowflakeId(m[1])) return m[1];
  }
  return null;
}

// Round 83 — the author handle as seen at ONE level of the
// slide: the [data-e2e="video-author-uniqueid"] text first
// (authoritative), else the first profile-only anchor
// (/@handle, no /video/ path) at that level. The nearest
// level with a handle wins — a higher level can hold another
// account's link (the dump's i=9 level has /@cookrate.com
// next to the author's /@sjjan474).
function tiktokAuthorHandleAt(el) {
  if (!el) return "";
  if (typeof el.querySelector === "function") {
    try {
      const node = el.querySelector('[data-e2e="video-author-uniqueid"]');
      const text = node && typeof node.textContent === "string" ? node.textContent.trim() : "";
      if (text && /^@?[A-Za-z0-9._]+$/.test(text)) return text.replace(/^@/, "");
    } catch {
      // A selector that throws is no handle source.
    }
  }
  if (typeof el.querySelectorAll === "function") {
    const anchors = el.querySelectorAll("a[href]") || [];
    for (const a of anchors) {
      const href = (a && a.href) || "";
      const m = /^(?:https?:\/\/(?:www\.)?tiktok\.com)?\/@([^/?#]+)\/?$/.exec(href);
      if (m) return m[1];
    }
  }
  return "";
}

// Round 83 — a TikTok feed video's own page from the player
// wrapper id (dump-proven: the live /foryou slide has NO
// /video/ anchor at all; the id lives in the xgplayer wrapper
// div's own id, xgwrapper-0-<id>). Walks up from the video
// taking the NEAREST valid id and the NEAREST handle, then
// builds the canonical URL — with an empty handle when none
// was found. Last resort: the xgwrapper ids among the
// descendants of the nearest slide container (ARTICLE /
// SECTION / [data-e2e="feed-video"]) — accepted only when
// exactly one distinct valid id exists there, so a neighbour
// slide's id can never be picked.
function tiktokVideoPageFromElement(video) {
  if (!video) return null;
  let id = tiktokIdFromVideoSrc(video);
  let handle = "";
  let el = video;
  let slide = null;
  for (let i = 0; i < 12 && el; i++) {
    if (!slide) {
      const tag = (el.tagName || "").toUpperCase();
      const e2e =
        typeof el.getAttribute === "function" ? el.getAttribute("data-e2e") : "";
      if (tag === "ARTICLE" || tag === "SECTION" || e2e === "feed-video") slide = el;
    }
    if (!id) id = tiktokIdFromAttributes(el);
    if (!handle) handle = tiktokAuthorHandleAt(el);
    if (id && handle) break;
    el = el.parentElement || el.parentNode;
  }
  if (!id && slide && typeof slide.querySelectorAll === "function") {
    const ids = new Set();
    try {
      for (const w of slide.querySelectorAll('[id^="xgwrapper-"]') || []) {
        const found = tiktokIdFromAttributes(w);
        if (found) ids.add(found);
      }
    } catch {
      // A selector that throws is no id source.
    }
    if (ids.size === 1) id = [...ids][0];
  }
  if (!id) return null;
  return `https://www.tiktok.com/@${handle}/video/${id}`;
}

// Round 83 — pages that are feeds, not video pages: a resolve
// must never run for one of these. TikTok's /foryou and its
// siblings, YouTube's home / feed pages. A Facebook reel or
// Instagram reel URL is the video page itself, so those hosts
// are never feeds here.
function isFeedPageUrl(href) {
  let u;
  try {
    u = new URL(href, typeof location !== "undefined" ? location.href : "https://example.invalid");
  } catch {
    return false;
  }
  const host = (u.hostname || "").toLowerCase();
  const path = u.pathname || "";
  if (/(^|\.)tiktok\.com$/.test(host)) {
    return (
      path === "/foryou" ||
      path === "/following" ||
      path === "/friends" ||
      path === "/explore" ||
      path === "/live" ||
      path.startsWith("/tag/") ||
      path.startsWith("/music/")
    );
  }
  if (/(^|\.)youtube\.com$/.test(host) || /(^|\.)youtu\.be$/.test(host)) {
    return path === "/" || path.startsWith("/feed/");
  }
  return false;
}

// Round 81 — the playing video's page URL: walk up from the
// video element to the nearest ancestor anchor that points at
// a video page (see above). On a TikTok feed the video sits
// inside the slide's link to its own page; on a YouTube watch
// page there is no such anchor and this returns null, so the
// caller falls back to the page URL. Plain parentElement walk
// (no closest): testable with plain objects in Node.
// Round 82 — the link is not always an ANCESTOR of the
// <video>: on TikTok's feed it can be a sibling overlay inside
// the same slide, so the ancestor walk alone returns null and
// the resolve runs for the feed URL (Round 80+81 retest). The
// second pass walks the same ancestors and, at each one, looks
// for video-page anchors among its descendants — accepting one
// only when it is the SOLE such anchor in that subtree, so a
// neighbour slide's link can never be picked. A subtree with
// none, or with several, is skipped; once a level has several,
// every level above it has at least as many, so the search
// stops. Nodes without querySelectorAll (the plain-object test
// doubles) skip the second pass entirely.
// Round 83 — the Round 82 pass counted ANCHORS, not videos
// (Claude, PROVEN in code): a thumbnail link + a caption link
// to the SAME video counted 2 and returned null. The pass now
// counts DISTINCT video ids (a Set); it accepts when the
// subtree holds exactly one distinct id and rejects when it
// holds two or more. And the live feed has no /video/ anchor
// at all (dump-proven) — the third pass builds the page URL
// from the xgplayer wrapper id instead (see
// tiktokVideoPageFromElement).
function videoPageUrlFromElement(video) {
  if (!video) return null;
  let el = video;
  for (let i = 0; i < 12 && el; i++) {
    if ((el.tagName || "").toUpperCase() === "A" && el.href) {
      const url = videoPageUrlIfVideoPage(el.href);
      if (url) return url;
    }
    // parentNode as well as parentElement: the test harness
    // (and some shadow hosts) only set parentNode.
    el = el.parentElement || el.parentNode;
  }
  el = video;
  for (let i = 0; i < 12 && el; i++) {
    if (typeof el.querySelectorAll === "function") {
      const ids = new Set();
      let firstUrl = null;
      const anchors = el.querySelectorAll("a[href]") || [];
      for (const a of anchors) {
        const url = videoPageUrlIfVideoPage((a && a.href) || "");
        if (url) {
          // Distinct videos, not anchors: two links to the
          // same video (thumbnail + caption) share one id.
          // A non-TikTok video page has no /video/ id, so
          // the URL itself is the key there.
          ids.add(tiktokVideoIdFromUrl(url) || url);
          if (!firstUrl) firstUrl = url;
          if (ids.size > 1) return null;
        }
      }
      if (ids.size === 1) return firstUrl;
    }
    el = el.parentElement || el.parentNode;
  }
  return tiktokVideoPageFromElement(video);
}

// Shared with the content script (same JS world when injected as
// files in order) and with the Node tests. In a classic script
// (the browser) the functions above are already globals; for the
// Node ESM test, which imports this file for its side effects, the
// same API is also attached to globalThis.
if (typeof globalThis !== "undefined") {
  globalThis.FetchlineVideoDetect = {
    KNOWN_MEDIA_EXTENSIONS,
    UNKNOWN_QUALITY,
    extensionOfUrl,
    isMediaUrl,
    formatOfUrl,
    qualityFromDimensions,
    qualityFromLabel,
    qualityFor,
    dedupeVariants,
    extractMediaFromHtml,
    scanLiveMedia,
    parseM3u8,
    playlistNote,
    SUPPORTED_PLATFORM_DOMAINS,
    platformOfUrl,
    isSupportedPlatform,
    platformName,
    shouldShowPlatformButton,
    normalizePageUrl,
    RESOLVE_CACHE_TTL_MS,
    createResolveCache,
    DEGRADED_WARNING_DEFAULT,
    degradedWarningText,
    filterResolvableFormats,
    qualityRowLabel,
    sanitizeTitleFilename,
    buildResolvedVariant,
    videoPageUrlIfVideoPage,
    videoPageUrlFromElement,
    isTiktokSnowflakeId,
    tiktokVideoIdFromUrl,
    tiktokVideoPageFromElement,
    isFeedPageUrl,
  };
}
if (typeof module !== "undefined" && module.exports) {
  module.exports = globalThis.FetchlineVideoDetect;
}
