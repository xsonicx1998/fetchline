//! Clipboard monitoring (Round 52, reworked Round 75): offer to
//! download a file link the user copied. Pure logic only — no
//! clipboard access here, so tests never touch the real clipboard.
//! The shell reads the OS clipboard and hands the text to
//! `DownloadManager::check_clipboard_offer` together with the
//! clipboard's change signal; the offer the shell shows is a
//! `ClipboardOffer`. Copying a link never downloads anything by
//! itself — the user must accept the offer.
//!
//! Round 75 — the change signal is the OS clipboard sequence
//! number (`GetClipboardSequenceNumber` on Windows): it increments
//! on every clipboard write, so a fresh copy of the same link is a
//! NEW sequence and is offered again, while the same clipboard
//! sitting unchanged across polls is offered exactly once. The old
//! design blocked re-offers three ways — the same-text hash, a
//! session-long `seen` set (remembered on offer AND on dismiss),
//! and the `url_exists` hard block — which is why clipboard
//! "worked once, then never again" on the user's PC. All three are
//! gone: the only dedupe left is the sequence number, and an
//! already-downloaded URL is offered with `already_in_list` set
//! instead of being swallowed.

use serde::{Deserialize, Serialize};

/// The offer the shell shows when the clipboard holds a fresh
/// download link: the URL, the filename the link points to, and
/// whether the URL is already in the download list (Round 75 —
///
/// an already-downloaded URL is still offered, with a visible
/// "This link is already in your list" note, instead of being
/// silently dropped).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardOffer {
    pub url: String,
    pub filename: String,
    pub already_in_list: bool,
}

/// Extensions the browser extension catches
/// (`DEFAULT_CATCH_EXTENSIONS` in `app/extension/background.js`) —
/// the source of truth for what counts as a download link. The two
/// lists must stay in sync: a clipboard link the extension would
/// not catch is not offered here either, and vice versa.
pub const CAPTURE_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "exe", "msi", "dmg", "iso", "apk", "pdf", "epub", "mp4",
    "mkv", "avi", "mov", "webm", "mp3", "flac", "wav", "aac", "ogg", "doc", "docx", "xls", "xlsx",
    "ppt", "pptx",
];

/// Maximum clipboard text considered: a longer paste is not one link.
pub const MAX_CLIPBOARD_TEXT_LEN: usize = 2048;

/// Parse clipboard text as a single download link, or `None` when it
/// is not one: empty text, text longer than 2048 chars, text with any
/// whitespace (a multi-line paste is not one link), a non-http(s)/ftp
/// scheme, an empty host, or a URL whose last path segment is empty
/// or has no capture extension. The filename is the percent-decoded
/// last path segment.
pub fn extract_download_link(text: &str) -> Option<ClipboardOffer> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_CLIPBOARD_TEXT_LEN {
        return None;
    }
    if trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let parsed = reqwest::Url::parse(trimmed).ok()?;
    if !matches!(parsed.scheme(), "http" | "https" | "ftp") {
        return None;
    }
    if parsed.host_str().map(str::is_empty).unwrap_or(true) {
        return None;
    }
    let filename = crate::manager::filename_from_url(trimmed)?;
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    if !CAPTURE_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    Some(ClipboardOffer {
        url: trimmed.to_string(),
        filename,
        // Round 75: set by `check_clipboard_offer` after the
        // `url_exists` lookup — a link is not known to be a
        // duplicate from its text alone.
        already_in_list: false,
    })
}

/// Per-session clipboard state: the last clipboard sequence number
/// seen (Round 75). `None` until the first check with a sequence.
/// Pure logic — no clipboard access.
pub struct ClipboardMonitor {
    pub last_sequence: Option<u32>,
}

impl Default for ClipboardMonitor {
    fn default() -> Self {
        Self {
            last_sequence: None,
        }
    }
}

impl ClipboardMonitor {
    pub fn new() -> Self {
        Self::default()
    }
}
