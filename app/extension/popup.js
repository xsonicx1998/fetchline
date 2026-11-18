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

function setStatus(state, text) {
  statusTextEl.textContent = text;
  statusEl.className = `status status-${state}`;
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

async function refreshStatus() {
  try {
    const reply = await sendToHost({ type: "ping" });
    if (reply && reply.ok !== false) {
      setStatus("ok", "Connected");
    } else {
      setStatus("bad", "Not connected");
    }
  } catch {
    setStatus("bad", "Not connected");
  }
}

document.addEventListener("DOMContentLoaded", () => {
  void refreshStatus();
});
