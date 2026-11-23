//! Independent verification tests for the site grabber (S06 / F06).
//! Written by the verifier, not the builder. Every test serves its own
//! counting fixture site on 127.0.0.1 (plus an off-site server on
//! localhost) — no real website is ever crawled. The fixture logs
//! every request with a timestamp so politeness, exclusion and the
//! 2-attempt rule can be measured, not just read from a constant.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use mock::*;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fetchline_engine::grabber::robots_disallows;
use fetchline_engine::{DownloadManager, Store};

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

fn set_excluded(store: &Store, sites: &[&str]) {
    let mut s = store.get_settings().unwrap();
    s.excluded_sites = sites.iter().map(|x| x.to_string()).collect();
    store.update_settings(&s).unwrap();
}

fn types(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

fn all_types() -> Vec<String> {
    types(&["image", "video", "audio", "document", "archive", "program"])
}

async fn wait_grabber(
    mgr: &DownloadManager,
    id: &str,
    secs: u64,
) -> fetchline_engine::GrabberProjectRow {
    let mut last: Option<fetchline_engine::GrabberProjectRow> = None;
    for _ in 0..secs * 10 {
        if let Ok(Some(p)) = mgr.get_grabber_project(id) {
            if matches!(p.status.as_str(), "done" | "failed" | "stopped") {
                return p;
            }
            last = Some(p);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("grabber {id} did not finish in {secs}s; last state: {last:?}")
}

// -- counting fixture -----------------------------------------------------

#[derive(Debug, Clone)]
struct Req {
    at: Instant,
    host: String, // "main" or "off"
    method: String,
    path: String,
}

struct CountSite {
    base: String,     // main site, 127.0.0.1
    off_base: String, // off-site, localhost (different host per same_host)
    page_url: String,
    log: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
}

impl CountSite {
    fn start(robots: Option<String>) -> Self {
        let log: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
        let main = TcpListener::bind("127.0.0.1:0").unwrap();
        let main_port = main.local_addr().unwrap().port();
        let off = TcpListener::bind("127.0.0.1:0").unwrap();
        let off_port = off.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{main_port}");
        let off_base = format!("http://localhost:{off_port}");
        let stop = Arc::new(AtomicBool::new(false));
        for (listener, host, is_off) in [
            (main, "main".to_string(), false),
            (off, "off".to_string(), true),
        ] {
            let s = Arc::clone(&stop);
            let l = Arc::clone(&log);
            let b = base.clone();
            let o = off_base.clone();
            let r = robots.clone();
            std::thread::spawn(move || {
                listener.set_nonblocking(true).unwrap();
                while !s.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                            let l = Arc::clone(&l);
                            let h = host.clone();
                            let b = b.clone();
                            let o = o.clone();
                            let r = r.clone();
                            std::thread::spawn(move || {
                                handle_count(stream, &h, is_off, &b, &o, r.as_deref(), &l)
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            });
        }
        Self {
            page_url: format!("{base}/index.html"),
            base,
            off_base,
            log,
            stop,
        }
    }

    fn requests(&self, host: &str, path: &str) -> Vec<Req> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.host == host && r.path == path)
            .cloned()
            .collect()
    }

    fn count(&self, host: &str, path: &str) -> usize {
        self.requests(host, path).len()
    }

    fn total(&self, host: &str) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.host == host)
            .count()
    }

    fn clear_log(&self) {
        self.log.lock().unwrap().clear();
    }
}

impl Drop for CountSite {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn page_html(base: &str, off_base: &str, path: &str) -> Option<String> {
    let html = match path {
        "/index.html" => format!(
            "<html><body>\
             <a href=\"page2.html\">p2</a><a href=\"/page3.html\">p3</a>\
             <a href=\"/private/page.html\">priv</a><a href=\"/private/open/page.html\">open</a>\
             <a href=\"pic.png\">pic</a><a href=\"doc.pdf\">doc</a>\
             <a href=\"{off_base}/evil.html\">off-site</a>\
             </body></html>"
        ),
        "/page2.html" => "<html><body>\
             <a href=\"deep.html\">deep</a><a href=\"song.mp3\">song</a>\
             </body></html>"
            .to_string(),
        "/page3.html" => "<html><body>\
             <a href=\"setup.exe\">exe</a><a href=\"notes.txt\">notes</a>\
             <a href=\"dead.pdf\">dead</a>\
             </body></html>"
            .to_string(),
        "/private/page.html" => {
            "<html><body><a href=\"/private/secret.pdf\">secret</a></body></html>".to_string()
        }
        "/private/open/page.html" => {
            "<html><body><a href=\"/private/open/open.pdf\">open</a></body></html>".to_string()
        }
        "/deep.html" => "<html><body>\
             <a href=\"deeper.html\">deeper</a><a href=\"archive.zip\">archive</a>\
             </body></html>"
            .to_string(),
        "/deeper.html" => "<html><body><a href=\"secret.pdf\">secret</a></body></html>".to_string(),
        "/hidden.html" => "<html><body><a href=\"hidden.pdf\">hidden</a></body></html>".to_string(),
        "/evil.html" => format!(
            "<html><body>\
             <a href=\"{base}/hidden.html\">back to main site</a>\
             <a href=\"{base}/page2.html\">back to page2</a>\
             <a href=\"evil.png\">evil file</a>\
             </body></html>"
        ),
        _ => return None,
    };
    Some(html)
}

fn count_files() -> Vec<(&'static str, &'static str, usize)> {
    vec![
        ("/pic.png", "image/png", 10 * 1024),
        ("/doc.pdf", "application/pdf", 20 * 1024),
        ("/song.mp3", "audio/mpeg", 50 * 1024),
        ("/setup.exe", "application/octet-stream", 300 * 1024),
        ("/notes.txt", "text/plain", 5 * 1024),
        ("/archive.zip", "application/zip", 200 * 1024),
        ("/private/secret.pdf", "application/pdf", 15 * 1024),
        ("/private/open/open.pdf", "application/pdf", 15 * 1024),
        ("/secret.pdf", "application/pdf", 15 * 1024),
        ("/hidden.pdf", "application/pdf", 15 * 1024),
        ("/evil.png", "image/png", 10 * 1024),
    ]
}

fn handle_count(
    mut stream: std::net::TcpStream,
    host: &str,
    is_off: bool,
    base: &str,
    off_base: &str,
    robots: Option<&str>,
    log: &Arc<Mutex<Vec<Req>>>,
) {
    let mut buf = [0u8; 8192];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first = req.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    log.lock().unwrap().push(Req {
        at: Instant::now(),
        host: host.to_string(),
        method: method.to_string(),
        path: path.to_string(),
    });
    if path == "/robots.txt" {
        match robots {
            Some(body) => {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                if method != "HEAD" {
                    let _ = stream.write_all(body.as_bytes());
                }
            }
            None => {
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        }
        return;
    }
    if path == "/login.html" {
        let _ = stream.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }
    if let Some(html) = page_html(base, off_base, path) {
        // The off-site server only serves /evil.html; the main server
        // only serves main pages. A cross-served page is a test bug.
        let is_main_page = !matches!(path, "/evil.html");
        if is_main_page == is_off {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        }
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
    for (file_path, ctype, size) in count_files() {
        if path == file_path {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {size}\r\nAccept-Ranges: bytes\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
            if method != "HEAD" {
                let _ = stream.write_all(&vec![0u8; size]);
            }
            return;
        }
    }
    let _ = stream
        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
}

// -- (a) off-site link-back ------------------------------------------------

#[tokio::test]
async fn verify_off_site_page_linking_back_is_never_followed() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = CountSite::start(None);
    let p = mgr
        .create_grabber_project(
            "verify-a",
            &site.page_url,
            &all_types(),
            5,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    // All 7 same-host pages, none via the off-site detour.
    assert_eq!(done.pages_explored, 7, "pages: {done:?}");
    assert_eq!(
        site.total("off"),
        0,
        "the off-site server must never be hit"
    );
    let urls: Vec<&str> = done.matched_files.iter().map(|f| f.url.as_str()).collect();
    assert!(
        !urls.iter().any(|u| u.contains("hidden.pdf")),
        "hidden.pdf is only reachable via the off-site page"
    );
    assert!(!urls.iter().any(|u| u.contains("evil.png")));
    assert_eq!(done.files_matched, 10);
    // (f) explore-only: zero downloads.
    assert!(
        mgr.list_downloads().unwrap().is_empty(),
        "explore-only must create no downloads"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// -- (b) depth boundaries ---------------------------------------------------

#[tokio::test]
async fn verify_depth_5_accepted_depth_6_rejected() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = CountSite::start(None);
    let err = mgr
        .create_grabber_project(
            "verify-b6",
            &site.page_url,
            &all_types(),
            6,
            200,
            "explore",
            None,
        )
        .unwrap_err();
    assert!(err.to_string().contains("too deep"), "{err}");
    assert!(mgr
        .create_grabber_project(
            "verify-b-1",
            &site.page_url,
            &all_types(),
            -1,
            200,
            "explore",
            None
        )
        .is_err());
    let p = mgr
        .create_grabber_project(
            "verify-b5",
            &site.page_url,
            &all_types(),
            5,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.pages_explored, 7);
    let _ = std::fs::remove_dir_all(&dir);
}

// -- (c) robots: Allow under Disallow ----------------------------------------

#[tokio::test]
async fn verify_robots_allow_under_disallow_same_group() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let robots = "User-agent: *\nDisallow: /private/\nAllow: /private/open/\n".to_string();
    let site = CountSite::start(Some(robots));
    let p = mgr
        .create_grabber_project(
            "verify-c1",
            &site.page_url,
            &all_types(),
            2,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    // index, page2, page3, /private/open/page.html, deep = 5;
    // /private/page.html is skipped, not counted.
    assert_eq!(done.pages_explored, 5, "pages: {done:?}");
    let urls: Vec<&str> = done.matched_files.iter().map(|f| f.url.as_str()).collect();
    assert!(
        urls.iter().any(|u| u.ends_with("/private/open/open.pdf")),
        "Allow for the sub-path must be honoured"
    );
    assert!(
        !urls.iter().any(|u| u.ends_with("/private/secret.pdf")),
        "the Disallow subtree must stay blocked"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn verify_robots_fetchline_allow_beats_star_disallow_both_orders() {
    // Cross-group precedence: an Allow in the Fetchline group must
    // beat a Disallow in the `*` group, whichever group comes first.
    let star_first =
        "User-agent: *\nDisallow: /private/\n\nUser-agent: Fetchline\nAllow: /private/open/\n";
    let fetchline_first =
        "User-agent: Fetchline\nAllow: /private/open/\n\nUser-agent: *\nDisallow: /private/\n";
    for (label, txt) in [
        ("star-first", star_first),
        ("fetchline-first", fetchline_first),
    ] {
        assert!(
            !robots_disallows(txt, "/private/open/page.html"),
            "{label}: Allow in the Fetchline group must beat the * Disallow"
        );
        assert!(
            robots_disallows(txt, "/private/page.html"),
            "{label}: the rest of /private/ must stay blocked"
        );
    }
}

#[tokio::test]
async fn verify_robots_allow_under_disallow_cross_group_crawl() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let robots =
        "User-agent: *\nDisallow: /private/\n\nUser-agent: Fetchline\nAllow: /private/open/\n"
            .to_string();
    let site = CountSite::start(Some(robots));
    let p = mgr
        .create_grabber_project(
            "verify-c2",
            &site.page_url,
            &all_types(),
            2,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.pages_explored, 5, "pages: {done:?}");
    let urls: Vec<&str> = done.matched_files.iter().map(|f| f.url.as_str()).collect();
    assert!(
        urls.iter().any(|u| u.ends_with("/private/open/open.pdf")),
        "cross-group Allow must be honoured in the crawl"
    );
    assert!(!urls.iter().any(|u| u.ends_with("/private/secret.pdf")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn verify_robots_start_url_under_disallow_fails_but_allowed_subpath_runs() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let robots = "User-agent: *\nDisallow: /private/\nAllow: /private/open/\n".to_string();
    let site = CountSite::start(Some(robots));
    let blocked = format!("{}/private/page.html", site.base);
    let p = mgr
        .create_grabber_project("verify-c3", &blocked, &all_types(), 2, 200, "explore", None)
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "failed");
    assert!(done
        .error_message
        .unwrap_or_default()
        .contains("robots.txt"));
    assert_eq!(done.pages_explored, 0);
    let allowed = format!("{}/private/open/page.html", site.base);
    let p2 = mgr
        .create_grabber_project("verify-c4", &allowed, &all_types(), 0, 200, "explore", None)
        .unwrap();
    mgr.run_grabber(&p2.id).await.unwrap();
    let done2 = wait_grabber(&mgr, &p2.id, 30).await;
    assert_eq!(done2.status, "done");
    assert_eq!(done2.pages_explored, 1);
    assert!(done2
        .matched_files
        .iter()
        .any(|f| f.url.ends_with("/private/open/open.pdf")));
    let _ = std::fs::remove_dir_all(&dir);
}

// -- (d) page cap -------------------------------------------------------------

#[tokio::test]
async fn verify_page_cap_stops_cleanly_without_extra_fetches() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = CountSite::start(None);
    let p = mgr
        .create_grabber_project(
            "verify-d",
            &site.page_url,
            &all_types(),
            5,
            3,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(
        done.status, "done",
        "the cap is a clean stop, not a failure"
    );
    assert_eq!(done.pages_explored, 3);
    assert_eq!(done.files_matched, 6); // pic, doc (index) + song (page2) + exe, notes, dead (page3)
                                       // Exactly 3 page fetches + 1 robots.txt fetch, nothing more.
    let page_gets = site
        .log
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.host == "main" && r.path.ends_with(".html"))
        .count();
    assert_eq!(page_gets, 3, "no page beyond the cap may be fetched");
    assert_eq!(
        site.count("main", "/robots.txt"),
        1,
        "robots.txt is fetched once per run"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// -- (e) exactly 2 attempts on a dead file -----------------------------------

#[tokio::test]
async fn verify_dead_file_is_attempted_exactly_twice() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = CountSite::start(None);
    let dead = format!("{}/dead.pdf", site.base);
    // Baseline: one direct add_download of the dead URL = 1 probe HEAD.
    site.clear_log();
    assert!(mgr
        .add_download(&dead, None, None, None, None, None)
        .await
        .is_err());
    let baseline = site.count("main", "/dead.pdf");
    assert_eq!(
        baseline, 1,
        "one direct add must probe exactly once, saw {baseline}"
    );
    // Grabber run: the same dead file must be attempted exactly twice.
    site.clear_log();
    let p = mgr
        .create_grabber_project(
            "verify-e",
            &site.page_url,
            &types(&["document"]),
            1,
            200,
            "download",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done.status, "done", "one dead file must not fail the run");
    assert_eq!(done.files_failed, 1);
    assert_eq!(done.files_downloaded, 4); // doc, notes, /private/secret, /private/open/open — depth-1 pages' files count too
    let attempts = site.count("main", "/dead.pdf");
    assert_eq!(
        attempts,
        2 * baseline,
        "dead file must be attempted exactly twice, saw {attempts} requests"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// -- (g) duplicate guard on re-run --------------------------------------------

#[tokio::test]
async fn verify_rerun_gives_grabbed_files_distinct_save_paths() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = CountSite::start(None);
    let p = mgr
        .create_grabber_project(
            "verify-g",
            &site.page_url,
            &types(&["image", "document"]),
            1,
            200,
            "download",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done1 = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done1.status, "done");
    // pic, doc, notes, /private/secret.pdf, /private/open/open.pdf (dead.pdf fails)
    assert_eq!(done1.files_matched, 6);
    assert_eq!(done1.files_downloaded, 5);
    mgr.run_grabber(&p.id).await.unwrap();
    // NOTE: run_grabber returns before the background task persists
    // "exploring", so the row still says "done" from run 1 at first —
    // wait until the new run is visibly running before waiting for
    // its terminal state (see BUG-019).
    for _ in 0..50 {
        if let Ok(Some(cur)) = mgr.get_grabber_project(&p.id) {
            if matches!(cur.status.as_str(), "exploring" | "downloading") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let done2 = wait_grabber(&mgr, &p.id, 60).await;
    assert_eq!(done2.status, "done");
    assert_eq!(done2.files_downloaded, 5);
    let downloads = mgr.list_downloads().unwrap();
    assert_eq!(downloads.len(), 10, "the re-run must add 5 fresh downloads");
    let paths: HashSet<&str> = downloads.iter().map(|d| d.save_path.as_str()).collect();
    assert_eq!(
        paths.len(),
        10,
        "BUG-002: every grabbed file needs a distinct save path"
    );
    assert!(downloads.iter().all(|d| d.status == "complete"));
    let db = rusqlite::Connection::open(dir.join("app.db")).unwrap();
    let tagged: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM downloads WHERE grabber_project_id = ?1",
            [p.id.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        tagged, 10,
        "every grabbed download must carry the project id"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// -- (h) excluded site: no request at all --------------------------------------

#[tokio::test]
async fn verify_excluded_start_site_makes_no_request() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = CountSite::start(None);
    set_excluded(mgr.store(), &["127.0.0.1"]);
    let err = mgr
        .create_grabber_project(
            "verify-h",
            &site.page_url,
            &types(&["image"]),
            2,
            200,
            "download",
            None,
        )
        .unwrap_err();
    assert!(err.to_string().contains("excluded"), "{err}");
    assert_eq!(
        site.total("main"),
        0,
        "an excluded start site must be refused before any network request"
    );
    assert_eq!(site.total("off"), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn verify_excluded_after_create_stops_the_run_before_any_fetch() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = CountSite::start(None);
    let p = mgr
        .create_grabber_project(
            "verify-h2",
            &site.page_url,
            &types(&["image"]),
            0,
            200,
            "explore",
            None,
        )
        .unwrap();
    // The site is excluded after the project exists, before the run.
    set_excluded(mgr.store(), &["127.0.0.1"]);
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    let seen: Vec<String> = site
        .log
        .lock()
        .unwrap()
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    eprintln!(
        "verify-h2: requests made after exclusion: {seen:?}; status={}",
        done.status
    );
    assert_eq!(
        site.total("main"),
        0,
        "a run against a now-excluded site must fetch nothing"
    );
    assert_eq!(
        done.files_matched, 0,
        "no files may be listed from an excluded site"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// -- (i) polite delay, measured -------------------------------------------------

#[tokio::test]
async fn verify_polite_delay_between_page_fetches_is_measured() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = CountSite::start(None);
    let p = mgr
        .create_grabber_project(
            "verify-i",
            &site.page_url,
            &all_types(),
            2,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.pages_explored, 6); // index, page2, page3, priv x2, deep
    let mut times: Vec<Instant> = site
        .log
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.host == "main" && r.method == "GET" && r.path.ends_with(".html"))
        .map(|r| r.at)
        .collect();
    times.sort();
    assert_eq!(times.len(), 6, "six page fetches expected");
    let mut min_gap = Duration::from_secs(999);
    for w in times.windows(2) {
        min_gap = min_gap.min(w[1].duration_since(w[0]));
    }
    assert!(
        min_gap >= Duration::from_millis(495),
        "consecutive page fetches must be >= 500 ms apart; smallest gap was {min_gap:?}"
    );
    eprintln!("verify-i: smallest gap between page fetches: {min_gap:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// -- run_grabber returns before the status leaves its old state ---------------

#[tokio::test]
async fn verify_rerun_status_is_stale_immediately_after_run() {
    // BUG-019: run_grabber spawns the crawl task and returns, but the
    // task is what first persists "exploring" — so a poller that
    // reads the project right after run_grabber can still see the
    // previous terminal state ("done") and conclude the run is over.
    // The dialog only polls while the status is exploring/downloading,
    // so a stale read can leave it showing the old summary (re-run)
    // or stuck on "idle" (fresh run) while the crawl runs unseen.
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = CountSite::start(None);
    let p = mgr
        .create_grabber_project(
            "verify-race",
            &site.page_url,
            &types(&["image"]),
            0,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    mgr.run_grabber(&p.id).await.unwrap();
    let stale = mgr.get_grabber_project(&p.id).unwrap().unwrap();
    eprintln!(
        "verify-race: status immediately after re-run: {}",
        stale.status
    );
    assert!(
        matches!(stale.status.as_str(), "exploring" | "downloading"),
        "right after run_grabber the project must already read as running, but it still says {:?}",
        stale.status
    );
    let _ = wait_grabber(&mgr, &p.id, 30).await;
    let _ = std::fs::remove_dir_all(&dir);
}

// -- (j) stop is prompt -----------------------------------------------------------

#[tokio::test]
async fn verify_stop_is_prompt() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let site = CountSite::start(None);
    let p = mgr
        .create_grabber_project(
            "verify-j",
            &site.page_url,
            &all_types(),
            5,
            200,
            "explore",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let t0 = Instant::now();
    mgr.stop_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    let elapsed = t0.elapsed();
    assert_eq!(done.status, "stopped");
    assert!(done.pages_explored < 7, "the run stopped before finishing");
    assert!(
        elapsed < Duration::from_secs(5),
        "stop took {elapsed:?}, expected a prompt stop (< 5 s)"
    );
    eprintln!(
        "verify-j: stop took {elapsed:?}, pages_explored={}",
        done.pages_explored
    );
    let _ = std::fs::remove_dir_all(&dir);
}
