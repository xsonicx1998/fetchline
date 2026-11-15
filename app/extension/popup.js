// Fetchline extension popup.
//
// Reads and writes the same chrome.storage.local keys the background
// worker uses (catchEnabled, excludedSites, desktopIntegration).
// Direction of truth: the desktop browser_integration flag wins —
// when it is off, the catch toggle is disabled here and catching
// stops, whatever the extension toggle says.

const HOST_NAME = "com.fetchline.host";

const $ = (id) => document.getElementById(id);
const statusEl = $("status");
const resultEl = $("result");
const pendingEl = $("pendingInfo");
const handoffEl = $("handoffInfo");
const catchToggle = $("catchToggle");
const siteToggle = $("siteToggle");
const siteLabel = $("siteLabel");
const desktopNote = $("desktopNote");

// Milestone 2 — what the popup shows about the handoff queue.
// Pure functions, so they are unit-tested without a browser
// (extension/tests/popup.test.js).
function formatPendingCount(count) {
  if (typeof count !== "number") return "Waiting links: unknown — check the desktop app.";
  if (count === 0) return "No links waiting in Sonic Manager.";
  if (count === 1) return "1 link waiting in Sonic Manager — it will start downloading shortly.";
  return `${count} links waiting in Sonic Manager — they will start downloading shortly.`;
}

function formatHandoff(handoff) {
  if (!handoff) return "Nothing sent to Sonic Manager yet.";
  const when = new Date(handoff.at).toLocaleTimeString();
  if (handoff.ok) return `Last sent ${when}: ${handoff.message || "Sent to Sonic Manager."}`;
  return `Last send failed ${when}: ${handoff.message || "Sonic Manager could not be reached."}`;
}

async function loadHandoffStatus() {
  const stored = await chrome.storage.local.get(["pendingCount", "lastHandoff"]);
  if (pendingEl) pendingEl.textContent = formatPendingCount(stored.pendingCount ?? null);
  if (handoffEl) handoffEl.textContent = formatHandoff(stored.lastHandoff || null);
}

async function recordHandoff(ok, message, pendingCount) {
  const update = { lastHandoff: { at: Date.now(), ok, message: message || "" } };
  if (typeof pendingCount === "number") update.pendingCount = pendingCount;
  await chrome.storage.local.set(update).catch(() => {});
  await loadHandoffStatus();
}

// Exposed for the Node tests (extension/tests/popup.test.js).
if (typeof globalThis !== "undefined") {
  globalThis.FetchlinePopup = { formatPendingCount, formatHandoff, loadHandoffStatus };
}

function hostOf(url) {
  try {
    return new URL(url).hostname.toLowerCase();
  } catch {
    return null;
  }
}

function sendToHost(message) {
  return new Promise((resolve, reject) => {
    try {
      chrome.runtime.sendNativeMessage(HOST_NAME, message, (reply) => {
        if (chrome.runtime.lastError) {
          reject(new Error(chrome.runtime.lastError.message));
          return;
        }
        resolve(reply || null);
      });
    } catch (err) {
      reject(err);
    }
  });
}

async function currentTab() {
  const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
  return tabs[0] || null;
}

function setStatus(state, text) {
  statusEl.textContent = text;
  statusEl.className = `status status-${state}`;
}

async function refreshStatus() {
  try {
    const reply = await sendToHost({ type: "ping" });
    if (reply && reply.ok !== false) {
      setStatus("ok", "Connected to the Sonic Manager desktop app");
    } else {
      setStatus("bad", "Desktop app answered with an error");
    }
  } catch {
    setStatus("bad", "Desktop app not reachable — is Sonic Manager installed and running?");
  }
  // Pull the desktop settings too, so the toggle below reflects
  // the desktop flag (and seeds the excluded-sites list).
  try {
    await chrome.runtime.sendMessage({ type: "refresh_desktop_settings" });
  } catch {
    // Background worker unavailable; the stored values below are
    // whatever was cached last time it ran.
  }
}

async function loadToggles() {
  const stored = await chrome.storage.local.get([
    "catchEnabled",
    "excludedSites",
    "desktopIntegration",
    "desktopExcluded",
  ]);
  const desktopOff = stored.desktopIntegration === false;
  catchToggle.checked = stored.catchEnabled !== false;
  catchToggle.disabled = desktopOff;
  if (desktopOff) {
    desktopNote.hidden = false;
    desktopNote.textContent =
      "Catching is turned off in Sonic Manager’s settings. Turn browser integration back on in the desktop app to catch downloads again.";
  } else {
    desktopNote.hidden = true;
    desktopNote.textContent = "";
  }

  const tab = await currentTab();
  const host = tab && tab.url ? hostOf(tab.url) : null;
  const excluded = stored.excludedSites || [];
  const desktopExcluded = stored.desktopExcluded || [];
  siteToggle.disabled = !host;
  siteToggle.checked = !!host && (excluded.includes(host) || desktopExcluded.includes(host));
  siteLabel.textContent = host
    ? `Don’t catch downloads from ${host}`
    : "Don’t catch downloads from this site";
  siteToggle.dataset.host = host || "";
  return host;
}

catchToggle.addEventListener("change", async () => {
  await chrome.storage.local.set({ catchEnabled: catchToggle.checked });
  resultEl.textContent = catchToggle.checked
    ? "Catching downloads is on."
    : "Catching downloads is off. Links can still be sent with the right-click menu.";
});

siteToggle.addEventListener("change", async () => {
  const host = siteToggle.dataset.host;
  if (!host) return;
  const stored = await chrome.storage.local.get(["excludedSites"]);
  let list = stored.excludedSites || [];
  if (siteToggle.checked) {
    if (!list.includes(host)) list = [...list, host];
  } else {
    list = list.filter((h) => h !== host);
  }
  await chrome.storage.local.set({ excludedSites: list });
  // Keep the desktop list in step: the desktop app is the other
  // half of the exclusion, so a site opted out here is reported
  // back as excluded only in the extension — the desktop list
  // itself is edited in Fetchline’s Settings, not from here.
  resultEl.textContent = siteToggle.checked
    ? `${host} will no longer be caught. Change this back here, or in Sonic Manager’s Settings for the desktop list.`
    : `${host} will be caught again.`;
});

$("sendTab").addEventListener("click", async () => {
  const tab = await currentTab();
  if (!tab || !tab.url || !tab.url.startsWith("http")) {
    resultEl.textContent = "This page has no web address to send.";
    return;
  }
  try {
    const reply = await sendToHost({ type: "download_link", url: tab.url, referrer: null });
    if (reply && reply.ok === false) throw new Error(reply.error || "Sonic Manager refused the link");
    resultEl.textContent = "Sent to Sonic Manager — it will appear in the download list.";
    await recordHandoff(
      true,
      "Sent to Sonic Manager — it will appear in the download list.",
      reply && typeof reply.pending_count === "number" ? reply.pending_count : undefined,
    );
  } catch (err) {
    resultEl.textContent = `Could not reach Sonic Manager: ${err.message}`;
    await recordHandoff(false, `Could not reach Sonic Manager: ${err.message}`);
  }
});

$("openApp").addEventListener("click", async () => {
  resultEl.textContent = "Checking…";
  await refreshStatus();
  await loadToggles();
  resultEl.textContent = "";
});

// Video grabber (S08): ask the background worker to inject the
// detector into the active tab — a user gesture, nothing automatic.
// The content script then shows a floating button on video players
// on that page; the scan summary below is what it reported back.
$("findVideos").addEventListener("click", async () => {
  resultEl.textContent = "Scanning this page for videos…";
  try {
    const reply = await chrome.runtime.sendMessage({ type: "scan_page_for_videos" });
    if (!reply || reply.ok === false) {
      throw new Error((reply && reply.error) || "The scan did not run");
    }
    // The content script reports back asynchronously; give it a moment.
    await new Promise((resolve) => setTimeout(resolve, 500));
    const stored = await chrome.storage.local.get(["lastVideoScan"]);
    const scan = stored.lastVideoScan;
    const tab = await currentTab();
    if (!scan || (tab && tab.url && scan.pageUrl !== tab.url)) {
      resultEl.textContent = "Scan finished — hover a video on the page to download it.";
      return;
    }
    if (scan.total === 0) {
      resultEl.textContent = "No downloadable video or audio found on this page.";
    } else if (scan.downloadable === 0) {
      resultEl.textContent = `Found ${scan.total} video(s), but none can be downloaded (protected or no stream).`;
    } else {
      resultEl.textContent = `Found ${scan.downloadable} downloadable video(s) on this page — hover a video to download it.`;
    }
  } catch (err) {
    resultEl.textContent = `Could not scan this page: ${err.message}`;
  }
});

document.addEventListener("DOMContentLoaded", async () => {
  await refreshStatus();
  await loadToggles();
  await loadHandoffStatus();
});
