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
//! desktop setting is the direction of truth).
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
/// session cookie for that URL, as a name/value pair only — no
/// domain, path, expiry or flags travel, because the `Cookie`
/// header needs nothing else and the host must never learn more
/// than it needs. Sent by the extension (background.js
/// `cookiesFor`), carried on the pending row until the drain, and
/// attached to that download's probe and segment requests for that
/// host only. Never logged, never written to settings, wiped from
/// the row when the drain decides the link.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CookiePair {
    pub name: String,
    pub value: String,
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
}

/// One reply to the extension. `ok` is false with `error` set for
/// every refusal — the extension shows the error instead of
/// guessing. `browser_integration` / `excluded_sites` are only set
/// on a `get_settings` reply. `pending_count` (Milestone 2) is the
/// number of links still waiting in the `pending_links` queue — the
/// popup shows it, so the user can see what the browser handed over
/// that the desktop app has not drained yet.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded_sites: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_count: Option<usize>,
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
            excluded_sites: None,
            pending_count: None,
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
            excluded_sites: None,
            pending_count: None,
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
    serde_json::from_slice(&payload).map(Some).map_err(EngineError::Json)
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
        HostMessage::Ping => HostReply::ok("pong"),
        HostMessage::GetSettings => {
            let settings = store.get_settings().unwrap_or_default();
            let mut reply = HostReply::ok("settings");
            reply.browser_integration = Some(settings.browser_integration);
            reply.excluded_sites = Some(settings.excluded_sites);
            reply.with_pending_count(store)
        }
        HostMessage::DownloadLink {
            url,
            referrer,
            cookies,
        } => {
            match enqueue_one(store, url, referrer.as_deref(), "browser", cookies) {
                Ok(row) => {
                    let mut reply = HostReply::ok("queued");
                    reply.pending_link_id = Some(row.id);
                    reply.message = Some("Link queued — Fetchline will pick it up".to_string());
                    reply.with_pending_count(store)
                }
                Err(e) => HostReply::err("error", e.to_string()),
            }
        }
        HostMessage::DownloadAll { page_url, links } => {
            if links.is_empty() {
                return HostReply::err(
                    "error",
                    "No links were sent with this page — the host expands the page itself".to_string(),
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
            let mut enqueued = 0usize;
            for variant in &accepted {
                match enqueue_one(store, &variant.url, Some(page_url), "video", cookies) {
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
