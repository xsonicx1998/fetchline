//! Native Messaging host protocol (S13 / F01, S08 / F05) — the one place the
//! framing lives, shared by the standalone host binary (`app/host/`)
//! and the engine tests.
//!
//! Wire format (Chrome / Edge / Firefox native messaging): each
//! message is a 4-byte little-endian length prefix followed by that
//! many bytes of UTF-8 JSON, on stdin for requests and stdout for
//! replies. One message is handled at a time; the host exits on a
//! clean EOF.
//!
//! Messages (see `replica/architecture.md`): `download_link`
//! `{url, referrer}`, `download_all` `{page_url, links[]}` and
//! `video_found` `{page_url, variants[]}`. Two additions the
//! architecture does not name: `ping` (the extension's connection
//! check) and `get_settings` (the extension reads the desktop
//! `browser_integration` flag and `excluded_sites` list, so the
//! desktop setting is the direction of truth). Round 88 adds
//! `relay_capture` (the extension's network-traffic handover:
//! `{url, finalUrl, page_url, referrer, content_type,
//! content_length, filename, kind, headers, cookies}` — see
//! `HostMessage::RelayCapture`).
//!
//! Privacy: the host never logs a message body. Cookies travel
//! only as name/value pairs for the handed-over URL (S13 finish),
//! are validated before they are stored on the pending row, and
//! are wiped from the row when the drain decides the link —
//! never logged, never written to settings, never kept past the
//! drain. Unknown fields in an incoming message are ignored.
//!
//! `video_found` (S08): each accepted variant is enqueued into the
//! SAME `pending_links` queue as S13, with source `"video"` and the
//! page URL as the referrer — there is no second queue. The desktop
//! app drains it through the real `add_download` path, so the
//! excluded-sites refusal, the `file_types_auto` auto-assign and the
//! duplicate-path guard all apply. Validation lives in
//! `crate::video::validate_video_variants`; this handler only
//! enqueues what validation accepted. A `video_found` that enqueues
//! nothing is an error, not `ok: true` (the BUG-012 rule).

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::error::{EngineError, Result};
use crate::store::{site_is_excluded, PendingLinkRow, Store};
use crate::video::{validate_video_variants, VideoVariantInput};

/// One quality/format of a video found on a page (S08 / F05).
/// Re-exported from `crate::video` so there is one definition.
pub type VideoVariant = VideoVariantInput;

/// One cookie for a handed-over URL (S13 finish): the browser's
/// session cookie for that URL, as a name/value pair — plus, since
/// Round 87, the cookie's own domain/path when the browser
/// reported them (background.js `cookiesFor`), so the scoped
/// writer can keep a CDN-host cookie on its own host instead of
/// rewriting every cookie to `.{page host}` + `/`. `domain` /
/// `path` are `None` for old stored rows and for pairs rebuilt
/// from a `Cookie` header string, which carries no scope — the
/// `#[serde(default)]` keeps those parsing. Never logged, never
/// written to settings, wiped from the row when the drain decides
/// the link.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CookiePair {
    pub name: String,
    pub value: String,
    /// The cookie's own domain as the browser reported it
    /// (".facebook.com" for a domain cookie,
    /// "video-ord5-2.xx.fbcdn.net" for a host-only one).
    #[serde(default)]
    pub domain: Option<String>,
    /// The cookie's own path ("/" or a deeper path).
    #[serde(default)]
    pub path: Option<String>,
}

/// Most cookies the extension can send for one URL, and the most
/// a `Cookie` header may hold. The caps bound the pending row and
/// the header; a page with more cookies than this is not a login,
/// it is a tracking pile, and the drain refuses it rather than
/// truncating it silently.
pub const MAX_COOKIES_PER_LINK: usize = 32;
pub const MAX_COOKIE_HEADER_BYTES: usize = 8192;

/// Build the `Cookie` header value for a set of pairs, validating
/// each one first. A name may not be empty and may not contain
/// whitespace, a control character, `;`, `,` or `=` (any of those
/// would smuggle a second cookie — or a second header — into the
/// value); a value may not contain `;`, `,` or a control
/// character. An invalid pair refuses the whole hand-off, so a bad
/// cookie can never corrupt a good one.
pub fn cookies_to_header(pairs: &[CookiePair]) -> Result<String> {
    if pairs.len() > MAX_COOKIES_PER_LINK {
        return Err(EngineError::InvalidState(format!(
            "Too many cookies for one link ({} > {MAX_COOKIES_PER_LINK})",
            pairs.len()
        )));
    }
    let mut parts = Vec::with_capacity(pairs.len());
    for pair in pairs {
        if pair.name.is_empty()
            || pair.name.len() > 128
            || pair
                .name
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || matches!(c, ';' | ',' | '='))
        {
            return Err(EngineError::InvalidState(format!(
                "Invalid cookie name: {:?}",
                pair.name
            )));
        }
        if pair.value.len() > 4096
            || pair
                .value
                .chars()
                .any(|c| c.is_control() || matches!(c, ';' | ','))
        {
            return Err(EngineError::InvalidState(format!(
                "Invalid cookie value for {:?}",
                pair.name
            )));
        }
        parts.push(format!("{}={}", pair.name, pair.value));
    }
    let header = parts.join("; ");
    if header.len() > MAX_COOKIE_HEADER_BYTES {
        return Err(EngineError::InvalidState(format!(
            "Cookies for one link are too large ({} bytes > {MAX_COOKIE_HEADER_BYTES})",
            header.len()
        )));
    }
    Ok(header)
}

/// Serialise validated pairs for the pending row. The row holds
/// them only until the drain decides the link
/// (`mark_pending_link` wipes the column), and the desktop app
/// keeps the header in memory only — cookies are never stored
/// past the drain.
pub fn cookies_to_json(pairs: &[CookiePair]) -> Result<String> {
    let _ = cookies_to_header(pairs)?;
    serde_json::to_string(pairs).map_err(EngineError::Json)
}

/// Parse a stored cookies column back into pairs. A corrupt value
/// (which enqueueing can never write — it validates first) reads
/// as no cookies rather than failing the drain for a column the
/// download does not need to exist.
pub fn cookies_from_json(json: &str) -> Vec<CookiePair> {
    serde_json::from_str(json).unwrap_or_default()
}

/// Largest single native message accepted, in bytes. A
/// `download_all` with hundreds of links is still far below this;
/// anything larger is a malformed or hostile sender, not a page.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// The host part of a URL, lowercased, or `None` when it does not parse.
pub fn host_of_url(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

/// One message from the extension. Unknown fields are ignored —
/// never stored, never logged. A `cookies` field on
/// `download_link` / `video_found` is NOT unknown: it carries the
/// browser's cookies for that URL (see `CookiePair`), validated
/// and stored on the pending row until the drain.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    Ping,
    GetSettings,
    DownloadLink {
        url: String,
        #[serde(default)]
        referrer: Option<String>,
        /// The browser's cookies for this URL, if any (S13 finish).
        /// Validated and stored on the pending row until the drain;
        /// never logged, never stored past the drain.
        #[serde(default)]
        cookies: Vec<CookiePair>,
    },
    DownloadAll {
        page_url: String,
        #[serde(default)]
        links: Vec<String>,
    },
    VideoFound {
        page_url: String,
        #[serde(default)]
        variants: Vec<VideoVariant>,
        /// The browser's cookies for the chosen variant's URL, if
        /// any (S13 finish) — same rules as `DownloadLink`.
        #[serde(default)]
        cookies: Vec<CookiePair>,
    },
    /// Round 78 — the extension asks the HOST to resolve a
    /// platform video page with yt-dlp (the extension cannot
    /// reach the engine). The host runs the resolver itself and
    /// replies with a trimmed format list — never raw `-J`
    /// (Chrome native-messaging messages are ~1 MB). Handled by
    /// the host binary's async loop, NOT by the sync
    /// `handle_host_message` (a resolve takes up to ~30 s).
    ResolveVideo {
        page_url: String,
        request_id: String,
        #[serde(default)]
        with_cookies: bool,
        #[serde(default)]
        cookies: Vec<CookiePair>,
        #[serde(default)]
        extractor_args: Option<String>,
    },
    /// Round 88 — the relay: the extension watched the
    /// browser's network traffic and hands over a request the
    /// user never clicked (an inline video, an HLS/DASH
    /// manifest, a file served inline). The shape is plan
    /// §4.4: `url` is the FIRST url, before redirects — stored
    /// as `source_url` for future renewal of an expired signed
    /// link, NEVER as the download target; `final_url` is what
    /// the engine probes and downloads. `headers` is the
    /// browser's sent-headers map (UA / Referer /
    /// Accept-Language / Origin / Sec-Fetch-* / Sec-CH-UA*) —
    /// `Cookie` is NEVER in it (cookies ride the validated
    /// `cookies` array only). `kind` is "media" | "manifest" |
    /// "file" — parsed but not persisted (the pending row has
    /// no kind column); the confirm path distinguishes a
    /// relayed manifest by its `.m3u8` / `.mpd` URL instead.
    RelayCapture {
        url: String,
        #[serde(default, rename = "finalUrl")]
        final_url: Option<String>,
        #[serde(default)]
        page_url: Option<String>,
        #[serde(default)]
        referrer: Option<String>,
        #[serde(default)]
        content_type: Option<String>,
        #[serde(default)]
        content_length: Option<i64>,
        #[serde(default)]
        filename: Option<String>,
        #[serde(default)]
        kind: Option<String>,
        /// The browser's sent headers for this request, as a
        /// string map. `Cookie` keys are stripped at the
        /// boundary before anything is stored (defence in
        /// depth — the extension already strips them).
        #[serde(default)]
        headers: std::collections::HashMap<String, String>,
        /// The browser's cookies for the destination URL, if
        /// any — same rules as `DownloadLink`.
        #[serde(default)]
        cookies: Vec<CookiePair>,
    },
}

/// One reply to the extension. `ok` is false with `error` set for
/// every refusal — the extension shows the error instead of
/// guessing. `browser_integration` / `excluded_sites` are only set
/// on a `get_settings` reply. `pending_count` (Milestone 2) is the
/// number of links still waiting in the `pending_links` queue — the
/// popup shows it, so the user can see what the browser handed over
/// that the desktop app has not drained yet. `app_alive` (Round 75)
/// is only set on a `ping` reply: whether the desktop app itself is
/// running (the named mutex), as opposed to just this standalone
/// host answering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostReply {
    pub ok: bool,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_link_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enqueued: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_integration: Option<bool>,
    // Round 88.3 — the relay master switch, read from the same
    // settings row. The host NEVER sent this before (the
    // GetSettings handler set browser_integration and
    // excluded_sites only), so the extension's
    // `reply.relay_enabled !== false` always read `undefined`
    // and the capture gate was stuck ON — the app's relay
    // checkbox could never gate capture (only the engine-side
    // click-time check in enqueue_relay enforced it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded_sites: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_alive: Option<bool>,
    /// Round 78 — `resolve_video` reply fields: `request_id`
    /// echoes the request; `title` / `is_live` / `degraded` /
    /// `warning` describe the resolved video; `formats` is the
    /// TRIMMED quality list (one row per height, 360p and up —
    /// never raw `-J`); `needs_login` + `error_kind` are set on
    /// an error reply (see `YtDlpErrorKind::kind_str`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_live: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formats: Option<Vec<crate::ytdlp::QualityRow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs_login: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
}

/// Round 75 — is the desktop app alive? The app creates a NAMED
/// MUTEX at the very start of `run()` and holds it for the process
/// lifetime; this host (a separate process the extension spawns)
/// probes it here. The old popup check pinged the host directly,
/// but the host is standalone and answered even with the app
/// closed, so the popup said "Connected" forever. A named mutex is
/// released by the OS the instant the app dies — even on a crash —
/// so this is exact, with no polling and no disk I/O. Off Windows
/// there is no named mutex: `None`, and the popup treats "unknown"
/// as connected when the host answers (the old behaviour — the app
/// only ships on Windows).
pub const APP_ALIVE_MUTEX_NAME: &str = "Local\\FetchlineAppRunning";

/// A held app-alive mutex. Dropping it closes the handle, and when
/// it is the last handle the OS destroys the mutex — which is
/// exactly what happens when the app crashes. The app holds one in
/// Tauri managed state for the process lifetime; tests hold one to
/// simulate a running app. A second launch opening the same named
/// mutex gets its own handle to the SAME mutex object — closing it
/// on exit does not disturb the primary instance's handle.
pub struct AppAliveMutexGuard(#[cfg(windows)] windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for AppAliveMutexGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

// Round 75b — the VPS build failed with E0277: `app.manage(guard)`
// (lib.rs:3207) requires `T: Send + Sync + 'static`, but the guard
// wraps a raw windows-sys HANDLE (`*mut c_void`), which is neither.
// SAFETY: the guard is just an owned Windows HANDLE — a process-wide
// kernel-object value. Moving it between threads is safe, and sharing
// &references is safe because the only thing that touches the handle
// is Drop, which needs &mut self. The guard is never cloned or
// copied, so CloseHandle runs exactly once.
#[cfg(windows)]
unsafe impl Send for AppAliveMutexGuard {}
#[cfg(windows)]
unsafe impl Sync for AppAliveMutexGuard {}

/// Create (or open, when the app is already running) the
/// app-alive mutex and hold it. `None` when the mutex cannot be
/// created, or off Windows (no named mutex there).
pub fn create_app_alive_mutex() -> Option<AppAliveMutexGuard> {
    #[cfg(windows)]
    {
        let name = mutex_name_wide();
        let handle = unsafe {
            windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, name.as_ptr())
        };
        if handle.is_null() {
            None
        } else {
            Some(AppAliveMutexGuard(handle))
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Whether the desktop app is alive right now: `Some(true)` when
/// the mutex opens, `Some(false)` when it does not exist, `None`
/// off Windows.
pub fn app_is_alive() -> Option<bool> {
    #[cfg(windows)]
    {
        // SYNCHRONIZE (0x00100000) — the only right needed to
        // test for the mutex's existence.
        const SYNCHRONIZE: u32 = 0x0010_0000;
        let name = mutex_name_wide();
        let handle = unsafe {
            windows_sys::Win32::System::Threading::OpenMutexW(SYNCHRONIZE, 0, name.as_ptr())
        };
        if handle.is_null() {
            Some(false)
        } else {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(handle);
            }
            Some(true)
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The mutex name as a null-terminated UTF-16 string for the
/// Win32 calls. Built from the one `APP_ALIVE_MUTEX_NAME` const
/// so create and probe can never drift apart.
#[cfg(windows)]
fn mutex_name_wide() -> Vec<u16> {
    APP_ALIVE_MUTEX_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

impl HostReply {
    fn ok(kind: &str) -> Self {
        Self {
            ok: true,
            kind: kind.to_string(),
            message: None,
            error: None,
            pending_link_id: None,
            enqueued: None,
            browser_integration: None,
            relay_enabled: None,
            excluded_sites: None,
            pending_count: None,
            app_alive: None,
            request_id: None,
            title: None,
            is_live: None,
            degraded: None,
            warning: None,
            formats: None,
            needs_login: None,
            error_kind: None,
        }
    }

    fn err(kind: &str, error: String) -> Self {
        Self {
            ok: false,
            kind: kind.to_string(),
            message: None,
            error: Some(error),
            pending_link_id: None,
            enqueued: None,
            browser_integration: None,
            relay_enabled: None,
            excluded_sites: None,
            pending_count: None,
            app_alive: None,
            request_id: None,
            title: None,
            is_live: None,
            degraded: None,
            warning: None,
            formats: None,
            needs_login: None,
            error_kind: None,
        }
    }

    /// Attach the current pending-links count to a reply, so the
    /// extension can show how many links are waiting for a drain.
    /// A store error leaves the field unset rather than failing the
    /// reply — the count is informational, never the verdict.
    fn with_pending_count(mut self, store: &Store) -> Self {
        if let Ok(rows) = store.list_pending_links(Some("pending")) {
            self.pending_count = Some(rows.len());
        }
        self
    }
}

/// Encode one message: 4-byte little-endian length + JSON bytes.
pub fn encode_message(value: &serde_json::Value) -> Vec<u8> {
    let payload = serde_json::to_vec(value).expect("a JSON value always serialises");
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

/// Read one length-prefixed JSON message. Returns `Ok(None)` on a
/// clean EOF (the extension closed the channel); any short read,
/// oversized length or bad JSON is an error.
pub fn decode_message<R: Read>(reader: &mut R) -> Result<Option<serde_json::Value>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(EngineError::Io(e)),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_MESSAGE_BYTES {
        return Err(EngineError::InvalidState(format!(
            "Native message is too large ({len} bytes, maximum {MAX_MESSAGE_BYTES})"
        )));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).map_err(EngineError::Io)?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(EngineError::Json)
}

/// Write one length-prefixed JSON message and flush, so the
/// extension gets the reply at once instead of on process exit.
pub fn write_message<W: Write>(writer: &mut W, value: &serde_json::Value) -> Result<()> {
    writer.write_all(&encode_message(value))?;
    writer.flush()?;
    Ok(())
}

/// Handle one extension message against the store. This is the
/// host boundary: it enforces the desktop `browser_integration`
/// flag and the `excluded_sites` list before anything is enqueued,
/// and it never enqueues a link the store itself would refuse.
/// `download_all` with links enqueues each link (a bad link is
/// skipped and counted in the reply's error text, not fatal); an
/// empty `links` list is refused here — the host binary expands it.
pub fn handle_host_message(store: &Store, msg: &HostMessage) -> HostReply {
    match msg {
        HostMessage::Ping => {
            // Round 75 — the ping reply carries whether the APP is
            // alive (named mutex), not just whether this host
            // answered: the popup shows Connected only on
            // app_alive, and "Sonic Manager is closed" otherwise.
            let mut reply = HostReply::ok("pong");
            reply.app_alive = app_is_alive();
            reply
        }
        HostMessage::GetSettings => {
            let settings = store.get_settings().unwrap_or_default();
            let mut reply = HostReply::ok("settings");
            reply.browser_integration = Some(settings.browser_integration);
            // Round 88.3 — the relay switch rides the same reply
            // (it never did before: the extension's capture gate
            // read `reply.relay_enabled`, which was always
            // undefined, so the gate was stuck ON).
            reply.relay_enabled = Some(settings.relay_enabled);
            reply.excluded_sites = Some(settings.excluded_sites);
            reply.with_pending_count(store)
        }
        HostMessage::DownloadLink {
            url,
            referrer,
            cookies,
        } => match enqueue_one(store, url, referrer.as_deref(), "browser", cookies) {
            Ok(row) => {
                let mut reply = HostReply::ok("queued");
                reply.pending_link_id = Some(row.id);
                reply.message = Some("Link queued — Fetchline will pick it up".to_string());
                reply.with_pending_count(store)
            }
            Err(e) => HostReply::err("error", e.to_string()),
        },
        HostMessage::RelayCapture {
            url,
            final_url,
            page_url,
            referrer,
            content_type,
            content_length,
            filename,
            kind,
            headers,
            cookies,
        } => match enqueue_relay(
            store,
            url,
            final_url.as_deref(),
            page_url.as_deref().or(referrer.as_deref()),
            kind.as_deref(),
            content_type.as_deref(),
            *content_length,
            filename.as_deref(),
            headers,
            cookies,
        ) {
            Ok(row) => {
                let mut reply = HostReply::ok("queued");
                reply.pending_link_id = Some(row.id);
                reply.message = Some("Link queued — Fetchline will pick it up".to_string());
                reply.with_pending_count(store)
            }
            Err(e) => HostReply::err("error", e.to_string()),
        },
        HostMessage::DownloadAll { page_url, links } => {
            if links.is_empty() {
                return HostReply::err(
                    "error",
                    "No links were sent with this page — the host expands the page itself"
                        .to_string(),
                );
            }
            let mut enqueued = 0usize;
            let mut skipped: Vec<String> = Vec::new();
            for url in links {
                // A download_all carries no cookies: the extension
                // sends no links of its own (the host expands the
                // page), and the expansion cannot use them.
                match enqueue_one(store, url, Some(page_url), "download_all", &[]) {
                    Ok(_) => enqueued += 1,
                    Err(e) => skipped.push(format!("{url}: {e}")),
                }
            }
            // BUG-012: when nothing was queued, the reply must be an
            // error, not ok:true with the refusal buried in a
            // message — the extension treats ok:true as success and
            // the user is never told nothing was queued.
            if enqueued == 0 && !skipped.is_empty() {
                return HostReply::err(
                    "error",
                    format!("No links could be queued: {}", skipped.join("; ")),
                );
            }
            let mut reply = HostReply::ok("queued");
            reply.enqueued = Some(enqueued);
            if !skipped.is_empty() {
                reply.message = Some(format!(
                    "Queued {enqueued} link(s); skipped {}: {}",
                    skipped.len(),
                    skipped.join("; ")
                ));
            }
            reply.with_pending_count(store)
        }
        HostMessage::VideoFound {
            page_url,
            variants,
            cookies,
        } => {
            let settings = store.get_settings().unwrap_or_default();
            let (accepted, mut skipped) =
                match validate_video_variants(page_url, variants, &settings) {
                    Ok(v) => v,
                    Err(e) => return HostReply::err("error", e.to_string()),
                };
            // Enqueue each accepted variant into the SAME pending_links
            // queue S13 uses (source "video", page URL as referrer).
            // The drain puts it through the real add_download path,
            // so the duplicate-path guard (BUG-002) applies there.
            // Round 78: a resolved variant carries the resolver's
            // metadata and goes through `enqueue_resolved`, so the
            // row stores it for the Add URL dialog.
            let mut enqueued = 0usize;
            for variant in &accepted {
                let queued = if variant.resolved {
                    enqueue_resolved(store, variant, page_url, cookies)
                } else {
                    enqueue_one(store, &variant.url, Some(page_url), "video", cookies)
                };
                match queued {
                    Ok(_) => enqueued += 1,
                    Err(e) => skipped.push(format!("{}: {e}", variant.url)),
                }
            }
            // BUG-012 rule: a message that queues nothing is an error,
            // not ok:true with the refusal buried in a message.
            if enqueued == 0 {
                return HostReply::err(
                    "error",
                    format!("No video could be queued: {}", skipped.join("; ")),
                );
            }
            let mut reply = HostReply::ok("queued");
            reply.enqueued = Some(enqueued);
            if !skipped.is_empty() {
                reply.message = Some(format!(
                    "Queued {enqueued} video(s); skipped {}: {}",
                    skipped.len(),
                    skipped.join("; ")
                ));
            } else {
                reply.message = Some(format!(
                    "Queued {enqueued} video(s) — Fetchline will pick them up"
                ));
            }
            reply.with_pending_count(store)
        }
        HostMessage::ResolveVideo { .. } => {
            // Round 78 — a resolve runs yt-dlp for up to ~30 s
            // with a heartbeat, so it lives in the host binary's
            // async loop, not in this synchronous handler.
            HostReply::err(
                "error",
                "Video resolving needs the host's async loop — this message cannot be handled synchronously".to_string(),
            )
        }
    }
}

/// Enqueue one link, after the host-boundary checks: the desktop
/// integration flag must be on, and the link's site must not be
/// excluded. The store re-checks exclusion on enqueue, so a caller
/// that skips this function still cannot enqueue an excluded link.
/// Cookies are validated (a bad pair refuses the hand-off) and
/// stored on the row as JSON until the drain decides the link.
fn enqueue_one(
    store: &Store,
    url: &str,
    referrer: Option<&str>,
    source: &str,
    cookies: &[CookiePair],
) -> Result<PendingLinkRow> {
    let settings = store.get_settings().unwrap_or_default();
    if !settings.browser_integration {
        return Err(EngineError::InvalidState(
            "Browser integration is turned off in Fetchline settings".to_string(),
        ));
    }
    if let Some(host) = host_of_url(url) {
        if site_is_excluded(&host, &settings.excluded_sites) {
            return Err(EngineError::InvalidState(format!(
                "This site is excluded in Settings (excluded sites): {host}"
            )));
        }
    }
    let cookies_json = if cookies.is_empty() {
        None
    } else {
        Some(cookies_to_json(cookies)?)
    };
    store.enqueue_pending_link_with_cookies(url, referrer, source, cookies_json.as_deref())
}

/// Round 88.6 — host-side mirror of the extension's
/// `relayIsFileAttachment` (background.js, Round 88.5).
/// `filename` is the capture's filename (the disposition name
/// when the server sent one — the extension's `relayFilename`
/// returns it first — else the URL basename), `content_type`
/// the response type (parameters, if a stale worker sent them,
/// are stripped here), `url` the download target. A named file
/// is a file whatever the server claims its type is (a
/// mislabeled PDF is still a PDF); an unnamed JSON/text
/// attachment, and the canonical "f.txt" name, never are. The
/// extension list is `CAPTURE_EXTENSIONS` (clipboard.rs), the
/// documented mirror of the extension's
/// `DEFAULT_CATCH_EXTENSIONS` — the two stay in sync there.
fn relay_is_file_attachment(filename: Option<&str>, content_type: Option<&str>, url: &str) -> bool {
    let name = filename.map(str::trim).filter(|s| !s.is_empty());
    let ext = name
        .and_then(|n| n.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()))
        .filter(|e| !e.is_empty())
        .or_else(|| crate::video::extension_of_url(url));
    if let Some(ext) = ext {
        if crate::clipboard::CAPTURE_EXTENSIONS.contains(&ext.as_str()) {
            return true;
        }
    }
    let content_type = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if content_type == "application/json" || content_type.starts_with("text/") {
        return false;
    }
    if let Some(name) = name {
        if name.eq_ignore_ascii_case("f.txt") {
            return false;
        }
    }
    true
}

/// Round 88 — enqueue one relay capture, after the same
/// host-boundary checks as `DownloadLink` (integration flag,
/// excluded site, cookie validation) plus the relay master
/// switch (`relay_enabled`). The engine downloads `final_url`;
/// `url` (the first URL, before redirects) is stored strictly
/// as `source_url` for future renewal of an expired signed
/// link — never as the download target. `kind`, when present,
/// must be one of the three the extension sends ("media" |
/// "manifest" | "file"); it is validated here but not stored
/// (the pending row has no kind column — the confirm path
/// distinguishes a relayed manifest by its `.m3u8` / `.mpd`
/// URL). `Cookie` keys are stripped from the headers map
/// before anything is stored (defence in depth — the extension
/// already strips them; the engine must not trust that).
/// Headers are stored on the row's existing `headers` column
/// as a JSON map, the same column
/// `enqueue_pending_link_resolved` writes.
/// Round 88.6 — a kind=="file" capture must also pass the
/// `relay_is_file_attachment` rule above: the extension
/// classifies attachments, but a STALE worker (an unpacked
/// extension does not reload when the files on disk change —
/// the Round 88.5 VPS retest's f.txt storm was captured by the
/// resident 88.4 worker before the explicit Reload) can still
/// send a "file" capture the current gate would block, and
/// the host is the last line of defence — refused here, no
/// pending row and no Add URL dialog is ever created for it.
/// Media/manifest captures reach the host only through an
/// explicit user click (`relaySendBuffered`), which a
/// heuristic must never block, so the rule gates "file" only.
fn enqueue_relay(
    store: &Store,
    url: &str,
    final_url: Option<&str>,
    referrer: Option<&str>,
    kind: Option<&str>,
    content_type: Option<&str>,
    content_length: Option<i64>,
    filename: Option<&str>,
    headers: &std::collections::HashMap<String, String>,
    cookies: &[CookiePair],
) -> Result<PendingLinkRow> {
    let settings = store.get_settings().unwrap_or_default();
    if !settings.browser_integration {
        return Err(EngineError::InvalidState(
            "Browser integration is turned off in Fetchline settings".to_string(),
        ));
    }
    if !settings.relay_enabled {
        return Err(EngineError::InvalidState(
            "Relay capture is turned off in Fetchline settings".to_string(),
        ));
    }
    if let Some(kind) = kind.map(str::trim).filter(|k| !k.is_empty()) {
        if !matches!(kind, "media" | "manifest" | "file") {
            return Err(EngineError::InvalidState(format!(
                "Unknown relay capture kind: {kind}"
            )));
        }
    }
    // The engine probes and downloads `final_url`; with no
    // redirect the first URL IS the final one.
    let target = final_url
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or(url);
    if let Some(host) = host_of_url(target) {
        if site_is_excluded(&host, &settings.excluded_sites) {
            return Err(EngineError::InvalidState(format!(
                "This site is excluded in Settings (excluded sites): {host}"
            )));
        }
    }
    // Round 88.6 — the host-side file-attachment gate (see
    // `relay_is_file_attachment` above): a kind=="file" capture
    // that is not a file by this rule is refused before any row
    // is created, so a stale worker can never storm the Add URL
    // dialog with API answers dressed as attachments.
    if kind.map(str::trim) == Some("file")
        && !relay_is_file_attachment(filename, content_type, target)
    {
        return Err(EngineError::InvalidState(
            "Relay capture is not a downloadable file (API answer served as an attachment)"
                .to_string(),
        ));
    }
    let cookies_json = if cookies.is_empty() {
        None
    } else {
        Some(cookies_to_json(cookies)?)
    };
    let mut map = serde_json::Map::new();
    for (k, v) in headers {
        if k.eq_ignore_ascii_case("cookie") {
            continue;
        }
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        map.insert(k.clone(), serde_json::Value::String(v.to_string()));
    }
    let headers_json = if map.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&map).map_err(EngineError::Json)?)
    };
    store.enqueue_pending_link_relay(
        target,
        referrer,
        cookies_json.as_deref(),
        &crate::store::RelayLink {
            filename: filename
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            headers: headers_json,
            source_url: Some(url.trim().to_string()),
            filesize: content_length.filter(|s| *s > 0).map(|s| s as u64),
        },
    )
}

/// Round 87.5 (E4) — the filename a resolved video variant is
/// stored under: the variant's own filename when it already
/// carries an extension, else the sanitized title with the
/// declared format appended (`"My Title"` + `"MP4"` →
/// `"My Title.mp4"`); a missing filename with a declared format
/// becomes `"video.<ext>"`. Without a declared format the
/// filename is left exactly as-is — there is no type signal to
/// recover, and inventing one would miscategorise the download.
fn resolved_filename_with_format(
    filename: &Option<String>,
    format: Option<&str>,
) -> Option<String> {
    let name = filename.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let format = format.map(str::trim).filter(|s| !s.is_empty());
    match (name, format) {
        (Some(name), Some(format)) => {
            let has_ext = std::path::Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| !e.is_empty())
                .unwrap_or(false);
            if has_ext {
                Some(name.to_string())
            } else {
                Some(crate::ytdlp::sanitize_title_filename(name, format))
            }
        }
        (None, Some(format)) => Some(crate::ytdlp::sanitize_title_filename("", format)),
        (name, _) => name.map(str::to_string),
    }
}

/// Enqueue one resolved platform video (Round 78): the variant
/// carries the resolver's metadata (filename, headers, source
/// URL, format id, DASH audio URL, fragmented flag) and the row
/// stores it via `enqueue_pending_link_resolved`, so the Add URL
/// dialog and `confirm_pending_link` can use it without a second
/// resolve. The same host-boundary checks as `enqueue_one` apply
/// (integration flag, platform-downloads flag, excluded site,
/// cookie validation).
pub fn enqueue_resolved(
    store: &Store,
    variant: &VideoVariant,
    page_url: &str,
    cookies: &[CookiePair],
) -> Result<PendingLinkRow> {
    let settings = store.get_settings().unwrap_or_default();
    if !settings.browser_integration {
        return Err(EngineError::InvalidState(
            "Browser integration is turned off in Fetchline settings".to_string(),
        ));
    }
    if !settings.platform_downloads_enabled {
        return Err(EngineError::InvalidState(
            "Platform downloads are turned off in Fetchline settings".to_string(),
        ));
    }
    if let Some(host) = host_of_url(&variant.url) {
        if site_is_excluded(&host, &settings.excluded_sites) {
            return Err(EngineError::InvalidState(format!(
                "This site is excluded in Settings (excluded sites): {host}"
            )));
        }
    }
    let cookies_json = if cookies.is_empty() {
        None
    } else {
        Some(cookies_to_json(cookies)?)
    };
    // The headers map was written by the resolver with Cookie
    // keys already stripped; strip them again here (defence in
    // depth — headers must never carry a session past the cookie
    // validation).
    let headers = variant.headers.as_deref().and_then(|json| {
        let mut map: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(json).ok()?;
        map.retain(|k, _| !k.eq_ignore_ascii_case("cookie"));
        serde_json::to_string(&map).ok()
    });
    store.enqueue_pending_link_resolved(
        &variant.url,
        Some(page_url),
        cookies_json.as_deref(),
        &crate::store::ResolvedLink {
            // Round 87.5 (E4) — a resolved variant whose filename
            // carries no extension keeps the declared format as
            // its only type signal: the extension normally
            // appends it, but a bare title would otherwise save
            // extension-less AND land in Documents, since the
            // category pick reads the filename first. Rebuild
            // the name here, at the boundary where the format is
            // still known — the pending row stores no format
            // column, so past this point the signal is gone.
            filename: resolved_filename_with_format(&variant.filename, variant.format.as_deref()),
            headers,
            source_url: variant.source_url.clone(),
            format_id: variant.format_id.clone(),
            audio_url: variant.audio_url.clone(),
            fragmented: variant.fragmented,
            filesize: variant.filesize,
        },
    )
}
