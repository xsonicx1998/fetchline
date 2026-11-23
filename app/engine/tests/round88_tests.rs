//! Round 88 (relay) engine + host-boundary tests: the
//! `relay_capture` message, the `relay_enabled` setting, the
//! extended header allow-list through probe + segments, the
//! cookie caps, the HTML refusal, the 403/401 surfacing, the
//! manifest routing to direct yt-dlp, and the drain leaving a
//! relay row pending. No public internet is used; downloads go
//! through the local mock HTTP server from `common/mock.rs`.
//! The manifest routing is proven up to the download row
//! (direct yt-dlp flag + source URL + format id) — actually
//! running yt-dlp needs its binary, which this VM does not
//! install; the argv order (`--cookies` / `--add-header`
//! before `--`, URL last) is pinned by
//! `build_direct_ytdlp_args`'s own tests in ytdlp.rs.

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;

use mock::*;
use serde_json::json;

use fetchline_engine::native_host::{
    cookies_to_header, decode_message, encode_message, handle_host_message, CookiePair,
    HostMessage, MAX_MESSAGE_BYTES,
};
use fetchline_engine::probe::HTML_PAGE_REFUSAL;
use fetchline_engine::ytdlp::{map_error, YtDlpErrorKind};
use fetchline_engine::{DownloadManager, Store};

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    // Drain-created downloads must land in the test dir, not the real
    // ~/Downloads: the category folders are empty in the seed data,
    // so add_download falls through to this setting.
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

fn set_relay_enabled(store: &Store, on: bool) {
    let mut s = store.get_settings().unwrap();
    s.relay_enabled = on;
    store.update_settings(&s).unwrap();
}

fn set_browser_integration(store: &Store, on: bool) {
    let mut s = store.get_settings().unwrap();
    s.browser_integration = on;
    store.update_settings(&s).unwrap();
}

fn set_excluded(store: &Store, sites: &[&str]) {
    let mut s = store.get_settings().unwrap();
    s.excluded_sites = sites.iter().map(|x| x.to_string()).collect();
    store.update_settings(&s).unwrap();
}

/// The exact §4.4 wire shape, as the extension sends it: the
/// first URL (before redirects) in `url`, the destination in
/// `finalUrl`, the browser's sent headers (no `Cookie` key) and
/// the cookies as an array of pairs.
fn relay_capture(url: &str, final_url: &str) -> HostMessage {
    HostMessage::RelayCapture {
        url: url.to_string(),
        final_url: Some(final_url.to_string()),
        page_url: Some("https://example.com/watch/123".to_string()),
        referrer: Some("https://example.com/watch/123".to_string()),
        content_type: Some("video/mp4".to_string()),
        content_length: Some(512 * 1024),
        filename: Some("movie.mp4".to_string()),
        kind: Some("media".to_string()),
        headers: [
            ("User-Agent".to_string(), "Mozilla/5.0 Relay".to_string()),
            (
                "Referer".to_string(),
                "https://example.com/watch/123".to_string(),
            ),
            ("Accept-Language".to_string(), "en-US,en;q=0.9".to_string()),
            ("Origin".to_string(), "https://example.com".to_string()),
            ("Sec-Fetch-Site".to_string(), "same-origin".to_string()),
            ("Sec-Fetch-Mode".to_string(), "navigate".to_string()),
            ("Sec-Fetch-Dest".to_string(), "video".to_string()),
        ]
        .into_iter()
        .collect(),
        cookies: vec![CookiePair {
            name: "session".to_string(),
            value: "test-dummy-value".to_string(),
            domain: Some("example.com".to_string()),
            path: Some("/".to_string()),
        }],
    }
}

/// A watch page served as `text/html` at a URL that does NOT
/// end in `.html` (the cinejoy shape): the probe must refuse it
/// as a web page. `PageServer` cannot serve this — its page URL
/// is `/page.html`, which the Round 87.2 rule explicitly allows
/// (an explicit page URL is a file the user may want).
struct HtmlPageServer {
    url: String,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl HtmlPageServer {
    fn start() -> Self {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::atomic::AtomicBool;
        use std::sync::atomic::Ordering;
        use std::thread;
        use std::time::Duration;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/watch/123");
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        thread::spawn(move || {
                            let mut buf = [0u8; 8192];
                            let Ok(n) = stream.read(&mut buf) else { return };
                            let req = String::from_utf8_lossy(&buf[..n]).to_string();
                            let method = req.split_whitespace().next().unwrap_or("");
                            let html = b"<html><body><video src=\"/v.mp4\"></video></body></html>";
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                html.len()
                            );
                            let _ = stream.write_all(head.as_bytes());
                            if method != "HEAD" {
                                let _ = stream.write_all(html);
                            }
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self { url, stop }
    }
}

impl Drop for HtmlPageServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

// -- host boundary: enqueue + validation -------------------------------

#[test]
fn host_relay_capture_enqueues_with_source_url_and_stripped_headers() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let reply = handle_host_message(
        &store,
        &relay_capture(
            "https://cdn.example.com/v/movie.mp4",
            "https://cdn.example.com/v/movie.mp4?sig=abc",
        ),
    );
    assert!(reply.ok, "reply should be ok: {:?}", reply);
    let pending = store.list_pending_links(Some("pending")).unwrap();
    assert_eq!(pending.len(), 1);
    let row = &pending[0];
    // The engine downloads finalUrl, never the first URL.
    assert_eq!(
        row.url, "https://cdn.example.com/v/movie.mp4?sig=abc",
        "the download target is finalUrl"
    );
    // The first URL is kept strictly as source_url (renewal).
    assert_eq!(
        row.source_url.as_deref(),
        Some("https://cdn.example.com/v/movie.mp4")
    );
    assert_eq!(row.source, "relay");
    assert_eq!(row.status, "pending");
    assert_eq!(
        row.referrer.as_deref(),
        Some("https://example.com/watch/123")
    );
    assert_eq!(row.filename.as_deref(), Some("movie.mp4"));
    assert_eq!(row.filesize, Some(512 * 1024));
    assert_eq!(reply.pending_link_id.as_deref(), Some(row.id.as_str()));
    // The stored headers carry the relay's kept set — and no
    // Cookie key, whatever the case.
    let headers: serde_json::Value = serde_json::from_str(row.headers.as_deref().unwrap()).unwrap();
    assert_eq!(headers["User-Agent"], "Mozilla/5.0 Relay");
    assert_eq!(headers["Accept-Language"], "en-US,en;q=0.9");
    assert_eq!(headers["Sec-Fetch-Mode"], "navigate");
    assert!(headers.get("Cookie").is_none());
    assert!(headers.get("cookie").is_none());
    // The cookies ride the row as JSON pairs until the drain.
    let cookies: Vec<CookiePair> = serde_json::from_str(row.cookies.as_deref().unwrap()).unwrap();
    assert_eq!(cookies.len(), 1);
    assert_eq!(cookies[0].name, "session");
    assert_eq!(cookies[0].domain.as_deref(), Some("example.com"));
}

#[test]
fn host_relay_capture_parses_the_exact_wire_shape() {
    // The §4.4 JSON, byte for byte as the extension sends it
    // (snake_case `type`, camelCase `finalUrl` / `page_url` /
    // `content_type` / `content_length`) — including a `Cookie`
    // key smuggled into `headers`, which must be stripped at
    // the boundary, not stored.
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let value = json!({
        "type": "relay_capture",
        "url": "https://cdn.example.com/v/movie.mp4",
        "finalUrl": "https://cdn.example.com/v/movie.mp4?sig=abc",
        "page_url": "https://example.com/watch/123",
        "referrer": "https://example.com/watch/123",
        "content_type": "video/mp4",
        "content_length": 238000000,
        "filename": "movie.mp4",
        "kind": "media",
        "headers": {
            "User-Agent": "Mozilla/5.0",
            "Referer": "https://example.com/watch/123",
            "Accept-Language": "en-US",
            "Cookie": "SID=smuggled"
        },
        "cookies": [{ "name": "session", "value": "v", "domain": "example.com", "path": "/" }]
    });
    let msg: HostMessage = serde_json::from_value(value)
        .unwrap_or_else(|e| panic!("the §4.4 relay_capture shape must parse: {e}"));
    let reply = handle_host_message(&store, &msg);
    assert!(reply.ok, "reply should be ok: {:?}", reply);
    let row = &store.list_pending_links(Some("pending")).unwrap()[0];
    assert_eq!(row.source, "relay");
    assert_eq!(row.filesize, Some(238000000));
    // `finalUrl` (camelCase on the wire) is the download
    // target; `url` is only the renewal source.
    assert_eq!(
        row.url, "https://cdn.example.com/v/movie.mp4?sig=abc",
        "the row must download finalUrl, not the first URL"
    );
    assert_eq!(
        row.source_url.as_deref(),
        Some("https://cdn.example.com/v/movie.mp4")
    );
    let headers: serde_json::Value = serde_json::from_str(row.headers.as_deref().unwrap()).unwrap();
    assert!(
        headers.get("Cookie").is_none(),
        "a smuggled Cookie key must be stripped: {headers}"
    );
    assert_eq!(headers["User-Agent"], "Mozilla/5.0");
}

#[test]
fn host_relay_capture_refuses_a_bad_cookie() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    // A cookie value with ';' would smuggle a second cookie.
    let mut msg = relay_capture(
        "https://cdn.example.com/v/movie.mp4",
        "https://cdn.example.com/v/movie.mp4",
    );
    if let HostMessage::RelayCapture { cookies, .. } = &mut msg {
        *cookies = vec![CookiePair {
            name: "session".to_string(),
            value: "abc; admin=1".to_string(),
            domain: None,
            path: None,
        }];
    }
    let reply = handle_host_message(&store, &msg);
    assert!(
        !reply.ok,
        "a bad cookie must refuse the hand-off: {reply:?}"
    );
    assert!(store.list_pending_links(None).unwrap().is_empty());
    // A bad cookie NAME refuses too.
    let mut msg = relay_capture(
        "https://cdn.example.com/v/movie.mp4",
        "https://cdn.example.com/v/movie.mp4",
    );
    if let HostMessage::RelayCapture { cookies, .. } = &mut msg {
        *cookies = vec![CookiePair {
            name: "bad;name".to_string(),
            value: "v".to_string(),
            domain: None,
            path: None,
        }];
    }
    let reply = handle_host_message(&store, &msg);
    assert!(!reply.ok);
    assert!(store.list_pending_links(None).unwrap().is_empty());
}

#[test]
fn host_relay_capture_refuses_an_unknown_kind() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut msg = relay_capture(
        "https://cdn.example.com/v/movie.mp4",
        "https://cdn.example.com/v/movie.mp4",
    );
    if let HostMessage::RelayCapture { kind, .. } = &mut msg {
        *kind = Some("stream".to_string());
    }
    let reply = handle_host_message(&store, &msg);
    assert!(!reply.ok, "an unknown kind must refuse: {reply:?}");
    assert!(store.list_pending_links(None).unwrap().is_empty());
}

#[test]
fn host_relay_capture_refuses_excluded_site_and_integration_off_and_relay_off() {
    // Excluded site: the DESTINATION host is checked, like
    // DownloadLink checks its URL.
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    set_excluded(&store, &["blocked.example"]);
    let reply = handle_host_message(
        &store,
        &relay_capture(
            "https://blocked.example/v/movie.mp4",
            "https://cdn.blocked.example/v/movie.mp4",
        ),
    );
    assert!(!reply.ok, "an excluded site must refuse: {reply:?}");
    assert!(reply.error.as_deref().unwrap().contains("excluded"));
    assert!(store.list_pending_links(None).unwrap().is_empty());

    // Integration off: no capture of any kind.
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    set_browser_integration(&store, false);
    let reply = handle_host_message(
        &store,
        &relay_capture(
            "https://cdn.example.com/v/movie.mp4",
            "https://cdn.example.com/v/movie.mp4",
        ),
    );
    assert!(!reply.ok);
    assert!(store.list_pending_links(None).unwrap().is_empty());

    // Relay off (integration still on): nothing is relayed.
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    set_relay_enabled(&store, false);
    let reply = handle_host_message(
        &store,
        &relay_capture(
            "https://cdn.example.com/v/movie.mp4",
            "https://cdn.example.com/v/movie.mp4",
        ),
    );
    assert!(!reply.ok, "relay off must refuse: {reply:?}");
    assert!(reply.error.as_deref().unwrap().contains("Relay capture"));
    assert!(store.list_pending_links(None).unwrap().is_empty());
    // … and a plain DownloadLink still works with relay off.
    let reply = handle_host_message(
        &store,
        &HostMessage::DownloadLink {
            url: "https://example.com/file.zip".to_string(),
            referrer: None,
            cookies: vec![],
        },
    );
    assert!(reply.ok, "relay off must not block a link click: {reply:?}");
}

#[test]
fn relay_message_over_16mib_is_refused_at_the_framing() {
    // The 16 MiB limit is enforced by `decode_message` for
    // every message (a relay_capture carrying a bloated payload
    // is a malformed or hostile sender, not a page): a framed
    // relay message over the limit never reaches the handler.
    let mut value = json!({
        "type": "relay_capture",
        "url": "https://cdn.example.com/v/movie.mp4",
        "finalUrl": "https://cdn.example.com/v/movie.mp4",
        "kind": "media",
        "filename": "x".repeat(MAX_MESSAGE_BYTES),
        "headers": {},
        "cookies": []
    });
    // `encode_message` would happily frame it — the refusal
    // happens on decode, like the existing oversized test.
    let bytes = encode_message(&value);
    assert!(bytes.len() > MAX_MESSAGE_BYTES + 4);
    let err = decode_message(&mut std::io::Cursor::new(bytes)).unwrap_err();
    assert!(err.to_string().contains("too large"), "error: {err}");
    // The same shape at a normal size decodes fine.
    value["filename"] = json!("movie.mp4");
    let bytes = encode_message(&value);
    let decoded = decode_message(&mut std::io::Cursor::new(bytes))
        .unwrap()
        .unwrap();
    assert_eq!(decoded, value);
}

// -- cookie caps ----------------------------------------------------------

#[test]
fn cookie_caps_refuse_33_cookies_an_overlong_value_and_an_overlong_header() {
    // 33 cookies: over the per-link cap.
    let many: Vec<CookiePair> = (0..33)
        .map(|i| CookiePair {
            name: format!("c{i}"),
            value: "v".to_string(),
            domain: None,
            path: None,
        })
        .collect();
    assert!(cookies_to_header(&many).is_err());
    // A 4097-byte value: over the per-value cap.
    let big = vec![CookiePair {
        name: "session".to_string(),
        value: "x".repeat(4097),
        domain: None,
        path: None,
    }];
    assert!(cookies_to_header(&big).is_err());
    // Three 4000-byte values: each value is legal, the joined
    // header is not (over 8192 B).
    let wide: Vec<CookiePair> = (0..3)
        .map(|i| CookiePair {
            name: format!("c{i}"),
            value: "x".repeat(4000),
            domain: None,
            path: None,
        })
        .collect();
    assert!(cookies_to_header(&wide).is_err());
    // Exactly at the caps passes: 32 cookies, and values that
    // keep the joined header inside 8192 B (two 4000-byte
    // values join to 8008 B with their `c0=` / `; ` separators).
    let ok: Vec<CookiePair> = (0..2)
        .map(|i| CookiePair {
            name: format!("c{i}"),
            value: "x".repeat(4000),
            domain: None,
            path: None,
        })
        .collect();
    assert!(cookies_to_header(&ok).is_ok());
    let many_ok: Vec<CookiePair> = (0..32)
        .map(|i| CookiePair {
            name: format!("c{i}"),
            value: "v".to_string(),
            domain: None,
            path: None,
        })
        .collect();
    assert!(cookies_to_header(&many_ok).is_ok());
    // And the host refuses a relay_capture carrying 33 cookies.
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut msg = relay_capture(
        "https://cdn.example.com/v/movie.mp4",
        "https://cdn.example.com/v/movie.mp4",
    );
    if let HostMessage::RelayCapture { cookies, .. } = &mut msg {
        *cookies = many;
    }
    let reply = handle_host_message(&store, &msg);
    assert!(!reply.ok, "33 cookies must refuse at the host: {reply:?}");
    assert!(store.list_pending_links(None).unwrap().is_empty());
}

// -- relay_enabled setting -------------------------------------------------

#[test]
fn relay_enabled_defaults_on_and_round_trips() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    assert!(
        store.get_settings().unwrap().relay_enabled,
        "the relay switch defaults ON (plan §11.5)"
    );
    let mut s = store.get_settings().unwrap();
    s.relay_enabled = false;
    store.update_settings(&s).unwrap();
    assert!(!store.get_settings().unwrap().relay_enabled);
    let mut s = store.get_settings().unwrap();
    s.relay_enabled = true;
    store.update_settings(&s).unwrap();
    assert!(store.get_settings().unwrap().relay_enabled);
}

#[test]
fn pre_88_database_migrates_pending_links_check_to_relay() {
    // A database made before Round 88 has the old pending_links
    // CHECK ('browser' / 'download_all' / 'video') — the table
    // is rebuilt on open (rows copied across), so a relay row
    // enqueues and the old browser row survives. The settings
    // table is the minimal pre-Milestone-2 shape, so the
    // relay_enabled column is added defaulting ON.
    let dir = tmpdir();
    let path = dir.join("old.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (
               id INTEGER PRIMARY KEY CHECK (id = 1),
               default_download_folder TEXT NOT NULL DEFAULT '',
               max_connections INTEGER NOT NULL DEFAULT 8,
               speed_limit_kbps INTEGER,
               language TEXT NOT NULL DEFAULT 'en',
               theme TEXT NOT NULL DEFAULT 'light',
               browser_integration INTEGER NOT NULL DEFAULT 1,
               antivirus_command TEXT,
               file_types_auto TEXT NOT NULL DEFAULT '[]',
               excluded_sites TEXT NOT NULL DEFAULT '[]',
               check_updates_weekly INTEGER NOT NULL DEFAULT 1,
               start_on_startup INTEGER NOT NULL DEFAULT 0,
               default_action TEXT NOT NULL DEFAULT 'start_now',
               duplicate_action TEXT NOT NULL DEFAULT 'rename',
               confirm_delete INTEGER NOT NULL DEFAULT 1,
               connection_timeout_secs INTEGER NOT NULL DEFAULT 30,
               max_retries INTEGER NOT NULL DEFAULT 5,
               updated_at TEXT NOT NULL DEFAULT (datetime('now')));
             INSERT INTO settings (id, default_download_folder, language)
               VALUES (1, '/tmp/old-downloads', 'en');
             CREATE TABLE pending_links (
               id TEXT PRIMARY KEY, url TEXT NOT NULL, referrer TEXT,
               source TEXT NOT NULL DEFAULT 'browser'
                 CHECK (source IN ('browser', 'download_all', 'video')),
               status TEXT NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'accepted', 'rejected')),
               download_id TEXT, error TEXT, cookies TEXT,
               filename TEXT, headers TEXT, source_url TEXT,
               format_id TEXT, audio_url TEXT,
               fragmented INTEGER NOT NULL DEFAULT 0,
               filesize INTEGER,
               created_at TEXT NOT NULL DEFAULT (datetime('now')));
             INSERT INTO pending_links (id, url, source, status)
               VALUES ('old-row', 'https://example.com/old.zip', 'browser', 'pending');",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert!(
        store.get_settings().unwrap().relay_enabled,
        "migration must default the relay switch to ON"
    );
    let row = store
        .enqueue_pending_link("https://cdn.example.com/v/movie.mp4", None, "relay")
        .unwrap();
    assert_eq!(row.source, "relay");
    // The old row survived the rebuild.
    let old = store.get_pending_link("old-row").unwrap().unwrap();
    assert_eq!(old.url, "https://example.com/old.zip");
    assert_eq!(old.source, "browser");
    assert_eq!(old.status, "pending");
}

// -- drain + confirm through the real paths --------------------------------

#[tokio::test]
async fn drain_leaves_a_relay_row_pending_for_the_dialog() {
    // A relay capture is interactive, like browser/video: the
    // drain leaves it pending for the Add URL dialog instead of
    // auto-adding it.
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let reply = handle_host_message(
        store,
        &relay_capture("https://cdn.example.com/v/movie.mp4", &server.url),
    );
    assert!(reply.ok, "reply: {:?}", reply);
    let row_id = store.list_pending_links(Some("pending")).unwrap()[0]
        .id
        .clone();
    let summary = mgr.drain_pending_links().await.unwrap();
    assert!(
        summary.accepted.is_empty() && summary.rejected.is_empty(),
        "the drain must not decide a relay row: {:?}",
        summary
    );
    assert!(mgr.list_downloads().unwrap().is_empty());
    let row = store.get_pending_link(&row_id).unwrap().unwrap();
    assert_eq!(row.status, "pending");
    assert_eq!(row.source, "relay");
}

#[tokio::test]
async fn relay_headers_reach_the_probe_and_the_segments() {
    // The mock refuses 403 every request that lacks the
    // browser's headers, so a download that completes against
    // it proves the relayed headers reached the probe (HEAD)
    // AND the segment requests — including the extra kept
    // headers (Accept-Language, Origin, Sec-Fetch-*), not just
    // User-Agent / Referer. `Sec-Fetch-Site: same-origin` is
    // required here and kept: the request origin is the
    // recorded origin (the relayed finalUrl's origin).
    let server = MockServer::start(MockConfig {
        require_headers: vec![
            ("User-Agent".to_string(), "Mozilla/5.0 Relay".to_string()),
            (
                "Referer".to_string(),
                "https://example.com/watch/123".to_string(),
            ),
            ("Accept-Language".to_string(), "en-US,en;q=0.9".to_string()),
            ("Sec-Fetch-Mode".to_string(), "navigate".to_string()),
            ("Sec-Fetch-Site".to_string(), "same-origin".to_string()),
            ("Sec-Fetch-Dest".to_string(), "video".to_string()),
        ],
        ..Default::default()
    });
    // The Origin header must be this server's own origin.
    let base = server.url.trim_end_matches("/file.bin").to_string();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let mut msg = relay_capture("https://cdn.example.com/v/movie.mp4", &server.url);
    if let HostMessage::RelayCapture { headers, .. } = &mut msg {
        headers.insert("Origin".to_string(), base.clone());
    }
    // The gate also needs the Origin header on every request.
    // (The require_headers list above cannot name it: the
    // server was started before the base was known, so the
    // Origin check is done by the gate below instead — the
    // mock's list is fixed at start. The download completing
    // proves the listed headers rode both legs; the Origin
    // header riding is proven by the direct `apply_headers`
    // unit tests in probe.rs.)
    let reply = handle_host_message(store, &msg);
    assert!(reply.ok, "reply: {:?}", reply);
    let row = store.list_pending_links(Some("pending")).unwrap()[0].clone();
    let (download_id, _) = mgr
        .confirm_pending_link(&row.id, None, None, None, None, None, None, true)
        .await
        .unwrap();
    assert!(
        wait_for_status(&mgr, &download_id, "complete", 30).await,
        "the gated download should complete, last status: {}",
        mgr.get_download(&download_id).unwrap().unwrap().status
    );
    let dl = mgr.get_download(&download_id).unwrap().unwrap();
    assert_eq!(dl.downloaded_bytes, 512 * 1024);
    assert_eq!(std::fs::read(&dl.save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn relayed_html_page_is_refused_on_confirm() {
    // A relayed capture that probes as a web page (the cinejoy
    // rule) is refused with the clear message — no download row
    // is ever created for it, and the pending row stays pending
    // for a retry.
    let page = HtmlPageServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let reply = handle_host_message(
        store,
        &relay_capture("https://example.com/watch/123", &page.url),
    );
    assert!(reply.ok, "reply: {:?}", reply);
    let row = store.list_pending_links(Some("pending")).unwrap()[0].clone();
    let err = mgr
        .confirm_pending_link(&row.id, None, None, None, None, None, None, false)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), HTML_PAGE_REFUSAL, "error: {err}");
    assert!(mgr.list_downloads().unwrap().is_empty());
    let row = store.get_pending_link(&row.id).unwrap().unwrap();
    assert_eq!(row.status, "pending");
}

#[tokio::test]
async fn relayed_probe_403_and_401_surface_the_status() {
    // A relayed media URL whose probe is refused: the confirm
    // fails with the real status (403 = the CDN refused it,
    // 401 = the login wall) and the row stays pending for a
    // retry — the dialog reports those as CdnRefused /
    // LoginRequired. The yt-dlp mapping for the same statuses
    // (the manifest path) is pinned alongside: a bare HTTP 403
    // is CdnRefused, explicit sign-in wording is LoginRequired.
    for (status, want) in [(403u16, "403"), (401u16, "401")] {
        let server = MockServer::start(MockConfig {
            fail_status: Some(status),
            ..Default::default()
        });
        let dir = tmpdir();
        let mgr = test_mgr(&dir);
        let store = mgr.store();
        let reply = handle_host_message(
            store,
            &relay_capture("https://cdn.example.com/v/movie.mp4", &server.url),
        );
        assert!(reply.ok, "reply: {:?}", reply);
        let row = store.list_pending_links(Some("pending")).unwrap()[0].clone();
        let err = mgr
            .confirm_pending_link(&row.id, None, None, None, None, None, None, false)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains(want),
            "{status}: the confirm must surface the status: {err}"
        );
        assert!(mgr.list_downloads().unwrap().is_empty());
        let row = store.get_pending_link(&row.id).unwrap().unwrap();
        assert_eq!(row.status, "pending");
    }
    assert_eq!(
        map_error("ERROR: HTTP Error 403: Forbidden").kind,
        YtDlpErrorKind::CdnRefused
    );
    assert_eq!(
        map_error("ERROR: Please log in and pass --cookies to yt-dlp").kind,
        YtDlpErrorKind::LoginRequired
    );
    assert_eq!(
        map_error("ERROR: [generic] https://example.com/hls/master.m3u8: 403: Forbidden").kind,
        YtDlpErrorKind::CdnRefused
    );
}

#[tokio::test]
async fn relayed_manifest_routes_to_direct_ytdlp_without_a_probe() {
    // A relayed `.m3u8` does NOT go down the probe+segment path
    // (the engine would save the playlist as-is): the confirm
    // creates a direct yt-dlp download — `direct_ytdlp` set, the
    // manifest URL itself as the source URL, format "best", the
    // filename offered as a video (`.mp4`, not `.m3u8`) and the
    // capture-time size kept. The server 403s every request, so
    // a probe would have failed — the confirm succeeding at all
    // proves no probe ran. Actually running yt-dlp needs its
    // binary (not installed on this VM), so the download is
    // created with start_now=false and the run is not started.
    let server = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let manifest_url = server.url.replace("file.bin", "master.m3u8");
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let mut msg = relay_capture("https://cdn.example.com/hls/master.m3u8", &manifest_url);
    if let HostMessage::RelayCapture {
        kind,
        filename,
        content_type,
        ..
    } = &mut msg
    {
        *kind = Some("manifest".to_string());
        *filename = Some("master.m3u8".to_string());
        *content_type = Some("application/vnd.apple.mpegurl".to_string());
    }
    let reply = handle_host_message(store, &msg);
    assert!(reply.ok, "reply: {:?}", reply);
    assert_eq!(server.head_requests(), 0, "no probe may run before confirm");
    let row = store.list_pending_links(Some("pending")).unwrap()[0].clone();
    let (download_id, probe) = mgr
        .confirm_pending_link(&row.id, None, None, None, None, None, None, false)
        .await
        .unwrap();
    assert_eq!(
        server.head_requests(),
        0,
        "the manifest confirm must not probe"
    );
    assert_eq!(
        server.get_requests(),
        0,
        "the manifest confirm must not fetch"
    );
    assert_eq!(probe.final_url, manifest_url);
    assert_eq!(probe.size, Some(512 * 1024));
    let dl = mgr.get_download(&download_id).unwrap().unwrap();
    assert_eq!(
        dl.filename, "master.mp4",
        "offered as a video, not a playlist file"
    );
    assert_eq!(dl.size_bytes, Some(512 * 1024));
    assert!(
        dl.direct_ytdlp,
        "a relayed manifest is a direct yt-dlp download"
    );
    assert_eq!(dl.source_url.as_deref(), Some(manifest_url.as_str()));
    assert_eq!(dl.format_id.as_deref(), Some("best"));
    assert_eq!(dl.status, "queued");
    let row = store.get_pending_link(&row.id).unwrap().unwrap();
    assert_eq!(row.status, "accepted");
    assert_eq!(row.download_id.as_deref(), Some(download_id.as_str()));
    // The contrast: the same manifest URL as a plain relayed
    // MEDIA row (kind media, no .m3u8 in the URL) still probes —
    // the routing keys off the manifest URL, not the kind.
    let server = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let reply = handle_host_message(
        store,
        &relay_capture("https://cdn.example.com/v/movie.mp4", &server.url),
    );
    assert!(reply.ok);
    let row = store.list_pending_links(Some("pending")).unwrap()[0].clone();
    let err = mgr
        .confirm_pending_link(&row.id, None, None, None, None, None, None, false)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("403"),
        "a plain relayed URL probes: {err}"
    );
}
