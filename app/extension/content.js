// Fetchline video panel (S08 / F05) — content script.
//
// Injected ONLY on a user gesture: the popup's "Find videos on this
// page" button (background.js, chrome.scripting + activeTab). It is
// never injected into every page — there is no content_scripts entry
// in manifest.json, no host permission and no <all_urls>.
//
// What it does on the page:
//  * scans the live DOM for <video> / <audio> elements and their
//    <source> children (video-detect.js, loaded first in the same
//    injection), using the real videoWidth/videoHeight for quality
//    and the playback state for live / DRM;
//  * shows a Fetchline-branded floating "Download this video" button
//    while the pointer is over a video player that has a
//    downloadable stream;
//  * opens a small panel listing that video's qualities/formats
//    (MP4 / WebM / FLV / MP3 where actually available). Picking one
//    sends video_found {page_url, variants: [chosen]} to the
//    background worker, which hands it to the desktop app over
//    Native Messaging. Only the chosen variant is sent — the desktop
//    app queues exactly what the user picked, not every quality.
//  * a DRM-protected video, or one with no downloadable stream,
//    shows an honest "can't be downloaded" note and NO download
//    button. A live stream's .m3u8 is offered as-is, labelled as a
//    playlist file, never as a merged video.
//
// The panel is fresh Fetchline design (violet #6d28d9, Inter) — no
// artwork or wording from any other download manager. It never
// reads cookies, never sends them, and never touches page content
// beyond the media elements it lists.

(function () {
  if (window.__fetchlineVideoInstalled) return;
  window.__fetchlineVideoInstalled = true;

  const ACCENT = "#6d28d9";
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
    b.textContent = "Download this video";
    b.setAttribute("aria-label", "Download this video with Fetchline");
    const icon = document.createElement("span");
    icon.innerHTML = DOWNLOAD_ICON;
    icon.style.display = "inline-flex";
    b.prepend(icon);
    Object.assign(b.style, {
      position: "fixed",
      zIndex: "2147483647",
      display: "inline-flex",
      alignItems: "center",
      gap: "6px",
      padding: "8px 12px",
      border: "0",
      borderRadius: "8px",
      fontFamily: "Inter, system-ui, sans-serif",
      fontSize: "14px",
      fontWeight: "600",
      lineHeight: "20px",
      color: "#ffffff",
      background: ACCENT,
      boxShadow: "0 4px 16px rgba(0,0,0,0.35)",
      cursor: "pointer",
    });
    b.addEventListener("mouseenter", () => clearTimeout(hideTimer));
    b.addEventListener("mouseleave", scheduleHide);
    b.addEventListener("click", () => showPanel(currentVideo));
    return b;
  }

  function positionButton(video) {
    const r = video.getBoundingClientRect();
    button.style.left = `${Math.max(8, r.left + 12)}px`;
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

  function showPanel(video) {
    const variants = byElement.get(video) || [];
    if (!panel) {
      panel = document.createElement("div");
      panel.setAttribute("role", "dialog");
      panel.setAttribute("aria-label", "Download this video with Fetchline");
      Object.assign(panel.style, {
        position: "fixed",
        zIndex: "2147483647",
        width: "300px",
        maxWidth: "calc(100vw - 16px)",
        padding: "16px",
        borderRadius: "12px",
        fontFamily: "Inter, system-ui, sans-serif",
        fontSize: "14px",
        lineHeight: "20px",
        color: "#171a21",
        background: "#ffffff",
        boxShadow: "0 8px 32px rgba(0,0,0,0.35)",
      });
      document.body.appendChild(panel);
    }
    const r = video.getBoundingClientRect();
    panel.style.left = `${Math.max(8, Math.min(window.innerWidth - 316, r.left + 12))}px`;
    panel.style.top = `${Math.max(8, Math.min(window.innerHeight - 220, r.top + 56))}px`;

    panel.innerHTML = "";
    const title = document.createElement("h2");
    title.textContent = "Download this video";
    Object.assign(title.style, { margin: "0 0 4px", fontSize: "16px", lineHeight: "24px" });
    panel.appendChild(title);
    const sub = document.createElement("p");
    sub.textContent = "Fetchline saves the video to your downloads. Only openly downloadable streams are offered.";
    Object.assign(sub.style, { margin: "0 0 12px", fontSize: "13px", color: "#5a6472" });
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
        borderBottom: "1px solid #eceef1",
      });
      const label = document.createElement("span");
      label.style.flex = "1";
      label.textContent = `${v.format} · ${v.quality}${v.live ? " · live stream" : ""}`;
      li.appendChild(label);
      if (v.note) {
        const note = document.createElement("span");
        note.style.cssText = "display: block; font-size: 12px; color: #5a6472;";
        note.textContent = v.note;
        label.appendChild(note);
      }
      if (v.drm) {
        const note = document.createElement("span");
        note.style.cssText = "font-size: 13px; color: #b42318;";
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
          background: ACCENT,
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
      note.style.cssText = "margin: 0 0 12px; font-size: 13px; color: #b42318;";
      note.textContent =
        "This video can't be downloaded by Fetchline — it is protected, or it has no downloadable stream.";
      panel.appendChild(note);
    } else if (variants.some((v) => v.live)) {
      const note = document.createElement("p");
      note.style.cssText = "margin: 0 0 12px; font-size: 13px; color: #5a6472;";
      note.textContent =
        "A live stream is saved as its playlist file — it is not merged into one video file.";
      panel.appendChild(note);
    } else if (variants.some((v) => v.format === "MPD")) {
      const note = document.createElement("p");
      note.style.cssText = "margin: 0 0 12px; font-size: 13px; color: #5a6472;";
      note.textContent =
        "A DASH manifest is saved as-is — its segments are not merged into one video file.";
      panel.appendChild(note);
    }

    const status = document.createElement("p");
    status.setAttribute("role", "status");
    status.style.cssText = "margin: 0 0 12px; font-size: 13px; color: #5a6472;";
    panel.appendChild(status);

    const close = document.createElement("button");
    close.type = "button";
    close.textContent = "Close";
    Object.assign(close.style, {
      padding: "8px 12px",
      border: "0",
      borderRadius: "8px",
      fontFamily: "inherit",
      fontSize: "14px",
      fontWeight: "600",
      color: "#171a21",
      background: "#eceef1",
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
            panel._status.textContent = "Sent to Fetchline — it will appear in the download list.";
          } else {
            panel._status.textContent = `Fetchline couldn't take this video: ${reply && reply.error ? reply.error : "the desktop app did not answer"}`;
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

  // Keep the button glued to the video while the page scrolls.
  window.addEventListener("scroll", () => {
    if (button && button.style.display !== "none" && currentVideo) {
      positionButton(currentVideo);
    }
  }, true);
})();
