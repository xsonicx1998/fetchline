//! Final verification round 2 (2026-11-15) — adversarial spot-checks
//! for the four finished partials, kept as regression tests.
//!
//! BUG-025 (found by this suite): the page-scan probes
//! (`fetch_page_links` / `detect_page_videos`, incl. HLS variant
//! probes and playlist fetches) never sent a saved site login — a
//! login-gated link showed a 401 in Download All / the video panel
//! even though the user had saved the credential and a manual add
//! would have succeeded. Every test serves its own fixture on
//! 127.0.0.1 — no real website, no real browser, is ever touched.
//! Credentials below are test-only dummies ("test-user" /
//! "test-pass-123") and are never a real account; passwords are
//! never logged.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use mock::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use fetchline_engine::options::SiteLoginInput;
use fetchline_engine::{DownloadManager, Store};

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

fn login_input(host: &str) -> SiteLoginInput {
    SiteLoginInput {
        id: None,
        host: host.to_string(),
        username: "test-user".to_string(),
        password: Some("test-pass-123".to_string()),
        auth_type: "basic".to_string(),
    }
}

/// A page + file server that demands HTTP Basic auth with the
/// test-only dummy credential on EVERY request (page, files,
/// playlists). A request without the right `Authorization` header
/// gets 401 and no body. Serves:
/// `/` -> HTML page linking /file.bin, /clip.mp4, /master.m3u8
/// `/file.bin` (64 KiB), `/clip.mp4` (32 KiB)
/// `/master.m3u8` -> master playlist with v720/v480 variants
/// `/v720.m3u8`, `/v480.m3u8` -> media playlists (2 segments each)
struct GatedPageServer {
    pub base: String,
    stop: Arc<AtomicBool>,
}

impl GatedPageServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let expected = basic_auth_value("test-user", "test-pass-123");
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let expected = expected.clone();
                        thread::spawn(move || handle_gated_page(stream, &expected));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            stop,
        }
    }
}

impl Drop for GatedPageServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// An open (unauthenticated) page server: `/` serves an HTML page
/// linking to one absolute URL supplied at start time.
struct OpenPageServer {
    pub base: String,
    stop: Arc<AtomicBool>,
}

impl OpenPageServer {
    fn start(link_url: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let html = format!("<html><body><a href=\"{link_url}\">gated file</a></body></html>");
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let html = html.clone();
                        thread::spawn(move || handle_open_page(stream, &html));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            stop,
        }
    }
}

impl Drop for OpenPageServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn handle_open_page(mut stream: TcpStream, html: &str) {
    let mut buf = [0u8; 8192];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first = req.lines().next().unwrap_or("");
    let method = first.split_whitespace().next().unwrap_or("");
    if method == "HEAD" {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
            html.len()
        );
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    let _ = stream.write_all(head.as_bytes());
}

fn gated_body(path: &str) -> Option<(Vec<u8>, &'static str)> {
    match path {
        "/" => Some((
            b"<html><body><a href=\"/file.bin\">file</a><video src=\"/clip.mp4\"></video><a href=\"/master.m3u8\">stream</a></body></html>"
                .to_vec(),
            "text/html",
        )),
        "/file.bin" => Some((payload(64 * 1024), "application/octet-stream")),
        "/clip.mp4" => Some((payload(32 * 1024), "video/mp4")),
        "/master.m3u8" => Some((
            b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=2500000,RESOLUTION=1280x720\nv720.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=1200000,RESOLUTION=854x480\nv480.m3u8\n"
                .to_vec(),
            "application/vnd.apple.mpegurl",
        )),
        "/v720.m3u8" | "/v480.m3u8" => Some((
            b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:10.0,\nseg0.ts\n#EXTINF:10.0,\nseg1.ts\n#EXT-X-ENDLIST\n"
                .to_vec(),
            "application/vnd.apple.mpegurl",
        )),
        _ => None,
    }
}

fn handle_gated_page(mut stream: TcpStream, expected: &str) {
    let mut buf = [0u8; 8192];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first = req.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/").to_string();
    let authorised = req
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
        .map(|l| l.splitn(2, ':').nth(1).unwrap_or("").trim().to_string())
        .as_deref()
        == Some(expected);
    if !authorised {
        let _ = stream.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"test\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }
    let Some((body, ctype)) = gated_body(&path) else {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    };
    if method == "HEAD" {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"gated1\"\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    let range = req.lines().find_map(|l| {
        let (name, value) = l.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("range") {
            Some(value.trim().to_string())
        } else {
            None
        }
    });
    let (status, out, content_range) = match range {
        Some(r) if r.starts_with("bytes=") => {
            let spec = &r["bytes=".len()..];
            let mut it = spec.split('-');
            let start: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let end: usize = it
                .next()
                .and_then(|s| if s.is_empty() { None } else { s.parse().ok() })
                .unwrap_or(body.len() - 1)
                .min(body.len() - 1);
            (
                "206 Partial Content",
                body[start..=end].to_vec(),
                Some(format!("bytes {start}-{end}/{}", body.len())),
            )
        }
        _ => ("200 OK", body.clone(), None),
    };
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"gated1\"\r\nContent-Type: {ctype}\r\n",
        out.len()
    );
    if let Some(cr) = content_range {
        head.push_str(&format!("Content-Range: {cr}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&out);
}

/// BUG-025: a saved site login must be sent by the page-scan
/// probes, not only by the add/download path. Gated page + gated
/// links, login saved -> every link probes OK with its real size.
#[tokio::test]
async fn verify2_page_scan_sends_saved_site_login() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let server = GatedPageServer::start();
    mgr.save_site_login(&login_input("127.0.0.1")).unwrap();

    let links = mgr.fetch_page_links(&server.base).await.unwrap();
    let by_url = |suffix: &str| links.iter().find(|l| l.url.ends_with(suffix)).unwrap();
    let file = by_url("/file.bin");
    assert_eq!(
        file.error, None,
        "gated file link must probe OK: {:?}",
        file.error
    );
    assert_eq!(file.size, Some(64 * 1024));
    let clip = by_url("/clip.mp4");
    assert_eq!(
        clip.error, None,
        "gated video link must probe OK: {:?}",
        clip.error
    );
    assert_eq!(clip.size, Some(32 * 1024));

    // The video panel path rides on the same scan: the gated clip
    // must be listed downloadable, not flagged with a 401.
    let videos = mgr.detect_page_videos(&server.base).await.unwrap();
    let clip_v = videos
        .iter()
        .find(|v| v.url.ends_with("/clip.mp4"))
        .unwrap();
    assert!(
        clip_v.downloadable,
        "gated clip must be downloadable, got {:?}",
        clip_v.error
    );
    assert_eq!(clip_v.size, Some(32 * 1024));
}

/// BUG-025, negative side: with no login saved, the gated page
/// itself cannot be scanned — the failure names 401.
#[tokio::test]
async fn verify2_page_scan_without_login_fails_gated_page() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let server = GatedPageServer::start();
    let err = mgr.fetch_page_links(&server.base).await.unwrap_err();
    assert!(
        err.to_string().contains("401"),
        "expected a 401 error, got: {err}"
    );
}

/// BUG-025, host scoping: a login saved for another host must NOT
/// be sent to the gated host (the link stays 401), while a login
/// saved for the gated host itself probes OK.
#[tokio::test]
async fn verify2_page_scan_login_not_sent_to_another_host() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let gated = GatedPageServer::start();
    let page = OpenPageServer::start(&format!("{}/file.bin", gated.base));
    mgr.save_site_login(&login_input("localhost")).unwrap();

    let links = mgr.fetch_page_links(&page.base).await.unwrap();
    let file = links.iter().find(|l| l.url.ends_with("/file.bin")).unwrap();
    assert!(
        file.error.as_deref().unwrap_or("").contains("401"),
        "login for another host must not unlock the gated link, got: {:?}",
        file.error
    );

    mgr.save_site_login(&login_input("127.0.0.1")).unwrap();
    let links = mgr.fetch_page_links(&page.base).await.unwrap();
    let file = links.iter().find(|l| l.url.ends_with("/file.bin")).unwrap();
    assert_eq!(
        file.error, None,
        "login for the gated host must probe OK: {:?}",
        file.error
    );
    assert_eq!(file.size, Some(64 * 1024));
}

/// BUG-025, HLS: the master-playlist fetch and each variant probe
/// in the video scan must also send the saved login, so a gated
/// master expands into its real variants instead of stalling on
/// the unauthenticated master item.
#[tokio::test]
async fn verify2_hls_variant_scan_sends_saved_site_login() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let server = GatedPageServer::start();
    mgr.save_site_login(&login_input("127.0.0.1")).unwrap();

    let videos = mgr.detect_page_videos(&server.base).await.unwrap();
    let variants: Vec<_> = videos
        .iter()
        .filter(|v| v.url.contains("/v720.m3u8") || v.url.contains("/v480.m3u8"))
        .collect();
    assert_eq!(
        variants.len(),
        2,
        "gated master must expand into 2 variants, got: {:?}",
        videos
            .iter()
            .map(|v| (&v.url, v.downloadable, &v.error))
            .collect::<Vec<_>>()
    );
    for v in &variants {
        assert!(
            v.downloadable,
            "gated variant {} must be downloadable, got {:?}",
            v.url, v.error
        );
        assert!(
            v.size.is_some(),
            "gated variant {} must carry its real size",
            v.url
        );
    }
    // The 720p variant carries its resolution from the playlist
    // (as the honest dimension label the engine derives).
    let v720 = variants
        .iter()
        .find(|v| v.url.contains("/v720.m3u8"))
        .unwrap();
    assert_eq!(v720.quality, "1280 × 720");
}
