//! yt-dlp resolver — turn a video page URL into downloadable
//! formats, without the engine ever speaking the site's protocol
//! itself.
//!
//! Why a sidecar, not a library: YouTube / TikTok / Instagram /
//! Facebook do not serve plain files. Their pages hide the media
//! behind signature challenges, SABR streaming and login walls
//! that change whenever the site changes. yt-dlp is the tool that
//! tracks those changes; re-implementing its extractors inside the
//! engine would be a second, permanently out-of-date copy. So the
//! engine bundles the yt-dlp executable (plus QuickJS for its
//! JavaScript challenge solver and ffmpeg for merging) and this
//! module is the thin, honest boundary around it:
//!
//! * `resolve` runs `yt-dlp --dump-json` for one page URL and
//!   parses the formats list. It never downloads media itself —
//!   the engine's download manager does that, with the resolved
//!   URLs, headers and per-host segment caps from here.
//! * The HOST process (not the download manager) runs the
//!   resolve, because the browser extension asks for a quality
//!   list while the app may be closed, and the host answers it
//!   directly with the trimmed `quality_rows`.
//! * Quality is NEVER invented: a height is offered only when a
//!   format really has it, at exactly that height, and a video
//!   that needs its audio merged says so (`needs_merge`).
//!
//! Out of scope, by design: DRM decryption or bypass, downloading
//! a live stream, and any login/cookie forwarding beyond the
//! browser handing the host its own session cookies for the page
//! being resolved (written to a temp Netscape file, deleted on
//! drop, never logged).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::native_host::{cookies_to_header, CookiePair};

/// How long a resolve may take before it is killed. A slow
/// extractor (cold start, a challenge to solve) still finishes
/// well inside this; past it the user is staring at a spinner
/// for a page that is probably never going to answer.
pub const RESOLVE_TIMEOUT_SECS: u64 = 30;

/// Prefix of the temp Netscape cookie files `resolve` writes
/// (see `write_cookies_tempfile`). `sweep_stale_cookie_files`
/// deletes exactly these, so a crashed resolve can never leave
/// session cookies lying in the temp dir.
pub const YTDLP_COOKIE_FILE_PREFIX: &str = "fetchline-ytdlp-cookies-";

/// The sidecar binaries, as found on disk. `ytdlp` is always a
/// path (it may not exist — `resolve` then reports
/// `BinaryMissing` instead of the host guessing); `qjs`,
/// `ffmpeg` and `ffprobe` are `Some` only when the file is
/// actually there, so a missing solver can never be passed to
/// yt-dlp as if it existed. `ffprobe` rides with `ffmpeg`:
/// yt-dlp's merge needs BOTH (its README: "Required for merging
/// separate video and audio files"), and a merge with only
/// ffmpeg fails with "ffprobe and ffmpeg not found".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YtDlpPaths {
    pub ytdlp: PathBuf,
    pub qjs: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
}

/// The user-writable dir an updated yt-dlp lands in
/// (`%APPDATA%\com.fetchline.app\fetchline\bin` on Windows,
/// `~/.local/share/com.fetchline.app/fetchline/bin` elsewhere).
/// `None` when the home/app-data location cannot be found at
/// all — the bundled copy next to the exe is then the only one.
pub fn appdata_bin_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .filter(|v| !v.is_empty())
            .map(|a| {
                PathBuf::from(a)
                    .join("com.fetchline.app")
                    .join("fetchline")
                    .join("bin")
            })
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|h| {
            PathBuf::from(h)
                .join(".local")
                .join("share")
                .join("com.fetchline.app")
                .join("fetchline")
                .join("bin")
        })
    }
}

/// The sidecar file name on this OS: `yt-dlp.exe` on Windows,
/// `yt-dlp` elsewhere.
fn binary_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// Find the sidecars for this install. yt-dlp prefers an updated
/// copy in the app-data bin dir when one exists (the updater
/// writes there — the install dir may not be writable), and
/// falls back to the bundled copy next to the exe. QuickJS,
/// ffmpeg and ffprobe have no update path in v1, so they come
/// from next to the exe only, and only when the file is really
/// there.
pub fn locate_sidecars(exe_dir: &Path) -> YtDlpPaths {
    let ytdlp_name = binary_name("yt-dlp");
    let ytdlp = match appdata_bin_dir() {
        Some(bin) => {
            let updated = bin.join(&ytdlp_name);
            if updated.is_file() {
                updated
            } else {
                exe_dir.join(&ytdlp_name)
            }
        }
        None => exe_dir.join(&ytdlp_name),
    };
    let qjs_path = exe_dir.join(binary_name("qjs"));
    let qjs = qjs_path.is_file().then_some(qjs_path);
    let ffmpeg_path = exe_dir.join(binary_name("ffmpeg"));
    let ffmpeg = ffmpeg_path.is_file().then_some(ffmpeg_path);
    let ffprobe_path = exe_dir.join(binary_name("ffprobe"));
    let ffprobe = ffprobe_path.is_file().then_some(ffprobe_path);
    YtDlpPaths {
        ytdlp,
        qjs,
        ffmpeg,
        ffprobe,
    }
}

/// Strip a Windows `\\?\` (or `\\?\UNC\`) prefix from a path,
/// for tools that do not accept it — yt-dlp's `--js-runtimes`
/// value is one (the Tauri shell has the same helper in lib.rs
/// `clean_windows_path`; the two must stay in sync).
pub fn clean_windows_path(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s
    }
}

/// One downloadable format from `yt-dlp --dump-json`. The
/// `vcodec` / `acodec` strings are kept exactly as yt-dlp
/// reports them, including `"none"` — the selection rules in
/// `select_quality` read that sentinel, so normalising it away
/// here would lose the video-only / audio-only distinction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFormat {
    pub format_id: String,
    pub ext: String,
    pub height: Option<u32>,
    pub width: Option<u32>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub filesize: Option<u64>,
    pub url: String,
    pub protocol: String,
    pub http_headers: HashMap<String, String>,
    /// True when the format is a list of fragments (DASH / HLS)
    /// rather than one file: the engine downloads the segments
    /// and joins them, it never hands the manifest URL to the
    /// plain single-file path.
    pub has_fragments: bool,
}

/// A resolved video page: what the extension shows the user.
/// `warnings` holds the yt-dlp stderr lines that mattered
/// (SABR degradation), and `degraded` is true when the site
/// withheld formats — the caller may retry or say so, but the
/// formats listed here are real either way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedVideo {
    pub title: String,
    pub is_live: bool,
    pub extractor: String,
    pub formats: Vec<ResolvedFormat>,
    pub warnings: Vec<String>,
    pub degraded: bool,
}

/// Why a resolve failed, in the host's vocabulary — not
/// yt-dlp's exit code. The host maps these to UI text with
/// `user_message`, so a new yt-dlp failure mode is one new
/// variant here, not a string the UI has to parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YtDlpErrorKind {
    LoginRequired,
    /// Round 81 — YouTube refusing the request itself (a bot
    /// check / SABR block on this connection), NOT a missing
    /// sign-in: the video plays in the browser, and the
    /// browser's cookies were already sent. Mapping it to
    /// LoginRequired told the user to sign in when he already
    /// was — and "Try again" just repeated the same refusal.
    BotBlocked,
    /// Round 82 — the video itself is gone from YouTube
    /// ("This video is unavailable"): NOT a sign-in problem and
    /// NOT a bot block. YouTube's player can report a removed
    /// video with a LOGIN_REQUIRED playability status, and an
    /// older yt-dlp prints that alongside the unavailable
    /// reason — so this check runs BEFORE the login needles in
    /// `map_error`, or a removed video is misreported as
    /// "needs you to be signed in" (Tom & Jerry MUyWv16cGys,
    /// Round 81 retest: host.log said login_required 9 times
    /// for a video the watch page says isn't available
    /// anymore). `needs_login` is false for this kind.
    VideoUnavailable,
    Private,
    GeoBlocked,
    Unsupported,
    RateLimited,
    ExtractorOutdated,
    Drm,
    Live,
    Timeout,
    BinaryMissing,
    SpawnFailed,
    ParseFailed,
    InvalidUrl,
    Other,
}

/// A resolve failure: its kind (for the UI) and the detail
/// message (for the log — never shown raw to the user except
/// for `Other`, where the detail IS the only information).
#[derive(Debug)]
pub struct YtDlpError {
    pub kind: YtDlpErrorKind,
    pub message: String,
}

impl YtDlpError {
    pub fn new(kind: YtDlpErrorKind, message: String) -> Self {
        Self { kind, message }
    }

    /// The stable machine-readable name of the kind, for logs
    /// and for the host's telemetry — never user-facing text.
    pub fn kind_str(&self) -> &'static str {
        match self.kind {
            YtDlpErrorKind::LoginRequired => "login_required",
            YtDlpErrorKind::BotBlocked => "bot_blocked",
            YtDlpErrorKind::VideoUnavailable => "video_unavailable",
            YtDlpErrorKind::Private => "private",
            YtDlpErrorKind::GeoBlocked => "geo_blocked",
            YtDlpErrorKind::Unsupported => "unsupported",
            YtDlpErrorKind::RateLimited => "rate_limited",
            YtDlpErrorKind::ExtractorOutdated => "extractor_outdated",
            YtDlpErrorKind::Drm => "drm",
            YtDlpErrorKind::Live => "live",
            YtDlpErrorKind::Timeout => "timeout",
            YtDlpErrorKind::BinaryMissing => "binary_missing",
            YtDlpErrorKind::SpawnFailed => "spawn_failed",
            YtDlpErrorKind::ParseFailed => "parse_failed",
            YtDlpErrorKind::InvalidUrl => "invalid_url",
            YtDlpErrorKind::Other => "other",
        }
    }

    /// What the user is told. Plain words, no yt-dlp jargon,
    /// no stderr dump — except `Other`, where the stderr tail
    /// is the only thing anyone knows, so it is shown as-is.
    pub fn user_message(&self) -> String {
        match self.kind {
            YtDlpErrorKind::LoginRequired => {
                "This video needs you to be signed in. Sign in in your browser and try again."
                    .to_string()
            }
            YtDlpErrorKind::BotBlocked => {
                "YouTube is blocking this video on this connection. It plays in your browser, but the download can't reach it right now."
                    .to_string()
            }
            YtDlpErrorKind::VideoUnavailable => {
                "This video is unavailable — it may have been removed from YouTube.".to_string()
            }
            YtDlpErrorKind::Private => "This video is private or has been removed.".to_string(),
            YtDlpErrorKind::GeoBlocked => {
                "This video is not available in your country.".to_string()
            }
            YtDlpErrorKind::Unsupported => "This link is not a supported video page.".to_string(),
            YtDlpErrorKind::RateLimited => {
                "The site is rate-limiting requests right now. Try again in a little while."
                    .to_string()
            }
            YtDlpErrorKind::ExtractorOutdated => {
                "This site changed. Updating the video component may fix it.".to_string()
            }
            YtDlpErrorKind::Drm => {
                "This video is protected (DRM) and cannot be downloaded.".to_string()
            }
            YtDlpErrorKind::Live => "Live streams cannot be downloaded yet.".to_string(),
            YtDlpErrorKind::Timeout => {
                "The video page took too long to answer. Try again.".to_string()
            }
            YtDlpErrorKind::BinaryMissing => {
                "yt-dlp is missing — reinstall Sonic Manager or update it from Settings".to_string()
            }
            YtDlpErrorKind::SpawnFailed => {
                "Could not start the video component. Try reinstalling Sonic Manager.".to_string()
            }
            YtDlpErrorKind::ParseFailed => {
                "The video page could not be understood. Updating the video component may fix it."
                    .to_string()
            }
            YtDlpErrorKind::InvalidUrl => "That is not a valid video link.".to_string(),
            YtDlpErrorKind::Other => {
                if self.message.is_empty() {
                    "Something went wrong while reading this video page.".to_string()
                } else {
                    self.message.clone()
                }
            }
        }
    }
}

impl std::fmt::Display for YtDlpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for YtDlpError {}

/// Map yt-dlp's stderr to a `YtDlpError` by substring.
/// yt-dlp has no stable error codes — its stderr is the API —
/// so this matches the phrases its extractors actually print,
/// most-specific first: a private video that also mentions
/// signing in is still a private video, and a 403 in resolve
/// context is a login wall, not a bug. Anything unrecognised
/// keeps the raw tail as an `Other`, so the log still says what
/// happened instead of collapsing to a generic failure.
pub fn map_error(stderr: &str) -> YtDlpError {
    let lower = stderr.to_ascii_lowercase();
    let has = |needle: &str| lower.contains(needle);
    let err = |kind: YtDlpErrorKind, message: &str| YtDlpError::new(kind, message.to_string());
    // Round 81 — the explicit bot check ("Sign in to confirm
    // you're not a bot") is YouTube refusing the request on
    // this connection, NOT a missing sign-in: the video plays
    // in the browser, and the browser's cookies were already
    // sent (Tom & Jerry, Round 81 reproduction — Rick Astley
    // resolves fine with the same cookies). It gets its own
    // kind now, so the UI says "blocked" instead of telling a
    // signed-in user to sign in. The check runs BEFORE the
    // Private rule below, or the bot block maps to Private.
    // A cookieless bot-block still gets the one cookie retry
    // (the extension offers it, like a login wall); a cookied
    // one never repeats. "login_required" / "playability
    // status" stay LoginRequired: the player genuinely wants a
    // login there, and the extension's cookied-failure message
    // covers the case where cookies didn't help.
    if has("confirm you're not a bot") || has("not a bot") {
        return err(
            YtDlpErrorKind::BotBlocked,
            "YouTube is blocking this video on this connection.",
        );
    }
    // Round 82 — an unavailable video is checked BEFORE the
    // login needles: YouTube can report a removed video with a
    // LOGIN_REQUIRED playability status, and an older yt-dlp
    // prints that next to the unavailable reason — checking
    // login first misreported a removed video (MUyWv16cGys) as
    // "needs you to be signed in" while the user was signed in.
    // The needle is "video is unavailable" (with "is") as well
    // as the short "video unavailable": the real stderr is
    // "This video is unavailable", which does NOT contain the
    // substring "video unavailable".
    // "has been removed" stays Private (a removed-by-uploader
    // video reads as private in the UI); only the explicit
    // "video unavailable" phrasing is VideoUnavailable.
    if has("video is unavailable") || has("video unavailable") {
        return err(
            YtDlpErrorKind::VideoUnavailable,
            "This video is unavailable — it may have been removed from YouTube.",
        );
    }
    if has("login_required") || has("playability status") {
        return err(
            YtDlpErrorKind::LoginRequired,
            "This video needs you to be signed in.",
        );
    }
    if has("private") || has("has been removed") {
        return err(
            YtDlpErrorKind::Private,
            "This video is private or has been removed.",
        );
    }
    if has("not available in your country") || has("geo") {
        return err(
            YtDlpErrorKind::GeoBlocked,
            "This video is not available in your country.",
        );
    }
    if has("sign in")
        || has("login required")
        || has("log in")
        || has("cookies")
        || has("401")
        || has("403")
    {
        return err(
            YtDlpErrorKind::LoginRequired,
            "This video needs you to be signed in.",
        );
    }
    if has("unsupported url") || has("no video") || has("not a valid url") {
        return err(
            YtDlpErrorKind::Unsupported,
            "This link is not a supported video page.",
        );
    }
    if has("429") || has("too many requests") {
        return err(
            YtDlpErrorKind::RateLimited,
            "The site is rate-limiting requests right now.",
        );
    }
    if has("signature")
        || has("n challenge")
        || has("challenge solving failed")
        || has("js runtime")
        || has("no supported javascript runtime")
        || has("extractor")
    {
        return err(
            YtDlpErrorKind::ExtractorOutdated,
            "This site changed. Updating the video component may fix it.",
        );
    }
    if has("drm") || has("widevine") || has("fairplay") || has("protected") {
        return err(
            YtDlpErrorKind::Drm,
            "This video is protected (DRM) and cannot be downloaded.",
        );
    }
    // "live" on its own would also match "deliver", so only the
    // phrases a live-stream refusal actually uses.
    if has("is_live")
        || has("is live")
        || has("live stream")
        || has("livestream")
        || has("live video")
        || has("live event")
        || has("live broadcast")
    {
        return err(
            YtDlpErrorKind::Live,
            "Live streams cannot be downloaded yet.",
        );
    }
    let tail: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if tail.is_empty() {
        return err(
            YtDlpErrorKind::Other,
            "yt-dlp exited with an error but printed no message.",
        );
    }
    // The last lines are the verdict; the earlier ones are
    // usually a traceback or a download log.
    let mut message = tail
        .iter()
        .rev()
        .take(3)
        .rev()
        .fold(String::new(), |mut acc, line| {
            if !acc.is_empty() {
                acc.push(' ');
            }
            acc.push_str(line);
            acc
        });
    if message.chars().count() > 500 {
        message = message.chars().take(500).collect();
    }
    YtDlpError::new(YtDlpErrorKind::Other, message)
}

/// A JSON string-or-number field, as text. `format_id` is a
/// string in practice, but a numeric id would otherwise vanish
/// into an empty string and two formats would collide on it.
fn json_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// Parse one `yt-dlp --dump-json` document. The shape is
/// yt-dlp's, not ours: a flat object with a `formats` array,
/// where each format carries its own codec pair, size, URL,
/// protocol and headers. Formats with no URL are skipped (the
/// SABR shape — a format yt-dlp knows about but cannot give
/// a URL for is not downloadable), and so are storyboards,
/// which are thumbnail sheets, not media. `filesize` falls
/// back to `filesize_approx` — an estimate is honest enough
/// for a size label, and no size at all is worse. The caller
/// attaches SABR warnings afterwards (`parse_sabr_warnings`),
/// because degradation is a property of the stderr, not of
/// the JSON.
pub fn parse_dump_json(json: &str) -> Result<ResolvedVideo, YtDlpError> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| {
        YtDlpError::new(
            YtDlpErrorKind::ParseFailed,
            format!("Could not read yt-dlp's video data: {e}"),
        )
    })?;
    let title = value
        .get("title")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or("video")
        .to_string();
    let extractor = value
        .get("extractor")
        .and_then(|e| e.as_str())
        .or_else(|| value.get("extractor_key").and_then(|e| e.as_str()))
        .unwrap_or("")
        .to_string();
    let is_live = value
        .get("is_live")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut formats = Vec::new();
    if let Some(list) = value.get("formats").and_then(|f| f.as_array()) {
        for f in list {
            let Some(url) = f
                .get("url")
                .and_then(|u| u.as_str())
                .filter(|u| !u.is_empty())
            else {
                continue;
            };
            let protocol = f
                .get("protocol")
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
            let vcodec = f.get("vcodec").and_then(|v| v.as_str()).map(str::to_string);
            let acodec = f.get("acodec").and_then(|a| a.as_str()).map(str::to_string);
            // A storyboard is a sheet of thumbnails in an .mhtml
            // wrapper (or a format with neither codec): offering
            // it as a video quality would be a lie.
            if protocol == "mhtml"
                || (vcodec.as_deref() == Some("none") && acodec.as_deref() == Some("none"))
            {
                continue;
            }
            let filesize = f
                .get("filesize")
                .and_then(|v| v.as_u64())
                .or_else(|| f.get("filesize_approx").and_then(|v| v.as_u64()));
            // A format is fragmented when yt-dlp lists its
            // segments, or when its protocol IS a playlist
            // protocol — either way the URL is not one file.
            let has_fragments = f
                .get("fragments")
                .and_then(|fr| fr.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false)
                || matches!(protocol.as_str(), "m3u8" | "m3u8_native" | "dash");
            let mut http_headers = HashMap::new();
            if let Some(obj) = f.get("http_headers").and_then(|h| h.as_object()) {
                for (k, v) in obj {
                    if let Some(s) = v.as_str() {
                        http_headers.insert(k.clone(), s.to_string());
                    }
                }
            }
            formats.push(ResolvedFormat {
                format_id: f.get("format_id").map(json_text).unwrap_or_default(),
                ext: f
                    .get("ext")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                height: f.get("height").and_then(|v| v.as_u64()).map(|h| h as u32),
                width: f.get("width").and_then(|v| v.as_u64()).map(|w| w as u32),
                vcodec,
                acodec,
                filesize,
                url: url.to_string(),
                protocol,
                http_headers,
                has_fragments,
            });
        }
    }
    Ok(ResolvedVideo {
        title,
        is_live,
        extractor,
        formats,
        warnings: Vec::new(),
        degraded: false,
    })
}

/// The SABR-degradation warnings in yt-dlp's stderr: lines
/// saying formats were skipped for missing URLs, or mentioning
/// SABR at all. Trimmed and de-duplicated, in first-seen order
/// — the host shows these next to the degraded quality list so
/// the user knows why 1080p is missing instead of assuming the
/// video is only 360p.
pub fn parse_sabr_warnings(stderr: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in stderr.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.contains("sabr") || lower.contains("missing a url") {
            if !out.iter().any(|l| l == trimmed) {
                out.push(trimmed.to_string());
            }
        }
    }
    out
}

/// True when yt-dlp warned that formats were withheld (SABR):
/// the resolve is usable, but the quality list is incomplete
/// and a retry is worth one attempt.
pub fn is_sabr_degraded(stderr: &str) -> bool {
    !parse_sabr_warnings(stderr).is_empty()
}

/// True when the codec field names a real codec: present and
/// not yt-dlp's `"none"` sentinel.
fn codec_is_real(codec: &Option<String>) -> bool {
    matches!(codec, Some(c) if c != "none")
}

/// True when the codec field names no codec: absent, or the
/// `"none"` sentinel. A video-only format has a real vcodec and
/// an absent acodec; audio-only is the mirror image.
fn codec_is_absent(codec: &Option<String>) -> bool {
    match codec {
        None => true,
        Some(c) => c == "none",
    }
}

/// True when the video codec is H.264 (`avc1…` or `h264`): the
/// codec every player and every merge path handles without
/// surprises, so it wins over VP9/AV1 at the same height.
fn codec_is_avc(vcodec: &Option<String>) -> bool {
    match vcodec.as_deref() {
        Some(c) => {
            let lower = c.to_ascii_lowercase();
            lower.starts_with("avc1") || lower.starts_with("h264")
        }
        None => false,
    }
}

/// True when the audio is AAC in an MP4-family container
/// (`m4a` ext, or an `aac`/`mp4a` codec): the pairing the
/// merge path handles, preferred over Opus-in-WebM.
fn audio_is_m4a_aac(format: &ResolvedFormat) -> bool {
    if format.ext == "m4a" {
        return true;
    }
    match format.acodec.as_deref() {
        Some(c) => {
            let lower = c.to_ascii_lowercase();
            lower.contains("aac") || lower.starts_with("mp4a")
        }
        None => false,
    }
}

/// What picking one height means: a single file with audio, a
/// video+audio pair that must be merged, or one fragmented
/// format whose segments must be joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectedQuality {
    Single(ResolvedFormat),
    Pair {
        video: ResolvedFormat,
        audio: ResolvedFormat,
    },
    Fragmented(ResolvedFormat),
}

/// Pick the best way to download exactly `height`, in order:
/// (1) a progressive (video+audio) format at that height —
/// mp4 first, then the smallest file; (2) the best video-only
/// format at that height paired with the best audio-only
/// format; (3) a fragmented format at that height. Only
/// `https` URLs are considered for the first two — a cleartext
/// media URL is not offered — and a fragmented format must be
/// https or a playlist protocol, never plain http. There is no
/// nearest-height fallback: asking for 1080p when only 720p
/// exists returns `None`, because offering 720p as 1080p
/// would be inventing a quality.
pub fn select_quality(formats: &[ResolvedFormat], height: u32) -> Option<SelectedQuality> {
    // (2) Progressive first: one file, no merge, no fragments.
    let progressive = formats.iter().filter(|f| {
        f.protocol == "https"
            && f.height == Some(height)
            && codec_is_real(&f.vcodec)
            && codec_is_real(&f.acodec)
    });
    if let Some(best) = progressive.min_by_key(|f| (f.ext != "mp4", f.filesize.unwrap_or(u64::MAX)))
    {
        return Some(SelectedQuality::Single(best.clone()));
    }
    // (3) Pair: best video-only at this height with the best
    // audio-only format. Both must exist — a video with no
    // audio to pair is not a pair, it falls through.
    let video = formats
        .iter()
        .filter(|f| {
            f.protocol == "https"
                && f.height == Some(height)
                && codec_is_real(&f.vcodec)
                && codec_is_absent(&f.acodec)
        })
        .min_by_key(|f| {
            (
                !codec_is_avc(&f.vcodec),
                f.ext != "mp4",
                f.filesize.unwrap_or(u64::MAX),
            )
        });
    let audio = formats
        .iter()
        .filter(|f| f.protocol == "https" && codec_is_absent(&f.vcodec) && codec_is_real(&f.acodec))
        .min_by_key(|f| {
            (
                !audio_is_m4a_aac(f),
                f.ext != "mp4",
                f.filesize.unwrap_or(u64::MAX),
            )
        });
    if let (Some(video), Some(audio)) = (video, audio) {
        return Some(SelectedQuality::Pair {
            video: video.clone(),
            audio: audio.clone(),
        });
    }
    // (4) Fragmented: the URL is a manifest / segment base, not
    // a file, so the https-only rule for plain files does not
    // apply the same way — but a plain-http manifest is still
    // refused. Prefer an mp4 container or an HLS protocol.
    let fragmented = formats.iter().filter(|f| {
        f.height == Some(height)
            && f.has_fragments
            && matches!(
                f.protocol.as_str(),
                "https" | "m3u8" | "m3u8_native" | "dash"
            )
    });
    if let Some(best) = fragmented.min_by_key(|f| {
        (
            f.ext != "mp4" && !matches!(f.protocol.as_str(), "m3u8" | "m3u8_native"),
            f.filesize.unwrap_or(u64::MAX),
        )
    }) {
        return Some(SelectedQuality::Fragmented(best.clone()));
    }
    None
}

/// One row of the quality list the extension shows: one height,
/// the format(s) behind it, and what downloading it entails.
/// `needs_merge` is true for a video+audio pair and for a
/// fragmented format — both need joining before the file plays
/// — and false for a single progressive file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QualityRow {
    pub height: u32,
    pub label: String,
    pub ext: String,
    pub has_audio: bool,
    pub needs_merge: bool,
    pub filesize: Option<u64>,
    pub url: String,
    pub audio_url: Option<String>,
    pub audio_format_id: Option<String>,
    pub format_id: String,
    pub headers: HashMap<String, String>,
    pub fragmented: bool,
}

/// One row per distinct height present in the formats, 360p
/// and up, sorted ascending, each chosen by `select_quality`.
/// Heights below 360p are dropped (a 144p row is noise in the
/// picker, and the file is still downloadable by URL), and a
/// height with no honest selection gets no row at all — so a
/// video with only audio formats yields an empty list, not a
/// row that promises video it does not have.
pub fn quality_rows(video: &ResolvedVideo) -> Vec<QualityRow> {
    let mut heights: Vec<u32> = video
        .formats
        .iter()
        .filter_map(|f| f.height)
        .filter(|h| (360..=4320).contains(h))
        .collect();
    heights.sort_unstable();
    heights.dedup();
    let mut rows = Vec::new();
    for height in heights {
        let Some(selected) = select_quality(&video.formats, height) else {
            continue;
        };
        let (format, audio, fragmented, needs_merge) = match &selected {
            SelectedQuality::Single(f) => (f, None, false, false),
            SelectedQuality::Pair { video, audio } => (video, Some(audio), false, true),
            SelectedQuality::Fragmented(f) => (f, None, true, true),
        };
        let has_audio = codec_is_real(&format.acodec) || audio.is_some();
        let filesize = match (format.filesize, audio.and_then(|a| a.filesize)) {
            (Some(v), Some(a)) => Some(v.saturating_add(a)),
            (Some(v), None) => Some(v),
            (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        rows.push(QualityRow {
            height,
            label: format_label(height, has_audio, needs_merge),
            ext: format.ext.clone(),
            has_audio,
            needs_merge,
            filesize,
            url: format.url.clone(),
            audio_url: audio.map(|a| a.url.clone()),
            audio_format_id: audio.map(|a| a.format_id.clone()),
            format_id: format.format_id.clone(),
            headers: format.http_headers.clone(),
            fragmented,
        });
    }
    rows
}

/// The picker label for a height: `1080p HD`, `720p HD`,
/// `480p`, `360p`. 720p and up get the HD suffix; 4K is just
/// its height, `2160p HD`. The audio/merge flags do not change
/// the label — the caller annotates those separately, so the
/// label stays a plain statement of resolution.
pub fn format_label(height: u32, _has_audio: bool, _needs_merge: bool) -> String {
    if height >= 720 {
        format!("{height}p HD")
    } else {
        format!("{height}p")
    }
}

/// A Windows-safe filename from a video title. The illegal
/// characters (`<>:"/\|?*` and control characters) become
/// `_`, whitespace runs collapse to one space, and leading /
/// trailing dots and spaces are stripped — a name ending in a
/// dot or a space is not creatable on Windows. The stem is cut
/// to 100 characters (Windows paths, not just names, have
/// limits, and titles can be paragraphs) and the extension is
/// appended exactly once, lowercased. An empty or unusable
/// title becomes `video`, never `videoplayback` — that is
/// YouTube's generic fallback name, and stamping it on a
/// title-less video would make every such download collide on
/// the same meaningless name.
pub fn sanitize_title_filename(title: &str, ext: &str) -> String {
    let ext: String = ext
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let mut stem: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Collapse whitespace runs (a title with newlines or double
    // spaces becomes one readable name, not a gap-filled one).
    let mut collapsed = String::with_capacity(stem.len());
    let mut last_was_space = false;
    for c in stem.chars() {
        if c.is_whitespace() {
            if !last_was_space {
                collapsed.push(' ');
            }
            last_was_space = true;
        } else {
            collapsed.push(c);
            last_was_space = false;
        }
    }
    stem = collapsed;
    stem = stem.trim_matches(|c| c == '.' || c == ' ').to_string();
    if stem.is_empty() || stem.eq_ignore_ascii_case("videoplayback") {
        stem = "video".to_string();
    }
    if stem.chars().count() > 100 {
        stem = stem.chars().take(100).collect();
        stem = stem.trim_end_matches(|c| c == '.' || c == ' ').to_string();
        if stem.is_empty() {
            stem = "video".to_string();
        }
    }
    if ext.is_empty() {
        return stem;
    }
    let suffix = format!(".{ext}");
    if stem.to_ascii_lowercase().ends_with(&suffix) {
        stem
    } else {
        format!("{stem}.{ext}")
    }
}

/// The segment cap for a resolved-media host: fewer parallel
/// parts than a plain file, because these CDNs punish parallel
/// range requests (TikTok answers 403/429, fbcdn stalls).
/// Suffix match on the lowercased host, so any subdomain of
/// the CDN counts but a lookalike domain (`notgooglevideo.com`)
/// does not. `None` means "no opinion — use the normal cap".
pub fn default_host_segment_cap(host: &str) -> Option<usize> {
    let host = host.trim().to_ascii_lowercase();
    for (suffix, cap) in [
        ("googlevideo.com", 8usize),
        ("fbcdn.net", 4usize),
        ("tiktokcdn.com", 2usize),
    ] {
        if host == suffix || host.ends_with(&format!(".{suffix}")) {
            return Some(cap);
        }
    }
    None
}

/// Options for one resolve. `paths` is what `locate_sidecars`
/// found; `cookies` are the browser's session cookies for the
/// page (never logged, written to a temp file only for the
/// resolve); `extractor_args` is the hidden override the host
/// uses when a site needs a different player client.
pub struct ResolveOptions {
    pub paths: YtDlpPaths,
    pub cookies: Vec<CookiePair>,
    pub extractor_args: Option<String>,
    pub timeout_secs: u64,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        Self {
            paths: YtDlpPaths::default(),
            cookies: Vec::new(),
            extractor_args: None,
            timeout_secs: RESOLVE_TIMEOUT_SECS,
        }
    }
}

/// Build the yt-dlp argv for a resolve. Pure, so the host and
/// the tests can inspect exactly what will run. The safety
/// flags are not optional: `--ignore-config` stops a user's
/// (or an attacker's) yt-dlp config file from changing what
/// runs, `--no-playlist` stops a video URL from resolving a
/// whole playlist, and `--` stops the URL itself from being
/// read as a flag. `--js-runtimes` is passed only when a
/// QuickJS sidecar was found, `--cookies` only when a temp
/// file was written, and `--extractor-args` only when set.
pub fn build_ytdlp_args(
    url: &str,
    opts: &ResolveOptions,
    cookies_file: Option<&Path>,
) -> Vec<String> {
    let mut args = vec![
        "--ignore-config".to_string(),
        "--no-playlist".to_string(),
        "--dump-json".to_string(),
        "--no-warnings".to_string(),
    ];
    if let Some(qjs) = &opts.paths.qjs {
        args.push("--js-runtimes".to_string());
        args.push(format!("quickjs:{}", clean_windows_path(qjs)));
    }
    if let Some(extractor_args) = opts
        .extractor_args
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        args.push("--extractor-args".to_string());
        args.push(extractor_args.to_string());
    }
    if let Some(file) = cookies_file {
        args.push("--cookies".to_string());
        args.push(file.display().to_string());
    }
    args.push("--".to_string());
    args.push(url.to_string());
    args
}

/// One spawn of yt-dlp: run it, wait (bounded), and turn its
/// stdout into a `ResolvedVideo` with the stderr warnings
/// attached. A nonzero exit maps through `map_error`; empty
/// stdout is a parse failure, not an empty video.
async fn resolve_once(
    url: &str,
    opts: &ResolveOptions,
    cookies_file: Option<&Path>,
) -> Result<ResolvedVideo, YtDlpError> {
    let args = build_ytdlp_args(url, opts, cookies_file);
    let mut cmd = std::process::Command::new(&opts.paths.ytdlp);
    cmd.args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        // No console window flashing up on the user's desktop
        // for a background resolve.
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    // Built on the std command (so the Windows creation flag
    // above applies) and converted: tokio's Command has no
    // stable way to set it directly.
    let mut child = tokio::process::Command::from(cmd)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                YtDlpError::new(
                    YtDlpErrorKind::BinaryMissing,
                    "yt-dlp is missing — reinstall Sonic Manager or update it from Settings"
                        .to_string(),
                )
            } else {
                YtDlpError::new(
                    YtDlpErrorKind::SpawnFailed,
                    format!("Could not start yt-dlp: {e}"),
                )
            }
        })?;
    let timeout = Duration::from_secs(opts.timeout_secs.max(5));
    // Drain stdout/stderr concurrently with the wait: a child
    // that fills a pipe buffer must never block forever while
    // nobody reads, and the wait must stay borrowable so a
    // timeout can kill and reap the child (wait_with_output
    // consumes it, which would leave a zombie on timeout).
    let mut stdout_pipe = child.stdout.take().expect("stdout was piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr was piped");
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut stdout_pipe, &mut buf).await;
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr_pipe, &mut buf).await;
        buf
    });
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => {
            return Err(YtDlpError::new(
                YtDlpErrorKind::SpawnFailed,
                format!("Could not wait for yt-dlp: {e}"),
            ));
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(YtDlpError::new(
                YtDlpErrorKind::Timeout,
                format!(
                    "Timed out after {}s waiting for yt-dlp to resolve this video.",
                    opts.timeout_secs.max(5)
                ),
            ));
        }
    };
    // Round 80 — a yt-dlp grandchild (JS runtime / ffmpeg) can
    // hold a pipe open after the child exits or is killed, so the
    // drain must never wait forever: bound it and continue with
    // what arrived. Without this bound a lingering grandchild
    // hangs the resolve even though the child itself is gone
    // (the timeout kill above reaps only the child).
    let stdout_bytes = match tokio::time::timeout(Duration::from_secs(5), stdout_task).await {
        Ok(bytes) => bytes.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let stderr_bytes = match tokio::time::timeout(Duration::from_secs(5), stderr_task).await {
        Ok(bytes) => bytes.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    if !status.success() {
        return Err(map_error(&stderr));
    }
    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    if stdout.trim().is_empty() {
        return Err(YtDlpError::new(
            YtDlpErrorKind::ParseFailed,
            "yt-dlp returned no video data.".to_string(),
        ));
    }
    let mut video = parse_dump_json(&stdout)?;
    video.warnings = parse_sabr_warnings(&stderr);
    video.degraded = is_sabr_degraded(&stderr);
    Ok(video)
}

/// Resolve one page URL with yt-dlp. The URL is validated
/// before anything spawns (a non-http(s) URL is refused, never
/// handed to a subprocess), cookies are written to a temp file
/// first and deleted when this returns, and the run is bounded
/// by `opts.timeout_secs` (at least 5s). When the first run
/// comes back SABR-degraded with formats, it is retried once
/// and the better result wins (more formats; on a tie the
/// first) — a retry that fails keeps the first result, so a
/// flaky second run can never lose a usable first one.
pub async fn resolve(url: &str, opts: &ResolveOptions) -> Result<ResolvedVideo, YtDlpError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| {
        YtDlpError::new(
            YtDlpErrorKind::InvalidUrl,
            format!("Not a valid http(s) URL: {url}"),
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(YtDlpError::new(
            YtDlpErrorKind::InvalidUrl,
            format!(
                "Only http:// and https:// links can be resolved, not {}://",
                parsed.scheme()
            ),
        ));
    }
    let cookies_file = if opts.cookies.is_empty() {
        None
    } else {
        let host = parsed.host_str().unwrap_or("");
        Some(write_cookies_tempfile(host, &opts.cookies)?)
    };
    let cookies_path = cookies_file.as_ref().map(|f| f.path.as_path());
    let mut first = resolve_once(url, opts, cookies_path).await?;
    if first.degraded && !first.formats.is_empty() {
        if let Ok(second) = resolve_once(url, opts, cookies_path).await {
            if second.formats.len() > first.formats.len() {
                first = second;
            }
        }
    }
    Ok(first)
}

/// A temp Netscape cookies file. Dropping it deletes the file
/// (errors ignored — the temp dir is swept anyway), so a
/// resolve's session cookies never outlive the resolve, even
/// on an early return.
#[derive(Debug)]
pub struct CookieTempFile {
    pub path: PathBuf,
}

impl Drop for CookieTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Write the browser's session cookies for one page host to a
/// temp Netscape cookies file for yt-dlp's `--cookies`.
///
/// `CookiePair` carries no domain — name and value only, by
/// design, so the host must pass the page host it came from;
/// guessing a domain (`.youtube.com` for every site) would
/// send one site's cookies to another. The domain column is
/// the host with a leading dot, the standard Netscape
/// domain-cookie form, which matches the host and its
/// subdomains. Pairs are validated with `cookies_to_header`
/// first: an invalid pair is an error, never silently dropped,
/// and validation also guarantees no name or value can break
/// a line (no tabs, no newlines). The file gets restrictive
/// Unix permissions (0o600) and a random name.
pub fn write_cookies_tempfile(
    host: &str,
    pairs: &[CookiePair],
) -> Result<CookieTempFile, YtDlpError> {
    if let Err(e) = cookies_to_header(pairs) {
        return Err(YtDlpError::new(
            YtDlpErrorKind::Other,
            format!("Invalid cookies: {e}"),
        ));
    }
    let host = host.trim();
    if host.is_empty() {
        return Err(YtDlpError::new(
            YtDlpErrorKind::Other,
            "Cannot write a cookies file without a page host.".to_string(),
        ));
    }
    let domain = if host.starts_with('.') {
        host.to_string()
    } else {
        format!(".{host}")
    };
    let mut text = String::from("# Netscape HTTP Cookie File\n");
    for pair in pairs {
        text.push_str(&format!(
            "{domain}\tTRUE\t/\tFALSE\t0\t{}\t{}\n",
            pair.name, pair.value
        ));
    }
    let path = std::env::temp_dir().join(format!(
        "{YTDLP_COOKIE_FILE_PREFIX}{}.txt",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&path, text).map_err(|e| {
        YtDlpError::new(
            YtDlpErrorKind::Other,
            format!("Could not write a temporary cookies file: {e}"),
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(CookieTempFile { path })
}

/// Delete stale yt-dlp cookie files in a dir — anything whose
/// name starts with `YTDLP_COOKIE_FILE_PREFIX`. Best-effort:
/// a missing dir, an unreadable entry or a file that will not
/// delete is ignored, never an error. Only files are removed,
/// never directories.
pub fn sweep_stale_cookie_files(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(YTDLP_COOKIE_FILE_PREFIX)
        {
            continue;
        }
        let path = entry.path();
        if path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The GitHub release download base for yt-dlp updates. The
/// `latest/download` redirect always points at the newest
/// release; the binary and its published checksums come from the
/// same release, so a checksum match proves the binary is the
/// one yt-dlp published.
pub const YTDLP_RELEASE_BASE: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";

/// Maximum size of a downloaded yt-dlp.exe (the real binary is
/// ~20 MB; anything far above that is not yt-dlp).
const YTDLP_UPDATE_MAX_BYTES: usize = 64 * 1024 * 1024;

/// SHA-256 of `bytes`, lowercase hex. The checksum the release
/// publishes is lowercase hex too, so the comparison is exact.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Pull the expected SHA-256 for `filename` out of a
/// `SHA2-256SUMS` file: lines are `<hex>  <filename>` (or
/// `<hex> *<filename>` for binary mode). Pure, so it is unit
/// tested without a network.
pub fn parse_sha256sums(text: &str, filename: &str) -> Option<String> {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(hex) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let name = name.strip_prefix('*').unwrap_or(name);
        if name == filename && hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hex.to_ascii_lowercase());
        }
    }
    None
}

/// The installed yt-dlp's version (`yt-dlp --version`, first
/// line), or `None` when the binary is missing or will not run.
/// Short timeout — this is a status check, not a resolve.
pub async fn ytdlp_version(paths: &YtDlpPaths) -> Option<String> {
    let mut cmd = std::process::Command::new(&paths.ytdlp);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        // No console window flashing up on the user's desktop
        // for a background version check.
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let child = tokio::process::Command::from(cmd)
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let out = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(first.to_string())
    }
}

/// Download the newest yt-dlp.exe from its GitHub release and
/// install it into the AppData bin dir (`locate_sidecars`
/// prefers that copy over the bundled one, so the update takes
/// effect on the next resolve without touching Program Files).
/// The published `SHA2-256SUMS` is fetched from the same
/// release and the binary is installed ONLY when its SHA-256
/// matches — a corrupt or tampered download is refused, never
/// installed. The user's proxy, if configured, is honoured.
/// A 5-minute total timeout bounds a slow download; the binary
/// is written to `yt-dlp.exe.new` first and renamed into place,
/// so a failed download never leaves a half-written binary.
pub async fn update_ytdlp(proxy: Option<&crate::proxy::ProxyConfig>) -> Result<(), YtDlpError> {
    let other = |msg: String| YtDlpError::new(YtDlpErrorKind::Other, msg);
    let dir = appdata_bin_dir()
        .ok_or_else(|| other("Cannot find the folder for yt-dlp updates".to_string()))?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| other(format!("Cannot create the update folder: {e}")))?;
    let client = crate::proxy::build_client_with_total_timeout(proxy, 30, Duration::from_secs(300))
        .map_err(|e| other(format!("Cannot start the update download: {e}")))?;
    let sums = client
        .get(format!("{YTDLP_RELEASE_BASE}/SHA2-256SUMS"))
        .send()
        .await
        .map_err(|e| other(format!("Cannot download the yt-dlp checksums: {e}")))?
        .error_for_status()
        .map_err(|e| other(format!("Cannot download the yt-dlp checksums: {e}")))?
        .text()
        .await
        .map_err(|e| other(format!("Cannot read the yt-dlp checksums: {e}")))?;
    let expected = parse_sha256sums(&sums, "yt-dlp.exe").ok_or_else(|| {
        YtDlpError::new(
            YtDlpErrorKind::ParseFailed,
            "The published yt-dlp checksums do not list yt-dlp.exe".to_string(),
        )
    })?;
    let bytes = client
        .get(format!("{YTDLP_RELEASE_BASE}/yt-dlp.exe"))
        .send()
        .await
        .map_err(|e| other(format!("Cannot download yt-dlp: {e}")))?
        .error_for_status()
        .map_err(|e| other(format!("Cannot download yt-dlp: {e}")))?
        .bytes()
        .await
        .map_err(|e| other(format!("Cannot read the yt-dlp download: {e}")))?;
    if bytes.len() > YTDLP_UPDATE_MAX_BYTES {
        return Err(other(
            "The downloaded yt-dlp.exe is unexpectedly large — it was not installed".to_string(),
        ));
    }
    if sha256_hex(&bytes) != expected {
        return Err(other(
            "The downloaded yt-dlp.exe failed its checksum — the download may be corrupt, so it was not installed"
                .to_string(),
        ));
    }
    let tmp = dir.join("yt-dlp.exe.new");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| other(format!("Cannot write the yt-dlp update: {e}")))?;
    std::fs::rename(&tmp, dir.join("yt-dlp.exe")).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        other(format!("Cannot install the yt-dlp update: {e}"))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const YOUTUBE_DASH: &str = include_str!("../tests/fixtures/ytdlp/youtube_dash.json");
    const YOUTUBE_SABR: &str = include_str!("../tests/fixtures/ytdlp/youtube_sabr.json");
    const YOUTUBE_SABR_STDERR: &str =
        include_str!("../tests/fixtures/ytdlp/youtube_sabr_stderr.txt");
    const TIKTOK: &str = include_str!("../tests/fixtures/ytdlp/tiktok.json");
    const INSTAGRAM: &str = include_str!("../tests/fixtures/ytdlp/instagram.json");
    const FACEBOOK: &str = include_str!("../tests/fixtures/ytdlp/facebook.json");

    /// A hand-built format for selection tests, so the test
    /// states exactly the codec pair / protocol / fragments it
    /// means — no fixture JSON to decode by eye.
    fn test_format(
        format_id: &str,
        ext: &str,
        height: Option<u32>,
        vcodec: Option<&str>,
        acodec: Option<&str>,
        filesize: Option<u64>,
        protocol: &str,
        has_fragments: bool,
    ) -> ResolvedFormat {
        let scheme = if protocol == "http" { "http" } else { "https" };
        ResolvedFormat {
            format_id: format_id.to_string(),
            ext: ext.to_string(),
            height,
            width: height.map(|h| h * 16 / 9),
            vcodec: vcodec.map(str::to_string),
            acodec: acodec.map(str::to_string),
            filesize,
            url: format!("{scheme}://example.com/{format_id}"),
            protocol: protocol.to_string(),
            http_headers: HashMap::new(),
            has_fragments,
        }
    }

    fn format_ids(video: &ResolvedVideo) -> Vec<&str> {
        video.formats.iter().map(|f| f.format_id.as_str()).collect()
    }

    fn temp_test_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fetchline-ytdlp-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("a temp test dir is created");
        dir
    }

    #[test]
    fn parse_youtube_dash_keeps_real_formats_and_skips_the_rest() {
        let video = parse_dump_json(YOUTUBE_DASH).expect("the dash fixture parses");
        assert_eq!(
            video.title,
            "Rick Astley - Never Gonna Give You Up (Official Video) (4K Remaster)"
        );
        assert_eq!(video.extractor, "youtube");
        assert!(!video.is_live);
        assert!(!video.degraded);
        assert!(video.warnings.is_empty());
        // Kept: 18, 137, 136, 140, 251 and the http-only 17
        // (parsing is faithful to -J; selection drops http).
        // Skipped: 133 (no URL) and sb0 (storyboard).
        assert_eq!(
            format_ids(&video),
            vec!["18", "137", "136", "140", "251", "17"]
        );
        let by_id = |id: &str| video.formats.iter().find(|f| f.format_id == id).unwrap();
        // filesize falls back to filesize_approx.
        assert_eq!(by_id("136").filesize, Some(23_456_789));
        assert_eq!(by_id("251").filesize, Some(2_345_678));
        assert_eq!(by_id("18").filesize, Some(12_345_678));
        // The DASH video-only formats carry their fragments.
        assert!(by_id("137").has_fragments);
        assert!(by_id("136").has_fragments);
        assert!(!by_id("18").has_fragments);
        // Headers survive the parse.
        assert!(by_id("18")
            .http_headers
            .get("User-Agent")
            .map(|v| v.contains("Mozilla"))
            .unwrap_or(false));
        // The http-only format is present after parsing but is
        // never offered: no quality row, no selection.
        let http = by_id("17");
        assert_eq!(http.protocol, "http");
        assert_eq!(select_quality(&video.formats, 144), None);
        assert!(quality_rows(&video).iter().all(|r| r.height != 144));
    }

    #[test]
    fn parse_youtube_sabr_has_only_the_360p_video_format() {
        let video = parse_dump_json(YOUTUBE_SABR).expect("the sabr fixture parses");
        assert_eq!(video.extractor, "youtube");
        // No 137/136: the SABR shape is itag 18 plus audio.
        assert_eq!(format_ids(&video), vec!["18", "140", "251", "17"]);
        let rows = quality_rows(&video);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].height, 360);
        assert!(!rows[0].needs_merge);
    }

    #[test]
    fn parse_tiktok_instagram_facebook() {
        let tiktok = parse_dump_json(TIKTOK).expect("the tiktok fixture parses");
        assert_eq!(tiktok.extractor, "TikTok");
        assert_eq!(format_ids(&tiktok), vec!["720p", "480p"]);
        assert_eq!(tiktok.formats[0].filesize, Some(8_765_432));

        let instagram = parse_dump_json(INSTAGRAM).expect("the instagram fixture parses");
        assert_eq!(instagram.extractor, "Instagram");
        assert_eq!(format_ids(&instagram), vec!["1080p", "720p"]);

        let facebook = parse_dump_json(FACEBOOK).expect("the facebook fixture parses");
        assert_eq!(facebook.extractor, "Facebook");
        assert_eq!(format_ids(&facebook), vec!["720p"]);
        assert_eq!(facebook.formats[0].height, Some(720));
    }

    #[test]
    fn parse_dump_json_rejects_bad_json_and_defaults_the_title() {
        assert!(matches!(
            parse_dump_json("not json at all"),
            Err(YtDlpError {
                kind: YtDlpErrorKind::ParseFailed,
                ..
            })
        ));
        let video = parse_dump_json(r#"{"extractor": "generic", "formats": []}"#)
            .expect("a minimal document parses");
        assert_eq!(video.title, "video");
        assert!(video.formats.is_empty());
        let video =
            parse_dump_json(r#"{"title": "   ", "formats": []}"#).expect("a blank title parses");
        assert_eq!(video.title, "video");
    }

    #[test]
    fn select_quality_prefers_progressive_mp4_then_smallest() {
        let formats = vec![
            test_format(
                "webm",
                "webm",
                Some(720),
                Some("vp9"),
                Some("opus"),
                Some(100),
                "https",
                false,
            ),
            test_format(
                "big",
                "mp4",
                Some(720),
                Some("avc1.4d401f"),
                Some("mp4a.40.2"),
                Some(200),
                "https",
                false,
            ),
            test_format(
                "small",
                "mp4",
                Some(720),
                Some("avc1.4d401f"),
                Some("mp4a.40.2"),
                Some(50),
                "https",
                false,
            ),
            test_format(
                "vonly",
                "mp4",
                Some(720),
                Some("avc1.4d401f"),
                Some("none"),
                Some(90),
                "https",
                false,
            ),
            test_format(
                "aonly",
                "m4a",
                None,
                Some("none"),
                Some("mp4a.40.2"),
                Some(10),
                "https",
                false,
            ),
        ];
        // mp4 beats webm, and among mp4 progressives the smaller file wins.
        assert_eq!(
            select_quality(&formats, 720),
            Some(SelectedQuality::Single(test_format(
                "small",
                "mp4",
                Some(720),
                Some("avc1.4d401f"),
                Some("mp4a.40.2"),
                Some(50),
                "https",
                false
            )))
        );
    }

    #[test]
    fn select_quality_pairs_1080p_video_with_the_best_audio() {
        let video = parse_dump_json(YOUTUBE_DASH).expect("the dash fixture parses");
        match select_quality(&video.formats, 1080) {
            Some(SelectedQuality::Pair { video, audio }) => {
                assert_eq!(video.format_id, "137");
                // m4a/AAC (140) beats opus-in-WebM (251).
                assert_eq!(audio.format_id, "140");
            }
            other => panic!("1080p should pair 137+140, got {other:?}"),
        }
        match select_quality(&video.formats, 360) {
            Some(SelectedQuality::Single(f)) => assert_eq!(f.format_id, "18"),
            other => panic!("360p should be the progressive 18, got {other:?}"),
        }
    }

    #[test]
    fn select_quality_prefers_avc_video_and_m4a_audio_when_pairing() {
        let formats = vec![
            test_format(
                "vp9",
                "webm",
                Some(720),
                Some("vp9"),
                Some("none"),
                Some(10),
                "https",
                false,
            ),
            test_format(
                "avc",
                "mp4",
                Some(720),
                Some("avc1.4d401f"),
                Some("none"),
                Some(20),
                "https",
                false,
            ),
            test_format(
                "opus",
                "webm",
                None,
                Some("none"),
                Some("opus"),
                Some(5),
                "https",
                false,
            ),
            test_format(
                "aac",
                "m4a",
                None,
                Some("none"),
                Some("mp4a.40.2"),
                Some(8),
                "https",
                false,
            ),
        ];
        match select_quality(&formats, 720) {
            Some(SelectedQuality::Pair { video, audio }) => {
                assert_eq!(video.format_id, "avc");
                assert_eq!(audio.format_id, "aac");
            }
            other => panic!("expected an avc+m4a pair, got {other:?}"),
        }
        // A video-only format with no audio to pair is not a pair.
        let no_audio = vec![test_format(
            "vonly",
            "mp4",
            Some(720),
            Some("avc1.4d401f"),
            Some("none"),
            Some(20),
            "https",
            false,
        )];
        assert_eq!(select_quality(&no_audio, 720), None);
    }

    #[test]
    fn select_quality_never_falls_back_to_another_height() {
        let tiktok = parse_dump_json(TIKTOK).expect("the tiktok fixture parses");
        assert_eq!(select_quality(&tiktok.formats, 1080), None);
        assert_eq!(select_quality(&tiktok.formats, 2160), None);
        assert!(matches!(
            select_quality(&tiktok.formats, 480),
            Some(SelectedQuality::Single(_))
        ));
        // SABR shape: no 1080p at all, even though audio exists.
        let sabr = parse_dump_json(YOUTUBE_SABR).expect("the sabr fixture parses");
        assert_eq!(select_quality(&sabr.formats, 1080), None);
    }

    #[test]
    fn select_quality_fragmented_when_nothing_else_fits() {
        // A DASH video-only format with fragments, no audio to pair.
        let formats = vec![test_format(
            "frag",
            "mp4",
            Some(720),
            Some("avc1.4d401f"),
            Some("none"),
            Some(90),
            "https",
            true,
        )];
        assert_eq!(
            select_quality(&formats, 720),
            Some(SelectedQuality::Fragmented(test_format(
                "frag",
                "mp4",
                Some(720),
                Some("avc1.4d401f"),
                Some("none"),
                Some(90),
                "https",
                true
            )))
        );
        // An HLS manifest protocol counts as fragmented too.
        let m3u8 = vec![test_format(
            "hls",
            "mp4",
            Some(480),
            Some("avc1.4d4015"),
            Some("none"),
            None,
            "m3u8",
            true,
        )];
        assert!(matches!(
            select_quality(&m3u8, 480),
            Some(SelectedQuality::Fragmented(_))
        ));
        // But a plain-http fragmented format is still refused.
        let http_frag = vec![test_format(
            "hfrag",
            "mp4",
            Some(480),
            Some("avc1.4d4015"),
            Some("none"),
            None,
            "http",
            true,
        )];
        assert_eq!(select_quality(&http_frag, 480), None);
    }

    #[test]
    fn quality_rows_cover_each_height_once_sorted_with_merge_flags() {
        let video = parse_dump_json(YOUTUBE_DASH).expect("the dash fixture parses");
        let rows = quality_rows(&video);
        let heights: Vec<u32> = rows.iter().map(|r| r.height).collect();
        assert_eq!(heights, vec![360, 720, 1080]);
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            vec!["360p", "720p HD", "1080p HD"]
        );
        // 360p is one progressive file; 720p/1080p need a merge.
        assert_eq!(
            rows.iter().map(|r| r.needs_merge).collect::<Vec<_>>(),
            vec![false, true, true]
        );
        assert!(rows.iter().all(|r| r.has_audio));
        assert!(rows.iter().all(|r| !r.fragmented));
        // Paired filesizes add up (video + audio, both known).
        assert_eq!(rows[0].filesize, Some(12_345_678));
        assert_eq!(rows[1].filesize, Some(23_456_789 + 3_456_789));
        assert_eq!(rows[2].filesize, Some(45_678_901 + 3_456_789));
        assert_eq!(rows[1].audio_format_id.as_deref(), Some("140"));
        assert_eq!(rows[2].audio_format_id.as_deref(), Some("140"));
        assert_eq!(rows[0].audio_format_id, None);
        assert_eq!(rows[2].format_id, "137");
        assert!(rows[2].url.contains("id=137"));
    }

    #[test]
    fn quality_rows_empty_when_only_audio() {
        let video = ResolvedVideo {
            title: "audio only".to_string(),
            is_live: false,
            extractor: "youtube".to_string(),
            formats: vec![test_format(
                "140",
                "m4a",
                None,
                Some("none"),
                Some("mp4a.40.2"),
                Some(100),
                "https",
                false,
            )],
            warnings: Vec::new(),
            degraded: false,
        };
        assert!(quality_rows(&video).is_empty());
    }

    #[test]
    fn format_label_marks_hd_from_720p_up() {
        assert_eq!(format_label(360, true, false), "360p");
        assert_eq!(format_label(480, true, false), "480p");
        assert_eq!(format_label(720, true, false), "720p HD");
        assert_eq!(format_label(1080, false, true), "1080p HD");
        assert_eq!(format_label(2160, true, true), "2160p HD");
        // The flags never change the label.
        assert_eq!(
            format_label(720, false, false),
            format_label(720, true, true)
        );
    }

    #[test]
    fn sanitize_title_filename_handles_illegal_chars_empty_and_long() {
        assert_eq!(
            sanitize_title_filename("My: Video? *Title* <2026>", "mp4"),
            "My_ Video_ _Title_ _2026_.mp4"
        );
        assert_eq!(sanitize_title_filename("", "mp4"), "video.mp4");
        assert_eq!(sanitize_title_filename("   ", "webm"), "video.webm");
        // A title-less input never becomes "videoplayback".
        assert_eq!(sanitize_title_filename("videoplayback", "mp4"), "video.mp4");
        assert_eq!(sanitize_title_filename("", "MP4"), "video.mp4");
        // The extension is appended once, lowercased.
        assert_eq!(sanitize_title_filename("clip.mp4", "mp4"), "clip.mp4");
        assert_eq!(sanitize_title_filename("clip", ".MP4"), "clip.mp4");
        // Leading/trailing dots and spaces are stripped.
        assert_eq!(
            sanitize_title_filename("...Hello...   ", "mp4"),
            "Hello.mp4"
        );
        // Whitespace runs collapse; control chars become "_".
        assert_eq!(sanitize_title_filename("a  b\nc\td", "mp4"), "a b_c_d.mp4");
        // A 150-char title is cut to a 100-char stem.
        let long = "a".repeat(150);
        let got = sanitize_title_filename(&long, "mp4");
        assert_eq!(got.len(), 100 + ".mp4".len());
        assert!(got.starts_with(&"a".repeat(100)));
    }

    #[test]
    fn map_error_covers_every_kind() {
        let cases: Vec<(&str, YtDlpErrorKind)> = vec![
            (
                "ERROR: [youtube] x: Private video. Sign in if you've been granted access",
                YtDlpErrorKind::Private,
            ),
            (
                "ERROR: This video has been removed by the uploader",
                YtDlpErrorKind::Private,
            ),
            ("ERROR: Video unavailable", YtDlpErrorKind::VideoUnavailable),
            // Round 82 — a removed video is its own kind now
            // (VideoUnavailable), not Private and never
            // LoginRequired: YouTube can report it with a
            // LOGIN_REQUIRED playability status, so the
            // unavailable check runs before the login needles.
            // The MUyWv16cGys stderr is the exact text yt-dlp
            // 2026.10.26 prints for it (verified live on this
            // VM); the host used to log it as login_required.
            (
                "ERROR: [youtube] MUyWv16cGys: This video is unavailable",
                YtDlpErrorKind::VideoUnavailable,
            ),
            (
                "WARNING: [youtube] MUyWv16cGys: Playability status: LOGIN_REQUIRED\nERROR: [youtube] MUyWv16cGys: This video is unavailable",
                YtDlpErrorKind::VideoUnavailable,
            ),
            // Round 79 — a genuinely private video that also
            // mentions signing in still maps to Private: the
            // bot-check needles above are specific ("not a bot",
            // "login_required"), so this phrasing falls through.
            (
                "ERROR: [youtube] Private video. Sign in if you've been granted access to view this video",
                YtDlpErrorKind::Private,
            ),
            // Round 81 — the explicit bot check is its own
            // kind now (BotBlocked), not LoginRequired: it is
            // YouTube refusing the request on this connection,
            // not a missing sign-in. The bot-check needles
            // still run before the Private rule, so a blocked
            // video never maps to Private.
            (
                "ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm you're not a bot. This helps protect our community.",
                YtDlpErrorKind::BotBlocked,
            ),
            (
                "ERROR: [youtube] dQw4w9WgXcQ: Video unavailable. Sign in to confirm you're not a bot",
                YtDlpErrorKind::BotBlocked,
            ),
            (
                "WARNING: [youtube] dQw4w9WgXcQ: Playability status: LOGIN_REQUIRED",
                YtDlpErrorKind::LoginRequired,
            ),
            (
                "ERROR: [youtube] This video is not available in your country",
                YtDlpErrorKind::GeoBlocked,
            ),
            (
                "ERROR: This video is geo-restricted",
                YtDlpErrorKind::GeoBlocked,
            ),
            (
                "ERROR: [youtube] Sign in to confirm your age",
                YtDlpErrorKind::LoginRequired,
            ),
            (
                "ERROR: HTTP Error 403: Forbidden",
                YtDlpErrorKind::LoginRequired,
            ),
            (
                "ERROR: Login required to access this video",
                YtDlpErrorKind::LoginRequired,
            ),
            (
                "ERROR: Please log in and pass --cookies to yt-dlp",
                YtDlpErrorKind::LoginRequired,
            ),
            (
                "ERROR: [generic] https://example.com/page: Unsupported URL",
                YtDlpErrorKind::Unsupported,
            ),
            (
                "ERROR: No video found at this URL",
                YtDlpErrorKind::Unsupported,
            ),
            (
                "ERROR: HTTP Error 429: Too Many Requests",
                YtDlpErrorKind::RateLimited,
            ),
            (
                "ERROR: [youtube] Signature extraction failed",
                YtDlpErrorKind::ExtractorOutdated,
            ),
            (
                "ERROR: [youtube] n challenge solving failed",
                YtDlpErrorKind::ExtractorOutdated,
            ),
            (
                "ERROR: No supported JavaScript runtime available",
                YtDlpErrorKind::ExtractorOutdated,
            ),
            (
                "ERROR: [youtube] The extractor is outdated",
                YtDlpErrorKind::ExtractorOutdated,
            ),
            ("ERROR: This video is DRM protected", YtDlpErrorKind::Drm),
            ("ERROR: Widevine protected content", YtDlpErrorKind::Drm),
            (
                "ERROR: This video is a live stream and cannot be downloaded",
                YtDlpErrorKind::Live,
            ),
            (
                "ERROR: Something completely unexpected happened",
                YtDlpErrorKind::Other,
            ),
        ];
        for (stderr, kind) in cases {
            let err = map_error(stderr);
            assert_eq!(err.kind, kind, "for stderr: {stderr}");
            assert!(!err.user_message().is_empty());
            assert!(!err.kind_str().is_empty());
        }
        // "deliver" must not trip the live rule.
        assert_eq!(
            map_error("ERROR: failed to deliver the file").kind,
            YtDlpErrorKind::Other
        );
        // Empty stderr is Other, not a panic and not a made-up kind.
        let empty = map_error("");
        assert_eq!(empty.kind, YtDlpErrorKind::Other);
        assert!(empty.message.contains("no message"));
        // The private message is the exact user-facing wording.
        assert_eq!(
            map_error("ERROR: Private video").user_message(),
            "This video is private or has been removed."
        );
        assert_eq!(
            map_error("ERROR: Signature extraction failed").user_message(),
            "This site changed. Updating the video component may fix it."
        );
    }

    #[test]
    fn parse_sabr_warnings_finds_and_dedupes() {
        let warnings = parse_sabr_warnings(YOUTUBE_SABR_STDERR);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("missing a URL"));
        assert!(is_sabr_degraded(YOUTUBE_SABR_STDERR));
        let dupes = "WARNING: [youtube] SABR streaming is forced\n\
            WARNING: [youtube] SABR streaming is forced\n\
            WARNING: [youtube] Some web https formats have been skipped as they are missing a URL\n\
            [download] Downloading video 1 of 1\n";
        let warnings = parse_sabr_warnings(dupes);
        assert_eq!(warnings.len(), 2);
        assert!(is_sabr_degraded(dupes));
        assert!(parse_sabr_warnings("[download] Downloading video 1 of 1\n").is_empty());
        assert!(!is_sabr_degraded(""));
    }

    #[test]
    fn build_ytdlp_args_has_the_safety_flags_and_nothing_extra() {
        let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        let minimal = ResolveOptions::default();
        assert_eq!(
            build_ytdlp_args(url, &minimal, None),
            vec![
                "--ignore-config",
                "--no-playlist",
                "--dump-json",
                "--no-warnings",
                "--",
                url,
            ]
        );
        let mut opts = ResolveOptions::default();
        opts.paths.qjs = Some(PathBuf::from(r"C:\Sonic Manager\qjs.exe"));
        opts.extractor_args = Some("youtube:player_client=android".to_string());
        let cookies =
            PathBuf::from(r"C:\Users\ADMINI~1\AppData\Local\Temp\fetchline-ytdlp-cookies-x.txt");
        let args = build_ytdlp_args(url, &opts, Some(&cookies));
        assert_eq!(args[0], "--ignore-config");
        assert_eq!(args[1], "--no-playlist");
        assert!(args.contains(&"--js-runtimes".to_string()));
        assert!(args
            .windows(2)
            .any(|w| w[0] == "--js-runtimes" && w[1].starts_with("quickjs:")));
        assert!(args.contains(&"--extractor-args".to_string()));
        assert!(args.contains(&"youtube:player_client=android".to_string()));
        assert!(args.contains(&"--cookies".to_string()));
        // "--" is the last flag, immediately before the URL.
        assert_eq!(args[args.len() - 2], "--");
        assert_eq!(args[args.len() - 1], url);
        // An empty / blank extractor-args adds nothing.
        opts.extractor_args = Some("   ".to_string());
        let args = build_ytdlp_args(url, &opts, None);
        assert!(!args.contains(&"--extractor-args".to_string()));
        // No qjs, no cookies file: neither flag appears.
        opts.paths.qjs = None;
        opts.extractor_args = None;
        let args = build_ytdlp_args(url, &opts, None);
        assert!(!args.contains(&"--js-runtimes".to_string()));
        assert!(!args.contains(&"--cookies".to_string()));
    }

    #[test]
    fn default_host_segment_cap_matches_cdn_suffixes() {
        assert_eq!(
            default_host_segment_cap("rr1---sn-abc.googlevideo.com"),
            Some(8)
        );
        assert_eq!(default_host_segment_cap("googlevideo.com"), Some(8));
        assert_eq!(default_host_segment_cap("GOOGLEVIDEO.COM"), Some(8));
        assert_eq!(default_host_segment_cap("video.fbcdn.net"), Some(4));
        assert_eq!(default_host_segment_cap("fbcdn.net"), Some(4));
        assert_eq!(default_host_segment_cap("v16m.tiktokcdn.com"), Some(2));
        assert_eq!(default_host_segment_cap("tiktokcdn.com"), Some(2));
        assert_eq!(default_host_segment_cap("example.com"), None);
        assert_eq!(default_host_segment_cap("youtube.com"), None);
        // A lookalike domain is not the CDN.
        assert_eq!(default_host_segment_cap("notgooglevideo.com"), None);
        assert_eq!(default_host_segment_cap("googlevideo.com.evil.com"), None);
    }

    #[test]
    fn resolve_options_default_is_30s_with_nothing_else() {
        let opts = ResolveOptions::default();
        assert_eq!(opts.timeout_secs, RESOLVE_TIMEOUT_SECS);
        assert_eq!(opts.timeout_secs, 30);
        assert!(opts.cookies.is_empty());
        assert!(opts.extractor_args.is_none());
    }

    #[test]
    fn write_cookies_tempfile_round_trip_and_drop_deletes() {
        let pairs = vec![
            CookiePair {
                name: "session".to_string(),
                value: "abc123".to_string(),
            },
            CookiePair {
                name: "consent".to_string(),
                value: "yes".to_string(),
            },
        ];
        let path = {
            let file = write_cookies_tempfile("www.youtube.com", &pairs)
                .expect("valid cookies write a temp file");
            assert!(file.path.exists());
            assert!(file
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(YTDLP_COOKIE_FILE_PREFIX));
            let text = std::fs::read_to_string(&file.path).expect("the file reads back");
            assert!(text.starts_with("# Netscape HTTP Cookie File\n"));
            assert!(text.contains(".www.youtube.com\tTRUE\t/\tFALSE\t0\tsession\tabc123"));
            assert!(text.contains(".www.youtube.com\tTRUE\t/\tFALSE\t0\tconsent\tyes"));
            file.path.clone()
        };
        // Dropping the guard deletes the file.
        assert!(!path.exists());
        // An invalid pair is an error, never silently dropped.
        let bad = vec![CookiePair {
            name: "bad name".to_string(),
            value: "x".to_string(),
        }];
        assert!(write_cookies_tempfile("example.com", &bad).is_err());
        // No host, no file.
        assert!(write_cookies_tempfile("", &pairs).is_err());
    }

    #[test]
    fn sweep_stale_cookie_files_removes_only_prefixed_files() {
        let dir = temp_test_dir();
        let stale = dir.join(format!("{YTDLP_COOKIE_FILE_PREFIX}old.txt"));
        let stale2 = dir.join(format!("{YTDLP_COOKIE_FILE_PREFIX}older.txt"));
        let keep = dir.join("important.txt");
        let prefixed_dir = dir.join(format!("{YTDLP_COOKIE_FILE_PREFIX}dir"));
        std::fs::write(&stale, "x").unwrap();
        std::fs::write(&stale2, "x").unwrap();
        std::fs::write(&keep, "x").unwrap();
        std::fs::create_dir(&prefixed_dir).unwrap();
        sweep_stale_cookie_files(&dir);
        assert!(!stale.exists());
        assert!(!stale2.exists());
        assert!(keep.exists());
        // Directories are never removed, even with the prefix.
        assert!(prefixed_dir.exists());
        // A missing dir is a no-op, not a panic.
        sweep_stale_cookie_files(&dir.join("does-not-exist"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn locate_sidecars_prefers_the_bundled_copy_when_no_update_exists() {
        let dir = temp_test_dir();
        std::fs::write(dir.join(binary_name("yt-dlp")), "").unwrap();
        std::fs::write(dir.join(binary_name("ffmpeg")), "").unwrap();
        // No qjs file in the dir: qjs stays None.
        let paths = locate_sidecars(&dir);
        assert_eq!(paths.ytdlp, dir.join(binary_name("yt-dlp")));
        assert_eq!(paths.qjs, None);
        assert_eq!(paths.ffmpeg, Some(dir.join(binary_name("ffmpeg"))));
        // The app-data bin dir is the update location.
        let bin = appdata_bin_dir().expect("an app-data bin dir resolves");
        assert!(bin
            .to_string_lossy()
            .ends_with("com.fetchline.app/fetchline/bin"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn resolve_rejects_non_http_urls_without_spawning() {
        // The binary path is bogus on purpose: InvalidUrl (not
        // BinaryMissing) proves validation ran before any spawn.
        let mut opts = ResolveOptions::default();
        opts.paths.ytdlp = PathBuf::from("/definitely/not/here/yt-dlp");
        for bad in [
            "ftp://example.com/video.mp4",
            "not a url",
            "file:///tmp/x.mp4",
            "",
        ] {
            let err = resolve(bad, &opts)
                .await
                .expect_err("non-http(s) is refused");
            assert_eq!(err.kind, YtDlpErrorKind::InvalidUrl, "for url: {bad:?}");
        }
    }

    #[tokio::test]
    async fn resolve_reports_binary_missing_without_hanging() {
        let mut opts = ResolveOptions::default();
        opts.paths.ytdlp = PathBuf::from("/definitely/not/here/yt-dlp");
        opts.timeout_secs = 5;
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            resolve("https://www.youtube.com/watch?v=dQw4w9WgXcQ", &opts),
        )
        .await;
        let err = result
            .expect("resolve returned within the test timeout")
            .expect_err("a missing binary is an error");
        assert_eq!(err.kind, YtDlpErrorKind::BinaryMissing);
        assert_eq!(
            err.user_message(),
            "yt-dlp is missing — reinstall Sonic Manager or update it from Settings"
        );
    }

    #[test]
    fn sha256_hex_matches_the_known_digest() {
        // SHA-256("abc") is the textbook test vector.
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn parse_sha256sums_finds_the_right_file() {
        let sums = "aaaa000000000000000000000000000000000000000000000000000000000000  other.exe\n\
            ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  yt-dlp.exe\n\
            bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb *yt-dlp.tar.gz\n";
        assert_eq!(
            parse_sha256sums(sums, "yt-dlp.exe"),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string())
        );
        assert_eq!(parse_sha256sums(sums, "missing.exe"), None);
        // A short (non-SHA-256) hex field is not a checksum.
        assert_eq!(parse_sha256sums("abcd  yt-dlp.exe\n", "yt-dlp.exe"), None);
        // Uppercase hex is normalised to lowercase.
        assert_eq!(
            parse_sha256sums(
                "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD  yt-dlp.exe\n",
                "yt-dlp.exe"
            ),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string())
        );
    }

    #[test]
    fn clean_windows_path_strips_the_verbatim_prefix() {
        assert_eq!(
            clean_windows_path(Path::new(r"\\?\C:\Sonic Manager\qjs.exe")),
            r"C:\Sonic Manager\qjs.exe"
        );
        assert_eq!(
            clean_windows_path(Path::new(r"\\?\UNC\server\share\qjs.exe")),
            r"\\server\share\qjs.exe"
        );
        assert_eq!(
            clean_windows_path(Path::new(r"C:\Sonic Manager\qjs.exe")),
            r"C:\Sonic Manager\qjs.exe"
        );
    }

    #[tokio::test]
    async fn ytdlp_version_is_none_when_the_binary_is_missing() {
        let paths = YtDlpPaths {
            ytdlp: PathBuf::from("/definitely/not/here/yt-dlp"),
            ..Default::default()
        };
        assert_eq!(ytdlp_version(&paths).await, None);
    }
}
