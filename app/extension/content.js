// Sonic Manager video panel (S08 / F05) — content script.
//
// Round 73 — AUTO-INJECTED on every http(s) page now
// (manifest.json `content_scripts`, `document_idle`, all_frames
// + match_about_blank): the floating Download button appears
// next to any video without any click, like IDM. The
// right-click menu's "Find videos on this page" still works —
// it asks this script to re-scan (`rescan_videos`), and
// re-injection is a no-op thanks to the
// `__fetchlineVideoInstalled` guard below.
//
// The per-site opt-out / switched-off integration check that
// used to happen BEFORE injection (background.js
// `injectVideoDetector`) now happens HERE, directly against
// chrome.storage.local — NO service-worker round-trip: the
// script scans and shows the button immediately, and the
// storage read (which resolves locally in <5 ms) hides the
// button again if the page turns out to be opted out or
// integration is off. A failed read defaults to ALLOWED —
// the button is the user's core spec and must not vanish on
// a storage hiccup; the opt-out is a preference, not a
// security boundary. Videos added after injection are picked
// up by a capturing `loadedmetadata` listener and a debounced
// MutationObserver; hover is a throttled `mousemove` using
// document.elementsFromPoint + video-rect containment, so
// overlays on top of the player no longer hide the button.
//
// What it does on the page:
//  * scans the live DOM for <video> / <audio> elements and their
//    <source> children (video-detect.js, loaded first in the same
//    injection), using the real videoWidth/videoHeight for quality
//    and the playback state for live / DRM;
//  * shows a Sonic-branded floating "Download" button at the top
//    right of a video player that has a downloadable stream;
//  * opens a small panel listing that video's qualities (1080p /
//    720p / 480p / 360p where actually available, with a Full HD /
//    HD tag). Sizes are NOT shown — nothing in the extension
//    ever knows a variant's size, and no HEAD/fetch is ever
//    made to find one. Picking one sends video_found {page_url,
//    variants: [chosen]} to the background worker, which hands it
//    to the desktop app over Native Messaging. Only the chosen
//    variant is sent — the desktop app queues exactly what the
//    user picked, not every quality, and the Add URL dialog opens
//    with it pre-filled (Round 67).
//  * a DRM-protected video, or one with no downloadable stream,
//    shows an honest "can't be downloaded" note and NO download
//    button. A live stream's .m3u8 is offered as-is, labelled as a
//    playlist file, never as a merged video.
//
// The panel is Sonic design (dark #0B0F24, purple #8B20FF → pink
// #F02DCD gradient, cyan #00C8FF) — no artwork or wording from any
// other download manager. It never reads cookies, never sends
// them, and never touches page content beyond the media elements
// it lists.

(function () {
  if (window.__fetchlineVideoInstalled) return;
  window.__fetchlineVideoInstalled = true;

  const ACCENT = "#8B20FF";
  const ACCENT_GRADIENT = "linear-gradient(90deg, #8B20FF, #F02DCD)";
  const PANEL_BG = "#0B0F24";
  const PANEL_TEXT = "#E8ECF8";
  const PANEL_MUTED = "#9AA6C7";
  const PANEL_BORDER = "rgba(139, 32, 255, 0.5)";
  const DOWNLOAD_ICON =
    '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" ' +
    'stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
    '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/>' +
    '<polyline points="7 10 12 15 17 10"/>' +
    '<line x1="12" y1="15" x2="12" y2="3"/></svg>';

  // Round 73 — timing logs behind this flag only. Silent
  // (no console output at all) when false.
  const DEBUG_VIDEO = false;
  const DEBUG_T0 = Date.now();
  function debugVideo(label) {
    if (!DEBUG_VIDEO) return;
    try {
      if (typeof console !== "undefined" && typeof console.debug === "function") {
        console.debug(`[Sonic video] ${label} +${Date.now() - DEBUG_T0}ms`);
      }
    } catch {
      // Logging must never break the detector.
    }
  }
  debugVideo("injection");

  // Round 73 — the gate, read DIRECTLY from chrome.storage.local
  // (no service-worker round-trip, so a cold worker can no longer
  // delay the first scan/show by seconds). Same semantics as the
  // background's old reply for this check: allowed =
  // desktopIntegration !== false && !isExcluded(host, state),
  // where isExcluded checks both lists with suffix matching.
  // The read is async but never blocks the first scan: init()
  // scans and shows immediately, and this promise hides the
  // button again (and bars later shows) if the page turns out
  // to be disallowed. A failed read defaults to ALLOWED.
  let allowed = true;
  let allowedResolved = false;
  let allowedPromise = null;

  function hostOf(url) {
    try {
      return new URL(url).hostname.toLowerCase();
    } catch {
      return null;
    }
  }

  function isExcluded(host, state) {
    if (!host) return false;
    const lists = [state.excludedSites || [], state.desktopExcluded || []];
    return lists.some((list) =>
      (list || []).some((entry) => {
        const e = String(entry).toLowerCase();
        return host === e || host.endsWith(`.${e}`);
      }),
    );
  }

  function checkAllowed() {
    if (allowedPromise) return allowedPromise;
    allowedPromise = (async () => {
      try {
        const state = await chrome.storage.local.get([
          "desktopIntegration",
          "desktopExcluded",
          "excludedSites",
        ]);
        const ok =
          state.desktopIntegration !== false &&
          !isExcluded(hostOf(location.href), state || {});
        debugVideo(`storage-resolve allowed=${ok}`);
        allowedResolved = true;
        allowed = ok;
        if (!ok) {
          hideButton();
          if (panel) panel.style.display = "none";
        }
        return ok;
      } catch {
        debugVideo("storage-resolve allowed=true (read failed)");
        allowedResolved = true;
        allowed = true;
        return true;
      }
    })();
    return allowedPromise;
  }

  let button = null;
  let panel = null;
  let currentVideo = null;
  let hideTimer = null;
  let initialized = false;
  let firstScanLogged = false;
  let firstShowLogged = false;
  let firstMouseMoveLogged = false;
  // Videos already in the map, and videos that already have a
  // mouseenter listener (a rescan must not attach a second one).
  const byElement = new Map();
  const attached = new Set();

  // (Re-)scan the live DOM for <video> / <audio> elements and
  // their <source> children (video-detect.js, loaded first in
  // the same injection), using the real videoWidth/videoHeight
  // for quality and the playback state for live / DRM. Group
  // variants by their video element, so each floating button
  // offers only the qualities of the video it sits on. Returns
  // the scan summary, and reports it to the popup (via the
  // background worker) — on the first run and on every rescan,
  // so the popup's scan line is never stale.
  function scanAndAttach() {
    if (!firstScanLogged) {
      firstScanLogged = true;
      debugVideo("first-scan");
    }
    const media = scanLiveMedia(document, location.href);
    // Drop videos that left the page since the last scan.
    for (const video of byElement.keys()) {
      if (!document.contains(video)) {
        byElement.delete(video);
        attached.delete(video);
      }
    }
    document.querySelectorAll("video").forEach((el) => {
      const variants = [];
      const collect = (url) => {
        if (!url) return;
        let absolute;
        try {
          absolute = new URL(url, location.href).toString();
        } catch {
          return;
        }
        const hit = media.find((v) => v.url === absolute);
        if (hit) variants.push(hit);
      };
      collect(el.currentSrc || el.getAttribute("src") || "");
      el.querySelectorAll("source").forEach((s) => collect(s.getAttribute("src") || ""));
      // Round 73 — dedupe by URL: currentSrc and a <source>
      // child usually name the same file, which used to list
      // the same quality twice (e.g. "320×176" twice).
      const deduped =
        typeof dedupeVariants === "function" ? dedupeVariants(variants) : variants;
      if (deduped.length > 0) {
        byElement.set(el, deduped);
        if (!attached.has(el)) {
          attached.add(el);
          el.addEventListener("mouseenter", () => {
            clearTimeout(hideTimer);
            showButton(el);
          });
          el.addEventListener("mouseleave", scheduleHide);
        }
      }
    });
    const downloadable = media.filter((v) => !v.drm).length;
    const summary = { total: media.length, downloadable };
    chrome.runtime.sendMessage({
      type: "videos_found",
      pageUrl: location.href,
      total: summary.total,
      downloadable: summary.downloadable,
    });
    return summary;
  }

  function makeButton() {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = "Download";
    b.setAttribute("aria-label", "Download this video with Sonic Manager");
    const icon = document.createElement("span");
    icon.innerHTML = DOWNLOAD_ICON;
    icon.style.display = "inline-flex";
    b.prepend(icon);
    Object.assign(b.style, {
      position: "fixed",
      zIndex: "2147483647",
      display: "inline-flex",
      alignItems: "center",
      gap: "7px",
      padding: "6px 10px 6px 6px",
      border: "1px solid rgba(139, 32, 255, 0.7)",
      borderRadius: "8px",
      fontFamily: "Inter, system-ui, sans-serif",
      fontSize: "12.5px",
      fontWeight: "700",
      lineHeight: "20px",
      color: "#ffffff",
      background: "rgba(11, 15, 36, 0.88)",
      boxShadow: "0 0 16px rgba(139, 32, 255, 0.55)",
      cursor: "pointer",
    });
    b.addEventListener("mouseenter", () => clearTimeout(hideTimer));
    b.addEventListener("mouseleave", scheduleHide);
    b.addEventListener("click", () => showPanel(currentVideo));
    return b;
  }

  // Top-right corner of the video, like IDM's floating button
  // (`position: fixed` + `right` keeps it glued there while the
  // page scrolls).
  function positionButton(video) {
    const r = video.getBoundingClientRect();
    button.style.left = "auto";
    button.style.right = `${Math.max(8, window.innerWidth - r.right + 12)}px`;
    button.style.top = `${Math.max(8, r.top + 12)}px`;
  }

  function showButton(video) {
    if (allowedResolved && !allowed) return;
    if (!byElement.has(video)) return;
    if (!button) {
      button = makeButton();
      document.body.appendChild(button);
    }
    currentVideo = video;
    positionButton(video);
    button.style.display = "inline-flex";
    if (!firstShowLogged) {
      firstShowLogged = true;
      debugVideo("first-show");
    }
  }

  function hideButton() {
    if (button) button.style.display = "none";
    currentVideo = null;
  }

  function scheduleHide() {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (panel && panel.style.display !== "none") return;
      hideButton();
    }, 300);
  }

  function closePanel() {
    if (panel) panel.style.display = "none";
    hideButton();
  }

  // Quality tag for the panel, from the approved mockup:
  // 1080p is Full HD, 720p is HD. Dimensions give the same
  // answer ("1920 × 1080" is 1080p); anything else gets no tag.
  function qualityTag(quality) {
    if (quality === "1080p" || quality === "1920 × 1080") return "Full HD";
    if (quality === "720p" || quality === "1280 × 720") return "HD";
    return null;
  }

  function showPanel(video) {
    const variants = byElement.get(video) || [];
    if (!panel) {
      panel = document.createElement("div");
      panel.setAttribute("role", "dialog");
      panel.setAttribute("aria-label", "Download this video with Sonic Manager");
      Object.assign(panel.style, {
        position: "fixed",
        zIndex: "2147483647",
        width: "300px",
        maxWidth: "calc(100vw - 16px)",
        padding: "16px",
        border: `1px solid ${PANEL_BORDER}`,
        borderRadius: "12px",
        fontFamily: "Inter, system-ui, sans-serif",
        fontSize: "14px",
        lineHeight: "20px",
        color: PANEL_TEXT,
        background: PANEL_BG,
        boxShadow: "0 16px 44px rgba(0,0,0,0.65), 0 0 24px rgba(139,32,255,0.25)",
      });
      document.body.appendChild(panel);
    }
    const r = video.getBoundingClientRect();
    panel.style.left = `${Math.max(8, Math.min(window.innerWidth - 316, r.left + 12))}px`;
    panel.style.top = `${Math.max(8, Math.min(window.innerHeight - 220, r.top + 56))}px`;

    panel.innerHTML = "";
    const title = document.createElement("h2");
    title.textContent = "Download this video";
    Object.assign(title.style, { margin: "0 0 4px", fontSize: "16px", lineHeight: "24px", color: "#ffffff" });
    panel.appendChild(title);
    const sub = document.createElement("p");
    sub.textContent = "Sonic Manager saves the video to your downloads. Only openly downloadable streams are offered.";
    Object.assign(sub.style, { margin: "0 0 12px", fontSize: "13px", color: PANEL_MUTED });
    panel.appendChild(sub);

    const list = document.createElement("ul");
    Object.assign(list.style, { margin: "0 0 12px", padding: "0", listStyle: "none" });
    variants.forEach((v) => {
      const li = document.createElement("li");
      Object.assign(li.style, {
        display: "flex",
        alignItems: "center",
        gap: "8px",
        padding: "8px 0",
        borderBottom: "1px solid rgba(255,255,255,0.08)",
      });
      const label = document.createElement("span");
      label.style.flex = "1";
      label.textContent = `${v.format} · ${v.quality}${v.live ? " · live stream" : ""}`;
      // The mockup's quality tags: 1080p is Full HD, 720p is HD.
      // Round 73 — sizes are NOT shown: nothing in the
      // extension ever knows a variant's size, and no HEAD/fetch
      // is made to find one, so there is no size here at all.
      const tag = qualityTag(v.quality);
      if (tag) {
        const tagEl = document.createElement("span");
        tagEl.textContent = tag;
        tagEl.style.cssText = "font-size: 11px; font-weight: 700; color: #00C8FF;";
        label.appendChild(document.createTextNode(" "));
        label.appendChild(tagEl);
      }
      li.appendChild(label);
      if (v.note) {
        const note = document.createElement("span");
        note.style.cssText = "display: block; font-size: 12px; color: #9AA6C7;";
        note.textContent = v.note;
        label.appendChild(note);
      }
      if (v.drm) {
        const note = document.createElement("span");
        note.style.cssText = "font-size: 13px; color: #F02DCD;";
        note.textContent = "Protected — can't be downloaded";
        li.appendChild(note);
      } else {
        const btn = document.createElement("button");
        btn.type = "button";
        btn.textContent = v.format === "MPD" ? "Download manifest" : v.live ? "Download playlist" : "Download";
        Object.assign(btn.style, {
          padding: "6px 10px",
          border: "0",
          borderRadius: "8px",
          fontFamily: "inherit",
          fontSize: "13px",
          fontWeight: "600",
          color: "#ffffff",
          background: ACCENT_GRADIENT,
          cursor: "pointer",
        });
        btn.addEventListener("click", () => sendVariant(v, btn));
        li.appendChild(btn);
      }
      list.appendChild(li);
    });
    panel.appendChild(list);

    const anyDownloadable = variants.some((v) => !v.drm);
    if (!anyDownloadable) {
      const note = document.createElement("p");
      note.style.cssText = "margin: 0 0 12px; font-size: 13px; color: #F02DCD;";
      note.textContent =
        "This video can't be downloaded by Sonic Manager — it is protected, or it has no downloadable stream.";
      panel.appendChild(note);
    } else if (variants.some((v) => v.live)) {
      const note = document.createElement("p");
      note.style.cssText = "margin: 0 0 12px; font-size: 13px; color: #9AA6C7;";
      note.textContent =
        "A live stream is saved as its playlist file — it is not merged into one video file.";
      panel.appendChild(note);
    } else if (variants.some((v) => v.format === "MPD")) {
      const note = document.createElement("p");
      note.style.cssText = "margin: 0 0 12px; font-size: 13px; color: #9AA6C7;";
      note.textContent =
        "A DASH manifest is saved as-is — its segments are not merged into one video file.";
      panel.appendChild(note);
    }

    const status = document.createElement("p");
    status.setAttribute("role", "status");
    status.style.cssText = "margin: 0 0 12px; font-size: 13px; color: #9AA6C7;";
    panel.appendChild(status);

    const close = document.createElement("button");
    close.type = "button";
    close.textContent = "Close";
    Object.assign(close.style, {
      padding: "8px 12px",
      border: "1px solid rgba(255,255,255,0.12)",
      borderRadius: "8px",
      fontFamily: "inherit",
      fontSize: "14px",
      fontWeight: "600",
      color: "#E8ECF8",
      background: "rgba(255,255,255,0.06)",
      cursor: "pointer",
    });
    close.addEventListener("click", closePanel);
    panel.appendChild(close);
    panel.style.display = "block";
    panel._status = status;
  }

  function sendVariant(variant, btn) {
    btn.disabled = true;
    btn.textContent = "Sending…";
    chrome.runtime.sendMessage(
      { type: "video_found", page_url: location.href, variants: [variant] },
      (reply) => {
        if (panel && panel._status) {
          if (reply && reply.ok !== false) {
            panel._status.textContent = "Sent to Sonic Manager — it will appear in the download list.";
          } else {
            panel._status.textContent = `Sonic Manager couldn't take this video: ${reply && reply.error ? reply.error : "the desktop app did not answer"}`;
          }
        }
        btn.disabled = false;
        btn.textContent = variant.format === "MPD" ? "Download manifest" : variant.live ? "Download playlist" : "Download";
      },
    );
  }

  // Round 73 — re-scan helpers. A rescan updates the map in
  // place (scanAndAttach is idempotent, button/panel are
  // singletons) and shows the button ONLY when none is
  // currently visible — e.g. a video that appeared after
  // injection. It never switches or re-pops an already-visible
  // button while the user is reading.
  function maybeShowFirst() {
    if (allowedResolved && !allowed) return;
    if (button && button.style.display !== "none") return;
    for (const [video, variants] of byElement) {
      if (variants.length > 0) {
        showButton(video);
        break;
      }
    }
  }

  let observerTimer = null;
  function scheduleRescan() {
    clearTimeout(observerTimer);
    observerTimer = setTimeout(() => {
      scanAndAttach();
      maybeShowFirst();
    }, 200);
  }

  // Round 73 — hover: a throttled (~100 ms, trailing edge)
  // document-level mousemove. Real players put overlays on top
  // of the <video>, so e.target is almost never the video
  // itself; instead the pointer's position is checked against
  // every mapped video's rect (document.elementsFromPoint is
  // consulted for what is under the pointer, e.g. the button /
  // panel). The button sits at the video's top-right INSIDE
  // its rect, so moving toward it keeps the button shown.
  // When the pointer is inside no video rect and not over the
  // button/panel, the button hides on the usual delay.
  let lastMouseMoveAt = 0;
  let mouseMoveTimer = null;
  let pendingMouseMove = null;

  function handleMouseMove(e) {
    if (!firstMouseMoveLogged) {
      firstMouseMoveLogged = true;
      debugVideo("first-mousemove");
    }
    const x = e && typeof e.clientX === "number" ? e.clientX : null;
    const y = e && typeof e.clientY === "number" ? e.clientY : null;
    if (x === null || y === null) return;
    let atPoint = [];
    try {
      if (typeof document.elementsFromPoint === "function") {
        atPoint = document.elementsFromPoint(x, y) || [];
      }
    } catch {
      atPoint = [];
    }
    if (
      (button && atPoint.includes(button)) ||
      (panel && atPoint.includes(panel)) ||
      (e.target === button) ||
      (e.target === panel)
    ) {
      clearTimeout(hideTimer);
      return;
    }
    for (const [video] of byElement) {
      let r = null;
      try {
        r = video.getBoundingClientRect();
      } catch {
        r = null;
      }
      if (!r) continue;
      if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) {
        clearTimeout(hideTimer);
        showButton(video);
        return;
      }
    }
    scheduleHide();
  }

  function onMouseMove(e) {
    const now = Date.now();
    if (now - lastMouseMoveAt >= 100) {
      lastMouseMoveAt = now;
      handleMouseMove(e);
      return;
    }
    pendingMouseMove = e;
    if (!mouseMoveTimer) {
      mouseMoveTimer = setTimeout(() => {
        mouseMoveTimer = null;
        lastMouseMoveAt = Date.now();
        const ev = pendingMouseMove;
        pendingMouseMove = null;
        if (ev) handleMouseMove(ev);
      }, 100 - (now - lastMouseMoveAt));
    }
  }

  // First run: scan IMMEDIATELY, then wire the button up once.
  // The storage read runs in parallel (checkAllowed, not
  // awaited) and hides the button if the page is disallowed.
  // A rescan (the `rescan_videos` message below) only re-scans
  // — the listeners below are one-time, and the immediate show
  // is first-run only, so a rescan never pops the button up
  // while the user is reading the page.
  async function init() {
    if (initialized) {
      if (allowedResolved && !allowed) return null;
      return scanAndAttach();
    }
    initialized = true;
    // Fire-and-forget: must not delay the scan/show below.
    void checkAllowed();
    const summary = scanAndAttach();
    // Round 71 (A4) — the button must appear even when the mouse
    // is ALREADY over the video when this script is injected (the
    // user clicks the context menu while hovering the player, so
    // no fresh mouseenter ever fires — the Round 70 retest hovered
    // and saw nothing). Two fixes:
    //   1. document-level throttled mousemove with
    //      elementsFromPoint + rect containment (Round 73):
    //      any video in the map shows the button when the
    //      pointer moves over it, including videos added after
    //      injection (a rescan puts them in the map), even
    //      when an overlay covers the video element itself;
    //   2. show the button immediately for the first video with
    //      variants, so a video page shows it without any hover.
    // The button is NEVER gated on native-host health (a dead host
    // must not hide it — the user still sees the qualities, and
    // native messaging is re-attempted only when they click a
    // Download button, in sendVariant).
    document.addEventListener("mousemove", onMouseMove);
    // Round 73 — re-scan triggers for videos that appear after
    // injection: a capturing loadedmetadata listener (a video
    // whose metadata just loaded now has a real source/size to
    // offer) and a debounced MutationObserver for dynamically
    // added <video>/<source> elements.
    document.addEventListener("loadedmetadata", () => {
      scanAndAttach();
      maybeShowFirst();
    }, true);
    try {
      if (typeof MutationObserver !== "undefined" && MutationObserver) {
        const observer = new MutationObserver(() => scheduleRescan());
        const target = document.body || document.documentElement || document;
        observer.observe(target, { childList: true, subtree: true });
      }
    } catch {
      // No MutationObserver in this context: loadedmetadata and
      // on-demand rescans still cover videos added later.
    }
    for (const [video, variants] of byElement) {
      if (variants.length > 0) {
        showButton(video);
        break;
      }
    }
    // Keep the button glued to the video while the page scrolls.
    window.addEventListener("scroll", () => {
      if (button && button.style.display !== "none" && currentVideo) {
        positionButton(currentVideo);
      }
    }, true);
    return summary;
  }

  // Round 72 (A) — re-scan on demand: the popup asks for a fresh
  // scan when it opens, and the right-click menu asks for one
  // too. The reply carries the summary, so the popup can show
  // this page's result without waiting on storage. `null` means
  // the page is disallowed (opted out / integration off).
  chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
    if (message && message.type === "rescan_videos") {
      init().then((summary) => {
        if (summary) sendResponse({ ok: true, total: summary.total, downloadable: summary.downloadable });
        else sendResponse({ ok: false, error: "Videos are not shown on this page." });
      });
      return true;
    }
    return false;
  });

  void init();
})();
