// Sonic Manager extension popup (Round 68, popup v2).
//
// Deliberately simple: the connection state, the developer, and
// a link to his Facebook page — nothing else. There are no
// switches or buttons here any more: catching is governed by the
// desktop app's browser-integration setting, per-site opt-outs
// live in the desktop Settings, and videos are found
// automatically on every page (Round 72) or from the right-click
// menu ("Find videos on this page"). The background worker still
// owns the catch toggle / excluded-sites storage and reads it on
// every download; this popup only reports whether the desktop app
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
      // Round 75 — the host is standalone and answers even with
      // the app closed, so a ping alone never meant "Connected".
      // The ping reply carries app_alive (the named mutex the
      // host probes): Connected ONLY when the app itself is
      // alive. app_alive is absent off Windows, where the old
      // behaviour stands — a host answer means Connected.
      if (reply.app_alive === false) {
        setStatus("bad", "Sonic Manager is closed - open it to catch downloads");
      } else {
        setStatus("ok", "Connected");
      }
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
//
// Round 72 (A) — trigger a FRESH scan on popup open and show ITS
// result. The old code only read the stored `lastVideoScan`, which
// belonged to an earlier page, and presented it as this page's —
// "1 downloadable video found" on a page with no video (the Round
// 71 retest). The stored scan is only a fallback, and it is
// labelled with the page it was for, so a stale result can never
// pose as the current page. A fresh-scan failure never replaces
// the stored line: it only shows when there is nothing stored.
async function refreshScanLine() {
  const el = $("scanLine");
  if (!el) return;
  let currentUrl = null;
  try {
    const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
    currentUrl = tabs && tabs[0] && tabs[0].url ? tabs[0].url : null;
  } catch {
    // No tabs access in this context (or a test fake without it):
    // the stored scan below still renders, just without the
    // fresh/stale distinction.
  }
  try {
    const reply = await chrome.runtime.sendMessage({ type: "scan_page_for_videos" });
    if (reply && reply.ok && typeof reply.total === "number" && typeof reply.downloadable === "number") {
      // The rescan answered with this page's summary — show it
      // directly, no storage round-trip.
      renderScan(el, reply.total, reply.downloadable, "This page");
      return;
    }
    if (reply && reply.ok) {
      // The scan ran through the injection fallback, which reports
      // to storage a moment later — give it a beat, then read it.
      await new Promise((r) => setTimeout(r, 300));
    } else if (reply && reply.error) {
      // A fresh failure with nothing stored shows below; with a
      // stored scan, the stored line wins (see the fallback).
      const stored = await chrome.storage.local.get("lastVideoScan");
      if (!stored || !stored.lastVideoScan) {
        el.textContent = `Video scan failed: ${reply.error}`;
        return;
      }
    }
  } catch {
    // No content script / background unreachable: fall through to
    // the stored scan below.
  }
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
    const fresh = currentUrl && scan.pageUrl && scan.pageUrl === currentUrl;
    const prefix = fresh
      ? "This page"
      : scan.pageUrl
        ? `Last scan (${hostOf(scan.pageUrl)})`
        : "Last scan";
    renderScan(el, total, downloadable, prefix);
  } catch {
    // Storage unreadable: the line simply stays hidden.
  }
}

// `prefix` is "This page" for a fresh scan of the open tab, or
// "Last scan (<host>)" for a stored scan from another page.
function renderScan(el, total, downloadable, prefix) {
  if (total === 0) {
    el.textContent = `${prefix}: no videos found.`;
  } else if (downloadable === 0) {
    el.textContent = `${prefix}: ${total} video${total === 1 ? "" : "s"} found, none downloadable.`;
  } else {
    el.textContent = `${prefix}: ${downloadable} downloadable video${downloadable === 1 ? "" : "s"} found — hover a video to download it.`;
  }
}

function hostOf(url) {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}
