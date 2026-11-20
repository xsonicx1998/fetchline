// Round 71 (A1) — the host MUST stay a console (CUI) binary.
// Round 70 made it GUI (`windows_subsystem = "windows"`) to stop
// the console flash, but a stdio native host MUST be console: a
// GUI binary detaches from its stdio pipes, so Chrome can spawn
// it and still never talk to it — exactly the "Error when
// communicating with the native messaging host" the Round 70
// retest saw, even though a manual framed ping answered pong
// (the binary works; Chrome's launch of it does not). Chrome
// spawns native hosts hidden (CREATE_NO_WINDOW) anyway, so a
// console host does NOT flash a window; the flash the user saw
// came from the app's own PowerShell spawns, fixed in Round 70
// with CREATE_NO_WINDOW in lib.rs. The attribute is therefore
// removed, NOT worked around.
// Fetchline native-messaging host (S13 / F01).
//
// The browser extension cannot touch the desktop app's database
// directly, so Chrome / Edge / Firefox spawn this small process and
// talk to it over stdin/stdout: each message is a 4-byte
// little-endian length prefix followed by JSON (see
// `fetchline_engine::native_host` for the framing — this binary
// owns no framing code of its own).
//
// What it does per message:
//  * `ping` / `get_settings` / `download_link` / `download_all`
//    with links — handled by `handle_host_message`, which enforces
//    the desktop `browser_integration` flag and the `excluded_sites`
//    list before anything is enqueued.
//  * `download_all` with no links — the extension has no content
//    script, so it cannot list the page's links. The host expands
//    the page itself via `enqueue_page_links` (fetch + probe with
//    the real engine) and enqueues what it finds.
//  * `video_found` — implemented (S08 / F05): the chosen variant
//    is validated by `handle_host_message` (http(s) only, known
//    media extension, dedup, excluded sites, DRM refused) and
//    enqueued into `pending_links` with source "video", so the
//    desktop app drains it through the real `add_download` path.
//  * `resolve_video` — Round 78: the extension asks the host to
//    resolve a platform video page with yt-dlp (the extension
//    cannot reach the engine, and the host must answer even when
//    the app is closed). The host runs the shared resolver
//    itself, writes a `resolve_progress` heartbeat about every
//    5 s so the extension's `connectNative` port stays alive,
//    and replies with a TRIMMED format list (one row per height,
//    360p and up — never raw `-J`, Chrome native-messaging
//    messages are ~1 MB). Cookies are tried WITHOUT first and
//    only sent after a login-required error, per the extension's
//    consent flow; the temp Netscape file is deleted on drop and
//    stale ones are swept at startup.
//
// The host only ENQUEUES, into the `pending_links` table of the
// SAME SQLite file the desktop app uses. It never starts a
// download: the desktop app drains the queue through the real
// `add_download` path on startup and every 30 s. The host never
// logs a message body — only the message type goes to stderr, so
// no URL (and never a cookie — none is ever sent) can leak into a log.
//
// Database location, in order: `--db-path <path>`,
// `FETCHLINE_DB_PATH`, then the first existing candidate under the
// OS app-data folder, else the first candidate (created on open).
// The candidates mirror the Tauri shell's
// `app_data_dir()/fetchline/app.db`; if the desktop app was built
// with a different identifier, point the host at the file
// explicitly — a host pointed at the wrong file silently queues
// links the app never sees, which is worse than failing loudly.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use fetchline_engine::native_host::{
    decode_message, handle_host_message, write_message, CookiePair, HostMessage,
};
use fetchline_engine::ytdlp;
use fetchline_engine::{DownloadManager, EngineError, Store};

fn print_usage() {
    eprintln!(
        "Usage: fetchline-host [--db-path <path>]\n\
         Reads native-messaging frames on stdin, writes replies on stdout.\n\
         --db-path overrides the database location (or set FETCHLINE_DB_PATH)."
    );
}

/// Candidate database paths, most likely first. Tauri's app-data
/// folder is `%APPDATA%\\<identifier>` on Windows,
/// `~/Library/Application Support/<identifier>` on macOS and
/// `~/.local/share/<identifier>` on Linux; the shell then appends
/// `fetchline/app.db`. The plain `fetchline/app.db` form is kept as
/// a fallback for a hand-rolled install.
fn db_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("APPDATA") {
        out.push(
            PathBuf::from(&p)
                .join("com.fetchline.app")
                .join("fetchline")
                .join("app.db"),
        );
        out.push(PathBuf::from(&p).join("fetchline").join("app.db"));
    }
    if let Ok(home) = std::env::var("HOME") {
        out.push(
            PathBuf::from(&home)
                .join("Library")
                .join("Application Support")
                .join("com.fetchline.app")
                .join("fetchline")
                .join("app.db"),
        );
        let xdg = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(&home).join(".local").join("share"));
        out.push(
            xdg.join("com.fetchline.app")
                .join("fetchline")
                .join("app.db"),
        );
        out.push(xdg.join("fetchline").join("app.db"));
    }
    out
}

fn resolve_db_path(flag: Option<PathBuf>) -> PathBuf {
    if let Some(p) = flag {
        return p;
    }
    if let Ok(p) = std::env::var("FETCHLINE_DB_PATH") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let candidates = db_candidates();
    for c in &candidates {
        if c.exists() {
            return c.clone();
        }
    }
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("fetchline.db"))
}

/// What the command line asked for. `--db-path` and `--help` are
/// ours; everything else a browser adds when it spawns the host is
/// ignored — see `is_browser_launch_arg`.
struct HostArgs {
    db_path: Option<PathBuf>,
    help: bool,
}

/// Parse the host's command line.
///
/// BUG-043: Chrome starts a native-messaging host with TWO extra
/// arguments — the calling extension's origin
/// (`chrome-extension://<id>/`) and `--parent-window=<hwnd>`
/// (Chromium's native_message_process_host; the HWND is the
/// browser window, 0 in headless). Firefox passes its origin
/// (`moz-extension://<uuid>/`) the same way. The old parser
/// rejected ANY unknown argument with exit(2), so the host died
/// instantly on every Chrome spawn — Chrome's write to its stdin
/// then failed with -101 (ERR_CONNECTION_RESET) and the popup
/// reported "Desktop app not reachable". The host answers fine
/// when driven directly (CI Phase 1, the host smoke tests), so
/// this only ever showed up in a real browser spawn — which is
/// why it was never caught. Unknown arguments that are NOT
/// browser launch args still exit(2).
fn parse_host_args<I, S>(args: I) -> Result<HostArgs, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = HostArgs {
        db_path: None,
        help: false,
    };
    let mut it = args.into_iter().peekable();
    while let Some(arg) = it.next() {
        let arg = arg.as_ref();
        match arg {
            "--db-path" => match it.next() {
                Some(p) => out.db_path = Some(PathBuf::from(p.as_ref())),
                None => return Err("--db-path needs a path".to_string()),
            },
            "--help" | "-h" => out.help = true,
            _ if is_browser_launch_arg(arg) => {
                // Chrome's --parent-window is passed as
                // --parent-window=<hwnd>; the bare form takes the
                // handle as the NEXT argument — consume it, but
                // only when it looks like a handle, so a following
                // real flag is never eaten.
                if arg == "--parent-window" {
                    if let Some(next) = it.peek() {
                        if next.as_ref().chars().all(|c| c.is_ascii_digit()) {
                            it.next();
                        }
                    }
                }
            }
            other => return Err(format!("Unknown argument: {other}")),
        }
    }
    Ok(out)
}

/// True for the arguments a browser adds when it spawns a native
/// host (BUG-043): the caller's extension origin and the
/// --parent-window handle. Anything else unknown is still an
/// error — see parse_host_args.
fn is_browser_launch_arg(arg: &str) -> bool {
    arg.starts_with("chrome-extension://")
        || arg.starts_with("moz-extension://")
        || arg == "--parent-window"
        || arg.starts_with("--parent-window=")
}

/// Append one line to the host log, best-effort: the file lives next
/// to the database (host.log beside app.db). Never stdout — anything
/// but framed replies on stdout corrupts the native-messaging
/// protocol. A failed write is silently ignored: logging must never
/// break the host. Chrome discards the host's stderr, so this file
/// is the only place a startup or message failure is visible.
fn log_line(log_path: &std::path::Path, msg: &str) {
    use std::io::Write as _;
    if let Some(parent) = log_path.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        let _ = writeln!(f, "[{secs}] {msg}");
    }
}

/// Open the store (and the manager built on it) on first use — the
/// host must answer a ping even when the database cannot be opened.
/// Round 70: on the user's PC the host died at startup with
/// exit(1) while the desktop app held app.db, so Chrome reported
/// "Error when communicating with the native messaging host" and
/// the real error went to a stderr nobody reads. A failed open is
/// reported to the caller as a framed error reply, never an exit,
/// and logged to host.log.
fn ensure_store(
    db_path: &std::path::Path,
    log_path: &std::path::Path,
    store: &mut Option<Arc<Store>>,
    manager: &mut Option<Arc<DownloadManager>>,
) -> Result<(Arc<Store>, Arc<DownloadManager>), String> {
    if let (Some(s), Some(m)) = (store.as_ref(), manager.as_ref()) {
        return Ok((Arc::clone(s), Arc::clone(m)));
    }
    match Store::open(db_path) {
        Ok(opened) => {
            let opened = Arc::new(opened);
            let mgr = DownloadManager::new(Arc::clone(&opened), 8);
            *store = Some(Arc::clone(&opened));
            *manager = Some(Arc::clone(&mgr));
            Ok((opened, mgr))
        }
        Err(e) => {
            let msg = format!("cannot open database {}: {e}", db_path.display());
            log_line(log_path, &format!("fetchline-host: {msg}"));
            Err(msg)
        }
    }
}

/// Round 78 — resolve one platform video page with yt-dlp and
/// return the framed reply value, or `None` when stdout closed
/// mid-resolve (the browser is gone; the host loop exits).
/// The resolve is store-optional: platform downloads are gated by
/// the setting when the database opens, but a database that cannot
/// be opened must not block the resolve — the extension's quality
/// pick goes through the app, which re-checks the flag. The
/// stored extractor-args override (hidden setting) is used when
/// the request carries none of its own.
async fn resolve_video(
    db_path: &std::path::Path,
    log_path: &std::path::Path,
    store: &mut Option<Arc<Store>>,
    manager: &mut Option<Arc<DownloadManager>>,
    output: &mut std::io::StdoutLock<'static>,
    page_url: String,
    request_id: String,
    with_cookies: bool,
    cookies: &[CookiePair],
    extractor_args: Option<&str>,
) -> Option<serde_json::Value> {
    let mut platform_enabled = true;
    let mut stored_extractor_args: Option<String> = None;
    if let Ok((s, _)) = ensure_store(db_path, log_path, store, manager) {
        if let Ok(settings) = s.get_settings() {
            platform_enabled = settings.platform_downloads_enabled;
            stored_extractor_args = settings.ytdlp_extractor_args;
        }
    }
    if !platform_enabled {
        return Some(serde_json::json!({
            "type": "error", "ok": false, "request_id": request_id,
            "error": "Platform downloads are turned off in Fetchline settings",
            "needs_login": false, "error_kind": "unsupported",
        }));
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let opts = ytdlp::ResolveOptions {
        paths: ytdlp::locate_sidecars(&exe_dir),
        cookies: if with_cookies {
            cookies.to_vec()
        } else {
            Vec::new()
        },
        extractor_args: extractor_args
            .map(|s| s.to_string())
            .or(stored_extractor_args),
        timeout_secs: ytdlp::RESOLVE_TIMEOUT_SECS,
    };
    // Round 79 — log the request (page URL + whether cookies
    // rode along) so a failed resolve in host.log can be tied
    // to the attempt that produced it.
    log_line(
        log_path,
        &format!("resolve: url={page_url} with_cookies={with_cookies}"),
    );
    resolve_with_heartbeat(output, log_path, &request_id, page_url, opts).await
}

/// Round 78 — run the resolve while writing a `resolve_progress`
/// heartbeat about every 5 s, so the extension's `connectNative`
/// port sees traffic during a long resolve and Chrome does not
/// consider the channel idle. Dropping the pinned future on a
/// closed stdout kills yt-dlp (kill-on-drop).
/// Round 79 — a failed resolve is logged to host.log with the
/// real yt-dlp error (kind + stderr tail), so the next retest
/// can tell a bot block from a stale host from a missing binary
/// instead of guessing from the extension's message. The detail
/// is yt-dlp's stderr tail, never cookies.
async fn resolve_with_heartbeat(
    output: &mut std::io::StdoutLock<'static>,
    log_path: &std::path::Path,
    request_id: &str,
    page_url: String,
    opts: ytdlp::ResolveOptions,
) -> Option<serde_json::Value> {
    let fut = ytdlp::resolve(&page_url, &opts);
    tokio::pin!(fut);
    let mut elapsed: u64 = 0;
    loop {
        tokio::select! {
            r = &mut fut => {
                if let Err(e) = &r {
                    log_line(
                        log_path,
                        &format!("resolve failed: kind={} {}", e.kind_str(), e.message),
                    );
                }
                return Some(resolve_to_reply(request_id, r));
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                elapsed += 5;
                let beat = serde_json::json!({
                    "type": "resolve_progress", "ok": true,
                    "request_id": request_id, "elapsed_secs": elapsed,
                });
                if write_message(output, &beat).is_err() {
                    return None; // stdout closed: the browser is gone
                }
                let _ = output.flush();
            }
        }
    }
}

/// Round 78 — turn a resolve outcome into the framed reply:
/// success carries the trimmed quality list (never raw `-J`);
/// an error carries the human message plus `needs_login` and
/// the machine `error_kind` the extension switches on.
fn resolve_to_reply(
    request_id: &str,
    result: Result<ytdlp::ResolvedVideo, ytdlp::YtDlpError>,
) -> serde_json::Value {
    match result {
        Ok(video) => serde_json::json!({
            "type": "resolved", "ok": true, "request_id": request_id,
            "title": video.title, "is_live": video.is_live,
            "degraded": video.degraded,
            "warning": video.warnings.first(),
            "formats": ytdlp::quality_rows(&video),
        }),
        Err(e) => serde_json::json!({
            "type": "error", "ok": false, "request_id": request_id,
            "error": e.user_message(),
            "needs_login": e.kind == ytdlp::YtDlpErrorKind::LoginRequired,
            "error_kind": e.kind_str(),
        }),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let parsed = match parse_host_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            print_usage();
            std::process::exit(2);
        }
    };
    if parsed.help {
        print_usage();
        return;
    }

    let db_path = resolve_db_path(parsed.db_path);
    // Round 70 — the host log lives next to the database.
    let log_path = db_path.with_file_name("host.log");
    log_line(
        &log_path,
        &format!("fetchline-host: started (db {})", db_path.display()),
    );
    // Round 78 — sweep stale yt-dlp cookie temp files left by a
    // crashed host (the cookie file is deleted on drop in the
    // normal path; a crash leaves it behind).
    ytdlp::sweep_stale_cookie_files(&std::env::temp_dir());
    // Round 70 — the store opens lazily, on the first message that
    // needs it. A ping is answered with no store at all.
    let mut store: Option<Arc<Store>> = None;
    let mut manager: Option<Arc<DownloadManager>> = None;

    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    loop {
        let value = match decode_message(&mut input) {
            Ok(Some(v)) => v,
            Ok(None) => break, // clean EOF: the browser closed the channel
            Err(e) => {
                // BUG-013: a malformed JSON payload used to kill the
                // host with no reply at all. The length prefix was
                // read fine, so the stream is still aligned — answer
                // with a framed error and keep going, exactly like
                // an unknown message type. Only a framing failure
                // (oversized length, truncated payload, I/O error)
                // ends the process, because the stream can no longer
                // be trusted.
                if matches!(e, EngineError::Json(_)) {
                    let reply = serde_json::json!({
                        "type": "error", "ok": false,
                        "error": format!("Could not understand the message from the browser: {e}"),
                    });
                    if write_message(&mut output, &reply).is_err() {
                        break; // stdout closed: the browser is gone
                    }
                    let _ = output.flush();
                    continue;
                }
                eprintln!("fetchline-host: bad native message: {e}");
                log_line(
                    &log_path,
                    &format!("fetchline-host: bad native message: {e}"),
                );
                break;
            }
        };
        let msg: std::result::Result<HostMessage, _> = serde_json::from_value(value);
        // Round 70 — ping is store-free: the popup's connectivity
        // check must succeed even when the database cannot be
        // opened, so the extension reports the real problem instead
        // of "Error when communicating with the native messaging
        // host".
        if let Ok(HostMessage::Ping) = &msg {
            let reply = serde_json::json!({"type": "pong", "ok": true});
            if write_message(&mut output, &reply).is_err() {
                break; // stdout closed: the browser is gone
            }
            let _ = output.flush();
            continue;
        }
        let reply_value = match msg {
            Ok(HostMessage::ResolveVideo {
                page_url,
                request_id,
                with_cookies,
                cookies,
                extractor_args,
            }) => {
                // Round 78 — a resolve runs yt-dlp for up to ~30 s
                // with a heartbeat; it cannot go through the sync
                // `handle_host_message` (which refuses it).
                match resolve_video(
                    &db_path,
                    &log_path,
                    &mut store,
                    &mut manager,
                    &mut output,
                    page_url,
                    request_id,
                    with_cookies,
                    &cookies,
                    extractor_args.as_deref(),
                )
                .await
                {
                    Some(v) => v,
                    None => break, // stdout closed mid-resolve: the browser is gone
                }
            }
            Ok(HostMessage::DownloadAll { page_url, links }) if links.is_empty() => {
                // No links supplied: expand the page with the engine
                // (fetch + probe), then enqueue what it found.
                match ensure_store(&db_path, &log_path, &mut store, &mut manager) {
                    Ok((store, manager)) => match manager.enqueue_page_links(&page_url).await {
                        Ok(n) => {
                            let pending = store
                                .list_pending_links(Some("pending"))
                                .map(|rows| rows.len())
                                .unwrap_or(0);
                            serde_json::json!({
                                "type": "queued", "ok": true, "enqueued": n,
                                "pending_count": pending,
                                "message": format!("Queued {n} link(s) from the page"),
                            })
                        }
                        Err(e) => serde_json::json!({
                            "type": "error", "ok": false, "error": e.to_string(),
                        }),
                    },
                    Err(e) => serde_json::json!({
                        "type": "error", "ok": false, "error": e,
                    }),
                }
            }
            Ok(m) => match ensure_store(&db_path, &log_path, &mut store, &mut manager) {
                Ok((store, _)) => {
                    let reply = handle_host_message(&store, &m);
                    serde_json::to_value(reply).unwrap_or_else(|_| {
                        serde_json::json!({"type": "error", "ok": false, "error": "Could not serialise the reply"})
                    })
                }
                Err(e) => serde_json::json!({
                    "type": "error", "ok": false, "error": e,
                }),
            },
            Err(e) => serde_json::json!({
                "type": "error", "ok": false,
                "error": format!("Could not understand the message from the browser: {e}"),
            }),
        };
        if write_message(&mut output, &reply_value).is_err() {
            break; // stdout closed: the browser is gone
        }
        let _ = output.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Chrome's exact launch arguments for the host (proven by the
    // VPS spawn log, BUG-043): the extension origin, then
    // --parent-window=<hwnd> (a real HWND for a windowed spawn,
    // 0 in headless).
    const CHROME_ARGV: &[&str] = &[
        "chrome-extension://mioamdgogndabpcjbdekggcnlkkfpmbj/",
        "--parent-window=1966140",
    ];

    #[test]
    fn chrome_launch_args_are_ignored() {
        let a = parse_host_args(CHROME_ARGV.iter().copied()).unwrap();
        assert!(a.db_path.is_none());
        assert!(!a.help);
        // Reversed order and a headless (0) parent window.
        let a = parse_host_args(
            [
                "--parent-window=0",
                "chrome-extension://mioamdgogndabpcjbdekggcnlkkfpmbj/",
            ]
            .iter()
            .copied(),
        )
        .unwrap();
        assert!(a.db_path.is_none());
        // Mixed with our own flags, bare --parent-window form.
        let a = parse_host_args(
            [
                "chrome-extension://mioamdgogndabpcjbdekggcnlkkfpmbj/",
                "--parent-window",
                "12345",
                "--db-path",
                r"C:\x\app.db",
            ]
            .iter()
            .copied(),
        )
        .unwrap();
        assert_eq!(a.db_path, Some(PathBuf::from(r"C:\x\app.db")));
        // Firefox passes its origin the same way.
        let a = parse_host_args(
            ["moz-extension://01234567-89ab-cdef-0123-456789abcdef/"]
                .iter()
                .copied(),
        )
        .unwrap();
        assert!(a.db_path.is_none());
    }

    #[test]
    fn unknown_args_still_rejected() {
        assert!(parse_host_args(["--bogus"].iter().copied()).is_err());
        assert!(parse_host_args(["--db-path"].iter().copied()).is_err());
        // A bare --parent-window followed by a non-numeric arg must
        // NOT eat that arg — it is still parsed as a flag.
        let a = parse_host_args(["--parent-window", "--db-path", "x.db"].iter().copied()).unwrap();
        assert_eq!(a.db_path, Some(PathBuf::from("x.db")));
    }

    // Round 78 — the resolve reply carries a TRIMMED quality
    // list (one row per height, 360p and up), never raw `-J`:
    // a 1080p video-only format becomes a row that needs a
    // merge, a 720p progressive format a row that does not.
    fn resolved_video_fixture() -> ytdlp::ResolvedVideo {
        use std::collections::HashMap;
        ytdlp::ResolvedVideo {
            title: "Test video".to_string(),
            is_live: false,
            extractor: "youtube".to_string(),
            formats: vec![
                ytdlp::ResolvedFormat {
                    format_id: "137".to_string(),
                    ext: "mp4".to_string(),
                    height: Some(1080),
                    width: Some(1920),
                    vcodec: Some("avc1.640028".to_string()),
                    acodec: None,
                    filesize: Some(90_000_000),
                    url: "https://rr1---sn.googlevideo.com/videoplayback?id=137".to_string(),
                    protocol: "https".to_string(),
                    http_headers: HashMap::new(),
                    has_fragments: false,
                },
                ytdlp::ResolvedFormat {
                    format_id: "22".to_string(),
                    ext: "mp4".to_string(),
                    height: Some(720),
                    width: Some(1280),
                    vcodec: Some("avc1.64001F".to_string()),
                    acodec: Some("mp4a.40.2".to_string()),
                    filesize: Some(40_000_000),
                    url: "https://rr1---sn.googlevideo.com/videoplayback?id=22".to_string(),
                    protocol: "https".to_string(),
                    http_headers: HashMap::new(),
                    has_fragments: false,
                },
                ytdlp::ResolvedFormat {
                    format_id: "140".to_string(),
                    ext: "m4a".to_string(),
                    height: None,
                    width: None,
                    vcodec: None,
                    acodec: Some("mp4a.40.2".to_string()),
                    filesize: Some(5_000_000),
                    url: "https://rr1---sn.googlevideo.com/videoplayback?id=140".to_string(),
                    protocol: "https".to_string(),
                    http_headers: HashMap::new(),
                    has_fragments: false,
                },
            ],
            warnings: vec![],
            degraded: false,
        }
    }

    #[test]
    fn resolve_reply_carries_trimmed_quality_rows() {
        let reply = resolve_to_reply("req-1", Ok(resolved_video_fixture()));
        assert_eq!(reply["type"], "resolved");
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["request_id"], "req-1");
        assert_eq!(reply["title"], "Test video");
        assert_eq!(reply["is_live"], false);
        assert_eq!(reply["degraded"], false);
        let formats = reply["formats"].as_array().expect("formats array");
        assert_eq!(formats.len(), 2);
        // Sorted ascending: 720p progressive first (no merge),
        // then 1080p video-only (needs merge, audio paired).
        assert_eq!(formats[0]["height"], 720);
        assert_eq!(formats[0]["needs_merge"], false);
        assert_eq!(formats[0]["has_audio"], true);
        assert_eq!(formats[1]["height"], 1080);
        assert_eq!(formats[1]["needs_merge"], true);
        assert_eq!(formats[1]["audio_format_id"], "140");
        // No raw dump: the reply has no `formats` dump, no
        // extractor internals, and no full format list.
        assert!(reply.get("formats_json").is_none());
        assert!(reply.get("extractor").is_none());
    }

    #[test]
    fn resolve_error_reply_carries_kind_and_login_flag() {
        let login = resolve_to_reply(
            "req-2",
            Err(ytdlp::YtDlpError::new(
                ytdlp::YtDlpErrorKind::LoginRequired,
                "sign in to confirm".to_string(),
            )),
        );
        assert_eq!(login["type"], "error");
        assert_eq!(login["ok"], false);
        assert_eq!(login["request_id"], "req-2");
        assert_eq!(login["needs_login"], true);
        assert_eq!(login["error_kind"], "login_required");
        assert!(login["error"].as_str().unwrap().contains("signed in"));

        let timeout = resolve_to_reply(
            "req-3",
            Err(ytdlp::YtDlpError::new(
                ytdlp::YtDlpErrorKind::Timeout,
                "timed out".to_string(),
            )),
        );
        assert_eq!(timeout["needs_login"], false);
        assert_eq!(timeout["error_kind"], "timeout");
    }
}
