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

use fetchline_engine::native_host::{decode_message, handle_host_message, write_message, HostMessage};
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
        out.push(PathBuf::from(&p).join("com.fetchline.app").join("fetchline").join("app.db"));
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
        out.push(xdg.join("com.fetchline.app").join("fetchline").join("app.db"));
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

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mut db_flag: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--db-path" => match args.next() {
                Some(p) => db_flag = Some(PathBuf::from(p)),
                None => {
                    eprintln!("--db-path needs a path");
                    print_usage();
                    std::process::exit(2);
                }
            },
            "--help" | "-h" => {
                print_usage();
                return;
            }
            other => {
                eprintln!("Unknown argument: {other}");
                print_usage();
                std::process::exit(2);
            }
        }
    }

    let db_path = resolve_db_path(db_flag);
    let store = match Store::open(&db_path) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            // The extension gets no reply if we die here — but
            // writing a framed error to stdout would corrupt the
            // protocol, so stderr is the only channel left.
            eprintln!("fetchline-host: cannot open database {}: {e}", db_path.display());
            std::process::exit(1);
        }
    };
    let manager = DownloadManager::new(Arc::clone(&store), 8);

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
                break;
            }
        };
        let msg: std::result::Result<HostMessage, _> = serde_json::from_value(value);
        let reply_value = match msg {
            Ok(HostMessage::DownloadAll { page_url, links }) if links.is_empty() => {
                // No links supplied: expand the page with the engine
                // (fetch + probe), then enqueue what it found.
                match manager.enqueue_page_links(&page_url).await {
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
                }
            }
            Ok(m) => {
                let reply = handle_host_message(&store, &m);
                serde_json::to_value(reply).unwrap_or_else(|_| {
                    serde_json::json!({"type": "error", "ok": false, "error": "Could not serialise the reply"})
                })
            }
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
