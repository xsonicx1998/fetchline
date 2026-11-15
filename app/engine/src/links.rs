//! Link extraction for Download All (S07) and shared batch helpers (S10).
//!
//! A page is fetched as plain HTML and its downloadable links are
//! extracted from `<a href>`, `<link href>`, `<img src>`, `<video src>`,
//! `<audio src>` and `<source src>` attributes. Only openly downloadable
//! http(s) links are returned — `javascript:`, `mailto:`, `tel:`, `data:`
//! and fragment-only links are skipped, and relative links are resolved
//! against the page URL. Nothing here loads the original application's
//! servers; the page is whatever URL the user asked for.

use serde::{Deserialize, Serialize};

/// File type of a link, used for the Download All type filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    Video,
    Audio,
    Image,
    Document,
    Archive,
    Program,
    Other,
}

impl LinkKind {
    pub fn label(self) -> &'static str {
        match self {
            LinkKind::Video => "Video",
            LinkKind::Audio => "Audio",
            LinkKind::Image => "Image",
            LinkKind::Document => "Document",
            LinkKind::Archive => "Archive",
            LinkKind::Program => "Program",
            LinkKind::Other => "Other",
        }
    }
}

/// One link found on a page, with what the probe learned about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageLink {
    /// Resolved absolute URL.
    pub url: String,
    /// File name from the URL path, for the checklist.
    pub filename: String,
    /// File type, from the extension.
    pub kind: LinkKind,
    /// Size in bytes, when the probe learned it.
    pub size: Option<u64>,
    /// Probe failure, when the link could not be checked (e.g. 404).
    /// Such links are listed but excluded from the selection.
    pub error: Option<String>,
    /// A live (not deleted) download with this URL already exists.
    /// Flagged in the checklist and skipped unless the user opts in.
    pub already_exists: bool,
}

/// Classify a file by its extension. Unknown / missing extensions are
/// "Other" — they can still be downloaded, they just have no filter group.
pub fn classify_kind(filename: &str) -> LinkKind {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    // A name with no dot (or a leading-dot name) has no extension.
    let ext = if filename.contains('.') { ext.as_str() } else { "" };
    match ext {
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "mpg" | "mpeg" | "m4v"
        | "m3u8" | "mpd" => LinkKind::Video,
        "mp3" | "flac" | "wav" | "aac" | "ogg" | "m4a" | "wma" => LinkKind::Audio,
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "ico" | "avif" => {
            LinkKind::Image
        }
        "pdf" | "doc" | "docx" | "txt" | "epub" | "md" | "rtf" | "xls" | "xlsx" | "ppt"
        | "pptx" | "csv" => LinkKind::Document,
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" => LinkKind::Archive,
        "exe" | "msi" | "dmg" | "apk" | "deb" | "rpm" => LinkKind::Program,
        _ => LinkKind::Other,
    }
}

/// File name for a link URL: the last path segment, percent-decoded.
/// A bare directory URL (or one with no usable segment) is "download".
pub fn filename_from_link_url(url: &str) -> String {
    let parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return "download".to_string(),
    };
    let last = parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or("");
    let name = percent_decode(last);
    if name.trim().is_empty() {
        "download".to_string()
    } else {
        name
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Extract downloadable links from `html`, resolved against `base_url`.
/// De-duplicated, in first-seen order. The page's own URL is excluded:
/// a page linking to itself is navigation, not a download.
pub fn extract_links(html: &str, base_url: &str) -> Vec<String> {
    let base = match reqwest::Url::parse(base_url) {
        Ok(u) => u,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (tag, attr) in scan_link_attrs(html) {
        let is_href = attr.0 == "href" && matches!(tag.as_str(), "a" | "link");
        let is_src = attr.0 == "src"
            && matches!(tag.as_str(), "img" | "video" | "audio" | "source");
        if !is_href && !is_src {
            continue;
        }
        let raw = attr.1.trim();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        if lower.starts_with("javascript:")
            || lower.starts_with("mailto:")
            || lower.starts_with("tel:")
            || lower.starts_with("data:")
        {
            continue;
        }
        let Ok(resolved) = base.join(raw) else {
            continue;
        };
        if resolved.scheme() != "http" && resolved.scheme() != "https" {
            continue;
        }
        // Drop the fragment: the same file with different anchors is
        // one download, and the fragment is never sent to the server.
        let mut resolved = resolved;
        resolved.set_fragment(None);
        let url = resolved.to_string();
        if url == base_url || seen.contains(&url) {
            continue;
        }
        seen.insert(url.clone());
        out.push(url);
    }
    out
}

/// Scan `<tag attr="value" ...>` pairs. A small hand-rolled scanner —
/// no HTML parser dependency — that only needs tag names and the
/// `href` / `src` attributes. Attribute values may be double-quoted,
/// single-quoted or unquoted.
fn scan_link_attrs(html: &str) -> Vec<(String, (String, String))> {
    let bytes = html.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Find the next '<' that starts a tag (not a comment/doctype).
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        if j < bytes.len() && (bytes[j] == b'!' || bytes[j] == b'/' || bytes[j] == b'?') {
            i = j;
            continue;
        }
        let name_start = j;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric()) {
            j += 1;
        }
        if j == name_start {
            i += 1;
            continue;
        }
        let tag = html[name_start..j].to_ascii_lowercase();
        // Scan attributes until '>'.
        while j < bytes.len() && bytes[j] != b'>' {
            // Skip whitespace and '/'.
            while j < bytes.len() && (bytes[j].is_ascii_whitespace() || bytes[j] == b'/') {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] == b'>' {
                break;
            }
            let attr_start = j;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'-' || bytes[j] == b'_')
            {
                j += 1;
            }
            if j == attr_start {
                j += 1;
                continue;
            }
            let attr = html[attr_start..j].to_ascii_lowercase();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'=' {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= bytes.len() {
                    break;
                }
                let value = if bytes[j] == b'"' || bytes[j] == b'\'' {
                    let quote = bytes[j];
                    j += 1;
                    let vstart = j;
                    while j < bytes.len() && bytes[j] != quote {
                        j += 1;
                    }
                    let v = html[vstart..j].to_string();
                    if j < bytes.len() {
                        j += 1;
                    }
                    v
                } else {
                    let vstart = j;
                    while j < bytes.len()
                        && !bytes[j].is_ascii_whitespace()
                        && bytes[j] != b'>'
                    {
                        j += 1;
                    }
                    html[vstart..j].to_string()
                };
                if attr == "href" || attr == "src" {
                    out.push((tag.clone(), (attr, value)));
                }
            }
        }
        i = if j < bytes.len() { j + 1 } else { j };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_known_extensions() {
        assert_eq!(classify_kind("movie.mp4"), LinkKind::Video);
        assert_eq!(classify_kind("song.MP3"), LinkKind::Audio);
        assert_eq!(classify_kind("photo.jpeg"), LinkKind::Image);
        assert_eq!(classify_kind("report.pdf"), LinkKind::Document);
        assert_eq!(classify_kind("bundle.tar.gz"), LinkKind::Archive);
        assert_eq!(classify_kind("setup.exe"), LinkKind::Program);
        assert_eq!(classify_kind("noext"), LinkKind::Other);
        assert_eq!(classify_kind("page.html"), LinkKind::Other);
    }

    #[test]
    fn extract_skips_non_downloadable_and_dedups() {
        let html = "<html><body>\
            <a href=\"video.mp4\">v</a><a href=\"video.mp4\">v2</a>\
            <a href=\"javascript:void(0)\">x</a><a href=\"mailto:a@b.c\">m</a>\
            <a href=\"#top\">t</a><img src=\"/pic.png\">\
            <video src=\"clip.webm\"></video>\
            <audio><source src=\"song.mp3\"></audio>\
            <link href=\"style.css\">\
        </body></html>";
        let links = extract_links(html, "http://127.0.0.1/page.html");
        assert_eq!(
            links,
            vec![
                "http://127.0.0.1/video.mp4",
                "http://127.0.0.1/pic.png",
                "http://127.0.0.1/clip.webm",
                "http://127.0.0.1/song.mp3",
                "http://127.0.0.1/style.css",
            ]
        );
    }
}
