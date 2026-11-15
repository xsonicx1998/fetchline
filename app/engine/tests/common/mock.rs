use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use fetchline_engine::DownloadManager;

/// Deterministic payload: byte i == (i % 251) as u8.
pub fn payload(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

pub struct MockConfig {
    /// Bytes per chunk the server sends before sleeping.
    pub chunk: usize,
    /// Sleep between chunks (0 = as fast as possible).
    pub delay: Duration,
    /// When true, the server answers HEAD without Content-Length.
    pub hide_length: bool,
    /// When true, the server answers HEAD with 405 (some servers
    /// refuse HEAD; the probe must fall back to a ranged GET).
    pub head_405: bool,
    /// When set, every request answers with this HTTP status and no body.
    pub fail_status: Option<u16>,
    /// File size in bytes (default 512 KiB; the version bump adds
    /// 1 KiB per bump on top of this).
    pub size_bytes: Option<usize>,
    /// When true, the server sends no ETag at all — the sync tests'
    /// Last-Modified-only and size-only fixtures.
    pub no_etag: bool,
    /// When true, the server sends a Last-Modified header (whose
    /// value `bump_last_modified` changes) — the sync tests' fixture
    /// for the Last-Modified comparison path, which the default
    /// mock never exercised.
    pub send_last_modified: bool,
}

impl Default for MockConfig {
    fn default() -> Self {
        Self {
            chunk: 65536,
            delay: Duration::ZERO,
            hide_length: false,
            head_405: false,
            fail_status: None,
            size_bytes: None,
            no_etag: false,
            send_last_modified: false,
        }
    }
}

pub struct MockServer {
    pub url: String,
    /// Flip to change the file the server serves (new size + new ETag).
    version: Arc<AtomicU64>,
    /// ETag-only version: bumped by `bump_etag` without changing size.
    etag_version: Arc<AtomicU64>,
    /// When set, GET requests fail with 500 (HEAD still works).
    fail_get: Arc<AtomicBool>,
    /// When set, GET responses are cut off mid-body (S03): the
    /// socket is closed without a response or part-way through the
    /// body, so the client sees a dropped connection, not an HTTP
    /// error. HEAD still works.
    drop_get: Arc<AtomicBool>,
    /// When set, GET requests ignore Range and answer 200 with the
    /// whole file (a server that dropped resume support).
    ignore_range: Arc<AtomicBool>,
    /// When set, every request answers 404 — the file was deleted
    /// from the server after it was downloaded (S05/F04 sync).
    gone: Arc<AtomicBool>,
    /// Last-Modified version: bumped by `bump_last_modified`
    /// without changing size or ETag (S05/F04 sync).
    lm_version: Arc<AtomicU64>,
    /// When non-zero, the served file size is exactly this, whatever
    /// the version says — set by `shrink_to` (S05/F04 sync).
    size_override: Arc<AtomicUsize>,
    /// How many GET / HEAD requests the server has served. A sync
    /// run that skips an unchanged file must make HEAD probes only
    /// and zero GETs — these counters prove it.
    get_requests: Arc<AtomicUsize>,
    head_requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl MockServer {
    pub fn start(config: MockConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let version = Arc::new(AtomicU64::new(1));
        let etag_version = Arc::new(AtomicU64::new(1));
        let fail_get = Arc::new(AtomicBool::new(false));
        let drop_get = Arc::new(AtomicBool::new(false));
        let ignore_range = Arc::new(AtomicBool::new(false));
        let gone = Arc::new(AtomicBool::new(false));
        let get_requests = Arc::new(AtomicUsize::new(0));
        let head_requests = Arc::new(AtomicUsize::new(0));
        let lm_version = Arc::new(AtomicU64::new(1));
        let size_override = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let config = Arc::new(config);
        let v = Arc::clone(&version);
        let ev = Arc::clone(&etag_version);
        let lm = Arc::clone(&lm_version);
        let so = Arc::clone(&size_override);
        let fg = Arc::clone(&fail_get);
        let dg0 = Arc::clone(&drop_get);
        let ir = Arc::clone(&ignore_range);
        let g = Arc::clone(&gone);
        let gr = Arc::clone(&get_requests);
        let hr = Arc::clone(&head_requests);
        let s = Arc::clone(&stop);
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let vv = Arc::clone(&v);
                        let ee = Arc::clone(&ev);
                        let ll = Arc::clone(&lm);
                        let oo = Arc::clone(&so);
                        let ff = Arc::clone(&fg);
                        let dg = Arc::clone(&dg0);
                        let ii = Arc::clone(&ir);
                        let gg = Arc::clone(&g);
                        let grr = Arc::clone(&gr);
                        let hrr = Arc::clone(&hr);
                        let cc = Arc::clone(&config);
                        thread::spawn(move || {
                            handle(
                                stream,
                                &cc,
                                vv.load(Ordering::SeqCst),
                                ee.load(Ordering::SeqCst),
                                ll.load(Ordering::SeqCst),
                                oo.load(Ordering::SeqCst),
                                ff.load(Ordering::SeqCst),
                                Arc::clone(&dg),
                                ii.load(Ordering::SeqCst),
                                gg.load(Ordering::SeqCst),
                                grr,
                                hrr,
                            )
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}/file.bin"),
            version,
            etag_version,
            fail_get,
            drop_get,
            ignore_range,
            gone,
            get_requests,
            head_requests,
            lm_version,
            size_override,
            stop,
        }
    }

    /// Change the served file: size grows by 1 KiB and the ETag changes.
    pub fn bump_version(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
        self.etag_version.fetch_add(1, Ordering::SeqCst);
    }

    /// Change only the ETag: same size, same bytes, new validator.
    pub fn bump_etag(&self) {
        self.etag_version.fetch_add(1, Ordering::SeqCst);
    }

    /// Change only the Last-Modified timestamp: same size, same
    /// bytes, no ETag change. The sync tests pair this with a
    /// `send_last_modified` + `no_etag` server so the
    /// Last-Modified comparison is the only signal that can fire.
    pub fn bump_last_modified(&self) {
        self.lm_version.fetch_add(1, Ordering::SeqCst);
    }

    /// Change only the size: +1 KiB, no ETag change. The sync tests
    /// pair this with a `no_etag` server so the size comparison is
    /// the only signal that can fire.
    pub fn bump_size(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    /// Shrink the served file to `size` bytes (and change the ETag,
    /// as a real server would when its file is replaced). The sync
    /// tests use this for the remote-file-shrank case.
    pub fn shrink_to(&self, size: usize) {
        self.size_override.store(size, Ordering::SeqCst);
        self.etag_version.fetch_add(1, Ordering::SeqCst);
    }

    /// Make GET requests fail with 500 from now on (HEAD still works).
    pub fn set_fail_get(&self, fail: bool) {
        self.fail_get.store(fail, Ordering::SeqCst);
    }

    /// Drop GET connections from now on (S03): a GET that arrives
    /// while this is set gets no response at all, and a GET already
    /// streaming its body is cut off at the next chunk boundary.
    /// Flip it back off and the next attempt succeeds — the retry
    /// path under test.
    pub fn set_drop_get(&self, drop: bool) {
        self.drop_get.store(drop, Ordering::SeqCst);
    }

    /// Make GET requests ignore Range and answer 200 with the whole file.
    pub fn set_ignore_range(&self, ignore: bool) {
        self.ignore_range.store(ignore, Ordering::SeqCst);
    }

    /// Make every request answer 404 — the file was deleted from
    /// the server after it was downloaded.
    pub fn set_gone(&self, gone: bool) {
        self.gone.store(gone, Ordering::SeqCst);
    }

    /// GET requests served so far (a sync skip must add zero).
    pub fn get_requests(&self) -> usize {
        self.get_requests.load(Ordering::SeqCst)
    }

    /// HEAD requests served so far (sync probes).
    pub fn head_requests(&self) -> usize {
        self.head_requests.load(Ordering::SeqCst)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub fn handle(
    mut stream: TcpStream,
    config: &MockConfig,
    version: u64,
    etag_version: u64,
    lm_version: u64,
    size_override: usize,
    fail_get: bool,
    drop_get: Arc<AtomicBool>,
    ignore_range: bool,
    gone: bool,
    get_requests: Arc<AtomicUsize>,
    head_requests: Arc<AtomicUsize>,
) {
    let mut buf = [0u8; 8192];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    // Forced failure mode (replica-test): every request answers with
    // this status and no body, e.g. 404 for the probe's HEAD.
    if let Some(code) = config.fail_status {
        let reason = match code {
            403 => "Forbidden",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "Error",
        };
        let head = format!(
            "HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    let mut lines = req.lines();
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    if method == "GET" {
        get_requests.fetch_add(1, Ordering::SeqCst);
    } else if method == "HEAD" {
        head_requests.fetch_add(1, Ordering::SeqCst);
    }
    // The file was deleted from the server: every request is a 404.
    if gone {
        let head = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    let size = if size_override != 0 {
        size_override
    } else {
        config.size_bytes.unwrap_or(512 * 1024) + (version - 1) as usize * 1024
    };
    let etag = format!("\"v{etag_version}\"");
    // The Last-Modified value is a plain distinct string per
    // version — sync compares it verbatim, so any two different
    // values prove the comparison works.
    let last_modified = format!("Wed, 01 Jan 2025 00:00:{lm_version:02} GMT");
    // HEAD refused: the probe must fall back to a ranged GET.
    if method == "HEAD" && config.head_405 {
        let head = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    // GET failures (probe HEAD still works, so adds succeed).
    if method == "GET" && fail_get {
        let head = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    // Dropped GET (S03): no response at all — the socket just goes
    // away, so the client sees a connection error, not a status.
    if method == "GET" && drop_get.load(Ordering::SeqCst) {
        return;
    }
    let mut status = "200 OK";
    // The handler serves one request per connection and then closes
    // the socket, so it must say so: without `Connection: close`
    // the client pools the connection, reuses it after the server
    // already closed it, and the next request fails with
    // "Connection reset by peer" (a flaky probe failure in tests
    // that add several downloads to the same mock server).
    let mut headers = vec![
        format!("Content-Length: {size}"),
        "Accept-Ranges: bytes".to_string(),
        "Content-Type: application/octet-stream".to_string(),
        "Connection: close".to_string(),
    ];
    if !config.no_etag {
        headers.push(format!("ETag: {etag}"));
    }
    if config.send_last_modified {
        headers.push(format!("Last-Modified: {last_modified}"));
    }
    let mut body: Vec<u8> = Vec::new();
    if method == "HEAD" {
        if config.hide_length {
            headers.retain(|h| !h.starts_with("Content-Length"));
        }
    } else {
        // Parse Range: bytes=a-b (header names are case-insensitive;
        // hyper writes them lowercase, so match without case).
        // When ignore_range is set the server answers 200 with the
        // whole file instead — a server that dropped resume support.
        let range = req.lines().find_map(|l| {
            let (name, value) = l.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("range") {
                Some(value.trim().to_string())
            } else {
                None
            }
        }).filter(|_| !ignore_range);
        if let Some(range) = range {
            if let Some(spec) = range.strip_prefix("bytes=") {
                let mut it = spec.split('-');
                let start: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let end: usize = it
                    .next()
                    .and_then(|s| if s.is_empty() { None } else { s.parse().ok() })
                    .unwrap_or(size - 1)
                    .min(size - 1);
                if start >= size {
                    status = "416 Range Not Satisfiable";
                    headers = vec![
                        format!("Content-Range: bytes */{size}"),
                        "Connection: close".to_string(),
                    ];
                } else {
                    status = "206 Partial Content";
                    headers = vec![
                        format!("Content-Length: {}", end - start + 1),
                        format!("Content-Range: bytes {start}-{end}/{size}"),
                        "Accept-Ranges: bytes".to_string(),
                        "Connection: close".to_string(),
                    ];
                    if !config.no_etag {
                        headers.push(format!("ETag: {etag}"));
                    }
                    if config.send_last_modified {
                        headers.push(format!("Last-Modified: {last_modified}"));
                    }
                    body = payload(size)[start..=end].to_vec();
                }
            }
        } else {
            body = payload(size);
        }
    }
    let head = format!("HTTP/1.1 {status}\r\n{}\r\n\r\n", headers.join("\r\n"));
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    if method == "HEAD" || body.is_empty() {
        return;
    }
    for chunk in body.chunks(config.chunk) {
        // Dropped mid-body (S03): stop writing part-way through, so
        // a download already in flight loses its connection.
        if drop_get.load(Ordering::SeqCst) {
            return;
        }
        if stream.write_all(chunk).is_err() {
            return;
        }
        if config.delay > Duration::ZERO {
            thread::sleep(config.delay);
        }
    }
}

/// A page server for Download All / batch tests: one HTML page full of
/// links plus the files it points at, each with a distinct size.
/// `page_url` is the page; file URLs are `{base}/video.mp4` etc.
pub struct PageServer {
    pub base: String,
    pub page_url: String,
    stop: Arc<AtomicBool>,
}

impl PageServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let page_url = format!("{base}/page.html");
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let page = page_url.clone();
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let page = page.clone();
                        thread::spawn(move || handle_page(stream, &page));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self { base, page_url, stop }
    }
}

impl Drop for PageServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Files the page links to: (path, content type, size in bytes).
fn page_files() -> Vec<(&'static str, &'static str, usize)> {
    vec![
        ("/video.mp4", "video/mp4", 100 * 1024),
        ("/song.mp3", "audio/mpeg", 50 * 1024),
        ("/pic.png", "image/png", 10 * 1024),
        ("/doc.pdf", "application/pdf", 20 * 1024),
        ("/file.zip", "application/zip", 200 * 1024),
        ("/setup.exe", "application/octet-stream", 300 * 1024),
        ("/notes.txt", "text/plain", 5 * 1024),
    ]
}

fn handle_page(mut stream: TcpStream, page_url: &str) {
    let mut buf = [0u8; 8192];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first = req.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let base = page_url.trim_end_matches("/page.html");
    if path == "/page.html" {
        // Absolute, root-relative and relative links; one duplicate;
        // one dead link; javascript/mailto/fragment links to skip.
        let html = format!(
            "<html><head><link href=\"style.css\"></head><body>\
             <a href=\"{base}/video.mp4\">video</a>\
             <a href=\"song.mp3\">song</a>\
             <a href=\"/pic.png\">pic</a>\
             <a href=\"doc.pdf\">doc</a>\
             <a href=\"{base}/file.zip\">zip</a>\
             <a href=\"{base}/setup.exe\">exe</a>\
             <a href=\"notes.txt\">notes</a>\
             <a href=\"{base}/video.mp4\">video again</a>\
             <a href=\"{base}/missing.mp4\">dead</a>\
             <a href=\"javascript:void(0)\">js</a>\
             <a href=\"mailto:someone@example.com\">mail</a>\
             <a href=\"#top\">anchor</a>\
             <img src=\"pic.png\">\
             <video><source src=\"video.mp4\"></video>\
             </body></html>"
        );
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            html.len()
        );
        let _ = stream.write_all(head.as_bytes());
        if method != "HEAD" {
            let _ = stream.write_all(html.as_bytes());
        }
        return;
    }
    for (file_path, ctype, size) in page_files() {
        if path == file_path {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {size}\r\nAccept-Ranges: bytes\r\nETag: \"page1\"\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
            if method != "HEAD" {
                let _ = stream.write_all(&payload(size));
            }
            return;
        }
    }
    if path == "/style.css" {
        let body = b"body { color: black; }";
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/css\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        if method != "HEAD" {
            let _ = stream.write_all(body);
        }
        return;
    }
    let head = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let _ = stream.write_all(head.as_bytes());
}

/// A minimal HTTP proxy for the proxy tests: it accepts plain HTTP
/// requests in absolute form (`GET http://host:port/path`), forwards
/// them to the target, and streams the response back. `hits` counts
/// forwarded requests; `saw_auth` records whether a client sent
/// `Proxy-Authorization`. CONNECT (HTTPS tunnelling) is refused —
/// the tests only proxy plain HTTP.
pub struct HttpProxyServer {
    pub url: String,
    pub port: u16,
    pub hits: Arc<AtomicUsize>,
    pub saw_auth: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl HttpProxyServer {
    pub fn start() -> Self {
        Self::start_with_auth(None)
    }

    /// Start a proxy that demands Basic auth: requests without the
    /// right `Proxy-Authorization` get 407 instead of being forwarded.
    pub fn start_with_auth(auth: Option<(&str, &str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let saw_auth = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let expected = Arc::new(auth.map(|(u, p)| basic_auth_value(u, p)));
        let h = Arc::clone(&hits);
        let a = Arc::clone(&saw_auth);
        let s = Arc::clone(&stop);
        let e = Arc::clone(&expected);
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let h = Arc::clone(&h);
                        let a = Arc::clone(&a);
                        let e = Arc::clone(&e);
                        thread::spawn(move || handle_http_proxy(stream, h, a, e));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self { url: format!("http://127.0.0.1:{port}"), port, hits, saw_auth, stop }
    }
}

/// The `Proxy-Authorization` header value reqwest sends for these
/// credentials (test helper; the mock is not the code under test).
pub fn basic_auth_value(user: &str, pass: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let data = format!("{user}:{pass}").into_bytes();
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    format!("Basic {out}")
}

impl Drop for HttpProxyServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn read_headers(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => return None,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    return Some(buf);
                }
                if buf.len() > 65536 {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
}

fn handle_http_proxy(
    mut stream: TcpStream,
    hits: Arc<AtomicUsize>,
    saw_auth: Arc<AtomicBool>,
    expected_auth: Arc<Option<String>>,
) {
    let Some(raw) = read_headers(&mut stream) else { return };
    let req = String::from_utf8_lossy(&raw).into_owned();
    let first = req.lines().next().unwrap_or("").to_string();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    if method.eq_ignore_ascii_case("CONNECT") {
        let _ = stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    if req.to_ascii_lowercase().contains("proxy-authorization:") {
        saw_auth.store(true, Ordering::SeqCst);
    }
    // When this proxy demands auth, a missing or wrong credential is
    // 407 — never forwarded, never counted as a hit.
    if let Some(expected) = expected_auth.as_ref() {
        let got = req
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("proxy-authorization:"))
            .map(|l| l.splitn(2, ':').nth(1).unwrap_or("").trim().to_string());
        if got.as_deref() != Some(expected.as_str()) {
            let _ = stream.write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"proxy\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        }
    }
    let rest = target.strip_prefix("http://").unwrap_or(&target);
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let mut upstream = match TcpStream::connect(authority) {
        Ok(s) => s,
        Err(_) => {
            let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        }
    };
    hits.fetch_add(1, Ordering::SeqCst);
    let mut out = format!("{method} {path} HTTP/1.1\r\n");
    for line in req.lines().skip(1) {
        if line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("proxy-authorization:") || lower.starts_with("proxy-connection:") {
            continue; // hop-by-hop: consumed by the proxy, not forwarded
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    if upstream.write_all(out.as_bytes()).is_err() {
        return;
    }
    // The origin server closes the connection after each response
    // (it sends `Connection: close`), so copying to EOF is the
    // whole response.
    let _ = std::io::copy(&mut upstream, &mut stream);
}

/// A minimal SOCKS5 relay for the proxy tests: greeting, optional
/// username/password auth, CONNECT (IPv4 + domain names), then a
/// plain byte relay in both directions. `hits` counts successful
/// CONNECTs; `saw_auth` records whether a client authenticated.
pub struct Socks5Server {
    /// "socks5://127.0.0.1:port" — the URL reqwest expects.
    pub url: String,
    pub port: u16,
    pub hits: Arc<AtomicUsize>,
    pub saw_auth: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Socks5Server {
    pub fn start() -> Self {
        Self::start_with_auth(None)
    }

    pub fn start_with_auth(auth: Option<(&str, &str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let saw_auth = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let h = Arc::clone(&hits);
        let a = Arc::clone(&saw_auth);
        let s = Arc::clone(&stop);
        let auth = auth.map(|(u, p)| (u.to_string(), p.to_string()));
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let h = Arc::clone(&h);
                        let a = Arc::clone(&a);
                        let auth = auth.clone();
                        thread::spawn(move || handle_socks5(stream, h, a, auth));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self { url: format!("socks5://127.0.0.1:{port}"), port, hits, saw_auth, stop }
    }
}

impl Drop for Socks5Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn handle_socks5(
    mut stream: TcpStream,
    hits: Arc<AtomicUsize>,
    saw_auth: Arc<AtomicBool>,
    auth: Option<(String, String)>,
) {
    // Greeting: VER, NMETHODS, METHODS…
    let mut head = [0u8; 2];
    if stream.read_exact(&mut head).is_err() || head[0] != 5 {
        return;
    }
    let mut methods = vec![0u8; head[1] as usize];
    if stream.read_exact(&mut methods).is_err() {
        return;
    }
    if let Some((user, pass)) = &auth {
        if !methods.contains(&0x02) {
            let _ = stream.write_all(&[5, 0xff]);
            return;
        }
        if stream.write_all(&[5, 0x02]).is_err() {
            return;
        }
        // Username/password sub-negotiation: VER, ULEN, user, PLEN, pass.
        let mut buf = [0u8; 2];
        if stream.read_exact(&mut buf).is_err() || buf[0] != 1 {
            return;
        }
        let mut u = vec![0u8; buf[1] as usize];
        if stream.read_exact(&mut u).is_err() {
            return;
        }
        let mut plen = [0u8; 1];
        if stream.read_exact(&mut plen).is_err() {
            return;
        }
        let mut p = vec![0u8; plen[0] as usize];
        if stream.read_exact(&mut p).is_err() {
            return;
        }
        if u != user.as_bytes() || p != pass.as_bytes() {
            let _ = stream.write_all(&[1, 1]);
            return;
        }
        saw_auth.store(true, Ordering::SeqCst);
        if stream.write_all(&[1, 0]).is_err() {
            return;
        }
    } else {
        if !methods.contains(&0x00) {
            let _ = stream.write_all(&[5, 0xff]);
            return;
        }
        if stream.write_all(&[5, 0x00]).is_err() {
            return;
        }
    }
    // Request: VER, CMD, RSV, ATYP, address, port.
    let mut hdr = [0u8; 4];
    if stream.read_exact(&mut hdr).is_err() || hdr[0] != 5 || hdr[1] != 1 {
        let _ = stream.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]);
        return;
    }
    let target: String = match hdr[3] {
        1 => {
            let mut addr = [0u8; 4];
            if stream.read_exact(&mut addr).is_err() {
                return;
            }
            let mut port = [0u8; 2];
            if stream.read_exact(&mut port).is_err() {
                return;
            }
            format!(
                "{}.{}.{}.{}:{}",
                addr[0],
                addr[1],
                addr[2],
                addr[3],
                u16::from_be_bytes(port)
            )
        }
        3 => {
            let mut len = [0u8; 1];
            if stream.read_exact(&mut len).is_err() {
                return;
            }
            let mut name = vec![0u8; len[0] as usize];
            if stream.read_exact(&mut name).is_err() {
                return;
            }
            let mut port = [0u8; 2];
            if stream.read_exact(&mut port).is_err() {
                return;
            }
            format!("{}:{}", String::from_utf8_lossy(&name), u16::from_be_bytes(port))
        }
        _ => {
            let _ = stream.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]);
            return;
        }
    };
    let mut upstream = match TcpStream::connect(&target) {
        Ok(s) => s,
        Err(_) => {
            let _ = stream.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]);
            return;
        }
    };
    hits.fetch_add(1, Ordering::SeqCst);
    if stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).is_err() {
        return;
    }
    // Relay both ways until either side closes.
    let mut up_read = match upstream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut down_write = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let t1 = thread::spawn(move || {
        let _ = std::io::copy(&mut up_read, &mut down_write);
    });
    let _ = std::io::copy(&mut stream, &mut upstream);
    let _ = t1.join();
}

/// A small site for the site-grabber (S06) tests: a few linked
/// pages, the files they point at, one off-site link, one dead
/// link, one login-only page, and a configurable robots.txt.
/// Nothing here touches a real website — every test serves its own
/// fixture on 127.0.0.1.
pub struct GrabberSite {
    pub base: String,
    pub page_url: String,
    pub login_url: String,
    stop: Arc<AtomicBool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RobotsMode {
    None,
    BlockAll,
}

impl GrabberSite {
    pub fn start(robots: RobotsMode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let page_url = format!("{base}/index.html");
        let login_url = format!("{base}/login.html");
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let b = base.clone();
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let b = b.clone();
                        thread::spawn(move || handle_grabber(stream, &b, robots));
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self { base, page_url, login_url, stop }
    }
}

impl Drop for GrabberSite {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Files the grabber site serves: (path, content type, size).
fn grabber_files() -> Vec<(&'static str, &'static str, usize)> {
    vec![
        ("/pic.png", "image/png", 10 * 1024),
        ("/doc.pdf", "application/pdf", 20 * 1024),
        ("/video.mp4", "video/mp4", 100 * 1024),
        ("/song.mp3", "audio/mpeg", 50 * 1024),
        ("/setup.exe", "application/octet-stream", 300 * 1024),
        ("/notes.txt", "text/plain", 5 * 1024),
        ("/archive.zip", "application/zip", 200 * 1024),
        ("/secret.pdf", "application/pdf", 15 * 1024),
    ]
}

fn grabber_page(path: &str) -> Option<String> {
    let html = match path {
        "/index.html" => "<html><body>\
             <a href=\"page2.html\">p2</a><a href=\"/page3.html\">p3</a>\
             <a href=\"pic.png\">pic</a><a href=\"doc.pdf\">doc</a>\
             <a href=\"video.mp4\">video</a>\
             <a href=\"http://localhost:9/evil.png\">off-site</a>\
             </body></html>",
        "/page2.html" => "<html><body>\
             <a href=\"deep.html\">deep</a>\
             <a href=\"song.mp3\">song</a><a href=\"pic.png\">pic again</a>\
             </body></html>",
        "/page3.html" => "<html><body>\
             <a href=\"setup.exe\">exe</a><a href=\"notes.txt\">notes</a>\
             <a href=\"dead.pdf\">dead</a>\
             </body></html>",
        "/deep.html" => "<html><body>\
             <a href=\"deeper.html\">deeper</a>\
             <a href=\"archive.zip\">archive</a>\
             </body></html>",
        "/deeper.html" => "<html><body><a href=\"secret.pdf\">secret</a></body></html>",
        // Milestone 4: a page whose only matched file is dead, so a
        // download run can finish with zero downloads (the
        // completion action must not arm — the BUG-003 rule).
        "/only-dead.html" => "<html><body><a href=\"dead.pdf\">dead</a></body></html>",
        _ => return None,
    };
    Some(html.to_string())
}

fn handle_grabber(mut stream: TcpStream, _base: &str, robots: RobotsMode) {
    let mut buf = [0u8; 8192];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first = req.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    if path == "/robots.txt" {
        match robots {
            RobotsMode::None => {
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
            RobotsMode::BlockAll => {
                let body = b"User-agent: *\nDisallow: /\n";
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                if method != "HEAD" {
                    let _ = stream.write_all(body);
                }
            }
        }
        return;
    }
    if path == "/login.html" {
        let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    if let Some(html) = grabber_page(path) {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            html.len()
        );
        let _ = stream.write_all(head.as_bytes());
        if method != "HEAD" {
            let _ = stream.write_all(html.as_bytes());
        }
        return;
    }
    for (file_path, ctype, size) in grabber_files() {
        if path == file_path {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {size}\r\nAccept-Ranges: bytes\r\nETag: \"grab1\"\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
            if method != "HEAD" {
                let _ = stream.write_all(&payload(size));
            }
            return;
        }
    }
    // /dead.pdf (and anything else): 404 for HEAD and GET alike, so
    // the probe fails and the grabber must give up after 2 attempts.
    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
}

pub fn tmpdir() -> std::path::PathBuf {
    // Unique per test AND per process: tests run in parallel in
    // one process, so a PID-only directory would make them share
    // one database and folder — and the counter alone is not
    // enough either. BUG-040: the counter restarts at 0 in every
    // test binary, and Windows reuses the PID of an exited
    // process, so a later binary can compute the exact same
    // `fetchline-test-{pid}-{n}` path as an earlier binary. When
    // the earlier binary's test left its directory behind — which
    // happens on Windows whenever `remove_dir_all` runs while a
    // spawned task still holds the database open (Windows refuses
    // to delete open files; Linux unlinks them happily) — the
    // later test silently opens the STALE app.db and sees the
    // earlier test's downloads. The nanosecond timestamp makes
    // the path unique across processes even when the PID is
    // reused (same precedent as `update_tests.rs`' own tmpdir);
    // the counter still disambiguates two calls in the same
    // nanosecond inside one process.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "fetchline-test-{}-{}-{}",
        std::process::id(),
        n,
        nanos
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub async fn wait_for_status(mgr: &DownloadManager, id: &str, want: &str, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        if let Ok(Some(row)) = mgr.get_download(id) {
            if row.status == want {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

