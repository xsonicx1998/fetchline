//! Video grabber (S08 / F05) — detection and validation for openly
//! downloadable streams only.
//!
//! Two halves, both pure (no network, no database):
//!
//! * Detection — `detect_videos_in_html` scans a page's HTML for
//!   `<video>` / `<audio>` / `<source>` elements and directly linked
//!   media files, and derives an honest quality label for each one.
//!   The browser extension runs the same logic in JavaScript
//!   (`app/extension/video-detect.js`) against the live DOM, where it
//!   can also read the real `videoWidth` / `videoHeight` and the
//!   playback state (live / DRM). A static scan can do neither, so it
//!   never claims a resolution it did not see.
//! * Validation — `validate_video_variants` is the host boundary for
//!   `video_found` messages: http(s) URLs only, known media
//!   extensions, de-duplicated, excluded-site and integration-flag
//!   checks, DRM variants refused, HLS playlists accepted as-is.
//!
//! Quality is NEVER invented. It comes from, in order: the element's
//! real dimensions, a resolution label in the URL or file name
//! (`720p`, `1080p`, `4K`), or — when neither exists — the literal
//! string "Unknown quality".
//!
//! Out of scope, by design (see `replica/recon.md`): DRM decryption
//! or bypass, downloading an HLS/DASH segmented stream as a single
//! file, signature deciphering, and any login/cookie forwarding.
//! An `.m3u8` playlist is offered as-is with a note saying exactly
//! that: the file saved is the playlist, not a merged video.
//!
//! HLS decision (Milestone 3, 2026-11-15 — decided, not deferred):
//! playlists stay as-is, option (b). Merging HLS segments into one
//! MP4 would need a remuxer: concatenating MPEG-TS segments gives a
//! `.ts` file, not an MP4, and a real TS→MP4 remux needs ffmpeg or
//! an equivalent muxer, which is not part of the Windows desktop
//! app and cannot be assumed on a user's machine (ffmpeg exists on
//! this Linux VM, but that proves nothing about Windows, and
//! bundling it is a packaging/licensing decision outside S08).
//! Segment URLs are also short-lived signed URLs, live streams have
//! no end to merge, and AES-128-encrypted streams cannot be merged
//! without their keys — which Fetchline must not fetch. So instead
//! of a broken merge, S08 now *understands* playlists: a master
//! playlist is expanded into its real variants (resolution and
//! bandwidth read from the playlist itself, never invented), a
//! media playlist reports its segment count and duration and
//! whether it is live, and an encrypted playlist is honestly not
//! downloadable. A DASH `.mpd` manifest is offered the same way:
//! as-is, labelled a manifest, never merged.

use serde::{Deserialize, Serialize};

use crate::links::{classify_kind, extract_links, filename_from_link_url, LinkKind};
use crate::store::Settings;

/// The label used when no honest quality can be derived. It is a
/// fact about the data, not a guess — callers show it as-is.
pub const UNKNOWN_QUALITY: &str = "Unknown quality";

/// Media extensions the grabber accepts. MP4 / WebM / FLV / MP3 are
/// the formats in `replica/features.csv`; the rest are the same
/// openly downloadable container/audio formats the download engine
/// already handles. `.m3u8` (HLS) is accepted as a playlist file,
/// never merged — see the module docs. `.mpd` (DASH) is accepted the
/// same way: a manifest file, saved as-is, never merged.
pub const KNOWN_MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "webm", "flv", "mp3", "m3u8", "mpd", "mkv", "mov", "avi", "m4v", "m4a", "wav", "ogg",
    "flac", "aac",
];

/// The most variants of one master playlist the desktop scan
/// expands into separate entries. A master with more renditions
/// than this is capped — the first entries in playlist order —
/// rather than flooding the panel with dozens of rows.
pub const MAX_HLS_VARIANTS: usize = 8;

/// A playlist fetched over HTTP is at most this large. A playlist
/// is text (a few KiB at most); anything bigger is not a playlist
/// Fetchline should try to parse.
pub const MAX_PLAYLIST_BYTES: usize = 256 * 1024;

/// One video (or audio track) found on a page, as the desktop S08
/// panel shows it. `live` is true for a stream whose duration is
/// unbounded (an `.m3u8` playlist with no end, or a live element);
/// `downloadable` is false when the panel knows the item cannot be
/// downloaded (DRM, or a probe failure) — the panel then shows the
/// honest "not downloadable" state instead of a download button.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedVideo {
    pub url: String,
    pub filename: String,
    pub format: String,
    pub quality: String,
    pub size: Option<u64>,
    pub error: Option<String>,
    pub live: bool,
    pub downloadable: bool,
    /// Honest note shown next to the item, e.g. that a playlist is
    /// saved as-is and never merged. `None` for plain files.
    /// `#[serde(default)]` so older JSON without the field still
    /// deserialises.
    #[serde(default)]
    pub note: Option<String>,
}

/// The file extension of a URL's path (lowercase, without the dot),
/// or `None` when the path has no extension. Query strings and
/// fragments are ignored — `clip.mp4?token=x` is still `mp4`.
pub fn extension_of_url(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let path = parsed.path();
    let last = path.rsplit('/').next().unwrap_or("");
    let dot = last.rfind('.')?;
    let ext = &last[dot + 1..];
    if ext.is_empty() || ext.contains('/') {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// True when the URL points at a media file the grabber accepts.
pub fn is_media_url(url: &str) -> bool {
    extension_of_url(url)
        .map(|e| KNOWN_MEDIA_EXTENSIONS.contains(&e.as_str()))
        .unwrap_or(false)
}

/// The format label for a media URL: its extension, uppercased
/// (`mp4` -> `MP4`), or `None` when it is not a media URL.
pub fn format_of_url(url: &str) -> Option<String> {
    extension_of_url(url)
        .filter(|e| KNOWN_MEDIA_EXTENSIONS.contains(&e.as_str()))
        .map(|e| e.to_ascii_uppercase())
}

/// Quality from real playback dimensions. Both dimensions give
/// `1280 × 720`; a height alone gives `720p`. `None` when there is
/// nothing to derive from — the caller must not invent a label.
pub fn quality_from_dimensions(width: u32, height: u32) -> Option<String> {
    match (width, height) {
        (0, 0) => None,
        (0, h) => Some(format!("{h}p")),
        (w, 0) => Some(format!("{w} wide")),
        (w, h) => Some(format!("{w} × {h}")),
    }
}

/// Quality from a resolution label in a URL or file name:
/// `…/clip-720p.mp4`, `…/1080p/stream.m3u8`, `…/trailer-4k.webm`.
/// Returns the label as written (`720p`, `4K`), never a resolution
/// the label does not state. `None` when no label is present.
pub fn quality_from_label(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    // Longest first, so "1080p" wins over a stray "80p"-style match.
    for label in ["2160p", "1440p", "1080p", "720p", "480p", "360p", "240p"] {
        if contains_label(&lower, label) {
            return Some(label.to_string());
        }
    }
    if contains_label(&lower, "4k") {
        return Some("4K".to_string());
    }
    if contains_label(&lower, "8k") {
        return Some("8K".to_string());
    }
    None
}

/// True when `label` appears in `text` bounded by non-alphanumeric
/// characters (or the ends of the string), so `720p` matches in
/// `clip-720p.mp4` but not inside `17209.mp4`.
fn contains_label(text: &str, label: &str) -> bool {
    let mut start = 0;
    while let Some(at) = text[start..].find(label) {
        let i = start + at;
        let before_ok = i == 0 || !text.as_bytes()[i - 1].is_ascii_alphanumeric();
        let after = i + label.len();
        let after_ok = after >= text.len() || !text.as_bytes()[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = i + 1;
    }
    false
}

/// The honest quality label for one variant: real dimensions first,
/// then a label in the URL or file name, then "Unknown quality".
/// The dimensions are `0` when the source did not report them.
pub fn quality_for(url: &str, width: u32, height: u32) -> String {
    if let Some(q) = quality_from_dimensions(width, height) {
        return q;
    }
    if let Some(q) = quality_from_label(url) {
        return q;
    }
    UNKNOWN_QUALITY.to_string()
}

/// The honest note for a playlist/manifest item, shown in the
/// panel next to the format and quality. It always says the file is
/// saved as-is and never merged — the HLS decision (see module
/// docs) — with the playlist facts appended when known.
pub fn playlist_note(format: &str, live: bool) -> String {
    if format == "MPD" {
        "DASH manifest — saved as-is; its segments are not merged into one video file.".to_string()
    } else if live {
        "Live stream — the playlist file is saved as-is; it is not merged into one video file."
            .to_string()
    } else {
        "Playlist file — saved as-is; it is not merged into one video file.".to_string()
    }
}

/// One rendition listed in an HLS master playlist
/// (`#EXT-X-STREAM-INF` + the URI on the next line). Resolution and
/// bandwidth come from the playlist itself; a variant with no
/// `RESOLUTION` has `resolution: None` and its quality is honestly
/// "Unknown quality" — never guessed from its bandwidth.
#[derive(Debug, Clone, PartialEq)]
pub struct HlsVariant {
    pub uri: String,
    pub resolution: Option<(u32, u32)>,
    pub bandwidth: Option<u64>,
}

/// What an `.m3u8` playlist actually is, parsed from its text.
/// `is_master` is true when the playlist lists variant streams;
/// a media playlist lists segments instead. `live` is true when a
/// media playlist has no `#EXT-X-ENDLIST` (the stream has no end);
/// a master playlist is never marked live here — liveness is a
/// property of the variant media playlist, checked per variant.
/// `encrypted` is true when an `#EXT-X-KEY` with a method other
/// than `NONE` is present: the segments need a key Fetchline will
/// not fetch, so the playlist alone will not play.
#[derive(Debug, Clone, PartialEq)]
pub struct HlsPlaylist {
    pub is_master: bool,
    pub variants: Vec<HlsVariant>,
    pub segments: usize,
    pub duration_secs: f64,
    pub live: bool,
    pub encrypted: bool,
}

/// Parse an `.m3u8` playlist. Returns `None` when the text is not
/// a playlist at all (it must start with `#EXTM3U`) — callers keep
/// the item as a plain playlist file instead of failing the scan.
/// Attribute values are read from the `#EXT-X-STREAM-INF` /
/// `#EXT-X-KEY` lines only; segment durations come from `#EXTINF`.
/// Nothing here is guessed: a missing `RESOLUTION` stays missing,
/// a missing `#EXT-X-ENDLIST` means live, and a duration that does
/// not parse is skipped, not invented.
pub fn parse_m3u8(text: &str) -> Option<HlsPlaylist> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next()?;
    if first != "#EXTM3U" {
        return None;
    }
    let mut variants: Vec<HlsVariant> = Vec::new();
    let mut segments = 0usize;
    let mut duration_secs = 0f64;
    let mut ended = false;
    let mut encrypted = false;
    let mut pending_stream_inf: Option<(Option<(u32, u32)>, Option<u64>)> = None;
    for line in lines {
        if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let mut resolution = None;
            let mut bandwidth = None;
            for attr in rest.split(',') {
                let (k, v) = match attr.split_once('=') {
                    Some((k, v)) => (k.trim(), v.trim().trim_matches('"')),
                    None => continue,
                };
                if k.eq_ignore_ascii_case("RESOLUTION") {
                    if let Some((w, h)) = v.split_once('x') {
                        if let (Ok(w), Ok(h)) = (w.trim().parse::<u32>(), h.trim().parse::<u32>()) {
                            resolution = Some((w, h));
                        }
                    }
                } else if k.eq_ignore_ascii_case("BANDWIDTH") {
                    bandwidth = v.parse::<u64>().ok();
                }
            }
            pending_stream_inf = Some((resolution, bandwidth));
            continue;
        }
        if line.starts_with("#EXT-X-KEY:") {
            // METHOD=NONE means the segments are not encrypted.
            // Strip the tag name first: the first attribute is
            // "METHOD=...", not "#EXT-X-KEY:METHOD=...".
            let method = line["#EXT-X-KEY:".len()..]
                .split(',')
                .find_map(|a| {
                    a.split_once('=')
                        .filter(|(k, _)| k.trim().eq_ignore_ascii_case("METHOD"))
                })
                .map(|(_, v)| v.trim().trim_matches('"').to_ascii_uppercase());
            if matches!(method.as_deref(), Some(m) if m != "NONE") {
                encrypted = true;
            }
            pending_stream_inf = None;
            continue;
        }
        if line == "#EXT-X-ENDLIST" {
            ended = true;
            pending_stream_inf = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let num = rest.split(',').next().unwrap_or("").trim();
            if let Ok(d) = num.parse::<f64>() {
                segments += 1;
                duration_secs += d;
            }
            pending_stream_inf = None;
            continue;
        }
        if line.starts_with('#') {
            pending_stream_inf = None;
            continue;
        }
        // A plain line: the URI of the pending variant stream, or a
        // media segment (already counted by its #EXTINF line).
        if let Some((resolution, bandwidth)) = pending_stream_inf.take() {
            variants.push(HlsVariant {
                uri: line.to_string(),
                resolution,
                bandwidth,
            });
        }
    }
    let is_master = !variants.is_empty();
    Some(HlsPlaylist {
        is_master,
        variants,
        segments,
        duration_secs,
        // A master playlist has no segments and no ENDLIST; it is
        // not itself a live stream — its variants are checked.
        live: !is_master && !ended,
        encrypted,
    })
}

/// Quality from a playlist variant's real resolution, or
/// "Unknown quality" when the playlist states none.
pub fn quality_from_resolution(resolution: Option<(u32, u32)>) -> String {
    match resolution {
        Some((w, h)) => {
            quality_from_dimensions(w, h).unwrap_or_else(|| UNKNOWN_QUALITY.to_string())
        }
        None => UNKNOWN_QUALITY.to_string(),
    }
}

/// Scan a page's HTML for downloadable media: `<video src>`,
/// `<audio src>`, `<source src>` and directly linked media files
/// (`<a href="…mp4">`). De-duplicated in first-seen order, resolved
/// against the page URL. A static scan cannot see playback state, so
/// `live` is set only for `.m3u8` playlists (which are streams by
/// construction) and `downloadable` is true for every item found —
/// DRM and login walls can only be discovered by probing, which the
/// caller does before offering the item.
pub fn detect_videos_in_html(html: &str, base_url: &str) -> Vec<DetectedVideo> {
    let mut out: Vec<DetectedVideo> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for url in extract_links(html, base_url) {
        let Some(format) = format_of_url(&url) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        let filename = filename_from_link_url(&url);
        let kind = classify_kind(&filename);
        if !matches!(kind, LinkKind::Video | LinkKind::Audio) {
            continue;
        }
        let live = format == "M3U8";
        let note = if format == "M3U8" || format == "MPD" {
            Some(playlist_note(&format, live))
        } else {
            None
        };
        out.push(DetectedVideo {
            quality: quality_for(&url, 0, 0),
            url,
            filename,
            format,
            size: None,
            error: None,
            live,
            downloadable: true,
            note,
        });
    }
    out
}

/// One quality/format of a video, as sent by the extension in a
/// `video_found` message. `live` marks a live stream (unbounded
/// duration); `drm` marks a DRM-protected element (`mediaKeys` set
/// on the element, or an encrypted-media event fired). `note` is
/// filled in by validation — the sender does not set it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoVariantInput {
    pub url: String,
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub drm: bool,
    #[serde(default)]
    pub note: Option<String>,
    /// Round 78 — a resolved platform video: the extension/host
    /// already ran yt-dlp, so this variant carries the resolver's
    /// metadata instead of a plain page-scraped URL. `filename`
    /// is the sanitized video title (never videoplayback.mp4);
    /// `headers` is a JSON map of User-Agent/Referer (Cookie
    /// keys stripped); `audio_url` is the DASH pair's audio
    /// stream; `source_url` the page URL (for re-resolve);
    /// `format_id` the yt-dlp format id; `fragmented` marks a
    /// (B)-fallback download (yt-dlp downloads it itself). All
    /// `#[serde(default)]` so old extension payloads (which
    /// never send them) still deserialise.
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub headers: Option<String>,
    #[serde(default)]
    pub audio_url: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub format_id: Option<String>,
    #[serde(default)]
    pub fragmented: bool,
}

/// Validate one `video_found` message against the desktop settings.
/// Returns `(accepted, skipped)`: the variants that may be queued,
/// and one human-readable reason per variant that may not. A
/// message-level problem (integration off, excluded page, no
/// variants at all) is an `Err` instead — nothing is queued then.
///
/// Rules, in order per variant:
/// * http(s) URL only — anything else is skipped, never queued.
/// * a known media extension — anything else is skipped.
/// * DRM-protected variants are refused, never queued or worked
///   around: "protected (DRM)".
/// * duplicates (same URL twice) are skipped after the first.
/// * a variant whose site is excluded is skipped.
/// * a variant whose extension is not in a non-empty
///   `file_types_auto` list is skipped — the list is the user's
///   auto-download allow-list (BUG-011 semantics), so a variant
///   outside it is not grabbed silently.
/// * an `.m3u8` playlist is accepted as-is, with a note saying the
///   file saved is the playlist, not a merged video; a live stream
///   gets the same treatment with a live note.
///
/// Quality is re-derived here, never trusted blindly: a variant
/// that claims a quality gets its claim kept only when it matches
/// a real label or a dimension-derived value — anything else, and
/// a missing quality, becomes "Unknown quality". (The extension
/// already derives it honestly; this is the host-side backstop, so
/// a compromised or buggy extension cannot make the desktop app
/// promise a quality it never saw.)
pub fn validate_video_variants(
    page_url: &str,
    variants: &[VideoVariantInput],
    settings: &Settings,
) -> Result<(Vec<VideoVariantInput>, Vec<String>), crate::error::EngineError> {
    use crate::error::EngineError;
    use crate::store::site_is_excluded;

    if !settings.browser_integration {
        return Err(EngineError::InvalidState(
            "Browser integration is turned off in Fetchline settings".to_string(),
        ));
    }
    let parsed_page =
        reqwest::Url::parse(page_url).map_err(|e| EngineError::InvalidUrl(e.to_string()))?;
    if !matches!(parsed_page.scheme(), "http" | "https") {
        return Err(EngineError::InvalidUrl(format!(
            "Only http:// and https:// pages can send videos, not {}://",
            parsed_page.scheme()
        )));
    }
    if let Some(host) = parsed_page.host_str().map(|h| h.to_ascii_lowercase()) {
        if site_is_excluded(&host, &settings.excluded_sites) {
            return Err(EngineError::InvalidState(format!(
                "This site is excluded in Settings (excluded sites): {host}"
            )));
        }
    }
    if variants.is_empty() {
        return Err(EngineError::InvalidState(
            "No video variants were sent — nothing to download".to_string(),
        ));
    }

    let mut accepted: Vec<VideoVariantInput> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for v in variants {
        let url = v.url.trim();
        let parsed = match reqwest::Url::parse(url) {
            Ok(u) => u,
            Err(e) => {
                skipped.push(format!("{url}: not a valid link ({e})"));
                continue;
            }
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            skipped.push(format!(
                "{url}: only http:// and https:// links can be downloaded, not {}://",
                parsed.scheme()
            ));
            continue;
        }
        // Round 78 — a resolved platform video carries no file
        // extension (googlevideo URLs have none) and its filename
        // comes from the sanitized title, so the extension check
        // is skipped for it. The http(s) check above, and the
        // DRM/exclusion/dedup checks around it, still apply — a
        // resolved variant is not trusted blindly. The
        // file-types allow-list below uses the variant's declared
        // format instead of a URL extension.
        let ext: Option<String> = if v.resolved {
            None
        } else {
            match extension_of_url(url) {
                Some(ext) => Some(ext),
                None => {
                    skipped.push(format!("{url}: no media file extension in the link"));
                    continue;
                }
            }
        };
        if let Some(ext) = &ext {
            if !KNOWN_MEDIA_EXTENSIONS.contains(&ext.as_str()) {
                skipped.push(format!(
                    "{url}: .{ext} is not a downloadable video or audio format"
                ));
                continue;
            }
        }
        if v.drm {
            skipped.push(format!(
                "{url}: this video is protected (DRM) — Fetchline cannot download protected streams"
            ));
            continue;
        }
        if !seen.insert(url.to_string()) {
            skipped.push(format!("{url}: duplicate of a variant already listed"));
            continue;
        }
        if let Some(host) = parsed.host_str().map(|h| h.to_ascii_lowercase()) {
            if site_is_excluded(&host, &settings.excluded_sites) {
                skipped.push(format!(
                    "{url}: this site is excluded in Settings (excluded sites): {host}"
                ));
                continue;
            }
        }
        // Round 78 — for a resolved variant the allow-list is
        // checked against the declared format (e.g. "MP4"); a
        // resolved variant with no declared format cannot be
        // checked, so it is allowed (yt-dlp already resolved a
        // real video).
        let ext_for_types: Option<String> = ext.clone().or_else(|| {
            v.format
                .as_deref()
                .map(str::trim)
                .filter(|f| !f.is_empty())
                .map(|f| f.to_ascii_lowercase())
        });
        if !settings.file_types_auto.is_empty() {
            if let Some(ext) = &ext_for_types {
                if !settings
                    .file_types_auto
                    .iter()
                    .any(|e| e.eq_ignore_ascii_case(ext))
                {
                    skipped.push(format!(
                        "{url}: .{ext} is not in the auto-download file types list (Settings > File Types)"
                    ));
                    continue;
                }
            }
        }
        // Quality backstop: keep a claimed label only when it IS a
        // clean label ("720p", "4K") or a dimension ("1280 × 720");
        // a sentence containing a resolution ("Ultra HD 8K Pro")
        // is not a label, and is replaced, never trusted.
        let claimed = v
            .quality
            .as_deref()
            .map(str::trim)
            .filter(|q| !q.is_empty());
        let quality = match claimed {
            Some(q) if is_clean_quality_label(q) => q.to_string(),
            Some(q) if q == UNKNOWN_QUALITY => UNKNOWN_QUALITY.to_string(),
            _ => quality_for(url, 0, 0),
        };
        let mut variant = v.clone();
        variant.url = url.to_string();
        variant.quality = Some(quality);
        variant.format = Some(v.format.clone().unwrap_or_else(|| {
            ext.clone()
                .unwrap_or_else(|| "mp4".to_string())
                .to_ascii_uppercase()
        }));
        if matches!(ext.as_deref(), Some("m3u8")) {
            variant.note = Some(if v.live {
                "Live stream — the playlist file is saved as-is; it is not merged into one video file."
                    .to_string()
            } else {
                "Playlist file — saved as-is; it is not merged into one video file.".to_string()
            });
        } else if matches!(ext.as_deref(), Some("mpd")) {
            variant.note = Some(
                "DASH manifest — saved as-is; its segments are not merged into one video file."
                    .to_string(),
            );
        } else if v.live {
            variant.note = Some(
                "Live stream — the stream has no end, so only what is available now can be saved."
                    .to_string(),
            );
        }
        accepted.push(variant);
    }
    Ok((accepted, skipped))
}

/// True when the label looks like a dimension (`1280 × 720`,
/// `1280x720`, `720p` is handled by `quality_from_label`).
fn is_dimension_label(q: &str) -> bool {
    let parts: Vec<&str> = q.split(|c| c == '×' || c == 'x').collect();
    parts.len() == 2
        && parts
            .iter()
            .all(|p| !p.trim().is_empty() && p.trim().chars().all(|c| c.is_ascii_digit()))
}

/// True when the claimed quality IS a clean label — the whole
/// string, not a sentence that happens to contain one: `720p`,
/// `1080p`, `4K`, `8K`, or a dimension label. Anything else (e.g.
/// "Ultra HD 8K Pro") is marketing copy, not a quality.
fn is_clean_quality_label(q: &str) -> bool {
    let lower = q.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "2160p" | "1440p" | "1080p" | "720p" | "480p" | "360p" | "240p" | "4k" | "8k"
    ) || is_dimension_label(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_never_invented() {
        assert_eq!(quality_for("http://x/clip.mp4", 0, 0), UNKNOWN_QUALITY);
        assert_eq!(quality_for("http://x/clip-720p.mp4", 0, 0), "720p");
        assert_eq!(quality_for("http://x/clip.mp4", 1280, 720), "1280 × 720");
        // Dimensions win over a label in the URL.
        assert_eq!(
            quality_for("http://x/clip-720p.mp4", 1920, 1080),
            "1920 × 1080"
        );
    }

    #[test]
    fn label_matching_is_bounded() {
        assert_eq!(
            quality_from_label("clip-720p.mp4"),
            Some("720p".to_string())
        );
        assert_eq!(
            quality_from_label("trailer-4k.webm"),
            Some("4K".to_string())
        );
        // "17209" is not a resolution label.
        assert_eq!(quality_from_label("file17209.mp4"), None);
        assert_eq!(quality_from_label("clip.mp4"), None);
    }

    #[test]
    fn extension_parsing_ignores_query_and_fragment() {
        assert_eq!(
            extension_of_url("https://example.com/clip.MP4?token=x#t=1").as_deref(),
            Some("mp4")
        );
        assert_eq!(extension_of_url("https://example.com/watch"), None);
        assert!(is_media_url("https://example.com/stream.m3u8"));
        assert!(!is_media_url("https://example.com/page.html"));
    }

    #[test]
    fn html_scan_finds_video_audio_and_linked_media() {
        let html = "<html><body>\
            <video src=\"clip.mp4\" width=\"640\" height=\"360\"></video>\
            <video><source src=\"a-720p.webm\"><source src=\"a-480p.webm\"></video>\
            <audio src=\"song.mp3\"></audio>\
            <a href=\"movie.mp4\">movie</a>\
            <img src=\"pic.png\">\
            <video src=\"clip.mp4\"></video>\
        </body></html>";
        let found = detect_videos_in_html(html, "http://127.0.0.1/page.html");
        let urls: Vec<&str> = found.iter().map(|v| v.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "http://127.0.0.1/clip.mp4",
                "http://127.0.0.1/a-720p.webm",
                "http://127.0.0.1/a-480p.webm",
                "http://127.0.0.1/song.mp3",
                "http://127.0.0.1/movie.mp4",
            ]
        );
        assert_eq!(found[1].quality, "720p");
        assert_eq!(found[1].format, "WEBM");
        assert_eq!(found[3].quality, UNKNOWN_QUALITY);
    }

    #[test]
    fn parse_m3u8_master_lists_real_variants() {
        let text = "#EXTM3U\n#EXT-X-VERSION:3\n\
            #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=1280x720\nv720.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1400000,RESOLUTION=1920x1080\nv1080.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=400000\nv-low.m3u8\n";
        let p = parse_m3u8(text).expect("a master playlist parses");
        assert!(p.is_master);
        assert!(!p.live, "a master playlist is not itself a live stream");
        assert!(!p.encrypted);
        assert_eq!(p.variants.len(), 3);
        assert_eq!(p.variants[0].resolution, Some((1280, 720)));
        assert_eq!(p.variants[0].bandwidth, Some(800_000));
        assert_eq!(p.variants[2].resolution, None);
        assert_eq!(
            quality_from_resolution(p.variants[0].resolution),
            "1280 × 720"
        );
        assert_eq!(
            quality_from_resolution(p.variants[2].resolution),
            UNKNOWN_QUALITY
        );
    }

    #[test]
    fn parse_m3u8_media_counts_segments_and_knows_live() {
        let vod = "#EXTM3U\n#EXT-X-TARGETDURATION:10\n\
            #EXTINF:10.0,\nseg1.ts\n#EXTINF:10.0,\nseg2.ts\n#EXTINF:5.0,\nseg3.ts\n#EXT-X-ENDLIST\n";
        let p = parse_m3u8(vod).expect("a VOD playlist parses");
        assert!(!p.is_master);
        assert!(!p.live, "ENDLIST means the stream has an end");
        assert_eq!(p.segments, 3);
        assert!((p.duration_secs - 25.0).abs() < 0.001);
        let live =
            "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10.0,\nseg1.ts\n#EXTINF:10.0,\nseg2.ts\n";
        let p = parse_m3u8(live).expect("a live playlist parses");
        assert!(p.live, "no ENDLIST means the stream has no end");
        assert_eq!(p.segments, 2);
    }

    #[test]
    fn parse_m3u8_encrypted_is_flagged_and_junk_is_not_a_playlist() {
        let enc = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n\
            #EXTINF:10.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        let p = parse_m3u8(enc).expect("an encrypted playlist parses");
        assert!(p.encrypted);
        let clear = "#EXTM3U\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:10.0,\nseg1.ts\n#EXT-X-ENDLIST\n";
        assert!(!parse_m3u8(clear).unwrap().encrypted);
        assert!(parse_m3u8("").is_none());
        assert!(parse_m3u8("<html>not a playlist</html>").is_none());
        assert!(parse_m3u8("#EXTM3U\n#EXTINF:xx,\nseg1.ts\n").is_some());
    }
}
