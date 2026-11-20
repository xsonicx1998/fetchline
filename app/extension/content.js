// Sonic Manager video download dropdown (S08 / F05) — content script.
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
//  * opens a qualities dropdown ANCHORED directly below the
//    button (like IDM's, in Sonic identity) listing that video's
//    qualities (1080p HD / 720p HD / 480p / 360p where actually
//    available). Sizes are NOT shown — nothing in the extension
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
// Round 78 (yt-dlp) — on a supported platform (YouTube,
// Facebook, Instagram, TikTok — see video-detect.js
// isSupportedPlatform) the <video> src is a blob:/MediaSource
// handle, never the file, so the direct scan finds nothing and
// the button still attaches. Clicking it resolves qualities
// LAZILY through the host's yt-dlp (background.js opens one
// connectNative port per resolve; heartbeats say "Checking… N
// s"); nothing resolves on page load, on hover, or from the
// popup scan. Resolved lists are cached in memory here, keyed
// by normalized page URL for a few minutes, and cleared when
// the page URL changes (SPA navigation). A needs_login error
// asks once per site whether the browser login may be used
// (stored consent grants silently next time; a denial is
// remembered for this session only). Round 79 — a stored grant
// sends the page's cookies on the FIRST resolve of a later
// click (no doomed cookieless attempt first), and "Try again"
// never repeats the identical failing request. No sizes are
// ever shown.
//
// The button and dropdown are Sonic design (dark #0B0F24, purple
// #8B20FF → pink #F02DCD → cyan #00C8FF gradient) — no artwork or
// wording from any other download manager. It never reads cookies,
// never sends them, and never touches page content beyond the
// media elements it lists.

(function () {
  if (window.__fetchlineVideoInstalled) return;
  window.__fetchlineVideoInstalled = true;

  const ACCENT = "#8B20FF";
  // Approved video-button look (2026-11-19): the floating button
  // is a gradient pill — purple → pink → cyan — with white text
  // and icon; the dropdown header carries the same gradient as a
  // tint, and rows hover with a purple → cyan tint.
  const BUTTON_GRADIENT = "linear-gradient(135deg, #8B20FF, #F02DCD 55%, #00C8FF)";
  const BUTTON_SHADOW = "0 4px 18px rgba(0, 0, 0, 0.5)";
  const BUTTON_SHADOW_HOVER = "0 0 0 1px #00C8FF, 0 0 18px rgba(0, 200, 255, 0.45)";
  const DROPDOWN_WIDTH = 232;
  const DROPDOWN_BG = "#0B0F24";
  const DROPDOWN_TEXT = "#E8ECF8";
  const DROPDOWN_BORDER = "#8B20FF";
  const DROPDOWN_HEADER_BG =
    "linear-gradient(90deg, rgba(139, 32, 255, 0.45), rgba(240, 45, 205, 0.4), rgba(0, 200, 255, 0.4))";
  const ROW_HOVER_BG =
    "linear-gradient(90deg, rgba(139, 32, 255, 0.35), rgba(0, 200, 255, 0.22))";
  const DOWNLOAD_ICON =
    '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" ' +
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
          closeDropdown();
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

  // Round 78 — resolve helpers live in video-detect.js (pure
  // logic, shared with the Node tests); they are globals in the
  // page (classic scripts share one scope) and on
  // FetchlineVideoDetect in the vm tests. Fallbacks keep an old
  // video-detect.js from breaking the direct path: without them
  // the platform button simply never attaches.
  const VD = (typeof globalThis !== "undefined" && globalThis.FetchlineVideoDetect) || {};
  const isSupportedPlatformFn =
    typeof isSupportedPlatform === "function" ? isSupportedPlatform : VD.isSupportedPlatform || (() => false);
  const normalizePageUrlFn =
    typeof normalizePageUrl === "function" ? normalizePageUrl : VD.normalizePageUrl || ((u) => String(u || ""));
  const degradedWarningTextFn =
    typeof degradedWarningText === "function" ? degradedWarningText : VD.degradedWarningText || (() => null);
  const filterResolvableFormatsFn =
    typeof filterResolvableFormats === "function" ? filterResolvableFormats : VD.filterResolvableFormats || ((f) => f || []);
  const qualityRowLabelFn =
    typeof qualityRowLabel === "function" ? qualityRowLabel : VD.qualityRowLabel || ((f) => (f && f.label) || "Unknown quality");
  const buildResolvedVariantFn =
    typeof buildResolvedVariant === "function" ? buildResolvedVariant : VD.buildResolvedVariant || null;
  const videoPageUrlFromElementFn =
    typeof videoPageUrlFromElement === "function"
      ? videoPageUrlFromElement
      : VD.videoPageUrlFromElement || null;
  const createResolveCacheFn =
    typeof createResolveCache === "function" ? createResolveCache : VD.createResolveCache || null;

  // Round 78 — resolved format lists, cached in memory HERE
  // (per page, never persisted — the URLs are signed and go
  // stale), keyed by normalized page URL. The background holds
  // no resolve state beyond one in-flight native port per
  // requesting port. A denial of the cookie consent is kept for
  // this session only; a grant is stored per site.
  const resolveCache = createResolveCacheFn
    ? createResolveCacheFn()
    : { get: () => null, getEntry: () => null, isStale: () => true, set: () => {}, clear: () => {} };
  const deniedConsentHosts = new Set();
  let activeResolve = null; // { requestId, port, video, withCookies, cookies, onResolved }
  // Round 79 — the last failed resolve attempt
  // ({ video, error, withCookies, cookies }), so "Try again"
  // never repeats the identical failing request.
  let lastResolveAttempt = null;
  let lastPageUrl = normalizePageUrlFn(location.href);
  let loadingNote = null;

  // Round 78 — SPA navigation (YouTube/Instagram/TikTok change
  // the URL with history.pushState, no reload): the cached
  // qualities belong to the OLD video and must never be offered
  // for the new one. Called from the history wrappers, popstate,
  // and every rescan (the MutationObserver backstop).
  function notePageUrlChanged() {
    const now = normalizePageUrlFn(location.href);
    if (now === lastPageUrl) return false;
    lastPageUrl = now;
    resolveCache.clear();
    closeDropdown();
    return true;
  }

  let button = null;
  let dropdown = null;
  let toast = null;
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
  // Round 78 — platform videos with no direct downloadable
  // source (blob: src): they still get a button, and clicking
  // it resolves qualities instead of listing direct variants.
  const resolveOnly = new Set();

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
    // Round 78 — a rescan is also the backstop for SPA
    // navigation: if the URL moved since the last scan, the
    // cached qualities for the old video are dropped here.
    notePageUrlChanged();
    const media = scanLiveMedia(document, location.href);
    // Drop videos that left the page since the last scan.
    for (const video of byElement.keys()) {
      if (!document.contains(video)) {
        byElement.delete(video);
        attached.delete(video);
      }
    }
    for (const video of resolveOnly) {
      if (!document.contains(video)) {
        resolveOnly.delete(video);
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
        resolveOnly.delete(el);
      } else if (isSupportedPlatformFn(location.href)) {
        // Round 78 — no direct source (blob: / empty src on a
        // platform player): the button still attaches, and the
        // click resolves qualities through yt-dlp instead.
        byElement.delete(el);
        resolveOnly.add(el);
      } else {
        byElement.delete(el);
        resolveOnly.delete(el);
      }
      if ((byElement.has(el) || resolveOnly.has(el)) && !attached.has(el)) {
        attached.add(el);
        el.addEventListener("mouseenter", () => {
          clearTimeout(hideTimer);
          showButton(el);
        });
        el.addEventListener("mouseleave", scheduleHide);
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
      padding: "8px 14px",
      border: "1px solid rgba(255, 255, 255, 0.28)",
      borderRadius: "999px",
      fontFamily: "Inter, system-ui, sans-serif",
      fontSize: "13.5px",
      fontWeight: "600",
      lineHeight: "20px",
      color: "#ffffff",
      background: BUTTON_GRADIENT,
      boxShadow: BUTTON_SHADOW,
      cursor: "pointer",
    });
    b.addEventListener("mouseenter", () => {
      clearTimeout(hideTimer);
      b.style.boxShadow = BUTTON_SHADOW_HOVER;
    });
    b.addEventListener("mouseleave", () => {
      b.style.boxShadow = BUTTON_SHADOW;
      scheduleHide();
    });
    // Toggle the dropdown anchored below the button (a second
    // click closes it again). Round 78: on a platform video with
    // no direct source the click resolves qualities instead;
    // repeat clicks are ignored while a resolve is in flight.
    b.addEventListener("click", () => {
      if (dropdown && dropdown.style.display !== "none" && dropdown._video === currentVideo) {
        closeDropdown();
        return;
      }
      if (activeResolve && activeResolve.video === currentVideo) return;
      if (needsResolve(currentVideo)) {
        openResolveDropdown(currentVideo);
      } else {
        openDropdown(currentVideo);
      }
    });
    return b;
  }

  // Round 78 — this video's qualities come from the resolver,
  // not the direct scan: either it is a platform video with no
  // direct source, or it has no direct downloadable variants at
  // all. A platform video WITH direct variants keeps the direct
  // dropdown — the file is right there, no resolve needed.
  function needsResolve(video) {
    if (!video) return false;
    if (!isSupportedPlatformFn(location.href)) return false;
    if (resolveOnly.has(video)) return true;
    const variants = byElement.get(video) || [];
    return variants.length === 0;
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
    if (!byElement.has(video) && !resolveOnly.has(video)) return;
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
      if (dropdown && dropdown.style.display !== "none") return;
      hideButton();
    }, 300);
  }

  // Hide the dropdown only (the button stays until the pointer
  // leaves the video, per scheduleHide). Round 78: closing it
  // also cancels an in-flight resolve — the port closes, which
  // kills the host's yt-dlp.
  function closeDropdown() {
    if (activeResolve) cancelResolve();
    if (dropdown) dropdown.style.display = "none";
  }

  // Round 78 — cancel the in-flight resolve, if any, by
  // closing its port to the background (which closes the native
  // port, killing yt-dlp). Silent: a user-cancelled resolve is
  // not an error to surface.
  function cancelResolve() {
    const current = activeResolve;
    activeResolve = null;
    if (button) button.disabled = false;
    if (!current || !current.port) return;
    try {
      current.port.postMessage({ type: "cancel_resolve", request_id: current.requestId });
    } catch {
      // Port already gone.
    }
    try {
      if (typeof current.port.disconnect === "function") current.port.disconnect();
    } catch {
      // Port already gone.
    }
  }

  // Is `node` the element `root` itself or inside it? Walks
  // parentNode, so it works for the button's icon span and the
  // dropdown rows' label spans (and in the Node test harness,
  // which has no Element.contains).
  function isInside(node, root) {
    let n = node;
    while (n) {
      if (n === root) return true;
      n = n.parentNode;
    }
    return false;
  }

  // Outside click / Escape closes the dropdown. Registered once
  // in init(); the button's own click is excluded (it toggles),
  // and clicks inside the dropdown (rows, labels) are excluded.
  function onDocumentClick(e) {
    if (!dropdown || dropdown.style.display === "none") return;
    const target = e && e.target;
    if (target === button || isInside(target, button) || isInside(target, dropdown)) return;
    closeDropdown();
  }

  function onDocumentKeyDown(e) {
    if (e && e.key === "Escape") closeDropdown();
  }

  // Row label for the dropdown, from the approved preview:
  // 1080p and 720p read "1080p HD" / "720p HD" (dimensions give
  // the same answer — "1920 × 1080" is 1080p); anything else is
  // shown as detected ("320 × 176", "480p", "Unknown quality").
  function qualityLabel(quality) {
    if (quality === "1080p" || quality === "1920 × 1080") return "1080p HD";
    if (quality === "720p" || quality === "1280 × 720") return "720p HD";
    return quality;
  }

  // Anchor the dropdown directly below the floating button: its
  // right edge lines up with the button's (both are top-right of
  // the same video rect), 6 px below the button's bottom. The
  // button rect is measured live when available; when it is not
  // (a zero rect), fall back to the video rect plus the button's
  // expected height (12 px offset + ~40 px tall + 6 px gap).
  function positionDropdown(video) {
    const r = video.getBoundingClientRect();
    const right = Math.max(8, window.innerWidth - r.right + 12);
    const left = window.innerWidth - right - DROPDOWN_WIDTH;
    if (left < 8) {
      dropdown.style.left = "8px";
      dropdown.style.right = "auto";
    } else {
      dropdown.style.left = "auto";
      dropdown.style.right = `${right}px`;
    }
    let top = null;
    if (button) {
      try {
        const br = button.getBoundingClientRect();
        if (br && br.bottom > 0) top = br.bottom + 6;
      } catch {
        top = null;
      }
    }
    if (top === null) top = r.top + 12 + 40 + 6;
    dropdown.style.top = `${Math.max(8, top)}px`;
  }

  // Small confirmation toast at the bottom of the page (same
  // look as the approved preview): shown after a quality is
  // picked, for both success and failure, then hides itself.
  function showToast(text) {
    if (!toast) {
      toast = document.createElement("div");
      toast.setAttribute("role", "status");
      Object.assign(toast.style, {
        position: "fixed",
        left: "50%",
        bottom: "26px",
        transform: "translateX(-50%)",
        zIndex: "2147483647",
        display: "none",
        padding: "11px 18px",
        border: "1px solid #00C8FF",
        borderRadius: "999px",
        fontFamily: "Inter, system-ui, sans-serif",
        fontSize: "13.5px",
        color: "#ffffff",
        background: DROPDOWN_BG,
        boxShadow: "0 10px 30px rgba(0, 0, 0, 0.6)",
        whiteSpace: "nowrap",
      });
      document.body.appendChild(toast);
    }
    toast.textContent = text;
    toast.style.display = "block";
    clearTimeout(toast._timer);
    toast._timer = setTimeout(() => {
      toast.style.display = "none";
    }, 2600);
  }

  // Create the dropdown element once and anchor it below the
  // button (shared by the direct dropdown and the Round 78
  // resolve dropdown).
  function ensureDropdown(video) {
    if (!dropdown) {
      dropdown = document.createElement("div");
      dropdown.setAttribute("role", "dialog");
      dropdown.setAttribute("aria-label", "Download this video with Sonic Manager");
      Object.assign(dropdown.style, {
        position: "fixed",
        zIndex: "2147483647",
        width: `${DROPDOWN_WIDTH}px`,
        maxWidth: "calc(100vw - 16px)",
        border: `1px solid ${DROPDOWN_BORDER}`,
        borderRadius: "12px",
        fontFamily: "Inter, system-ui, sans-serif",
        fontSize: "13.5px",
        lineHeight: "20px",
        color: DROPDOWN_TEXT,
        background: DROPDOWN_BG,
        boxShadow: "0 16px 40px rgba(0, 0, 0, 0.65)",
        overflow: "hidden",
      });
      document.body.appendChild(dropdown);
    }
    dropdown._video = video;
    positionDropdown(video);
    return dropdown;
  }

  function dropdownTitle() {
    const title = document.createElement("h3");
    title.textContent = "Download this video";
    Object.assign(title.style, {
      margin: "0",
      padding: "11px 14px",
      fontSize: "13px",
      fontWeight: "600",
      color: "#ffffff",
      background: DROPDOWN_HEADER_BG,
      borderBottom: "1px solid rgba(255, 255, 255, 0.08)",
    });
    return title;
  }

  function openDropdown(video) {
    const variants = byElement.get(video) || [];
    ensureDropdown(video);

    dropdown.innerHTML = "";
    dropdown.appendChild(dropdownTitle());

    // One row per variant, deduped by URL at scan time (Round
    // 73): the whole row is the button — quality on the left,
    // format on the right, no sizes. Nothing in the extension
    // ever knows a variant's size, and no HEAD/fetch is ever
    // made to find one, so there is no size here at all.
    variants.forEach((v) => {
      const row = document.createElement("button");
      row.type = "button";
      Object.assign(row.style, {
        display: "flex",
        width: "100%",
        justifyContent: "space-between",
        alignItems: "center",
        padding: "11px 14px",
        border: "0",
        background: "none",
        fontFamily: "inherit",
        fontSize: "13.5px",
        color: "#ffffff",
        textAlign: "left",
        cursor: "pointer",
      });
      const label = document.createElement("span");
      label.textContent = qualityLabel(v.quality) + (v.live ? " · live" : "");
      row.appendChild(label);
      const format = document.createElement("span");
      if (v.drm) {
        format.textContent = "Protected";
        format.style.cssText = "font-size: 12px; color: #F02DCD;";
        row.disabled = true;
        row.style.cursor = "default";
      } else {
        format.textContent = v.format;
        format.style.cssText = "font-size: 12px; color: #8f9bbd;";
        row.addEventListener("mouseenter", () => {
          if (!row.disabled) row.style.background = ROW_HOVER_BG;
        });
        row.addEventListener("mouseleave", () => {
          row.style.background = "none";
        });
        row.addEventListener("click", () => sendVariant(video, v, row, label));
      }
      row.appendChild(format);
      row._label = label;
      dropdown.appendChild(row);
    });

    const anyDownloadable = variants.some((v) => !v.drm);
    if (!anyDownloadable) {
      const note = document.createElement("p");
      note.style.cssText = "margin: 0; padding: 11px 14px; font-size: 13px; color: #F02DCD;";
      note.textContent =
        "This video can't be downloaded by Sonic Manager — it is protected, or it has no downloadable stream.";
      dropdown.appendChild(note);
    } else if (variants.some((v) => v.live)) {
      const note = document.createElement("p");
      note.style.cssText = "margin: 0; padding: 11px 14px; font-size: 12px; color: #9AA6C7;";
      note.textContent =
        "A live stream is saved as its playlist file — it is not merged into one video file.";
      dropdown.appendChild(note);
    } else if (variants.some((v) => v.format === "MPD")) {
      const note = document.createElement("p");
      note.style.cssText = "margin: 0; padding: 11px 14px; font-size: 12px; color: #9AA6C7;";
      note.textContent =
        "A DASH manifest is saved as-is — its segments are not merged into one video file.";
      dropdown.appendChild(note);
    }

    dropdown.style.display = "block";
  }

  // -- Round 78 (yt-dlp) resolve flow ---------------------------------
  //
  // Lazy only: nothing here runs until the user clicks the
  // button on a platform video. The dropdown opens in a
  // "Checking…" state, the background opens one connectNative
  // port to the host, and heartbeats update the elapsed seconds
  // until the final `resolved` or `error` reply arrives.

  // Round 79 — the first resolve goes WITH the page's cookies
  // when this site already has a stored cookie-consent grant:
  // a cookieless first attempt would only fail again with
  // needs_login and waste a round trip (the Round 78 retest's
  // click failed three times before the consent path was ever
  // reached). No grant stored: the old cookieless-first
  // behaviour — the host is never sent cookies it was not
  // allowed to have. The consent read is async, so the
  // dropdown shows "Checking…" first; if the user closes it
  // while the read is in flight, no resolve starts at all (a
  // late reply would otherwise pop the dropdown back open).
  // Round 81 — the URL a resolve runs for: the playing
  // video's own page when the element sits inside a link to
  // one (a TikTok feed slide links to its video page), else
  // the page URL itself (a YouTube watch page already is the
  // video page). The resolve cache, the consent host and the
  // cookie fetch all use this URL, so two feed videos never
  // share one cached list.
  function pageUrlForResolve(video) {
    if (videoPageUrlFromElementFn) {
      return videoPageUrlFromElementFn(video) || location.href;
    }
    return location.href;
  }

  async function openResolveDropdown(video) {
    if (activeResolve && activeResolve.video === video) return;
    const resolveUrl = pageUrlForResolve(video);
    const cached = resolveCache.get(normalizePageUrlFn(resolveUrl));
    if (cached) {
      renderResolved(video, cached);
      return;
    }
    renderResolveLoading(video);
    let withCookies = false;
    let cookies = [];
    const host = hostOf(resolveUrl);
    if (host) {
      try {
        const stored = await chrome.storage.local.get([`ytdlp_cookie_consent:${host}`]);
        if (stored && stored[`ytdlp_cookie_consent:${host}`] === true) {
          withCookies = true;
          cookies = await requestCookies(resolveUrl);
        }
      } catch {
        // Storage/cookie read failed: fall back to cookieless.
      }
    }
    if (!dropdown || dropdown.style.display === "none" || dropdown._video !== video) return;
    startResolve(video, { withCookies, cookies, onResolved: null });
  }

  function renderResolveLoading(video) {
    ensureDropdown(video);
    dropdown.innerHTML = "";
    dropdown.appendChild(dropdownTitle());
    loadingNote = document.createElement("p");
    loadingNote.style.cssText = "margin: 0; padding: 11px 14px; font-size: 13px; color: #E8ECF8;";
    loadingNote.textContent = "Checking… 0 s";
    dropdown.appendChild(loadingNote);
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.textContent = "Cancel";
    Object.assign(cancel.style, {
      display: "block",
      width: "100%",
      padding: "11px 14px",
      border: "0",
      background: "none",
      fontFamily: "inherit",
      fontSize: "13.5px",
      color: "#F02DCD",
      textAlign: "left",
      cursor: "pointer",
    });
    cancel.addEventListener("click", () => closeDropdown());
    dropdown.appendChild(cancel);
    dropdown.style.display = "block";
  }

  function updateResolveLoading(elapsedSecs) {
    if (!loadingNote) return;
    const secs = typeof elapsedSecs === "number" ? elapsedSecs : 0;
    loadingNote.textContent = `Checking… ${secs} s`;
  }

  // Round 81 — `allowRetry` false hides the "Try again"
  // button: some failures no retry can fix in this session
  // (a bot block, a login wall that already had the browser's
  // cookies), and a retry button there would only repeat the
  // identical failing request.
  function renderResolveError(video, message, allowRetry = true) {
    ensureDropdown(video);
    dropdown.innerHTML = "";
    loadingNote = null;
    dropdown.appendChild(dropdownTitle());
    const note = document.createElement("p");
    note.style.cssText = "margin: 0; padding: 11px 14px; font-size: 13px; color: #F02DCD;";
    note.textContent = message;
    dropdown.appendChild(note);
    if (allowRetry === false) {
      dropdown.style.display = "block";
      return;
    }
    const retry = document.createElement("button");
    retry.type = "button";
    retry.textContent = "Try again";
    Object.assign(retry.style, {
      display: "block",
      width: "100%",
      padding: "11px 14px",
      border: "0",
      background: "none",
      fontFamily: "inherit",
      fontSize: "13.5px",
      color: "#ffffff",
      textAlign: "left",
      cursor: "pointer",
    });
    // Round 79 — "Try again" never repeats the identical
    // failing request: retryResolve re-runs the consent flow
    // for a cookieless needs_login failure, and otherwise
    // retries with the same cookies the failed attempt used.
    retry.addEventListener("click", () => {
      renderResolveLoading(video);
      retryResolve(video);
    });
    dropdown.appendChild(retry);
    dropdown.style.display = "block";
  }

  // One row per resolved format, 360p and up, with the honest
  // note for video-only / stream rows. No sizes — nothing here
  // knows one. A degraded reply shows the warning at the top;
  // a live stream shows that it cannot be downloaded, and no
  // qualities at all.
  function renderResolved(video, entry) {
    const resolved = entry.resolved;
    ensureDropdown(video);
    dropdown.innerHTML = "";
    loadingNote = null;
    dropdown.appendChild(dropdownTitle());
    if (resolved.is_live) {
      const note = document.createElement("p");
      note.style.cssText = "margin: 0; padding: 11px 14px; font-size: 13px; color: #F02DCD;";
      note.textContent = "Live streams can't be downloaded";
      dropdown.appendChild(note);
      dropdown.style.display = "block";
      return;
    }
    const warning = degradedWarningTextFn(resolved);
    if (warning) {
      const note = document.createElement("p");
      note.style.cssText =
        "margin: 0; padding: 11px 14px; font-size: 12px; color: #F5C542; border-bottom: 1px solid rgba(255, 255, 255, 0.08);";
      note.textContent = warning;
      dropdown.appendChild(note);
    }
    const formats = filterResolvableFormatsFn(resolved.formats || []);
    if (formats.length === 0) {
      const note = document.createElement("p");
      note.style.cssText = "margin: 0; padding: 11px 14px; font-size: 13px; color: #F02DCD;";
      note.textContent = "No downloadable qualities were found for this video.";
      dropdown.appendChild(note);
      dropdown.style.display = "block";
      return;
    }
    formats.forEach((format) => {
      const row = document.createElement("button");
      row.type = "button";
      row._quality = true;
      Object.assign(row.style, {
        display: "flex",
        width: "100%",
        justifyContent: "space-between",
        alignItems: "center",
        padding: "11px 14px",
        border: "0",
        background: "none",
        fontFamily: "inherit",
        fontSize: "13.5px",
        color: "#ffffff",
        textAlign: "left",
        cursor: "pointer",
      });
      const label = document.createElement("span");
      label.textContent = qualityRowLabelFn(format);
      row.appendChild(label);
      const formatEl = document.createElement("span");
      formatEl.textContent = String((format.ext || "mp4")).toUpperCase();
      formatEl.style.cssText = "font-size: 12px; color: #8f9bbd;";
      row.appendChild(formatEl);
      row._label = label;
      row.addEventListener("mouseenter", () => {
        if (!row.disabled) row.style.background = ROW_HOVER_BG;
      });
      row.addEventListener("mouseleave", () => {
        row.style.background = "none";
      });
      row.addEventListener("click", () => pickResolvedFormat(video, format, entry, row, label));
      dropdown.appendChild(row);
    });
    dropdown.style.display = "block";
  }

  function startResolve(video, { withCookies, cookies, onResolved }) {
    const requestId =
      typeof crypto !== "undefined" && crypto.randomUUID
        ? crypto.randomUUID()
        : `resolve-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    let port;
    try {
      port = chrome.runtime.connect({ name: "sonic-resolve" });
    } catch {
      recordResolveFailure(video, { error: "Sonic Manager could not be reached.", needs_login: false }, withCookies, cookies);
      renderResolveError(video, "Sonic Manager could not be reached.");
      return;
    }
    if (!port || typeof port.postMessage !== "function") {
      recordResolveFailure(video, { error: "Sonic Manager could not be reached.", needs_login: false }, withCookies, cookies);
      renderResolveError(video, "Sonic Manager could not be reached.");
      return;
    }
    activeResolve = {
      requestId,
      port,
      video,
      pageUrl: pageUrlForResolve(video),
      withCookies: withCookies === true,
      cookies: cookies || [],
      onResolved: onResolved || null,
    };
    if (button) button.disabled = true;
    if (port.onMessage && typeof port.onMessage.addListener === "function") {
      port.onMessage.addListener((msg) => handleResolveMessage(msg, video));
    }
    if (port.onDisconnect && typeof port.onDisconnect.addListener === "function") {
      port.onDisconnect.addListener(() => {
        // The background went away mid-resolve: surface it,
        // unless we cancelled it ourselves (then there is
        // nothing to show — the dropdown is already closing).
        if (activeResolve && activeResolve.requestId === requestId) {
          activeResolve = null;
          if (button) button.disabled = false;
          renderResolveError(video, "Sonic Manager stopped answering.");
        }
      });
    }
    port.postMessage({
      type: "resolve_video",
      page_url: pageUrlForResolve(video),
      request_id: requestId,
      with_cookies: withCookies === true,
      cookies: cookies || [],
    });
  }

  function handleResolveMessage(msg, video) {
    if (!msg || !activeResolve) return;
    if (msg.request_id && msg.request_id !== activeResolve.requestId) return;
    if (msg.type === "resolve_progress") {
      updateResolveLoading(msg.elapsed_secs);
      return;
    }
    if (msg.type === "resolved") {
      const done = activeResolve;
      activeResolve = null;
      if (button) button.disabled = false;
      const entry = { resolved: msg, cookies: done.cookies || [] };
      resolveCache.set(normalizePageUrlFn(done.pageUrl || location.href), entry);
      if (done.onResolved) {
        done.onResolved(entry);
        return;
      }
      renderResolved(video, entry);
      return;
    }
    if (msg.type === "error") {
      const done = activeResolve;
      activeResolve = null;
      if (button) button.disabled = false;
      void handleResolveError(video, msg, done);
    }
  }

  // Round 79 — remember the last failed resolve attempt, so
  // "Try again" knows what failed and with which cookies.
  function recordResolveFailure(video, err, withCookies, cookies) {
    lastResolveAttempt = {
      video,
      error: err || null,
      withCookies: withCookies === true,
      cookies: Array.isArray(cookies) ? cookies : [],
    };
  }

  // The per-site cookie-consent flow, shared by the
  // needs_login error path and by "Try again": a stored grant
  // answers silently; a denial this session is never re-asked;
  // otherwise the user is asked once (confirm()) and a grant is
  // stored per site. Returns true when cookies may be used.
  async function askCookieConsent(host) {
    const key = `ytdlp_cookie_consent:${host}`;
    let granted = false;
    try {
      const stored = await chrome.storage.local.get([key]);
      granted = !!(stored && stored[key] === true);
    } catch {
      granted = false;
    }
    if (!granted && !deniedConsentHosts.has(host)) {
      if (typeof confirm === "function") {
        granted = confirm(
          "This video needs login. Allow Sonic Manager to use your browser login for this site to fetch its qualities?",
        );
      }
      if (granted) {
        try {
          await chrome.storage.local.set({ [key]: true });
        } catch {
          // Consent not persisted: the retry still runs once.
        }
      } else {
        deniedConsentHosts.add(host);
      }
    }
    return granted;
  }

  // Round 81 — the message for a resolve failure no retry
  // can fix in this session, or null when the failure is
  // retryable. A bot block is terminal even with a fresh
  // resolve (same connection, same refusal); a login wall
  // that already had the browser's cookies is terminal too —
  // signing in again cannot help, and "Try again" would only
  // repeat the identical request (Tom & Jerry, Round 81
  // reproduction: three identical failures in a row).
  function terminalResolveMessage(err, withCookies) {
    if (err && err.error_kind === "bot_blocked") {
      return err.error || "YouTube is blocking this video on this connection.";
    }
    // Round 82 — a removed video is terminal too: retrying it
    // can never succeed, and it is NOT a sign-in problem, so
    // the message says the video itself is gone — with no
    // "Try again" and no cookie-consent retry.
    if (err && err.error_kind === "video_unavailable") {
      return err.error || "This video is unavailable — it may have been removed.";
    }
    if (err && err.needs_login && withCookies) {
      return "You're already signed in, but this video is still blocked on this connection.";
    }
    return null;
  }

  // Round 79 — "Try again" after a failed resolve. It never
  // repeats the identical failing request: a needs_login
  // failure on a cookieless attempt runs the consent flow
  // instead (ask, unless denied this session; retry with
  // cookies if granted, else show the error again); anything
  // else retries with the SAME cookies the failed attempt used
  // (a cookied attempt that failed stays cookied).
  async function retryResolve(video) {
    const last = lastResolveAttempt;
    // Round 81 — a terminal failure re-renders its message
    // instead of starting a resolve: there is nothing new to
    // try in this session.
    if (last && last.video === video && last.error) {
      const terminal = terminalResolveMessage(last.error, last.withCookies);
      if (terminal !== null) {
        renderResolveError(video, terminal, false);
        return;
      }
    }
    if (last && last.video === video && last.error && last.error.needs_login && !last.withCookies) {
      const granted = await askCookieConsent(hostOf(location.href));
      if (granted) {
        const cookies = await requestCookies(location.href);
        startResolve(video, { withCookies: true, cookies, onResolved: null });
        return;
      }
      renderResolveError(
        video,
        (last.error && last.error.error) || "Sonic Manager could not fetch this video's qualities.",
      );
      return;
    }
    startResolve(video, {
      withCookies: !!(last && last.video === video && last.withCookies),
      cookies: last && last.video === video ? last.cookies || [] : [],
      onResolved: null,
    });
  }

  // Needs-login: ask once per site whether the browser login
  // may be used. A stored grant re-sends with cookies silently;
  // a denial is remembered for this session only, so the user
  // is not asked on every click. The retry carries the cookies
  // the background fetched for this page — and those same
  // cookies are attached to the video_found pick later.
  // Round 79 — this branch only runs for a COOKIELESS attempt
  // (done.withCookies is false): a cookied attempt that ALSO
  // fails with needs_login never reaches it — the cookies
  // didn't help, so the error is shown, with no loop and no
  // second consent ask. Round 81 — a cookieless bot block gets
  // the same one consent retry (cookies change the request);
  // a cookied bot block is terminal, like a cookied login
  // wall (see terminalResolveMessage).
  async function handleResolveError(video, err, done) {
    recordResolveFailure(video, err, done && done.withCookies, done && done.cookies);
    const cookied = !!(done && done.withCookies);
    if (err && (err.needs_login || err.error_kind === "bot_blocked") && !cookied) {
      const granted = await askCookieConsent(hostOf(location.href));
      if (granted) {
        const cookies = await requestCookies(location.href);
        renderResolveLoading(video);
        startResolve(video, {
          withCookies: true,
          cookies,
          onResolved: done.onResolved || null,
        });
        return;
      }
    }
    const terminal = terminalResolveMessage(err, cookied);
    if (terminal !== null) {
      renderResolveError(video, terminal, false);
      return;
    }
    renderResolveError(
      video,
      (err && err.error) || "Sonic Manager could not fetch this video's qualities.",
    );
  }

  // Content scripts cannot read chrome.cookies — the background
  // fetches the page's cookies for us. Only ever called after
  // the user granted the needs-login retry for this site.
  function requestCookies(url) {
    return new Promise((resolve) => {
      try {
        chrome.runtime.sendMessage({ type: "cookies_for", url }, (reply) => {
          resolve(reply && Array.isArray(reply.cookies) ? reply.cookies : []);
        });
      } catch {
        resolve([]);
      }
    });
  }

  // Picking a quality whose cached entry expired re-resolves
  // first, then sends the same quality from the fresh list —
  // the old URLs may be signed and dead by now.
  function pickResolvedFormat(video, format, entry, row, label) {
    const key = normalizePageUrlFn(pageUrlForResolve(video));
    if (resolveCache.isStale(key)) {
      renderResolveLoading(video);
      startResolve(video, {
        withCookies: !!(entry.cookies && entry.cookies.length > 0),
        cookies: entry.cookies || [],
        onResolved: (fresh) => {
          const freshFormat = (fresh.resolved.formats || []).find(
            (f) => f && f.format_id === format.format_id,
          );
          if (!freshFormat) {
            renderResolveError(video, "That quality is no longer available for this video.");
            return;
          }
          sendResolvedVariant(video, freshFormat, fresh, row, label);
        },
      });
      return;
    }
    sendResolvedVariant(video, format, entry, row, label);
  }

  function sendResolvedVariant(video, format, entry, row, label) {
    if (!buildResolvedVariantFn) {
      showToast("Sonic Manager couldn't take this video: the extension needs an update.");
      return;
    }
    // Round 81 — the variant's page URL is the video's own
    // page (a TikTok feed video's page, not the feed URL), so
    // a re-resolve after a stale cache hits the right video.
    const variant = buildResolvedVariantFn(format, entry.resolved, pageUrlForResolve(video));
    sendVariant(video, variant, row, label, entry.cookies || []);
  }

  function sendVariant(video, variant, row, label, cookies) {
    // The dropdown closes on pick (like the approved preview);
    // the toast confirms the outcome, and the row is restored
    // for a retry if the send failed. The send itself is
    // unchanged: video_found {page_url, variants: [chosen]} to
    // the background worker, which hands it to the desktop app.
    closeDropdown();
    const originalLabel = label.textContent;
    row.disabled = true;
    label.textContent = "Sending…";
    // Round 78: a resolved pick carries its resolve's cookies
    // (possibly []) as a top-level field; a direct pick sends
    // no cookies field at all, and the background attaches the
    // media URL's cookies as before.
    // Round 81 — the pick's page is the video's own page
    // (a TikTok feed video's page, not the feed URL): it is
    // the pending link's referrer, and a re-resolve must hit
    // the right video.
    const message = { type: "video_found", page_url: pageUrlForResolve(video), variants: [variant] };
    if (cookies !== undefined) message.cookies = cookies;
    chrome.runtime.sendMessage(
      message,
      (reply) => {
        if (reply && reply.ok !== false) {
          showToast("Sent to Sonic Manager — it will appear in the download list.");
        } else {
          // Round 81 — a failed pick must not fail silently:
          // the toast hides after 2.6 s and the Round 81
          // reproduction saw nothing on screen at all, so the
          // error is also re-rendered in the dropdown, where it
          // stays until the user closes it or retries.
          const error =
            reply && reply.error ? reply.error : "the desktop app did not answer";
          showToast(`Sonic Manager couldn't take this video: ${error}`);
          renderResolveError(video, `Sonic Manager couldn't take this video: ${error}`);
        }
        row.disabled = false;
        label.textContent = originalLabel;
      },
    );
  }

  // Round 73 — re-scan helpers. A rescan updates the map in
  // place (scanAndAttach is idempotent, button/dropdown are
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
    if (button && button.style.display !== "none") return;
    // Round 78 — a platform video with no direct source shows
    // the button too, even though it has no variants yet.
    for (const video of resolveOnly) {
      showButton(video);
      break;
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
  // dropdown). The button sits at the video's top-right INSIDE
  // its rect, so moving toward it keeps the button shown.
  // When the pointer is inside no video rect and not over the
  // button/dropdown, the button hides on the usual delay.
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
      (dropdown && atPoint.includes(dropdown)) ||
      (e.target === button) ||
      (e.target === dropdown)
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
    // The dropdown closes on an outside click or on Escape.
    document.addEventListener("click", onDocumentClick);
    document.addEventListener("keydown", onDocumentKeyDown);
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
        // Round 78 — attributes: ["src"] is the SPA backstop:
        // a player swapping its <video> src without navigating
        // still triggers a rescan, which drops the cached
        // qualities when the page URL moved with it.
        observer.observe(target, {
          childList: true,
          subtree: true,
          attributes: true,
          attributeFilter: ["src"],
        });
      }
    } catch {
      // No MutationObserver in this context: loadedmetadata and
      // on-demand rescans still cover videos added later.
    }
    // Round 78 — SPA navigation without a reload: wrap
    // pushState/replaceState and watch popstate, so the cached
    // qualities for the old video are dropped the moment the URL
    // changes, never offered for the new video.
    try {
      if (window.history && typeof window.history.pushState === "function") {
        const origPush = window.history.pushState.bind(window.history);
        const origReplace = window.history.replaceState.bind(window.history);
        window.history.pushState = (...args) => {
          const result = origPush(...args);
          notePageUrlChanged();
          return result;
        };
        window.history.replaceState = (...args) => {
          const result = origReplace(...args);
          notePageUrlChanged();
          return result;
        };
      }
    } catch {
      // History unwrappable here: rescans still catch the move.
    }
    window.addEventListener("popstate", () => notePageUrlChanged());
    for (const [video, variants] of byElement) {
      if (variants.length > 0) {
        showButton(video);
        break;
      }
    }
    // Round 78 — show the button immediately for the first
    // platform video with no direct source, too.
    if (!button || button.style.display === "none") {
      for (const video of resolveOnly) {
        showButton(video);
        break;
      }
    }
    // Keep the button (and an open dropdown) glued to the video
    // while the page scrolls.
    window.addEventListener("scroll", () => {
      if (button && button.style.display !== "none" && currentVideo) {
        positionButton(currentVideo);
        if (dropdown && dropdown.style.display !== "none") {
          positionDropdown(currentVideo);
        }
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
