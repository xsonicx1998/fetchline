// Sonic Manager video panel (S08 / F05) — content script.
//
// Injected ONLY on a user gesture: the right-click menu's "Find
// videos on this page" item (background.js, chrome.scripting +
// activeTab). It is never injected into every page — there is no
// content_scripts entry in manifest.json, no host permission and
// no <all_urls>.
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
//    HD tag). Sizes are shown only when known — the DOM scan never
//    invents one. Picking one sends video_found {page_url,
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

  const media = scanLiveMedia(document, location.href);
  const byElement = new Map();

  // Group variants by their video element, so each floating button
  // offers only the qualities of the video it sits on.
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
    if (variants.length > 0) byElement.set(el, variants);
  });

  // Tell the popup (via the background worker) what this page holds.
  const downloadable = media.filter((v) => !v.drm).length;
  chrome.runtime.sendMessage({
    type: "videos_found",
    pageUrl: location.href,
    total: media.length,
    downloadable,
  });

  let button = null;
  let panel = null;
  let currentVideo = null;
  let hideTimer = null;

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
    if (!byElement.has(video)) return;
    if (!button) {
      button = makeButton();
      document.body.appendChild(button);
    }
    currentVideo = video;
    positionButton(video);
    button.style.display = "inline-flex";
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
      // Sizes are shown only when known — the DOM scan never
      // invents one, so there is no size here unless the variant
      // carries it.
      const tag = qualityTag(v.quality);
      if (tag) {
        const tagEl = document.createElement("span");
        tagEl.textContent = tag;
        tagEl.style.cssText = "font-size: 11px; font-weight: 700; color: #00C8FF;";
        label.appendChild(document.createTextNode(" "));
        label.appendChild(tagEl);
      }
      if (v.size) {
        const sizeEl = document.createElement("span");
        sizeEl.textContent = v.size;
        sizeEl.style.cssText = "font-size: 12px; color: #9AA6C7;";
        label.appendChild(document.createTextNode(" "));
        label.appendChild(sizeEl);
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
        btn.textContent = v.format === "MPD" ? "Download manifest" : variant.live ? "Download playlist" : "Download";
      },
    );
  }

  byElement.forEach((_variants, video) => {
    video.addEventListener("mouseenter", () => {
      clearTimeout(hideTimer);
      showButton(video);
    });
    video.addEventListener("mouseleave", scheduleHide);
  });

  // Round 71 (A4) — the button must appear even when the mouse
  // is ALREADY over the video when this script is injected (the
  // user clicks the context menu while hovering the player, so
  // no fresh mouseenter ever fires — the Round 70 retest hovered
  // and saw nothing). Two fixes:
  //   1. document-level mouseover delegation (mouseenter does
  //      not bubble, mouseover does): any video in the map shows
  //      the button when the pointer moves over it, including
  //      videos added after injection;
  //   2. show the button immediately for the first video with
  //      variants, so a video page shows it without any hover.
  // The button is NEVER gated on native-host health (a dead host
  // must not hide it — the user still sees the qualities, and
  // native messaging is re-attempted only when they click a
  // Download button, in sendVariant).
  document.addEventListener("mouseover", (e) => {
    const t = e && e.target;
    if (t && byElement.has(t)) {
      clearTimeout(hideTimer);
      showButton(t);
    }
  });
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
})();
