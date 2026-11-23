// Relay fixture-server tests (Round 88). Run with:
//   node --test extension/tests/relay-fixture.test.js
// from app/. Boots the deterministic local fixture server
// (scripts/e2e/fixture-server.mjs) and asserts the relay
// routes respond correctly: the inline <video> MP4 returns a
// 206 with a full Content-Range total on the first fetch, the
// HLS master + variant playlists link their segments, the
// cookie-gated stream is 403 without the session cookie and
// 200 with it, and the attachment route stays watcher-only
// (Content-Disposition: attachment, no relay duplicate — the
// relay classifier skips it; see relay.test.js).
import test from "node:test";
import assert from "node:assert/strict";
import {
  startFixtureServer,
  RELAY_CLIP_SIZE,
  RELAY_VIDEO_PATH,
  RELAY_CLIP_PATH,
  RELAY_HLS_MASTER_PATH,
  RELAY_HLS_VARIANT_PATH,
  RELAY_GATED_PATH,
  RELAY_GATED_COOKIE,
} from "../../../scripts/e2e/fixture-server.mjs";

test("relay fixture routes respond correctly", async () => {
  const { server, baseUrl } = await startFixtureServer();
  try {
    // (1) Inline <video> MP4: the player's first fetch
    // (Range: bytes=0-) gets a 206 with a full total — the
    // size the relay parses as content_length.
    const first = await fetch(`${baseUrl}${RELAY_CLIP_PATH}`, {
      headers: { Range: "bytes=0-" },
    });
    assert.equal(first.status, 206);
    assert.equal(first.headers.get("content-range"), `bytes 0-${RELAY_CLIP_SIZE - 1}/${RELAY_CLIP_SIZE}`);
    assert.equal(first.headers.get("content-length"), String(RELAY_CLIP_SIZE));
    assert.equal(first.headers.get("content-type"), "video/mp4");

    // The same clip without a Range is a plain 200.
    const whole = await fetch(`${baseUrl}${RELAY_CLIP_PATH}`);
    assert.equal(whole.status, 200);
    assert.equal(whole.headers.get("content-length"), String(RELAY_CLIP_SIZE));

    // The page that plays it.
    const page = await fetch(`${baseUrl}${RELAY_VIDEO_PATH}`);
    assert.equal(page.status, 200);
    assert.match(await page.text(), /<video[^>]*src="\/relay-clip\.mp4"/);

    // (2) HLS: master links the variant, the variant lists
    // its segments and ends; segments are video/mp2t.
    const master = await fetch(`${baseUrl}${RELAY_HLS_MASTER_PATH}`);
    assert.equal(master.status, 200);
    assert.equal(master.headers.get("content-type"), "application/vnd.apple.mpegurl");
    assert.match(await master.text(), /variant\.m3u8/);
    const variant = await fetch(`${baseUrl}${RELAY_HLS_VARIANT_PATH}`);
    assert.equal(variant.status, 200);
    const variantText = await variant.text();
    assert.match(variantText, /seg0\.ts/);
    assert.match(variantText, /seg1\.ts/);
    assert.match(variantText, /#EXT-X-ENDLIST/);
    const seg = await fetch(`${baseUrl}/relay-hls/seg0.ts`);
    assert.equal(seg.status, 200);
    assert.equal(seg.headers.get("content-type"), "video/mp2t");

    // (3) Cookie-gated stream: 403 without the session
    // cookie, 200 with it.
    const denied = await fetch(`${baseUrl}${RELAY_GATED_PATH}`);
    assert.equal(denied.status, 403);
    const allowed = await fetch(`${baseUrl}${RELAY_GATED_PATH}`, {
      headers: { Cookie: RELAY_GATED_COOKIE },
    });
    assert.equal(allowed.status, 200);
    assert.equal(allowed.headers.get("content-type"), "video/mp4");

    // (4) The attachment route stays watcher-only: it still
    // serves Content-Disposition: attachment (the relay
    // classifier skips it, so there is no relay duplicate).
    const zip = await fetch(`${baseUrl}/small.zip`);
    assert.equal(zip.status, 200);
    assert.match(zip.headers.get("content-disposition") || "", /attachment/);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});
