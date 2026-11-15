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
  };
}
if (typeof module !== "undefined" && module.exports) {
  module.exports = globalThis.FetchlineVideoDetect;
}
