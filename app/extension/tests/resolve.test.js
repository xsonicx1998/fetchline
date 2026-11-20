// Round 78 (yt-dlp) pure-helper tests. Run with:
//   node --test extension/tests/resolve.test.js
// from app/. No browser is needed: video-detect.js is imported
// for its side effects (it attaches FetchlineVideoDetect to
// globalThis, like video-detect.test.js), and the assertions
// cover the pure Round 78 logic — supported platforms, the
// resolve cache, degraded warnings, quality-row labels, the
// 360p-and-up filter, and the video_found variant builder.
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import "../video-detect.js";

const {
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
} = globalThis.FetchlineVideoDetect;

const RESOLVED = {
  type: "resolved",
  ok: true,
  request_id: "r1",
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
};

test("isSupportedPlatform: all six domains, with www/mobile/m subdomains", () => {
  for (const url of [
    "https://www.youtube.com/watch?v=abc",
    "https://youtube.com/watch?v=abc",
    "https://m.youtube.com/watch?v=abc",
    "https://youtu.be/abc",
    "https://www.youtu.be/abc",
    "https://www.facebook.com/watch/?v=1",
    "https://facebook.com/mohand.kandil/",
    "https://m.facebook.com/watch/?v=1",
    "https://fb.watch/abc123/",
    "https://www.instagram.com/reel/abc/",
    "https://instagram.com/p/abc/",
    "https://www.tiktok.com/@user/video/123",
    "https://m.tiktok.com/@user/video/123",
    "https://tiktok.com/@user/video/123",
  ]) {
    assert.equal(isSupportedPlatform(url), true, url);
  }
  assert.equal(platformName("https://www.youtube.com/watch?v=abc"), "YouTube");
  assert.equal(platformName("https://fb.watch/abc"), "Facebook");
  assert.equal(platformName("https://www.instagram.com/reel/abc/"), "Instagram");
  assert.equal(platformName("https://www.tiktok.com/@u/video/1"), "TikTok");
});

test("isSupportedPlatform: negatives — lookalikes, other sites, bad URLs", () => {
  for (const url of [
    "https://notyoutube.com/watch?v=abc",
    "https://fakeyoutube.com/",
    "https://youtube.com.evil.example.com/",
    "https://example.com/video.mp4",
    "https://vimeo.com/123",
    "https://dailymotion.com/video/x1",
    "https://twitter.com/user/status/1",
    "http://127.0.0.1/page.html",
    "not a url",
    "",
  ]) {
    assert.equal(isSupportedPlatform(url), false, url);
  }
  assert.equal(platformName("https://example.com/"), null);
  assert.equal(shouldShowPlatformButton("https://www.youtube.com/watch?v=abc"), true);
  assert.equal(shouldShowPlatformButton("https://example.com/page.html"), false);
});

test("normalizePageUrl drops the fragment but keeps the query (the video id lives there)", () => {
  assert.equal(
    normalizePageUrl("https://www.youtube.com/watch?v=abc123#t=42s"),
    "https://www.youtube.com/watch?v=abc123",
  );
  assert.notEqual(
    normalizePageUrl("https://www.youtube.com/watch?v=abc123"),
    normalizePageUrl("https://www.youtube.com/watch?v=xyz999"),
    // Different ?v= is a different video — the keys must differ.
  );
  assert.equal(normalizePageUrl("not a url"), "not a url");
});

test("resolve cache: set/get, expiry at the TTL, clear, and SPA key separation", () => {
  // A few minutes, not seconds: the TTL is a "few minutes" value.
  assert.ok(RESOLVE_CACHE_TTL_MS >= 60 * 1000 && RESOLVE_CACHE_TTL_MS <= 10 * 60 * 1000);
  let now = 1_000_000;
  const cache = createResolveCache(RESOLVE_CACHE_TTL_MS, () => now);
  const a = "https://www.youtube.com/watch?v=aaa";
  const b = "https://www.youtube.com/watch?v=bbb";
  assert.equal(cache.get(a), null);
  assert.equal(cache.isStale(a), true);
  cache.set(a, { resolved: RESOLVED, cookies: [] });
  assert.equal(cache.get(a).resolved, RESOLVED);
  assert.equal(cache.isStale(a), false);
  assert.equal(cache.get(b), null, "another video never sees this video's qualities");
  assert.equal(cache.size(), 1);
  // A fragment jump is the same video: same cache entry.
  assert.equal(cache.get("https://www.youtube.com/watch?v=aaa#t=10s").resolved, RESOLVED);
  now += RESOLVE_CACHE_TTL_MS;
  assert.equal(cache.isStale(a), false, "exactly at the TTL it is still fresh");
  now += 1;
  assert.equal(cache.get(a), null, "past the TTL the entry is gone");
  assert.equal(cache.isStale(a), true);
  cache.set(a, { resolved: RESOLVED, cookies: [] });
  cache.set(b, { resolved: RESOLVED, cookies: [] });
  assert.equal(cache.size(), 2);
  cache.clear();
  assert.equal(cache.size(), 0);
  assert.equal(cache.get(a), null);
});

test("degraded warning: server text wins, default fallback, null when not degraded", () => {
  assert.equal(
    degradedWarningText({ degraded: true, warning: "Higher qualities are unavailable for this video." }),
    "Higher qualities are unavailable for this video.",
  );
  assert.equal(degradedWarningText({ degraded: true, warning: "" }), DEGRADED_WARNING_DEFAULT);
  assert.equal(degradedWarningText({ degraded: true, warning: null }), DEGRADED_WARNING_DEFAULT);
  assert.equal(degradedWarningText({ degraded: true }), DEGRADED_WARNING_DEFAULT);
  assert.match(DEGRADED_WARNING_DEFAULT, /limiting higher qualities/);
  assert.equal(degradedWarningText({ degraded: false, warning: "x" }), null);
  assert.equal(degradedWarningText({ degraded: false }), null);
  assert.equal(degradedWarningText(null), null);
});

test("quality-row labels: merge and stream notes, no sizes invented", () => {
  assert.equal(qualityRowLabel(RESOLVED.formats[0]), "1080p HD (video only — merged in app)");
  assert.equal(qualityRowLabel(RESOLVED.formats[1]), "720p HD (video only — merged in app)");
  assert.equal(qualityRowLabel(RESOLVED.formats[2]), "360p");
  assert.equal(qualityRowLabel(RESOLVED.formats[3]), "720p (stream)");
  assert.equal(qualityRowLabel({ label: "4K" }), "4K");
  assert.equal(qualityRowLabel({}), "Unknown quality");
  for (const f of RESOLVED.formats) {
    assert.ok(!/\d+(\.\d+)?\s*(KB|MB|GB)/i.test(qualityRowLabel(f)), `no size in: ${qualityRowLabel(f)}`);
  }
});

test("filterResolvableFormats keeps 360p and up, drops 144p, keeps unknown heights", () => {
  const kept = filterResolvableFormats(RESOLVED.formats);
  assert.deepEqual(
    kept.map((f) => f.format_id),
    ["137", "136", "18", "hls"],
  );
  assert.deepEqual(filterResolvableFormats([{ format_id: "x", height: 359 }]), []);
  assert.deepEqual(filterResolvableFormats([{ format_id: "x", height: 360 }]), [{ format_id: "x", height: 360 }]);
  assert.deepEqual(
    filterResolvableFormats([{ format_id: "x" }]).map((f) => f.format_id),
    ["x"],
    "no height reported: shown, not hidden",
  );
  assert.deepEqual(filterResolvableFormats(null), []);
  assert.deepEqual(filterResolvableFormats([]), []);
});

test("sanitizeTitleFilename strips Windows-illegal characters", () => {
  assert.equal(sanitizeTitleFilename("Big Buck Bunny", "mp4"), "Big Buck Bunny.mp4");
  assert.equal(sanitizeTitleFilename('A/B:C*D?E"F<G>H|I', "mp4"), "ABCDEFGHI.mp4");
  assert.equal(sanitizeTitleFilename("  spaced   out  ", "webm"), "spaced out.webm");
  assert.equal(sanitizeTitleFilename("", "mp4"), "video.mp4");
  assert.equal(sanitizeTitleFilename(null, "mp4"), "video.mp4");
  assert.equal(sanitizeTitleFilename("No ext", ""), "No ext");
});

test("buildResolvedVariant: the video_found variant for a picked quality", () => {
  const page = "https://www.youtube.com/watch?v=abc";
  const v = buildResolvedVariant(RESOLVED.formats[0], RESOLVED, page);
  assert.equal(v.url, "https://cdn.example.com/v1080.mp4");
  assert.equal(v.quality, "1080p");
  assert.equal(v.format, "MP4");
  assert.equal(v.live, false);
  assert.equal(v.drm, false);
  assert.equal(v.resolved, true);
  assert.equal(v.filename, "Big Buck Bunny.mp4");
  assert.equal(v.audio_url, "https://cdn.example.com/a140.m4a");
  assert.equal(v.source_url, page);
  assert.equal(v.format_id, "137");
  assert.equal(v.fragmented, false);
  // Round 81 — headers is a JSON-encoded STRING, not the raw
  // map: the engine's VideoVariantInput takes Option<String>,
  // and the raw map made the host refuse the whole pick with
  // "invalid type: map, expected a string".
  assert.equal(typeof v.headers, "string");
  assert.deepEqual(JSON.parse(v.headers), { "User-Agent": "x" });

  // A combined format has no audio_url and needs no merge note.
  const combined = buildResolvedVariant(RESOLVED.formats[2], RESOLVED, page);
  assert.equal(combined.quality, "360p");
  assert.equal(combined.audio_url, null);

  // A fragmented pick carries the manifest URL and fragmented:true.
  const stream = buildResolvedVariant(RESOLVED.formats[3], RESOLVED, page);
  assert.equal(stream.url, "https://cdn.example.com/master.m3u8");
  assert.equal(stream.fragmented, true);

  // A fragmented format with no manifest URL falls back to the page URL.
  const noManifest = buildResolvedVariant({ ...RESOLVED.formats[3], url: null }, RESOLVED, page);
  assert.equal(noManifest.url, page);
  assert.equal(noManifest.fragmented, true);

  // A live resolve marks the variant live; an illegal title is sanitized.
  const live = buildResolvedVariant(RESOLVED.formats[0], { ...RESOLVED, is_live: true, title: "Live: show?" }, page);
  assert.equal(live.live, true);
  assert.equal(live.filename, "Live show.mp4");
});

test("videoPageUrlIfVideoPage: only real video pages, never feeds or profiles", () => {
  assert.equal(
    videoPageUrlIfVideoPage("https://www.tiktok.com/@user/video/7551233954129399053"),
    "https://www.tiktok.com/@user/video/7551233954129399053",
  );
  assert.equal(videoPageUrlIfVideoPage("https://www.tiktok.com/foryou?lang=en"), null);
  assert.equal(videoPageUrlIfVideoPage("https://www.tiktok.com/@user"), null);
  assert.equal(videoPageUrlIfVideoPage("https://www.tiktok.com/"), null);
  assert.equal(
    videoPageUrlIfVideoPage("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
    "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
  );
  assert.equal(
    videoPageUrlIfVideoPage("https://www.youtube.com/shorts/abc123"),
    "https://www.youtube.com/shorts/abc123",
  );
  assert.equal(videoPageUrlIfVideoPage("https://www.youtube.com/feed/trending"), null);
  assert.equal(videoPageUrlIfVideoPage("https://example.com/video/123"), null);
});

test("videoPageUrlFromElement: a feed video resolves to its own page, a watch page to null", () => {
  // A TikTok feed slide: the video sits inside the slide's link
  // to its own video page. Plain objects stand in for DOM nodes
  // (the walk uses tagName / href / parentElement only).
  const anchor = {
    tagName: "A",
    href: "https://www.tiktok.com/@someone/video/7551233954129399053",
    parentElement: null,
  };
  const slide = { tagName: "DIV", parentElement: anchor };
  const video = { tagName: "VIDEO", parentElement: slide };
  assert.equal(
    videoPageUrlFromElement(video),
    "https://www.tiktok.com/@someone/video/7551233954129399053",
  );

  // A feed link that is NOT a video page (profile) is skipped,
  // and with no video anchor at all the answer is null — the
  // caller falls back to the page URL.
  const profileAnchor = {
    tagName: "A",
    href: "https://www.tiktok.com/@someone",
    parentElement: null,
  };
  assert.equal(
    videoPageUrlFromElement({ tagName: "VIDEO", parentElement: profileAnchor }),
    null,
  );
  assert.equal(videoPageUrlFromElement({ tagName: "VIDEO", parentElement: null }), null);
  assert.equal(videoPageUrlFromElement(null), null);
});

test("videoPageUrlFromElement: a sibling overlay link inside the same slide is found, a neighbour slide's link is not", () => {
  // Round 82 — on TikTok's feed the link to the video's own
  // page is not always an ANCESTOR of the <video>: it can be a
  // sibling overlay inside the same slide. The doubles below
  // implement querySelectorAll like a real element (returning
  // the anchors in that subtree).
  const videoHref = "https://www.tiktok.com/@someone/video/7551233954129399053";
  const slide = {
    tagName: "DIV",
    parentElement: null,
    querySelectorAll: () => [{ href: videoHref }],
  };
  const video = { tagName: "VIDEO", parentElement: slide };
  assert.equal(videoPageUrlFromElement(video), videoHref);

  // Two video-page anchors in one subtree (this slide's link
  // plus a neighbour's) must NOT resolve to either one — the
  // caller falls back to the page URL instead of risking the
  // wrong video.
  const feed = {
    tagName: "DIV",
    parentElement: null,
    querySelectorAll: () => [
      { href: videoHref },
      { href: "https://www.tiktok.com/@other/video/7551233954129399000" },
    ],
  };
  assert.equal(
    videoPageUrlFromElement({ tagName: "VIDEO", parentElement: feed }),
    null,
  );

  // A subtree whose only anchor is a profile link is not a
  // video page either.
  const profileSlide = {
    tagName: "DIV",
    parentElement: null,
    querySelectorAll: () => [{ href: "https://www.tiktok.com/@someone" }],
  };
  assert.equal(
    videoPageUrlFromElement({ tagName: "VIDEO", parentElement: profileSlide }),
    null,
  );
});

// Round 83 — the real /foryou dump (see
// fixtures/tiktok-foryou-dump.json, reconstructed from the VPS
// capture report 2026-11-20 ~21:00 EET). Build plain-object
// doubles from the fixture chain: each level gets tagName,
// parentElement (the next level), its own id attribute, a
// getAttribute for the recorded attrs, and querySelectorAll
// returning the level's recorded descendant anchors (and, for
// the '[id^="xgwrapper-"]' selector, the wrapper elements in
// its subtree). The fixture's `ids` (outerHTML regex matches)
// are NOT used to build the doubles — the code must never read
// outerHTML text, which also carries neighbour slides' ids.
function elementFromDumpChain(chain) {
  const levels = chain.map((level) => {
    const el = {
      tagName: level.tag,
      parentElement: null,
      id: level.attrs.id || "",
      getAttribute(name) {
        return Object.prototype.hasOwnProperty.call(level.attrs, name)
          ? level.attrs[name]
          : null;
      },
      querySelector() {
        return null;
      },
      querySelectorAll(selector) {
        if (String(selector || "").includes("xgwrapper")) {
          // Wrapper elements among this level's descendants:
          // the chain levels below it whose own id starts
          // with xgwrapper-.
          const idx = chain.indexOf(level);
          return chain
            .slice(0, idx)
            .filter((l) => (l.attrs.id || "").startsWith("xgwrapper-"))
            .map((l) => ({ id: l.attrs.id }));
        }
        return (level.anchors || []).map((href) => ({ href }));
      },
    };
    return el;
  });
  for (let i = 0; i < levels.length - 1; i++) levels[i].parentElement = levels[i + 1];
  return levels[0];
}

test("videoPageUrlFromElement: the real /foryou dump resolves to the playing video's own page", () => {
  const dump = JSON.parse(
    readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), "fixtures", "tiktok-foryou-dump.json"),
      "utf8",
    ),
  );
  assert.equal(dump.href, "https://www.tiktok.com/foryou");
  assert.equal(dump.shadow, false);
  assert.equal(dump.paused, false);
  assert.equal(dump.videos, 2);
  // The dump has NO /video/ anchor anywhere in the chain — the
  // Round 82 sole-anchor pass alone would return null, and the
  // feed URL must never be the answer.
  assert.equal(
    dump.chain.every((l) => !(l.anchors || []).some((h) => h.includes("/video/"))),
    true,
  );
  const video = elementFromDumpChain(dump.chain);
  assert.equal(
    videoPageUrlFromElement(video),
    "https://www.tiktok.com/@sjjan474/video/7692373771000352030",
  );
});

test("videoPageUrlFromElement: two anchors to the SAME video resolve (distinct ids, not anchors)", () => {
  // Round 83 — Claude's PROVEN bug: the Round 82 pass counted
  // anchors, so a thumbnail link + a caption link to the same
  // video counted 2 and returned null.
  const videoHref = "https://www.tiktok.com/@someone/video/7551233954129399053";
  const slide = {
    tagName: "DIV",
    parentElement: null,
    querySelectorAll: () => [
      { href: videoHref },
      { href: "https://www.tiktok.com/@someone/video/7551233954129399053?lang=en" },
    ],
  };
  assert.equal(
    videoPageUrlFromElement({ tagName: "VIDEO", parentElement: slide }),
    videoHref,
  );
  // Same video id under a different handle is still one video.
  const slide2 = {
    tagName: "DIV",
    parentElement: null,
    querySelectorAll: () => [
      { href: videoHref },
      { href: "https://www.tiktok.com/@other/video/7551233954129399053" },
    ],
  };
  assert.equal(
    videoPageUrlFromElement({ tagName: "VIDEO", parentElement: slide2 }),
    videoHref,
  );
});

test("isTiktokSnowflakeId: the dump's real id passes, its junk ids fail", () => {
  assert.equal(isTiktokSnowflakeId("7692373771000352030"), true);
  // 093800000000000001 decodes to 1970 — not a video id.
  assert.equal(isTiktokSnowflakeId("093800000000000001"), false);
  assert.equal(isTiktokSnowflakeId("123"), false);
  assert.equal(isTiktokSnowflakeId("not-a-number"), false);
  assert.equal(isTiktokSnowflakeId(""), false);
  assert.equal(isTiktokSnowflakeId(null), false);
});

test("videoPageUrlFromElement: a wrapper id that is not a snowflake resolves to nothing", () => {
  const wrapper = {
    tagName: "DIV",
    id: "xgwrapper-0-093800000000000001",
    parentElement: null,
  };
  const video = { tagName: "VIDEO", parentElement: wrapper };
  assert.equal(videoPageUrlFromElement(video), null);
});

test("videoPageUrlFromElement: a wrapper id with no author handle builds the empty-handle URL", () => {
  const wrapper = {
    tagName: "DIV",
    id: "xgwrapper-0-7692373771000352030",
    parentElement: null,
    querySelectorAll: () => [],
  };
  const video = { tagName: "VIDEO", parentElement: wrapper };
  assert.equal(
    videoPageUrlFromElement(video),
    "https://www.tiktok.com/@/video/7692373771000352030",
  );
  assert.equal(
    videoPageUrlIfVideoPage("https://www.tiktok.com/@/video/7692373771000352030"),
    "https://www.tiktok.com/@/video/7692373771000352030",
  );
});

test("videoPageUrlFromElement: the nearest wrapper id wins over a higher one", () => {
  const outer = {
    tagName: "DIV",
    id: "xgwrapper-0-7692374090094611230",
    parentElement: null,
  };
  const inner = {
    tagName: "DIV",
    id: "xgwrapper-0-7692373771000352030",
    parentElement: outer,
  };
  const video = { tagName: "VIDEO", parentElement: inner };
  assert.equal(
    videoPageUrlFromElement(video),
    "https://www.tiktok.com/@/video/7692373771000352030",
  );
});

test("isFeedPageUrl: feeds are feeds, video pages are not", () => {
  assert.equal(isFeedPageUrl("https://www.tiktok.com/foryou"), true);
  assert.equal(isFeedPageUrl("https://www.tiktok.com/following"), true);
  assert.equal(isFeedPageUrl("https://www.tiktok.com/explore"), true);
  assert.equal(isFeedPageUrl("https://www.tiktok.com/@user/video/7551233954129399053"), false);
  assert.equal(isFeedPageUrl("https://www.tiktok.com/@user"), false);
  assert.equal(isFeedPageUrl("https://www.youtube.com/"), true);
  assert.equal(isFeedPageUrl("https://www.youtube.com/feed/trending"), true);
  assert.equal(isFeedPageUrl("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), false);
  assert.equal(isFeedPageUrl("https://www.facebook.com/reel/1869227551096683"), false);
  assert.equal(isFeedPageUrl("https://www.instagram.com/reel/Df2xjtXt0jk/"), false);
});

test("tiktokVideoIdFromUrl: only TikTok video URLs carry an id", () => {
  assert.equal(
    tiktokVideoIdFromUrl("https://www.tiktok.com/@someone/video/7551233954129399053"),
    "7551233954129399053",
  );
  assert.equal(tiktokVideoIdFromUrl("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), null);
  assert.equal(tiktokVideoIdFromUrl("https://www.tiktok.com/foryou"), null);
});
