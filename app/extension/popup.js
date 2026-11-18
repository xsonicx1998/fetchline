// Sonic Manager extension popup (Round 68, popup v2).
//
// Deliberately simple: the connection state, the developer, and
// a link to his Facebook page — nothing else. There are no
// switches or buttons here any more: catching is governed by the
// desktop app's browser-integration setting, per-site opt-outs
// live in the desktop Settings, and videos are found from the
// right-click menu ("Find videos on this page") or the video
// panel itself. The background worker still owns the catch
// toggle / excluded-sites storage and reads it on every
// download; this popup only reports whether the desktop app
// answers.

const HOST_NAME = "com.fetchline.host";

const $ = (id) => document.getElementById(id);
const statusEl = $("status");
const statusTextEl = $("statusText");
const statusDetailEl = $("statusDetail");

function setStatus(state, text, detail) {
  statusTextEl.textContent = text;
  statusEl.className = `status status-${state}`;
  // Round 69 — show the host's own words when the ping fails:
  // Chrome's `lastError.message` (e.g. "Specified native
  // messaging host not found.") says WHY, so a future failure is
  // diagnosable instead of a bare "Not connected". Guarded: the
  // line is optional (older popup HTML, test fakes), and a missing
  // detail element must never break the status pill itself.
  if (statusDetailEl) statusDetailEl.textContent = detail || "";
}

function sendToHost(message) {
  return new Promise((resolve, reject) => {
    let settled = false;
    // Round 69 — same 15 s bound as the background worker: a
    // host that never answers must fail instead of leaving the
    // popup on "Checking connection…" forever.
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

async function refreshStatus() {
  try {
    const reply = await sendToHost({ type: "ping" });
    if (reply && reply.ok !== false) {
      setStatus("ok", "Connected");
    } else {
      setStatus(
        "bad",
        "Not connected",
        (reply && reply.error) || "Sonic Manager answered, but without an ok reply.",
      );
    }
  } catch (err) {
    setStatus("bad", "Not connected", err && err.message ? err.message : String(err));
  }
}

document.addEventListener("DOMContentLoaded", () => {
  void refreshStatus();
  void refreshScanLine();
});

// Round 69 — show the last video-scan outcome, so "Find videos on
// this page" doing nothing is diagnosable: the content script
// reports its summary (or the menu handler records the injection
// failure) in `lastVideoScan`, and the popup is the only place the
// user can see it.
async function refreshScanLine() {
  const el = $("scanLine");
  if (!el) return;
  try {
    const stored = await chrome.storage.local.get("lastVideoScan");
    const scan = stored && stored.lastVideoScan;
    if (!scan) return;
    if (scan.error) {
      el.textContent = `Video scan failed: ${scan.error}`;
      return;
    }
    const total = typeof scan.total === "number" ? scan.total : 0;
    const downloadable = typeof scan.downloadable === "number" ? scan.downloadable : 0;
    if (total === 0) {
      el.textContent = "Last scan: no videos found on this page.";
    } else if (downloadable === 0) {
      el.textContent = `Last scan: ${total} video${total === 1 ? "" : "s"} found, none downloadable.`;
    } else {
      el.textContent = `Last scan: ${downloadable} downloadable video${downloadable === 1 ? "" : "s"} found — hover a video to download it.`;
    }
  } catch {
    // Storage unreadable: the line simply stays hidden.
  }
}
