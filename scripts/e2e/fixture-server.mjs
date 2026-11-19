// Slow fixture server for the Round 54 E2E tests (see e2e-test.mjs).
//
// Serves two 20 MiB files, /slow-a.mp4 and /slow-b.mp4, from a local
// HTTP server on 127.0.0.1 (ephemeral port), throttled to 32 KiB/s per
// connection with real Range support (206 Partial Content) and HEAD.
//
// Round 73 — auto-capture routes added:
// - GET /test-link.html: a small page with four links for the
//   auto-capture E2E check: #dl-attr (download attribute),
//   #dl-attach (Content-Disposition: attachment), #dl-plain
//   (plain control — opens the media player, no capture
//   expected), #dl-zip (small .zip with a download attribute).
// - GET /slow-a-attach.mp4: the same 20 MiB payload as
//   /slow-a.mp4, same Range/HEAD/throttle behavior, with
//   Content-Disposition: attachment; filename="slow-a.mp4".
// - GET /small.zip: a small (4 KiB) fast payload,
//   Content-Type: application/zip, Content-Disposition:
//   attachment; filename="small.zip", no throttle.
//
// Why slow, why big, why Range:
// - The engine downloads in 4 segments (default max_connections); a
//   fast fixture finishes before Pause can land (the BUG-048 timing
//   trap). 20 MiB at 4 x 32 KiB/s lasts ~160 s, so the download is
//   still running through every pause/resume/delete step.
// - Range support matters: without it the engine falls back to a
//   single segment (16 KiB/s worst case, 20 MiB would take ~20 min
//   to finish — the test never needs it to finish, but percent
//   would barely move). With Range, percent moves ~1 point every
//   ~1.6 s and the speed column stays live.
// - The capture test's watcher fixture (512 KiB at ~80 KiB/s, no
//   Range) proves localhost downloads work in the real app; this
//   server is the same idea, stretched out for the GUI steps.

import http from 'node:http';
import crypto from 'node:crypto';

export const FIXTURE_SIZE = 20 * 1024 * 1024; // 20 MiB
export const CHUNK_BYTES = 32 * 1024; // 32 KiB per write
export const CHUNK_DELAY_MS = 1000; // 1 s between writes -> 32 KiB/s per connection
export const FIXTURE_PATHS = ['/slow-a.mp4', '/slow-b.mp4'];
export const ATTACH_PATH = '/slow-a-attach.mp4'; // same payload as /slow-a.mp4 (Round 73)
export const SMALL_ZIP_SIZE = 4 * 1024; // 4 KiB (Round 73)

// Content is irrelevant to the app (it never inspects it); random
// bytes are fine and fast to generate once.
const PAYLOAD = crypto.randomBytes(FIXTURE_SIZE);
// Small zip payload: fixed bytes (deterministic), no throttle.
const SMALL_ZIP = Buffer.alloc(SMALL_ZIP_SIZE, 0x50); // 'P' * 4 KiB

const TEST_LINK_HTML = `<!doctype html>
<html><head><meta charset="utf-8"><title>auto-capture test links</title></head>
<body>
<a id="dl-attr" href="/slow-a.mp4" download="slow-a.mp4">slow-a (download attr)</a>
<a id="dl-attach" href="/slow-a-attach.mp4">slow-a (attachment)</a>
<a id="dl-plain" href="/slow-a.mp4">slow-a (plain)</a>
<a id="dl-zip" href="/small.zip" download>small.zip</a>
</body></html>
`;

function handle(req, res) {
  const url = new URL(req.url ?? '/', 'http://127.0.0.1');
  if (url.pathname === '/test-link.html') {
    res.writeHead(200, { 'Content-Type': 'text/html', 'Content-Length': String(Buffer.byteLength(TEST_LINK_HTML)) });
    if (req.method === 'HEAD') {
      res.end();
      return;
    }
    res.end(TEST_LINK_HTML);
    return;
  }
  if (url.pathname === '/small.zip') {
    const headers = {
      'Content-Type': 'application/zip',
      'Content-Length': String(SMALL_ZIP_SIZE),
      'Content-Disposition': 'attachment; filename="small.zip"',
    };
    res.writeHead(200, headers);
    if (req.method === 'HEAD') {
      res.end();
      return;
    }
    res.end(SMALL_ZIP);
    return;
  }
  const isAttach = url.pathname === ATTACH_PATH;
  if (!FIXTURE_PATHS.includes(url.pathname) && !isAttach) {
    res.writeHead(404, { 'Content-Type': 'text/plain' });
    res.end('not found');
    return;
  }
  // /slow-a-attach.mp4 serves the same payload as /slow-a.mp4 and
  // reports that filename in Content-Disposition.
  const filename = isAttach ? 'slow-a.mp4' : url.pathname.slice(1);

  // Range: bytes=start-end (single range only; the engine asks for
  // one range per segment request).
  let start = 0;
  let end = FIXTURE_SIZE - 1;
  let status = 200;
  const range = req.headers.range;
  if (range) {
    const m = /^bytes=(\d*)-(\d*)$/.exec(range);
    if (m) {
      start = m[1] ? Number.parseInt(m[1], 10) : 0;
      end = m[2] ? Number.parseInt(m[2], 10) : FIXTURE_SIZE - 1;
      if (!Number.isFinite(start) || !Number.isFinite(end) || start >= FIXTURE_SIZE || end < start) {
        res.writeHead(416, { 'Content-Range': `bytes */${FIXTURE_SIZE}` });
        res.end();
        return;
      }
      status = 206;
    }
  }

  const length = end - start + 1;
  const headers = {
    'Content-Type': 'video/mp4',
    'Content-Length': String(length),
    'Accept-Ranges': 'bytes',
    'Content-Disposition': `attachment; filename="${filename}"`,
  };
  if (status === 206) headers['Content-Range'] = `bytes ${start}-${end}/${FIXTURE_SIZE}`;
  res.writeHead(status, headers);
  if (req.method === 'HEAD') {
    res.end();
    return;
  }

  // Stream the slice throttled: one CHUNK_BYTES write per second.
  // A closed connection (pause/cancel/delete) ends the loop silently.
  let offset = start;
  const pump = () => {
    if (offset > end) {
      res.end();
      return;
    }
    const n = Math.min(CHUNK_BYTES, end - offset + 1);
    let ok = true;
    try {
      ok = res.write(PAYLOAD.subarray(offset, offset + n));
    } catch {
      return;
    }
    offset += n;
    if (offset > end) {
      res.end();
      return;
    }
    const next = () => setTimeout(pump, CHUNK_DELAY_MS);
    if (ok) next();
    else res.once('drain', next);
  };
  pump();
}

/** Start the server on an ephemeral localhost port. Resolves with
 *  { server, port, baseUrl }. Callers stop it with server.close(). */
export function startFixtureServer() {
  return new Promise((resolve, reject) => {
    const server = http.createServer(handle);
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const port = server.address().port;
      resolve({ server, port, baseUrl: `http://127.0.0.1:${port}` });
    });
  });
}
