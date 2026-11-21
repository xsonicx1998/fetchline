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

use std::collections::{HashMap, VecDeque};
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

/// Round 83 — the direct yt-dlp download's format selector:
/// the exact format id first, then a fallback chain. A
/// format id can differ between extractions (TikTok's do), so
/// `-f <id>` alone fails with "Requested format is not
/// available" when the re-extraction returns a different id;
/// the chain falls back to the best mp4, then the best format,
/// instead of exiting 1. An id that already IS a selector
/// (contains '/') is left alone.
pub fn direct_download_format_selector(format_id: &str) -> String {
    let id = format_id.trim();
    if id.is_empty() {
        return "b[ext=mp4]/b".to_string();
    }
    if id.contains('/') {
        return id.to_string();
    }
    format!("{id}/b[ext=mp4]/b")
}

/// Round 83 — redact a yt-dlp argv / output tail before it is
/// logged or shown: URL query strings are cut (signed stream
/// URLs carry tokens) and temp cookie-file paths are masked
/// (the file itself is deleted on drop, but its name must
/// never appear in a log). Local save paths are left alone.
pub fn redact_ytdlp_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if start < i {
                out.push_str(&redact_ytdlp_token(&text[start..i]));
            }
            out.push(c);
            start = i + c.len_utf8();
        }
    }
    if start < text.len() {
        out.push_str(&redact_ytdlp_token(&text[start..]));
    }
    out
}

fn redact_ytdlp_token(token: &str) -> String {
    if token.contains(YTDLP_COOKIE_FILE_PREFIX) {
        return "<cookies-file>".to_string();
    }
    if let Some(scheme) = token.find("://") {
        if let Some(q) = token[scheme..].find('?') {
            let cut = scheme + q;
            return format!("{}?...", &token[..cut]);
        }
    }
    token.to_string()
}

/// Round 83 — parse a `Cookie` header value ("a=b; c=d") back
/// into name/value pairs for a temp Netscape file. Splits on
/// the FIRST '=' (values may contain '=' — base64 padding),
/// trims whitespace, skips empty names. Validation happens in
/// `write_cookies_tempfile` (via `cookies_to_header`), so an
/// invalid pair is an error there, never silently dropped.
pub fn cookie_header_to_pairs(header: &str) -> Vec<CookiePair> {
    header
        .split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let (name, value) = match part.split_once('=') {
                Some((n, v)) => (n.trim(), v.trim()),
                None => (part, ""),
            };
            if name.is_empty() {
                return None;
            }
            Some(CookiePair {
                name: name.to_string(),
                value: value.to_string(),
                // A `Cookie` header string carries no scope.
                domain: None,
                path: None,
            })
        })
        .collect()
}

/// Round 83 — the last lines of a yt-dlp run's stdout and
/// stderr, kept in ring buffers so a failed run can say WHY it
/// failed instead of only its exit code. Both streams are
/// kept: yt-dlp prints `[download]` progress to STDOUT
/// (verified live with yt-dlp 2026.10.26: one `[download]`
/// line on stdout, zero on stderr for a local download) while
/// errors go to stderr — the old code drained stdout and
/// discarded it and parsed only stderr for progress, so
/// progress never updated on the direct path and the stderr
/// tail was lost with it.
pub const YTDLP_TAIL_LINES: usize = 40;

#[derive(Debug, Default)]
pub struct YtDlpOutputTail {
    stdout: VecDeque<String>,
    stderr: VecDeque<String>,
}

impl YtDlpOutputTail {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_stdout(&mut self, line: &str) {
        if self.stdout.len() >= YTDLP_TAIL_LINES {
            self.stdout.pop_front();
        }
        self.stdout.push_back(line.to_string());
    }

    pub fn push_stderr(&mut self, line: &str) {
        if self.stderr.len() >= YTDLP_TAIL_LINES {
            self.stderr.pop_front();
        }
        self.stderr.push_back(line.to_string());
    }

    pub fn is_empty(&self) -> bool {
        self.stdout.is_empty() && self.stderr.is_empty()
    }

    /// The full tail for the log: stderr first (the verdict),
    /// then stdout.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if !self.stderr.is_empty() {
            out.push_str("--- stderr (last lines) ---\n");
            for line in &self.stderr {
                out.push_str(line);
                out.push('\n');
            }
        }
        if !self.stdout.is_empty() {
            out.push_str("--- stdout (last lines) ---\n");
            for line in &self.stdout {
                out.push_str(line);
                out.push('\n');
            }
        }
        out.trim_end().to_string()
    }

    /// Round 85 — the tail as ONE line, for the log: the same
    /// lines `render` shows, joined with " | " instead of
    /// newlines, so the tail text sits on the same log line as
    /// `tail:` and a grep / Select-String for the reason finds
    /// it. The caller redacts it first (`redact_ytdlp_text`),
    /// exactly like the multi-line render.
    pub fn render_one_line(&self) -> String {
        self.render().replace('\n', " | ")
    }

    /// The text the failure is mapped from: the stderr lines
    /// (the verdict lives there), falling back to stdout when
    /// stderr is empty. The combined `render` (with its
    /// "--- stderr ---" / "--- stdout ---" headers) must never
    /// be mapped — its headers would leak into the user message.
    pub fn error_text(&self) -> String {
        if self.stderr.is_empty() {
            self.stdout.iter().cloned().collect::<Vec<_>>().join("\n")
        } else {
            self.stderr.iter().cloned().collect::<Vec<_>>().join("\n")
        }
    }

    /// The short tail for the error message: the last stderr
    /// lines, falling back to stdout when stderr is empty.
    pub fn render_short(&self) -> String {
        let lines: &VecDeque<String> = if self.stderr.is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        lines
            .iter()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
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
    /// Round 85 — the CDN refused the request itself (HTTP
    /// 403/401 on the stream URL), NOT a missing sign-in: the
    /// video plays in the browser and the browser's cookies were
    /// already sent where the flow has them. Mapping every 403
    /// to LoginRequired told a signed-in user to sign in when
    /// the site was refusing the request — the R84 TikTok /
    /// YouTube 2160p "needs sign-in" misreport. `needs_login`
    /// is false for this kind (the host answers that from
    /// `kind == LoginRequired`).
    CdnRefused,
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
            YtDlpErrorKind::CdnRefused => "cdn_refused",
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
                // Round 83 — the cloud-server hint: this is
                // expected datacenter-IP behaviour (Looney Tunes
                // Fg_KxhW8-PY, C-IXI604uJ4, 4GzCcun60TQ all
                // bot-blocked from the VPS even WITH cookies);
                // no proxy / PO-token workaround is built.
                "YouTube is blocking this video on this connection. It plays in your browser, but the download can't reach it right now. This is common on cloud servers; it normally works on a home connection."
                    .to_string()
            }
            YtDlpErrorKind::VideoUnavailable => {
                "This video is unavailable — it may have been removed from YouTube.".to_string()
            }
            YtDlpErrorKind::CdnRefused => {
                "The site refused the download request (HTTP 403)".to_string()
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
            // Round 83 — the cloud-server hint (same text as
            // user_message): expected datacenter-IP behaviour,
            // normally fine on a home connection.
            "YouTube is blocking this video on this connection. This is common on cloud servers; it normally works on a home connection.",
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
    // Round 85 — the CDN refused the request itself (HTTP
    // 403/401 on the stream URL): NOT a missing sign-in. The
    // needles are the explicit HTTP-error phrasings, so a
    // "please log in" line still falls through to the login
    // rule below. Runs before the catch-all, which used to map
    // ANY bare "403" / "401" / "cookies" to LoginRequired —
    // the R84 "needs sign-in" misreport on TikTok and YouTube
    // 2160p. "HTTP Error 403: Forbidden" reaches this rule
    // (it contains both "http error 403" and "403: forbidden").
    if has("http error 403") || has("403: forbidden") || has("401") {
        return err(
            YtDlpErrorKind::CdnRefused,
            "The site refused the download request (HTTP 403).",
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
    // Round 85 — bare "403" / "401" / "cookies" no longer
    // reach here: a bare 403/401 is CdnRefused (the rule
    // above) and "cookies" alone is not a login wall. Only
    // explicit sign-in wording stays LoginRequired.
    if has("sign in") || has("login required") || has("log in") {
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

/// Counts from one `-J` document: how many formats yt-dlp
/// listed, how many survived the parse, and why the rest did
/// not — the evidence behind the zero-format diagnosis
/// (`zero_format_diagnosis`). `protocols` is a histogram of
/// the raw formats' protocols, in first-seen order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParseDiagnostics {
    pub raw_formats: usize,
    pub kept_formats: usize,
    pub skipped_no_url: usize,
    pub skipped_storyboard: usize,
    pub protocols: Vec<(String, usize)>,
    pub has_top_level_url: bool,
    pub unwrapped_single_entry_playlist: bool,
}

/// Why one format object was not a downloadable format.
enum FormatSkip {
    NoUrl,
    Storyboard,
}

/// "1280x720" (or "720x1280") into (width, height).
fn parse_resolution(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.split_once('x')?;
    let w: u32 = w.trim().parse().ok()?;
    let h: u32 = h.trim().parse().ok()?;
    if w == 0 || h == 0 {
        None
    } else {
        Some((w, h))
    }
}

/// One format object from a `-J` document, or the reason it is
/// not a downloadable format: no URL (the SABR shape — a
/// format yt-dlp knows about but cannot give a URL for is not
/// downloadable), or a storyboard (a sheet of thumbnails in an
/// .mhtml wrapper, or a format with neither codec). `filesize`
/// falls back to `filesize_approx` — an estimate is honest
/// enough for a size label, and no size at all is worse.
/// Height falls back to the `resolution` string ("1280x720")
/// when yt-dlp reports no height; codecs stay absent when
/// yt-dlp omits them — a missing codec is "unknown", never
/// invented, and selection treats it as such (see
/// `select_quality` tier 5 and the `quality_rows` fallback).
fn format_from_json(f: &serde_json::Value) -> Result<ResolvedFormat, FormatSkip> {
    let Some(url) = f
        .get("url")
        .and_then(|u| u.as_str())
        .filter(|u| !u.is_empty())
    else {
        return Err(FormatSkip::NoUrl);
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
        return Err(FormatSkip::Storyboard);
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
    let (mut width, mut height) = (
        f.get("width").and_then(|v| v.as_u64()).map(|w| w as u32),
        f.get("height").and_then(|v| v.as_u64()).map(|h| h as u32),
    );
    if width.is_none() || height.is_none() {
        let res = f
            .get("resolution")
            .and_then(|r| r.as_str())
            .and_then(parse_resolution);
        if let Some((rw, rh)) = res {
            if width.is_none() {
                width = Some(rw);
            }
            if height.is_none() {
                height = Some(rh);
            }
        }
    }
    Ok(ResolvedFormat {
        format_id: f.get("format_id").map(json_text).unwrap_or_default(),
        ext: f
            .get("ext")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        height,
        width,
        vcodec,
        acodec,
        filesize,
        url: url.to_string(),
        protocol,
        http_headers,
        has_fragments,
    })
}

/// Parse one `yt-dlp --dump-json` document. The shape is
/// yt-dlp's, not ours: a flat object with a `formats` array,
/// where each format carries its own codec pair, size, URL,
/// protocol and headers. Two shapes that used to come back
/// empty are handled here, because yt-dlp really does emit
/// them: a single-entry `_type: "playlist"` wrapper (the
/// one video inside is the video), and a document with no
/// `formats` array but a top-level `url` (the whole document
/// IS one format — the top-level fields are that format's).
/// The caller attaches SABR warnings afterwards
/// (`parse_sabr_warnings`), because degradation is a property
/// of the stderr, not of the JSON.
pub fn parse_dump_json(json: &str) -> Result<ResolvedVideo, YtDlpError> {
    parse_dump_json_with_diagnostics(json).map(|(video, _)| video)
}

/// `parse_dump_json` plus the counts behind it (see
/// `ParseDiagnostics`): how many formats yt-dlp listed, how
/// many survived, and why the rest did not. Pure, so the
/// zero-format diagnosis (`zero_format_diagnosis`) and the
/// tests can inspect exactly what the document held.
pub fn parse_dump_json_with_diagnostics(
    json: &str,
) -> Result<(ResolvedVideo, ParseDiagnostics), YtDlpError> {
    let mut value: serde_json::Value = serde_json::from_str(json).map_err(|e| {
        YtDlpError::new(
            YtDlpErrorKind::ParseFailed,
            format!("Could not read yt-dlp's video data: {e}"),
        )
    })?;
    let mut diag = ParseDiagnostics::default();
    // A single-entry playlist IS one video: unwrap it, so the
    // formats inside are the video's formats. A multi-entry
    // playlist is left alone — `--no-playlist` means it should
    // never arrive, and picking the first entry would silently
    // download the wrong video.
    if value.get("_type").and_then(|t| t.as_str()) == Some("playlist") {
        let single = value
            .get("entries")
            .and_then(|e| e.as_array())
            .filter(|entries| entries.len() == 1)
            .map(|entries| entries[0].clone());
        if let Some(entry) = single {
            value = entry;
            diag.unwrapped_single_entry_playlist = true;
        }
    }
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
    diag.has_top_level_url = value
        .get("url")
        .and_then(|u| u.as_str())
        .map(|u| !u.is_empty())
        .unwrap_or(false);
    let mut formats = Vec::new();
    if let Some(list) = value.get("formats").and_then(|f| f.as_array()) {
        diag.raw_formats = list.len();
        for f in list {
            if let Some(p) = f
                .get("protocol")
                .and_then(|p| p.as_str())
                .filter(|p| !p.is_empty())
            {
                match diag.protocols.iter_mut().find(|(name, _)| name == p) {
                    Some((_, n)) => *n += 1,
                    None => diag.protocols.push((p.to_string(), 1)),
                }
            }
            match format_from_json(f) {
                Ok(fmt) => formats.push(fmt),
                Err(FormatSkip::NoUrl) => diag.skipped_no_url += 1,
                Err(FormatSkip::Storyboard) => diag.skipped_storyboard += 1,
            }
        }
    }
    // No formats array, but a top-level URL: yt-dlp hands some
    // pages back as one format, not a list — the top-level
    // fields (format id, ext, height, codecs, headers) are
    // that format's, so read them the same way. This also
    // covers a formats array whose every entry was skipped:
    // the top-level URL is yt-dlp's default format, and it is
    // a real file.
    if formats.is_empty() && diag.has_top_level_url {
        match format_from_json(&value) {
            Ok(fmt) => formats.push(fmt),
            Err(FormatSkip::NoUrl) => diag.skipped_no_url += 1,
            Err(FormatSkip::Storyboard) => diag.skipped_storyboard += 1,
        }
    }
    diag.kept_formats = formats.len();
    Ok((
        ResolvedVideo {
            title,
            is_live,
            extractor,
            formats,
            warnings: Vec::new(),
            degraded: false,
        },
        diag,
    ))
}

/// The ONE line for the log when a resolve returns no usable
/// quality rows: the raw format count, how many were skipped
/// and why, the protocol histogram, whether a top-level URL
/// existed, and how long the resolve took. Count-only — no
/// URLs, no titles, no cookie values. Returns `None` when
/// there ARE rows (nothing to explain), and when yt-dlp
/// listed nothing at all for a NON-video URL (a feed page —
/// the dialog's "no video here" message already says that,
/// and a log line would add nothing).
///
/// Round 87 scope update — the result is IP-dependent:
/// Facebook reels WORK from the user's home PC but fail on
/// the VPS (EC2 IP), and the `-J` itself varies by IP — the
/// fixture reel fetched from this VM lists `sd`/`hd` with no
/// height and no codecs, while the R82 fixture (and the home
/// PC's resolve) carries full height/codec fields. So a zero
/// has TWO causes, and the line must tell them apart:
/// (1) yt-dlp listed formats but none survived — the parser
/// dropped them (the line names the counts); (2) yt-dlp
/// listed NOTHING for a video-page URL — the site served no
/// video to this IP (login wall / checkpoint / region block),
/// not a feed page. Case (2) gets its own line, naming the
/// host, so the next VPS retest can tell a site block from a
/// parser bug without re-running anything. The caller puts
/// the line first in `warnings`, so it rides the existing
/// warning channel to the dialog and the extension reply.
pub fn zero_format_diagnosis(
    video: &ResolvedVideo,
    diag: &ParseDiagnostics,
    url: &str,
    elapsed_ms: u64,
) -> Option<String> {
    if !quality_rows(video).is_empty() {
        return None;
    }
    if diag.raw_formats == 0 && !diag.has_top_level_url {
        // Nothing listed at all: silent for a feed page, but
        // a video-page URL that yields nothing is evidence —
        // the site served no video to THIS IP.
        if !is_video_page_url(url) {
            return None;
        }
        let host = reqwest::Url::parse(url.trim())
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .map(|h| h.strip_prefix("www.").unwrap_or(&h).to_string())
            .unwrap_or_default();
        return Some(format!(
            "resolve returned no usable formats: yt-dlp listed 0 formats for a video page on {host} — the site may have served no video to this IP (login wall / checkpoint / region block), not a feed page; {elapsed_ms}ms",
        ));
    }
    let protocols = if diag.protocols.is_empty() {
        "none".to_string()
    } else {
        diag.protocols
            .iter()
            .map(|(p, n)| format!("{p}×{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    Some(format!(
        "resolve returned no usable formats: yt-dlp listed {} format(s), kept {}, quality rows 0; skipped: no URL {}, storyboard {}; protocols: {}; top-level url: {}; {}ms",
        diag.raw_formats,
        diag.kept_formats,
        diag.skipped_no_url,
        diag.skipped_storyboard,
        protocols,
        if diag.has_top_level_url { "yes" } else { "no" },
        elapsed_ms
    ))
}

/// True when a URL is a platform video page (YouTube /
/// TikTok / Instagram / Facebook) — used only by the
/// zero-format diagnosis, to tell "the site served nothing
/// to this IP" apart from "a feed page". Deliberately
/// STRICTER than the dialog's `isVideoPageUrl` (which
/// decides "resolve this URL" and lets the zero-rows result
/// speak): a bare homepage or feed path is NOT a video page
/// here — facebook.com/, youtube.com/, instagram.com/ and
/// tiktok.com/ are feeds, /watch/ without a `v` is the
/// Watch feed — so an empty resolve for one stays silent
/// instead of crying "site block".
pub fn is_video_page_url(raw_url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(raw_url.trim()) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    let raw_host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    let host = raw_host.strip_prefix("www.").unwrap_or(&raw_host);
    if host == "youtu.be" {
        return true;
    }
    if host == "youtube.com" || host.ends_with(".youtube.com") {
        let path = parsed.path();
        return path == "/watch"
            || path.starts_with("/watch/")
            || path.starts_with("/shorts/")
            || path.starts_with("/live/")
            || path.starts_with("/embed/")
            || path.starts_with("/v/");
    }
    if host == "tiktok.com" || host.ends_with(".tiktok.com") {
        // /@user/video/<id> is a video; /, /following and
        // /explore are feeds.
        return parsed.path().contains("/video/");
    }
    if host == "instagram.com" || host.ends_with(".instagram.com") {
        let path = parsed.path();
        return path.starts_with("/reel/") || path.starts_with("/p/") || path.starts_with("/tv/");
    }
    if host == "fb.watch" || host.ends_with(".fb.watch") {
        return true;
    }
    if host == "facebook.com" || host.ends_with(".facebook.com") {
        let path = parsed.path();
        if path.starts_with("/reel/") || path.contains("/videos/") {
            return true;
        }
        // /watch/?v=<id> is a video; /watch/ on its own is
        // the Watch feed (the Round 80 case).
        if (path == "/watch" || path.starts_with("/watch/"))
            && parsed.query_pairs().any(|(k, v)| k == "v" && !v.is_empty())
        {
            return true;
        }
    }
    false
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
    // (5) Unknown codecs: a plain https file at this height
    // whose codecs yt-dlp did not report AT ALL — both fields
    // absent, not "none" (Facebook reels list `sd`/`hd` with
    // no vcodec/acodec — verified live against `-J`
    // 2026-11-21 — and the files ARE progressive mp4 with
    // audio). Only reached when no known-good progressive,
    // pair or fragmented pick exists, so a real codec never
    // loses to an unknown one. A video-only file (real vcodec,
    // no audio to pair) still gets no pick here — that is the
    // old behaviour, kept on purpose.
    let unknown = formats.iter().filter(|f| {
        f.protocol == "https"
            && f.height == Some(height)
            && !f.has_fragments
            && !f.ext.is_empty()
            && f.vcodec.is_none()
            && f.acodec.is_none()
    });
    if let Some(best) = unknown.min_by_key(|f| (f.ext != "mp4", f.filesize.unwrap_or(u64::MAX))) {
        return Some(SelectedQuality::Single(best.clone()));
    }
    None
}

/// Round 85 — the best audio-only format for a DASH re-resolve:
/// the same audio pick `select_quality` makes for a pair (https
/// only, no video codec, a real audio codec; M4A/AAC in mp4
/// first, then smallest file), without a height to match. The
/// DASH parent stores no audio format id, so the re-resolve
/// finds the audio half this way instead of guessing it.
pub(crate) fn best_audio_only_format(formats: &[ResolvedFormat]) -> Option<&ResolvedFormat> {
    formats
        .iter()
        .filter(|f| f.protocol == "https" && codec_is_absent(&f.vcodec) && codec_is_real(&f.acodec))
        .min_by_key(|f| {
            (
                !audio_is_m4a_aac(f),
                f.ext != "mp4",
                f.filesize.unwrap_or(u64::MAX),
            )
        })
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
///
/// Round 87 — no height anywhere: some extractors list
/// formats with no height at all (Facebook reels: `sd` /
/// `hd`, no height, no codecs — the live `-J`, 2026-11-21 —
/// and the files are 360p / 1080p progressive mp4). No height
/// is invented: the row's height is 0 and its label is the
/// format's own name, so the picker shows "sd" / "hd" instead
/// of a made-up "720p". Rows keep yt-dlp's own order (worst
/// first), so the last row stays the best pick.
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
    // No height-keyed row at all: fall back to one row per
    // downloadable height-less file (see the doc comment
    // above). The same shape `select_quality` tier 5 accepts
    // at a known height, minus the height: a plain https
    // single file whose codecs yt-dlp did not report at all
    // — not a manifest, not audio-only, not a format that
    // cannot carry video.
    if rows.is_empty() {
        for f in video.formats.iter().filter(|f| {
            f.protocol == "https"
                && !f.has_fragments
                && !f.ext.is_empty()
                && f.vcodec.is_none()
                && f.acodec.is_none()
        }) {
            rows.push(QualityRow {
                height: 0,
                label: if f.format_id.is_empty() {
                    "Video".to_string()
                } else {
                    f.format_id.clone()
                },
                ext: f.ext.clone(),
                has_audio: codec_is_real(&f.acodec),
                needs_merge: false,
                filesize: f.filesize,
                url: f.url.clone(),
                audio_url: None,
                audio_format_id: None,
                format_id: f.format_id.clone(),
                headers: f.http_headers.clone(),
                fragmented: false,
            });
        }
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

/// Round 87.1 — is this stem a reserved Windows device name
/// (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`)?
/// Case-insensitive, stem only (no extension — the caller
/// splits that off first). Windows refuses to create such a
/// file at all, so a video titled e.g. "CON" fails with os
/// error 123 before a single byte; renaming it (`CON_`) keeps
/// the download working. `CONSOLE` / `COM10` / `LPT0` are NOT
/// reserved and pass through unchanged.
pub(crate) fn is_reserved_windows_device_name(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0')
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
    // Round 87.1 — a reserved Windows device name as the whole
    // stem (`CON`, `COM1`, …) can never be created on Windows;
    // rename it, keeping the extension. The title may already
    // carry `.ext`, so the check sees the stem WITHOUT it; the
    // 100-char cut above cannot produce a reserved stem (they
    // are ≤ 4 chars), and the check runs after it so a long
    // title that truncates onto a reserved word is covered too.
    {
        let mut bare = stem.as_str();
        if !ext.is_empty() {
            let suffix = format!(".{ext}");
            if bare.to_ascii_lowercase().ends_with(&suffix) {
                bare = &bare[..bare.len() - suffix.len()];
            }
        }
        if is_reserved_windows_device_name(bare) {
            stem = if bare.len() < stem.len() {
                format!("{bare}_.{ext}")
            } else {
                format!("{bare}_")
            };
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
/// Round 84 — `tiktok.com` at 1: the R83 retest's TikTok CDN
/// host was `v16-webapp-prime.us.tiktok.com` (the videoplayback
/// host yt-dlp resolves), NOT a tiktokcdn.com host, so the old
/// tiktokcdn.com cap never applied and the engine could open
/// several parallel range requests against a host that 403s
/// them. One connection for any tiktok.com host.
pub fn default_host_segment_cap(host: &str) -> Option<usize> {
    let host = host.trim().to_ascii_lowercase();
    for (suffix, cap) in [
        ("googlevideo.com", 8usize),
        ("fbcdn.net", 4usize),
        ("tiktokcdn.com", 2usize),
        ("tiktok.com", 1usize),
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

/// Inputs for one direct (B) yt-dlp download — everything
/// `build_direct_ytdlp_args` needs, in one place, so the arg
/// list is built exactly once and can be unit-tested.
pub struct DirectYtDlpArgs {
    /// `-f` selector (see `direct_download_format_selector`).
    pub format_selector: String,
    /// `-P` directory, when the save path has a parent dir.
    pub out_dir: Option<String>,
    /// `-o` template (stem + ".%(ext)s", or the whole save path
    /// with ".%(ext)s" when there is no output dir).
    pub out_template: String,
    /// Temp Netscape cookies file, when the download carries
    /// browser cookies.
    pub cookies_file: Option<PathBuf>,
    /// `--add-header User-Agent: ...` (the format's UA).
    pub user_agent: Option<String>,
    /// `--add-header Referer: ...` (the page URL).
    pub referer: Option<String>,
    /// Directory holding ffmpeg + ffprobe (the caller checks
    /// BOTH are present) — passed as `--ffmpeg-location`.
    pub ffmpeg_dir: Option<PathBuf>,
    /// QuickJS sidecar — passed as `--js-runtimes quickjs:<path>`.
    pub qjs: Option<PathBuf>,
    /// The page URL, always the LAST argument, after `--`.
    pub source_url: String,
}

/// Build the yt-dlp argv for a direct (B) download. Pure, so
/// the tests can inspect exactly what will run — and so the
/// arg ORDER is pinned: every flag rides BEFORE the "--"
/// separator and the URL is last.
///
/// Round 78 — the (B) fallback merges with yt-dlp itself, so
/// it must be told where the bundled ffmpeg/ffprobe live (they
/// are not on PATH): without `--ffmpeg-location` a merge fails
/// with "ffprobe and ffmpeg not found". yt-dlp needs BOTH for a
/// merge — a plain ffmpeg.exe is not enough (verified against
/// yt-dlp's README). The JS solver is needed here too: a
/// YouTube format that needs signature solving fails without it.
///
/// Round 85 (Claude delta) — the R84 bug was these two flags
/// appended to the COMMAND after `argv` already ended with
/// "--" + URL, so yt-dlp's parser took them as extra
/// positional URLs — five URLs with a fixed `-o` name is
/// exactly the "Fixed output name but more than one file to
/// download" error (yt-dlp 2026.10.26 YoutubeDL.download:
/// `len(url_list) > 1` with no '%' in the template). The log
/// hid it: it printed only `argv`, and the trailing options
/// were added to `cmd` after it, so the logged command looked
/// identical to the working hand run. The fix is structural:
/// the caller passes EXACTLY this list to Command and logs
/// EXACTLY this list (redacted) — nothing is ever added to
/// the command afterwards.
pub fn build_direct_ytdlp_args(a: &DirectYtDlpArgs) -> Vec<String> {
    let mut args = vec![
        "--ignore-config".to_string(),
        "--no-playlist".to_string(),
        "-f".to_string(),
        a.format_selector.clone(),
    ];
    if let Some(dir) = &a.out_dir {
        args.push("-P".to_string());
        args.push(dir.clone());
        args.push("-o".to_string());
        args.push(a.out_template.clone());
    } else {
        args.push("-o".to_string());
        args.push(a.out_template.clone());
    }
    if let Some(file) = &a.cookies_file {
        args.push("--cookies".to_string());
        args.push(file.display().to_string());
    }
    if let Some(ua) = a
        .user_agent
        .as_deref()
        .filter(|s| !s.contains(['\r', '\n']))
    {
        args.push("--add-header".to_string());
        args.push(format!("User-Agent: {ua}"));
    }
    if let Some(referer) = a.referer.as_deref().filter(|s| !s.contains(['\r', '\n'])) {
        args.push("--add-header".to_string());
        args.push(format!("Referer: {referer}"));
    }
    if let Some(dir) = &a.ffmpeg_dir {
        args.push("--ffmpeg-location".to_string());
        args.push(clean_windows_path(dir));
    }
    if let Some(qjs) = &a.qjs {
        args.push("--js-runtimes".to_string());
        args.push(format!("quickjs:{}", clean_windows_path(qjs)));
    }
    // Round 85 (Jumani) — force the merged container to mp4:
    // with the ".%(ext)s" template a DASH merge could otherwise
    // land as webm, and yt-dlp aborts with "Fixed output name"
    // on separate streams/sidecars; the mp4 merge makes
    // `find_direct_ytdlp_output`'s webm-rename path a fallback
    // only. Like every other flag, it rides BEFORE "--".
    args.push("--merge-output-format".to_string());
    args.push("mp4".to_string());
    args.push("--".to_string());
    args.push(a.source_url.clone());
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
    // Round 87 — the resolve clock, for the zero-format
    // diagnosis (`zero_format_diagnosis` reports it).
    let started = std::time::Instant::now();
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
    let (mut video, diag) = parse_dump_json_with_diagnostics(&stdout)?;
    video.warnings = parse_sabr_warnings(&stderr);
    video.degraded = is_sabr_degraded(&stderr);
    // Round 87 — a resolve that returns no usable rows gets
    // ONE diagnosis line, first in `warnings`, so it rides
    // the existing warning channel to the dialog and the
    // extension reply instead of vanishing as a bare "no
    // formats". Nothing is logged when yt-dlp listed nothing
    // at all for a non-video URL (a feed page — see
    // `zero_format_diagnosis`); a video-page URL that yields
    // nothing gets the site-block line instead.
    if let Some(line) =
        zero_format_diagnosis(&video, &diag, url, started.elapsed().as_millis() as u64)
    {
        video.warnings.insert(0, line);
    }
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

/// One browser cookie with the scope the browser reported
/// for it: the domain it was set for and the path it is
/// scoped to. `CookiePair` (native_host) carries name and
/// value only, so a scoped cookie is what the extension sends
/// and what the Netscape writer needs — the plain
/// `write_cookies_tempfile` builds these with no scope and
/// falls back to the page host, exactly as before. Until the
/// host carries the scope through (`CookiePair` has no
/// domain/path fields yet), the scoped writer is used by the
/// tests and by any caller that already has scoped cookies;
/// the resolve path still goes through the plain writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedCookie {
    pub name: String,
    pub value: String,
    /// The cookie's own domain, as the browser reported it
    /// (".facebook.com" for a domain cookie,
    /// "video-ord5-2.xx.fbcdn.net" for a host-only one) —
    /// `None` means "the page host", the old behaviour.
    pub domain: Option<String>,
    /// The cookie's own path ("/" or a deeper path) —
    /// `None` means "/".
    pub path: Option<String>,
}

impl ScopedCookie {
    /// A `CookiePair` with its own scope when the browser
    /// reported one — a CDN-host cookie keeps its host (the
    /// plain writer routes through here, so `resolve()` and
    /// the direct retry keep the scope too). `None` means the
    /// old behaviour: the page host and "/".
    pub fn from_pair(pair: &CookiePair) -> Self {
        Self {
            name: pair.name.clone(),
            value: pair.value.clone(),
            domain: pair.domain.clone(),
            path: pair.path.clone(),
        }
    }
}

/// A cookie domain for a Netscape file: no whitespace,
/// control characters, slashes or separators, no "..", no
/// trailing dot. A single leading dot is the domain-cookie
/// form and is kept as-is.
fn valid_cookie_domain(domain: &str) -> bool {
    let d = domain.trim();
    !d.is_empty()
        && d.len() <= 253
        && !d.ends_with('.')
        && !d.contains("..")
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// A cookie path for a Netscape file: starts with "/", no
/// whitespace or control characters (those would break the
/// tab-separated line).
fn valid_cookie_path(path: &str) -> bool {
    !path.is_empty()
        && path.starts_with('/')
        && path.chars().all(|c| !c.is_whitespace() && !c.is_control())
}

/// Write the browser's session cookies for one page host to a
/// temp Netscape cookies file for yt-dlp's `--cookies`.
///
/// Each cookie is written with ITS OWN domain and path: a
/// cookie bound to a CDN host keeps that host (yt-dlp only
/// sends a cookie to a host its domain matches — the old code
/// rewrote every cookie to `.{page host}` + `/`, so a CDN
/// cookie never matched the CDN host it was set for), and a
/// cookie with a non-root path keeps that path. A cookie
/// without a reported domain/path falls back to `.{page
/// host}` + `/`, the old behaviour. The domain column's
/// TRUE/FALSE flag follows the domain: TRUE for a leading-dot
/// domain cookie (matches subdomains), FALSE for a host-only
/// domain. Pairs are validated with `cookies_to_header`
/// first: an invalid pair is an error, never silently
/// dropped, and validation also guarantees no name or value
/// can break a line (no tabs, no newlines). A bad domain or
/// path is an error too, never silently rewritten. The file
/// gets restrictive Unix permissions (0o600) and a random
/// name.
pub fn write_cookies_tempfile_scoped(
    host: &str,
    cookies: &[ScopedCookie],
) -> Result<CookieTempFile, YtDlpError> {
    let pairs: Vec<CookiePair> = cookies
        .iter()
        .map(|c| CookiePair {
            name: c.name.clone(),
            value: c.value.clone(),
            // Validation only needs name/value; the scoped
            // file below is written from `cookies` itself.
            domain: None,
            path: None,
        })
        .collect();
    if let Err(e) = cookies_to_header(&pairs) {
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
    let fallback_domain = if host.starts_with('.') {
        host.to_string()
    } else {
        format!(".{host}")
    };
    let mut text = String::from("# Netscape HTTP Cookie File\n");
    for cookie in cookies {
        let (domain, include_subdomains) = match cookie
            .domain
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            Some(d) => {
                if !valid_cookie_domain(d) {
                    return Err(YtDlpError::new(
                        YtDlpErrorKind::Other,
                        format!("Invalid cookie domain: {d:?}"),
                    ));
                }
                (d.to_string(), d.starts_with('.'))
            }
            None => (fallback_domain.clone(), true),
        };
        let path = match cookie.path.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => {
                if !valid_cookie_path(p) {
                    return Err(YtDlpError::new(
                        YtDlpErrorKind::Other,
                        format!("Invalid cookie path: {p:?}"),
                    ));
                }
                p.to_string()
            }
            None => "/".to_string(),
        };
        text.push_str(&format!(
            "{domain}\t{}\t{path}\tFALSE\t0\t{}\t{}\n",
            if include_subdomains { "TRUE" } else { "FALSE" },
            cookie.name,
            cookie.value
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

/// Write the browser's session cookies for one page host to a
/// temp Netscape cookies file for yt-dlp's `--cookies`, from
/// plain name/value pairs (no scope — see `ScopedCookie`).
/// Every cookie is written to `.{page host}` + `/`, the old
/// behaviour; callers that know each cookie's own domain/path
/// use `write_cookies_tempfile_scoped` instead.
pub fn write_cookies_tempfile(
    host: &str,
    pairs: &[CookiePair],
) -> Result<CookieTempFile, YtDlpError> {
    let scoped: Vec<ScopedCookie> = pairs.iter().map(ScopedCookie::from_pair).collect();
    write_cookies_tempfile_scoped(host, &scoped)
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
    /// Round 87 — the REAL `-J` for
    /// https://www.facebook.com/reel/2141490136612275/
    /// (yt-dlp 2026.10.26, fetched 2026-11-21 FROM THIS VM
    /// through its egress proxy — NOT from the VPS/EC2 IP,
    /// so it may differ from what the VPS sees: the home
    /// PC's resolve works while the VPS one fails, and the
    /// `-J` shape itself varies by IP. The signed query
    /// strings are redacted to `?redacted` and the
    /// uploader's payment handles are trimmed from the
    /// title). The regression: its two formats (`sd`, `hd`)
    /// carry NO height, NO vcodec, NO acodec — the old
    /// parse kept them but `quality_rows` dropped them all,
    /// so Add URL said "no downloadable video" for a reel
    /// that downloads fine by hand.
    const FACEBOOK_REEL: &str = include_str!("../tests/fixtures/ytdlp/facebook_reel.json");

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

    /// Round 87 — the Facebook reel regression: the real
    /// `-J` lists `sd`/`hd` with no height and no codecs,
    /// and the old code turned that into zero quality rows
    /// (Add URL: "No downloadable video was found"). The
    /// fixture reel must resolve to >= 1 format AND >= 1
    /// row. The rows carry no invented height (height 0)
    /// and the format's own name as the label; the best row
    /// is `hd`, in yt-dlp's own order, with the real CDN URL
    /// and headers. Ground truth for the files (ffprobe on
    /// the hand downloads, 2026-11-21): sd is 640x360 h264
    /// + aac, hd is 1920x1080 av1 + aac — both progressive
    /// mp4 with audio, which yt-dlp simply does not report.
    #[test]
    fn parse_facebook_reel_resolves_to_rows() {
        let (video, diag) =
            parse_dump_json_with_diagnostics(FACEBOOK_REEL).expect("the reel fixture parses");
        // The real `-J` reports the extractor lowercase
        // ("facebook"; the old fixture said "Facebook").
        assert_eq!(video.extractor, "facebook");
        assert_eq!(format_ids(&video), vec!["sd", "hd"]);
        assert_eq!(diag.raw_formats, 2);
        assert_eq!(diag.kept_formats, 2);
        assert_eq!(diag.skipped_no_url, 0);
        assert_eq!(diag.skipped_storyboard, 0);
        assert_eq!(diag.protocols, vec![("https".to_string(), 2)]);
        assert!(diag.has_top_level_url);
        assert!(!diag.unwrapped_single_entry_playlist);
        // No height anywhere in the document.
        assert!(video.formats.iter().all(|f| f.height.is_none()));
        assert!(video.formats.iter().all(|f| f.vcodec.is_none()));
        assert!(video.formats.iter().all(|f| f.acodec.is_none()));

        let rows = quality_rows(&video);
        assert_eq!(rows.len(), 2, "the reel must offer both qualities");
        assert_eq!(rows[0].label, "sd");
        assert_eq!(rows[1].label, "hd");
        assert!(rows.iter().all(|r| r.height == 0));
        assert!(rows.iter().all(|r| !r.needs_merge && !r.fragmented));
        let best = rows.last().unwrap();
        assert_eq!(best.format_id, "hd");
        assert!(best.url.contains("video-ord5-1.xx.fbcdn.net"));
        assert!(best
            .headers
            .get("User-Agent")
            .map(|v| v.contains("facebookexternalhit"))
            .unwrap_or(false));
        // Rows exist, so there is nothing to diagnose.
        assert!(zero_format_diagnosis(
            &video,
            &diag,
            "https://www.facebook.com/reel/2141490136612275/",
            12_000
        )
        .is_none());
    }

    #[test]
    fn parse_single_entry_playlist_unwraps_the_one_video() {
        let doc = r#"{
            "_type": "playlist", "extractor": "Facebook",
            "entries": [{
                "title": "Reel", "extractor": "Facebook",
                "formats": [{
                    "format_id": "hd", "ext": "mp4", "height": 720,
                    "vcodec": "h264", "acodec": "aac",
                    "url": "https://video.fbcdn.net/hd.mp4", "protocol": "https"
                }]
            }]
        }"#;
        let (video, diag) =
            parse_dump_json_with_diagnostics(doc).expect("a single-entry playlist parses");
        assert!(diag.unwrapped_single_entry_playlist);
        assert_eq!(format_ids(&video), vec!["hd"]);
        assert_eq!(quality_rows(&video).len(), 1);
    }

    #[test]
    fn parse_multi_entry_playlist_is_not_unwrapped() {
        // `--no-playlist` means this should never arrive; if
        // it does, guessing the first entry would download the
        // wrong video, so the document stays a playlist with
        // no formats — the feed message, not a wrong file.
        let doc = r#"{
            "_type": "playlist", "extractor": "generic",
            "entries": [
                {"title": "One", "formats": []},
                {"title": "Two", "formats": []}
            ]
        }"#;
        let (video, diag) =
            parse_dump_json_with_diagnostics(doc).expect("a multi-entry playlist parses");
        assert!(!diag.unwrapped_single_entry_playlist);
        assert!(video.formats.is_empty());
    }

    #[test]
    fn parse_top_level_url_becomes_one_format() {
        let doc = r#"{
            "title": "Direct", "extractor": "generic",
            "format_id": "0", "format": "0 - unknown",
            "ext": "mp4", "height": 720, "vcodec": "h264", "acodec": "aac",
            "url": "https://example.com/direct.mp4", "protocol": "https"
        }"#;
        let (video, diag) =
            parse_dump_json_with_diagnostics(doc).expect("a top-level-url document parses");
        assert!(diag.has_top_level_url);
        assert_eq!(format_ids(&video), vec!["0"]);
        assert_eq!(quality_rows(&video).len(), 1);
    }

    #[test]
    fn parse_resolution_fills_a_missing_height() {
        let doc = r#"{
            "title": "Res", "extractor": "generic",
            "formats": [{
                "format_id": "a", "ext": "mp4", "resolution": "1280x720",
                "vcodec": "h264", "acodec": "aac",
                "url": "https://example.com/a.mp4", "protocol": "https"
            }]
        }"#;
        let video = parse_dump_json(doc).expect("a resolution-only format parses");
        assert_eq!(video.formats[0].height, Some(720));
        assert_eq!(video.formats[0].width, Some(1280));
        assert_eq!(quality_rows(&video).len(), 1);
    }

    #[test]
    fn select_quality_accepts_unknown_codec_progressive_last() {
        // A 720p https mp4 with no codecs at all: no
        // known-good pick exists, so the unknown-codec tier
        // takes it as a single file rather than nothing.
        let unknown = test_format("u", "mp4", Some(720), None, None, Some(100), "https", false);
        let picked = select_quality(std::slice::from_ref(&unknown), 720)
            .expect("an unknown-codec progressive is picked");
        assert!(matches!(picked, SelectedQuality::Single(f) if f.format_id == "u"));
        // But a real progressive at the same height wins.
        let known = test_format(
            "k",
            "mp4",
            Some(720),
            Some("avc1.4d401f"),
            Some("mp4a.40.2"),
            Some(200),
            "https",
            false,
        );
        let picked = select_quality(&[unknown, known], 720).expect("a pick exists");
        assert!(matches!(picked, SelectedQuality::Single(f) if f.format_id == "k"));
        // Audio-only never qualifies, even as the last tier.
        let audio_only = test_format(
            "a",
            "m4a",
            Some(720),
            None,
            Some("mp4a.40.2"),
            Some(50),
            "https",
            false,
        );
        assert_eq!(select_quality(std::slice::from_ref(&audio_only), 720), None);
        // Neither can a format that cannot carry video.
        let no_video = test_format(
            "n",
            "mp4",
            Some(720),
            Some("none"),
            Some("mp4a.40.2"),
            Some(50),
            "https",
            false,
        );
        assert_eq!(select_quality(std::slice::from_ref(&no_video), 720), None);
    }

    #[test]
    fn zero_format_diagnosis_names_the_counts_and_stays_silent_otherwise() {
        // Audio-only: yt-dlp listed a format, kept it, but
        // no row can promise video — the line must say so.
        let audio_only = r#"{
            "title": "Audio", "extractor": "generic",
            "formats": [{
                "format_id": "140", "ext": "m4a", "acodec": "mp4a.40.2",
                "vcodec": "none",
                "url": "https://example.com/a.m4a", "protocol": "https"
            }]
        }"#;
        let (video, diag) =
            parse_dump_json_with_diagnostics(audio_only).expect("an audio-only document parses");
        assert!(quality_rows(&video).is_empty());
        let line = zero_format_diagnosis(&video, &diag, "https://example.com/page", 12_345)
            .expect("an audio-only resolve gets a diagnosis");
        assert!(line.contains("listed 1 format(s), kept 1, quality rows 0"));
        assert!(line.contains("protocols: https×1"));
        assert!(line.contains("top-level url: no"));
        assert!(line.contains("12345ms"));
        assert!(!line.contains("example.com"), "no URLs in the line");
        assert!(!line.contains('\n'), "the diagnosis is ONE line");
        // A feed page (nothing listed at all) stays silent —
        // the dialog's feed message already says that.
        let (feed, feed_diag) = parse_dump_json_with_diagnostics(
            r#"{"title": "Feed", "extractor": "Facebook", "formats": []}"#,
        )
        .expect("a feed document parses");
        assert!(
            zero_format_diagnosis(&feed, &feed_diag, "https://www.facebook.com/", 100).is_none()
        );
        // The same empty document for a VIDEO-page URL is
        // evidence, not a feed: the site served no video to
        // this IP. The line names the host and the suspected
        // cause, so a VPS retest can tell a site block from
        // a parser bug.
        let line = zero_format_diagnosis(
            &feed,
            &feed_diag,
            "https://www.facebook.com/reel/2141490136612275/",
            12_345,
        )
        .expect("an empty resolve for a video page gets a diagnosis");
        assert!(line.contains("listed 0 formats for a video page on facebook.com"));
        assert!(line.contains("served no video to this IP"));
        assert!(line.contains("12345ms"));
        assert!(
            !line.contains("2141490136612275"),
            "host only, never the full URL"
        );
        assert!(!line.contains('\n'), "the diagnosis is ONE line");
        // A document WITH rows stays silent too.
        let (reel, reel_diag) =
            parse_dump_json_with_diagnostics(FACEBOOK_REEL).expect("the reel fixture parses");
        assert!(zero_format_diagnosis(
            &reel,
            &reel_diag,
            "https://www.facebook.com/reel/2141490136612275/",
            100
        )
        .is_none());
    }

    #[test]
    fn is_video_page_url_mirrors_the_dialog() {
        assert!(is_video_page_url(
            "https://www.facebook.com/reel/2141490136612275/"
        ));
        assert!(is_video_page_url(
            "https://facebook.com/reel/2141490136612275/"
        ));
        assert!(is_video_page_url("https://www.facebook.com/watch/?v=123"));
        assert!(is_video_page_url("https://fb.watch/abc/"));
        assert!(is_video_page_url(
            "https://www.instagram.com/reel/DdwixbhzY3_/"
        ));
        assert!(is_video_page_url(
            "https://www.tiktok.com/@user/video/7223205454032178475"
        ));
        assert!(is_video_page_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        ));
        assert!(is_video_page_url("https://youtu.be/dQw4w9WgXcQ"));
        assert!(is_video_page_url("https://www.youtube.com/shorts/abc"));
        // Feeds are NOT video pages here (stricter than the
        // dialog's helper, on purpose — see the doc comment):
        // an empty resolve for one stays silent.
        assert!(!is_video_page_url("https://www.facebook.com/"));
        assert!(!is_video_page_url("https://www.facebook.com/watch/"));
        assert!(!is_video_page_url("https://www.youtube.com/"));
        assert!(!is_video_page_url("https://www.youtube.com/channel/UC123"));
        assert!(!is_video_page_url("https://www.instagram.com/"));
        assert!(!is_video_page_url("https://www.tiktok.com/"));
        assert!(!is_video_page_url("https://example.com/file.zip"));
        assert!(!is_video_page_url("not a url"));
    }

    #[test]
    fn write_cookies_tempfile_scoped_keeps_each_cookies_own_domain_and_path() {
        let cookies = vec![
            ScopedCookie {
                name: "c_user".to_string(),
                value: "123".to_string(),
                domain: Some(".facebook.com".to_string()),
                path: Some("/".to_string()),
            },
            ScopedCookie {
                name: "cdn_session".to_string(),
                value: "abc".to_string(),
                domain: Some("video-ord5-2.xx.fbcdn.net".to_string()),
                path: Some("/o1/v/t2/".to_string()),
            },
            ScopedCookie {
                name: "plain".to_string(),
                value: "x".to_string(),
                domain: None,
                path: None,
            },
        ];
        let path = {
            let file = write_cookies_tempfile_scoped("www.facebook.com", &cookies)
                .expect("scoped cookies write a temp file");
            let text = std::fs::read_to_string(&file.path).expect("the file reads back");
            // The domain cookie keeps its own domain and the
            // subdomain flag; the CDN cookie keeps its CDN
            // host (host-only: FALSE) and its non-root path.
            assert!(text.contains(".facebook.com\tTRUE\t/\tFALSE\t0\tc_user\t123"));
            assert!(text.contains(
                "video-ord5-2.xx.fbcdn.net\tFALSE\t/o1/v/t2/\tFALSE\t0\tcdn_session\tabc"
            ));
            // No scope reported: the old fallback, page host.
            assert!(text.contains(".www.facebook.com\tTRUE\t/\tFALSE\t0\tplain\tx"));
            // Nothing was rewritten to the page host.
            assert_eq!(text.matches(".www.facebook.com").count(), 1);
            file.path.clone()
        };
        assert!(!path.exists(), "dropping the guard deletes the file");
        // A bad domain / path is an error, never silently
        // rewritten to the page host.
        let bad_domain = vec![ScopedCookie {
            name: "a".to_string(),
            value: "b".to_string(),
            domain: Some("not a host".to_string()),
            path: None,
        }];
        assert!(write_cookies_tempfile_scoped("example.com", &bad_domain).is_err());
        let bad_path = vec![ScopedCookie {
            name: "a".to_string(),
            value: "b".to_string(),
            domain: None,
            path: Some("no-leading-slash".to_string()),
        }];
        assert!(write_cookies_tempfile_scoped("example.com", &bad_path).is_err());
    }

    #[test]
    fn write_cookies_tempfile_keeps_pair_domain_and_path() {
        // Round 87 (3) — the end-to-end wiring: a CookiePair
        // carrying its own domain/path (as the extension now
        // sends) must keep them through the plain writer, which
        // routes via ScopedCookie::from_pair — so resolve() and
        // the direct retry write the CDN host, not the page host.
        let pairs = vec![CookiePair {
            name: "cdn_session".to_string(),
            value: "abc".to_string(),
            domain: Some("video-ord5-2.xx.fbcdn.net".to_string()),
            path: Some("/o1/v/t2/".to_string()),
        }];
        let file = write_cookies_tempfile("www.facebook.com", &pairs)
            .expect("scoped pairs write a temp file");
        let text = std::fs::read_to_string(&file.path).expect("the file reads back");
        assert!(text
            .contains("video-ord5-2.xx.fbcdn.net\tFALSE\t/o1/v/t2/\tFALSE\t0\tcdn_session\tabc"));
        assert!(!text.contains(".www.facebook.com"));
        // Old stored rows carry no scope: they still parse
        // (the #[serde(default)] on the new fields), with
        // domain/path None — the page-host fallback — instead
        // of failing to parse and dropping the cookies.
        let old = crate::native_host::cookies_from_json(r#"[{"name":"x","value":"y"}]"#);
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].domain, None);
        assert_eq!(old[0].path, None);
        // New JSON round-trips with its scope.
        let new = crate::native_host::cookies_from_json(
            r#"[{"name":"x","value":"y","domain":".example.com","path":"/"}]"#,
        );
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].domain.as_deref(), Some(".example.com"));
        assert_eq!(new[0].path.as_deref(), Some("/"));
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

    /// Round 87.1 — the same reserved-device rule as
    /// `sanitize_filename`: a title that IS a device name
    /// (`CON`, `COM1`, …) is renamed, whether or not it
    /// already carries the extension; anything else passes
    /// through. (This sanitizer has no production callers
    /// today — the direct path uses the manager's
    /// `sanitize_filename` — but it is `pub` and the rule must
    /// hold wherever it is used.)
    #[test]
    fn sanitize_title_filename_renames_reserved_windows_device_names() {
        assert_eq!(sanitize_title_filename("CON", "mp4"), "CON_.mp4");
        assert_eq!(sanitize_title_filename("CON.mp4", "mp4"), "CON_.mp4");
        assert_eq!(sanitize_title_filename("com1", "mp4"), "com1_.mp4");
        assert_eq!(sanitize_title_filename("LPT9", "webm"), "LPT9_.webm");
        assert_eq!(sanitize_title_filename("NUL", "mp4"), "NUL_.mp4");
        assert_eq!(sanitize_title_filename("CONSOLE", "mp4"), "CONSOLE.mp4");
        assert_eq!(sanitize_title_filename("COM10", "mp4"), "COM10.mp4");
        assert_eq!(sanitize_title_filename("my.con", "mp4"), "my.con.mp4");
        assert_eq!(sanitize_title_filename("video", "mp4"), "video.mp4");
        // No extension at all: the renamed stem stands alone.
        assert_eq!(sanitize_title_filename("AUX", ""), "AUX_");
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
            // Round 85 — a bare HTTP 403 is a CDN refusal, NOT
            // a missing sign-in: the video plays in the browser
            // and the cookies were already sent where the flow
            // has them. (Was LoginRequired before Round 85.)
            (
                "ERROR: HTTP Error 403: Forbidden",
                YtDlpErrorKind::CdnRefused,
            ),
            (
                "ERROR: [tiktok] https://example.com/v.mp4: 403: Forbidden",
                YtDlpErrorKind::CdnRefused,
            ),
            (
                "ERROR: HTTP Error 401: Unauthorized",
                YtDlpErrorKind::CdnRefused,
            ),
            // Round 85 — explicit sign-in wording still maps to
            // LoginRequired even though the CDN-refusal rule runs
            // first: neither line carries a 403/401.
            (
                "ERROR: Log in for access",
                YtDlpErrorKind::LoginRequired,
            ),
            (
                "ERROR: Please sign in to watch this video",
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
            // Round 85 (ChatGPT follow-up) — an output-template
            // error is NEVER a sign-in error: the actual stderr
            // reason surfaces verbatim (kind Other), so the
            // dialog shows the real cause instead of "needs you
            // to be signed in". The video id contains "403" as
            // a substring, which is exactly why the old
            // catch-all mis-mapped it.
            (
                "ERROR: Fixed output name but more than one file to download: TikTok video #7223205454032178475 (N).mp4",
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
        // Round 83 — a bot block carries the cloud-server hint
        // in both texts: the mapped detail (what the extension
        // shows via the host) and the user message (in-app).
        let blocked =
            map_error("ERROR: [youtube] dQw4w9WgXcQ: Sign in to confirm you're not a bot.");
        assert_eq!(blocked.kind, YtDlpErrorKind::BotBlocked);
        assert!(
            blocked
                .message
                .contains("common on cloud servers; it normally works on a home connection"),
            "the mapped bot-block detail must carry the hint: {}",
            blocked.message
        );
        assert!(
            blocked
                .user_message()
                .contains("common on cloud servers; it normally works on a home connection"),
            "the bot-block user message must carry the hint: {}",
            blocked.user_message()
        );
        assert_eq!(
            map_error("ERROR: Signature extraction failed").user_message(),
            "This site changed. Updating the video component may fix it."
        );
        // Round 85 — the CDN-refusal wording is exact, and its
        // kind string is what the host logs and the extension
        // reads (needs_login stays false for it).
        assert_eq!(
            map_error("ERROR: HTTP Error 403: Forbidden").user_message(),
            "The site refused the download request (HTTP 403)"
        );
        assert_eq!(
            map_error("ERROR: HTTP Error 403: Forbidden").kind_str(),
            "cdn_refused"
        );
        // Round 85 (ChatGPT follow-up) — the template error's
        // mapped message is the actual stderr text, never a
        // sign-in wording.
        assert!(
            map_error("ERROR: Fixed output name but more than one file to download: TikTok video #7223205454032178475 (N).mp4")
                .user_message()
                .contains("Fixed output name"),
            "the template error must surface its actual reason"
        );
    }

    #[test]
    fn output_tail_one_line_is_greppable_and_redacted() {
        // Round 85 — the one-line tail is what startup.log
        // carries now: no newline anywhere, so `tail:` and the
        // reason sit on the same greppable line.
        let mut tail = YtDlpOutputTail::new();
        tail.push_stderr("ERROR: HTTP Error 403: Forbidden");
        tail.push_stderr("ERROR: [youtube] dQw4w9WgXcQ: Download failed");
        tail.push_stdout("[download] 12.0% of 10.00MiB");
        let one = tail.render_one_line();
        assert!(
            !one.contains('\n'),
            "one-line tail must have no newline: {one}"
        );
        assert!(one.contains("ERROR: HTTP Error 403: Forbidden"));
        assert!(one.contains(" | "), "lines are joined with ' | '");
        assert_eq!(YtDlpOutputTail::new().render_one_line(), "");
        // Redaction runs over the one-line form exactly like
        // the multi-line one: a signed URL's query is cut, a
        // cookie-file token is masked — and a "signed in" line
        // survives it (the redactor is token-based, ytdlp.rs
        // redact_ytdlp_token; it never wipes sign-in wording).
        let redacted = redact_ytdlp_text(&format!(
            "ERROR: sign in required https://example.com/v.mp4?sig=abc123 {one}"
        ));
        assert!(redacted.contains("sign in required"));
        assert!(!redacted.contains("sig=abc123"));
        assert!(redacted.contains("https://example.com/v.mp4?..."));
        assert!(redacted.contains("ERROR: HTTP Error 403: Forbidden"));
        let cookie_line = format!(
            "using cookies from {}",
            format!("{YTDLP_COOKIE_FILE_PREFIX}abc.txt")
        );
        assert_eq!(
            redact_ytdlp_text(&cookie_line),
            "using cookies from <cookies-file>"
        );
        // The late stderr line a drain race used to lose is
        // still the mapped verdict: mapping reads error_text,
        // which is the stderr ring, whatever arrived last.
        let mut late = YtDlpOutputTail::new();
        late.push_stderr("[download] Downloading video 1 of 1");
        late.push_stderr("ERROR: HTTP Error 403: Forbidden");
        assert_eq!(
            map_error(&redact_ytdlp_text(&late.error_text())).kind,
            YtDlpErrorKind::CdnRefused
        );
    }

    #[test]
    fn best_audio_only_format_prefers_m4a_aac_mp4() {
        // Round 85 — the DASH re-resolve's audio pick: M4A/AAC
        // in mp4 first, then any audio-only format; video-only
        // and progressive formats never qualify, nor does http.
        let formats = vec![
            test_format(
                "137",
                "mp4",
                Some(1080),
                Some("avc1"),
                Some("none"),
                Some(100),
                "https",
                false,
            ),
            test_format(
                "251",
                "webm",
                None,
                Some("none"),
                Some("opus"),
                Some(50),
                "https",
                false,
            ),
            test_format(
                "140",
                "m4a",
                None,
                Some("none"),
                Some("mp4a.40.2"),
                Some(80),
                "https",
                false,
            ),
            test_format(
                "139",
                "m4a",
                None,
                Some("none"),
                Some("mp4a.40.5"),
                Some(40),
                "http",
                false,
            ),
            test_format(
                "18",
                "mp4",
                Some(360),
                Some("avc1"),
                Some("mp4a.40.2"),
                Some(90),
                "https",
                false,
            ),
        ];
        let best = best_audio_only_format(&formats).expect("an audio format exists");
        assert_eq!(best.format_id, "140");
        // No audio-only format at all: None, not a video.
        let video_only = vec![test_format(
            "137",
            "mp4",
            Some(1080),
            Some("avc1"),
            Some("none"),
            Some(100),
            "https",
            false,
        )];
        assert!(best_audio_only_format(&video_only).is_none());
        assert!(best_audio_only_format(&[]).is_none());
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

    /// Round 85 (Claude delta) — the direct-download argv pins
    /// the arg order: "--" is second-to-last, the URL is last,
    /// nothing starting with "-" appears after "--", and
    /// --ffmpeg-location / --js-runtimes ride BEFORE "--".
    /// This is the regression test for the R84 bug, where those
    /// two flags were appended to the command AFTER "--" + URL
    /// and yt-dlp read them as extra positional URLs ("Fixed
    /// output name but more than one file to download").
    #[test]
    fn build_direct_ytdlp_args_flags_before_separator_url_last() {
        let url = "https://www.tiktok.com/@chisbetyu81/video/7223205454032178475";
        // Minimal: no cookies, no headers, no ffmpeg, no qjs.
        let minimal = DirectYtDlpArgs {
            format_selector: "h264_480p_721027-0/b[ext=mp4]/b".to_string(),
            out_dir: Some(r"C:\Users\Administrator\Downloads\Round76".to_string()),
            out_template: "TikTok video #7223205454032178475.%(ext)s".to_string(),
            cookies_file: None,
            user_agent: None,
            referer: None,
            ffmpeg_dir: None,
            qjs: None,
            source_url: url.to_string(),
        };
        let args = build_direct_ytdlp_args(&minimal);
        // (a) "--" is second-to-last, the URL is last.
        assert_eq!(args[args.len() - 2], "--");
        assert_eq!(args[args.len() - 1], url);
        // (b) nothing starting with "-" appears after "--".
        let sep = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(args.len(), sep + 2);
        assert!(args[sep + 1..].iter().all(|a| !a.starts_with('-')));
        // Full: everything set — the option flags are present
        // and BEFORE "--".
        let full = DirectYtDlpArgs {
            format_selector: "h264_480p_721027-0/b[ext=mp4]/b".to_string(),
            out_dir: Some(r"C:\Users\Administrator\Downloads\Round76".to_string()),
            out_template: "TikTok video #7223205454032178475.%(ext)s".to_string(),
            cookies_file: Some(PathBuf::from(
                r"C:\Users\ADMINI~1\AppData\Local\Temp\fetchline-ytdlp-cookies-x.txt",
            )),
            user_agent: Some(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36"
                    .to_string(),
            ),
            referer: Some(url.to_string()),
            ffmpeg_dir: Some(PathBuf::from(r"C:\Sonic Manager\bin")),
            qjs: Some(PathBuf::from(r"C:\Sonic Manager\qjs.exe")),
            source_url: url.to_string(),
        };
        let args = build_direct_ytdlp_args(&full);
        let sep = args.iter().position(|a| a == "--").unwrap();
        // (a) again, with everything set.
        assert_eq!(args[sep + 1], url);
        assert_eq!(args.len(), sep + 2);
        // (b) again.
        assert!(args[sep + 1..].iter().all(|a| !a.starts_with('-')));
        // (c) --ffmpeg-location and --js-runtimes are present
        // and BEFORE "--" — never after it.
        let before: &[String] = &args[..sep];
        assert!(before.contains(&"--ffmpeg-location".to_string()));
        assert!(before.contains(&"--js-runtimes".to_string()));
        assert!(before
            .windows(2)
            .any(|w| w[0] == "--js-runtimes" && w[1].starts_with("quickjs:")));
        assert!(before.contains(&"--cookies".to_string()));
        assert!(before.contains(&"--add-header".to_string()));
        // Round 85 (Jumani) — the merge is forced to mp4,
        // also before "--".
        assert!(before.contains(&"--merge-output-format".to_string()));
        assert!(before
            .windows(2)
            .any(|w| w[0] == "--merge-output-format" && w[1] == "mp4"));
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
        // Round 84 — the R83 retest's TikTok CDN host is a
        // tiktok.com host, not tiktokcdn.com: one connection.
        assert_eq!(
            default_host_segment_cap("v16-webapp-prime.us.tiktok.com"),
            Some(1)
        );
        assert_eq!(default_host_segment_cap("tiktok.com"), Some(1));
        assert_eq!(default_host_segment_cap("nottiktok.com"), None);
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
                domain: None,
                path: None,
            },
            CookiePair {
                name: "consent".to_string(),
                value: "yes".to_string(),
                domain: None,
                path: None,
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
            domain: None,
            path: None,
        }];
        assert!(write_cookies_tempfile("example.com", &bad).is_err());
        // No host, no file.
        assert!(write_cookies_tempfile("", &pairs).is_err());
    }

    #[test]
    fn direct_download_format_selector_falls_back_but_keeps_selectors() {
        // Round 83 — a bare id gets the fallback chain; a
        // DASH pair keeps its '+' and gets the chain too.
        assert_eq!(
            direct_download_format_selector("download_addr-0"),
            "download_addr-0/b[ext=mp4]/b"
        );
        assert_eq!(
            direct_download_format_selector("137+140"),
            "137+140/b[ext=mp4]/b"
        );
        // An id that already is a selector is left alone.
        assert_eq!(direct_download_format_selector("137/best"), "137/best");
        assert_eq!(direct_download_format_selector(""), "b[ext=mp4]/b");
    }

    #[test]
    fn redact_ytdlp_text_cuts_query_strings_and_masks_cookie_files() {
        let redacted = redact_ytdlp_text(
            "yt-dlp --cookies C:\\Users\\x\\AppData\\Local\\Temp\\fetchline-ytdlp-cookies-abc.txt -- https://example.com/video?token=secret&x=1",
        );
        assert!(!redacted.contains("token=secret"));
        assert!(!redacted.contains("fetchline-ytdlp-cookies-abc"));
        assert!(redacted.contains("<cookies-file>"));
        assert!(redacted.contains("https://example.com/video?..."));
        // Local save paths are left alone.
        assert_eq!(
            redact_ytdlp_text(r"C:\Users\x\Downloads\video.mp4"),
            r"C:\Users\x\Downloads\video.mp4"
        );
    }

    #[test]
    fn cookie_header_to_pairs_splits_on_the_first_equals() {
        let pairs = cookie_header_to_pairs("session=abc=def; consent=yes; empty=");
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0].name, "session");
        assert_eq!(pairs[0].value, "abc=def");
        assert_eq!(pairs[1].name, "consent");
        assert_eq!(pairs[1].value, "yes");
        assert_eq!(pairs[2].name, "empty");
        assert_eq!(pairs[2].value, "");
        assert!(cookie_header_to_pairs("").is_empty());
        assert!(cookie_header_to_pairs("  ;  ").is_empty());
    }

    #[test]
    fn output_tail_keeps_the_last_40_lines_per_stream() {
        let mut tail = YtDlpOutputTail::new();
        for i in 0..45 {
            tail.push_stderr(&format!("err {i}"));
            tail.push_stdout(&format!("out {i}"));
        }
        let rendered = tail.render();
        // The first 5 lines of each stream fell off the ring.
        assert!(!rendered.contains("err 4\n"));
        assert!(rendered.contains("err 5\n"));
        assert!(rendered.contains("err 44\n"));
        assert!(!rendered.contains("out 4\n"));
        // "out 44" is the last line, and render() trims the
        // trailing newline, so it ends the string (no "\n").
        assert!(rendered.ends_with("out 44"));
        // The short tail is the last 8 stderr lines.
        assert_eq!(
            tail.render_short(),
            "err 37\nerr 38\nerr 39\nerr 40\nerr 41\nerr 42\nerr 43\nerr 44"
        );
        // With stderr empty, the short tail falls back to stdout.
        let mut tail2 = YtDlpOutputTail::new();
        tail2.push_stdout("[download] 42.1% of 10.00MiB");
        assert_eq!(tail2.render_short(), "[download] 42.1% of 10.00MiB");
        // error_text maps stderr only (no "--- stdout ---" header
        // in the user message); with stderr empty it is stdout.
        // It is the whole kept stderr ring (lines 0-4 fell off),
        // not the last 8 — map_error takes its own last 3 lines.
        assert_eq!(
            tail.error_text(),
            (5..45)
                .map(|i| format!("err {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert_eq!(tail2.error_text(), "[download] 42.1% of 10.00MiB");
        assert!(YtDlpOutputTail::new().is_empty());
        assert_eq!(YtDlpOutputTail::new().render(), "");
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
