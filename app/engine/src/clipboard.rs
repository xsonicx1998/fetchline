//! Clipboard monitoring (Round 52): offer to download a file link
//! the user copied. Pure logic only — no clipboard access here, so
//! tests never touch the real clipboard. The shell reads the OS
//! clipboard and hands the text to
//! `DownloadManager::check_clipboard_offer`; the offer the shell
//! shows is a `ClipboardOffer`. Copying a link never downloads
//! anything by itself — the user must accept the offer.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

/// The offer the shell shows when the clipboard holds a fresh
/// download link: the URL and the filename the link points to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardOffer {
    pub url: String,
    pub filename: String,
}

/// Extensions the browser extension catches
/// (`DEFAULT_CATCH_EXTENSIONS` in `app/extension/background.js`) —
/// the source of truth for what counts as a download link. The two
/// lists must stay in sync: a clipboard link the extension would
/// not catch is not offered here either, and vice versa.
pub const CAPTURE_EXTENSIONS: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "exe", "msi", "dmg", "iso", "apk", "pdf", "epub", "mp4",
    "mkv", "avi", "mov", "webm", "mp3", "flac", "wav", "aac", "ogg", "doc", "docx", "xls",
    "xlsx", "ppt", "pptx",
];

/// Maximum clipboard text considered: a longer paste is not one link.
pub const MAX_CLIPBOARD_TEXT_LEN: usize = 2048;

/// How many offered-or-dismissed URLs are remembered, so the same
/// link is never offered twice in a row.
pub const MAX_SEEN_URLS: usize = 64;

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
    })
}

/// Per-session clipboard state: the hash of the last text seen (so
/// re-reading the same clipboard never re-offers) and the URLs
/// already offered or dismissed (so a re-copied link is not offered
/// again). Pure logic — no clipboard access.
pub struct ClipboardMonitor {
    pub last_text_hash: u64,
    pub seen: VecDeque<String>,
}

impl Default for ClipboardMonitor {
    fn default() -> Self {
        Self {
            last_text_hash: 0,
            seen: VecDeque::new(),
        }
    }
}

impl ClipboardMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hash of the trimmed clipboard text, for the same-text dedupe.
    pub fn hash_text(text: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }

    /// Record `url` as offered-or-dismissed (idempotent), keeping at
    /// most 64 entries — the oldest is dropped when full.
    pub fn remember(&mut self, url: &str) {
        if !self.seen.iter().any(|u| u == url) {
            self.seen.push_back(url.to_string());
            while self.seen.len() > MAX_SEEN_URLS {
                self.seen.pop_front();
            }
        }
    }
}
