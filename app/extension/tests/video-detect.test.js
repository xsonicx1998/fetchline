// Video detection tests (S08 / F05). Run with:
//   node --test extension/tests/video-detect.test.js
// from app/. No browser is needed: the detector is pure JS over
// HTML strings and a tiny fake DOM, and the fixtures are the same
// files the Rust engine tests assert against (engine/tests/
// video_tests.rs), so the two detectors cannot drift apart.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import "../video-detect.js";

const {
  UNKNOWN_QUALITY,
  isMediaUrl,
  formatOfUrl,
  qualityFor,
  qualityFromLabel,
  dedupeVariants,
  extractMediaFromHtml,
  scanLiveMedia,
  parseM3u8,
  playlistNote,
} = globalThis.FetchlineVideoDetect;

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const fixturesDir = path.join(__dirname, "fixtures");
const fixture = (name) => fs.readFileSync(path.join(fixturesDir, name), "utf8");
const PAGE = "http://127.0.0.1/page.html";

test("quality is never invented", () => {
  assert.equal(qualityFor("http://x/clip.mp4", 0, 0), UNKNOWN_QUALITY);
  assert.equal(qualityFor("http://x/clip-720p.mp4", 0, 0), "720p");
  assert.equal(qualityFor("http://x/clip.mp4", 1280, 720), "1280 × 720");
  // Dimensions win over a label in the URL.
  assert.equal(qualityFor("http://x/clip-720p.mp4", 1920, 1080), "1920 × 1080");
});

test("quality labels are bounded, not substrings", () => {
  assert.equal(qualityFromLabel("clip-720p.mp4"), "720p");
  assert.equal(qualityFromLabel("trailer-4k.webm"), "4K");
  assert.equal(qualityFromLabel("file17209.mp4"), null);
  assert.equal(qualityFromLabel("clip.mp4"), null);
});

test("media URL classification", () => {
  assert.equal(isMediaUrl("https://example.com/clip.mp4"), true);
  assert.equal(isMediaUrl("https://example.com/stream.m3u8"), true);
  assert.equal(isMediaUrl("https://example.com/stream.mpd"), true);
  assert.equal(isMediaUrl("https://example.com/song.mp3"), true);
  assert.equal(isMediaUrl("https://example.com/page.html"), false);
  assert.equal(isMediaUrl("https://example.com/pic.png"), false);
  assert.equal(formatOfUrl("https://example.com/clip.webm"), "WEBM");
  assert.equal(formatOfUrl("https://example.com/stream.mpd"), "MPD");
  assert.equal(formatOfUrl("https://example.com/page.html"), null);
});

test("parseM3u8: master playlist lists real variants", () => {
  const p = parseM3u8(fixture("master.m3u8"));
  assert.equal(p.isMaster, true);
  assert.equal(p.live, false);
  assert.equal(p.encrypted, false);
  assert.deepEqual(
    p.variants.map((v) => [v.uri, v.resolution]),
    [
      ["v720.m3u8", [1280, 720]],
      ["v1080.m3u8", [1920, 1080]],
      ["v-low.m3u8", null],
    ],
  );
  assert.equal(p.variants[0].bandwidth, 800000);
});

test("parseM3u8: VOD counts segments, live has no end, encrypted is flagged", () => {
  const vod = parseM3u8(fixture("vod.m3u8"));
  assert.equal(vod.isMaster, false);
  assert.equal(vod.live, false);
  assert.equal(vod.segments, 3);
  assert.equal(vod.durationSecs, 25);
  const live = parseM3u8(fixture("live.m3u8"));
  assert.equal(live.live, true);
  assert.equal(live.segments, 2);
  const enc = parseM3u8(fixture("encrypted.m3u8"));
  assert.equal(enc.encrypted, true);
  assert.equal(parseM3u8(""), null);
  assert.equal(parseM3u8("<html>not a playlist</html>"), null);
  assert.equal(
    playlistNote("M3U8", false),
    "Playlist file — saved as-is; it is not merged into one video file.",
  );
  assert.ok(playlistNote("MPD", false).includes("DASH manifest"));
  assert.ok(playlistNote("M3U8", true).includes("Live stream"));
});

test("fixture: video with sources and multiple qualities", () => {
  const found = extractMediaFromHtml(fixture("video-sources.html"), PAGE);
  assert.deepEqual(
    found.map((v) => v.url),
    [
      "http://127.0.0.1/clip-720p.mp4",
      "http://127.0.0.1/clip-480p.webm",
      "http://127.0.0.1/clip-1080p.mp4",
    ],
  );
  assert.equal(found[0].quality, "720p");
  assert.equal(found[0].format, "MP4");
  assert.equal(found[2].quality, "1080p");
  assert.equal(found.every((v) => v.drm === false), true);
});

test("fixture: audio only", () => {
  const found = extractMediaFromHtml(fixture("audio-only.html"), PAGE);
  assert.equal(found.length, 1);
  assert.equal(found[0].url, "http://127.0.0.1/song.mp3");
  assert.equal(found[0].format, "MP3");
  assert.equal(found[0].quality, UNKNOWN_QUALITY);
});

test("fixture: no video on the page", () => {
  assert.deepEqual(extractMediaFromHtml(fixture("no-video.html"), PAGE), []);
});

test("fixture: duplicate URLs are deduped", () => {
  const found = extractMediaFromHtml(fixture("duplicates.html"), PAGE);
  assert.deepEqual(
    found.map((v) => v.url),
    ["http://127.0.0.1/clip.mp4"],
  );
});

test("fixture: live stream is marked live", () => {
  const found = extractMediaFromHtml(fixture("live-stream.html"), PAGE);
  assert.equal(found.length, 1);
  assert.equal(found[0].format, "M3U8");
  assert.equal(found[0].live, true);
});

test("fixture: DRM-marked video is reported as drm, not hidden", () => {
  const found = extractMediaFromHtml(fixture("drm-video.html"), PAGE);
  assert.equal(found.length, 1);
  assert.equal(found[0].url, "http://127.0.0.1/protected.mp4");
  assert.equal(found[0].drm, true);
});

test("fixture: HLS master playlist is one playlist item with an honest note", () => {
  const found = extractMediaFromHtml(fixture("hls-master.html"), PAGE);
  assert.equal(found.length, 1);
  assert.equal(found[0].format, "M3U8");
  assert.equal(found[0].live, true);
  assert.ok(found[0].note.includes("not merged"), found[0].note);
});

test("fixture: DASH manifest is offered as-is, labelled a manifest", () => {
  const found = extractMediaFromHtml(fixture("dash-manifest.html"), PAGE);
  assert.equal(found.length, 1);
  assert.equal(found[0].url, "http://127.0.0.1/stream.mpd");
  assert.equal(found[0].format, "MPD");
  assert.equal(found[0].quality, UNKNOWN_QUALITY);
  assert.ok(found[0].note.includes("DASH manifest"), found[0].note);
  assert.ok(found[0].note.includes("not merged"), found[0].note);
});

test("fixture: only low quality available is honestly unknown", () => {
  const found = extractMediaFromHtml(fixture("low-quality.html"), PAGE);
  assert.equal(found.length, 1);
  assert.equal(found[0].url, "http://127.0.0.1/clip.mp4");
  assert.equal(found[0].quality, UNKNOWN_QUALITY);
});

test("dedupe keeps first-seen order", () => {
  const out = dedupeVariants([
    { url: "http://x/a.mp4" },
    { url: "http://x/b.mp4" },
    { url: "http://x/a.mp4" },
  ]);
  assert.deepEqual(
    out.map((v) => v.url),
    ["http://x/a.mp4", "http://x/b.mp4"],
  );
});

// A minimal fake DOM: enough for scanLiveMedia (querySelectorAll on
// the document and on elements, currentSrc, videoWidth/Height,
// duration, mediaKeys, getAttribute).
function fakeElement(tag, attrs, children = []) {
  return {
    tagName: tag.toUpperCase(),
    videoWidth: attrs.videoWidth || 0,
    videoHeight: attrs.videoHeight || 0,
    duration: attrs.duration ?? NaN,
    mediaKeys: attrs.mediaKeys || null,
    currentSrc: attrs.currentSrc || "",
    getAttribute: (name) => attrs[name] ?? null,
    querySelectorAll: (sel) => (sel === "source" ? children : []),
  };
}

test("live DOM scan reads real dimensions, live and DRM state", () => {
  const video = fakeElement(
    "video",
    {
      currentSrc: "http://127.0.0.1/movie.mp4",
      videoWidth: 1920,
      videoHeight: 1080,
      duration: 3600,
    },
    [fakeElement("source", { src: "http://127.0.0.1/movie-720p.mp4" })],
  );
  const live = fakeElement("video", {
    currentSrc: "http://127.0.0.1/stream.m3u8",
    duration: Infinity,
  });
  const drm = fakeElement("video", {
    currentSrc: "http://127.0.0.1/protected.mp4",
    mediaKeys: {},
    duration: 3600,
  });
  const audio = fakeElement("audio", { currentSrc: "http://127.0.0.1/song.mp3" });
  const doc = {
    querySelectorAll: (sel) => {
      if (sel === "video, audio") return [video, live, drm, audio];
      if (sel === "a[href]") return [];
      return [];
    },
  };
  const found = scanLiveMedia(doc, PAGE);
  assert.equal(found.length, 5);
  assert.equal(found[0].quality, "1920 × 1080");
  assert.equal(found[0].live, false);
  assert.equal(found[1].quality, "720p");
  assert.equal(found[2].live, true);
  assert.equal(found[2].format, "M3U8");
  assert.ok(found[2].note.includes("not merged"), found[2].note);
  assert.equal(found[3].drm, true);
  assert.equal(found[4].format, "MP3");
});

test("live DOM scan reports a DASH manifest as-is", () => {
  const dash = fakeElement(
    "video",
    { currentSrc: "" },
    [fakeElement("source", { src: "http://127.0.0.1/stream.mpd" })],
  );
  const doc = {
    querySelectorAll: (sel) => {
      if (sel === "video, audio") return [dash];
      if (sel === "a[href]") return [];
      return [];
    },
  };
  const found = scanLiveMedia(doc, PAGE);
  assert.equal(found.length, 1);
  assert.equal(found[0].format, "MPD");
  assert.ok(found[0].note.includes("DASH manifest"), found[0].note);
});
