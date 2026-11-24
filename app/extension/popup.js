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
  void refreshRelayLine();
  void refreshHandoffLine();
  // Round 88.3 — re-read the desktop settings whenever the
  // popup opens, so an app-side toggle (relay, integration)
  // reaches the background worker's capture gate without
  // waiting for a browser restart: the worker otherwise
  // refreshes only at install / startup, and a same-version
  // reload does not fire onInstalled. Fire-and-forget — the
  // popup shows the connection state, not the settings — and
  // rejection-safe (no background listening yet must never
  // surface as an unhandled rejection).
  void chrome.runtime
    .sendMessage({ type: "refresh_desktop_settings" })
    .catch(() => {});
});

// Round 88.8 — show the last download hand-off, or why the last
// download was NOT caught: the background worker records every
// refusal reason in `lastHandoff`, and this is the only place the
// user can see it. Without it, a Connected popup with a download
// sailing past into Chrome is undiagnosable. A success reads as
// confirmation ("Last download: Sent to Sonic Manager"); a
// refusal names the gate that stopped it.
async function refreshHandoffLine() {
  const el = $("handoffLine");
  if (!el) return;
  try {
    const stored = await chrome.storage.local.get("lastHandoff");
    const handoff = stored && stored.lastHandoff;
    if (!handoff || !handoff.message) return;
    el.textContent = handoff.ok
      ? `Last download: ${handoff.message}`
      : `Last download skipped: ${handoff.message}`;
  } catch {
    // Storage unreadable: the line simply stays hidden.
  }
}

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
      renderScan(el, reply.total, reply.downloadable, "This page", currentUrl);
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
    renderScan(el, total, downloadable, prefix, scan.pageUrl || currentUrl);
  } catch {
    // Storage unreadable: the line simply stays hidden.
  }
}

// `prefix` is "This page" for a fresh scan of the open tab, or
// "Last scan (<host>)" for a stored scan from another page.
// Round 78: on a supported platform (YouTube, Facebook,
// Instagram, TikTok) the direct scan finds nothing — the
// player's <video> src is a blob: — so "no videos found" would
// be a lie; the line instead says to click the Download button
// on the video to see qualities.
function renderScan(el, total, downloadable, prefix, pageUrl) {
  if (total === 0) {
    const name = platformNameFor(pageUrl);
    if (name) {
      el.textContent = `${prefix}: ${name} video — click the Download button on the video to see qualities.`;
      return;
    }
    el.textContent = `${prefix}: no videos found.`;
  } else if (downloadable === 0) {
    el.textContent = `${prefix}: ${total} video${total === 1 ? "" : "s"} found, none downloadable.`;
  } else {
    el.textContent = `${prefix}: ${downloadable} downloadable video${downloadable === 1 ? "" : "s"} found — hover a video to download it.`;
  }
}

// Round 78 — the platform display name for a page URL, or null
// off-platform. video-detect.js (loaded first in popup.html)
// owns the domain list; this mirrors its names only, so a page
// the detector does not know never gets a platform line here.
function platformNameFor(url) {
  const VD = (typeof globalThis !== "undefined" && globalThis.FetchlineVideoDetect) || {};
  if (VD.isSupportedPlatform && VD.platformName) {
    if (!VD.isSupportedPlatform(url)) return null;
    return VD.platformName(url);
  }
  let host = "";
  try {
    host = new URL(url).hostname.toLowerCase();
  } catch {
    return null;
  }
  const names = [
    ["youtube.com", "YouTube"],
    ["youtu.be", "YouTube"],
    ["facebook.com", "Facebook"],
    ["fb.watch", "Facebook"],
    ["instagram.com", "Instagram"],
    ["tiktok.com", "TikTok"],
  ];
  const hit = names.find(([domain]) => host === domain || host.endsWith(`.${domain}`));
  return hit ? hit[1] : null;
}

function hostOf(url) {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}

// Round 88 — the per-tab buffered relay list: items the relay
// caught while they played on this page, waiting for a click.
// Read from chrome.storage.session (the background mirrors the
// buffer there — this popup is an extension page, so it can
// read session storage directly), with a runtime-message
// fallback when the mirror is empty. Clicking a row sends
// relay_capture for it through the background worker, which
// is the only path to the host. Media + manifests only — a
// "maybe" is recorded, never offered.
async function refreshRelayLine() {
  const lineEl = $("relayLine");
  const listEl = $("relayList");
  if (!lineEl && !listEl) return; // older popup HTML
  let tabId = null;
  try {
    const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
    tabId = tabs && tabs[0] && tabs[0].id != null ? tabs[0].id : null;
  } catch {
    tabId = null;
  }
  if (tabId == null) return;
  let items = [];
  try {
    if (chrome.storage && chrome.storage.session && typeof chrome.storage.session.get === "function") {
      const stored = await chrome.storage.session.get(`relayBuffer:${tabId}`);
      const key = `relayBuffer:${tabId}`;
      if (stored && Array.isArray(stored[key])) items = stored[key];
    }
  } catch {
    items = [];
  }
  if (items.length === 0) {
    // The session mirror is empty (or unavailable): ask the
    // background, whose in-memory buffer is the source of
    // truth. The reply is already surfaced-only.
    try {
      const reply = await chrome.runtime.sendMessage({ type: "relay_get_buffered", tabId });
      if (reply && Array.isArray(reply.items)) items = reply.items;
    } catch {
      // No buffered items to show.
    }
  }
  const VD = (typeof globalThis !== "undefined" && globalThis.FetchlineVideoDetect) || {};
  if (typeof VD.filterRelayItems === "function") items = VD.filterRelayItems(items);
  else items = (items || []).filter((i) => i && (i.kind === "media" || i.kind === "manifest"));
  renderRelayList(lineEl, listEl, items, tabId);
}

function renderRelayList(lineEl, listEl, items, tabId) {
  const VD = (typeof globalThis !== "undefined" && globalThis.FetchlineVideoDetect) || {};
  if (lineEl) {
    lineEl.textContent =
      items.length === 0
        ? ""
        : `Relay: ${items.length} video${items.length === 1 ? "" : "s"} caught on this page — click one to download it.`;
  }
  if (!listEl) return;
  listEl.innerHTML = "";
  items.forEach((item) => {
    const row = document.createElement("li");
    const button = document.createElement("button");
    button.type = "button";
    button.textContent =
      typeof VD.relayItemLabel === "function"
        ? VD.relayItemLabel(item)
        : (item && item.filename) || "Unknown";
    button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        const reply = await chrome.runtime.sendMessage({
          type: "relay_send_buffered",
          url: (item && (item.url || item.finalUrl)) || "",
          tabId,
        });
        if (reply && reply.ok !== false) {
          // The item left the buffer: re-read the list.
          await refreshRelayLine();
          return;
        }
      } catch {
        // The send failed: leave the row for a retry.
      }
      button.disabled = false;
    });
    row.appendChild(button);
    listEl.appendChild(row);
  });
}
