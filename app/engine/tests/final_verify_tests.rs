//! Final comprehensive verification (2026-11-15) — independent
//! adversarial spot-checks across the five finished milestones:
//! (a) periodic sync S05, (b) browser catch S13, (c) video grabber
//! S08, (d) site grabber S06 (incl. BUG-017/018/019 regressions),
//! (e) progress dialog S03. Every test serves its own fixture on
//! 127.0.0.1 — no real website, no real browser, is ever touched.
//! Kept tests: they stay in the suite whether they pass or fail.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use mock::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use fetchline_engine::timeutil::{now_unix, unix_to_iso};
use fetchline_engine::video::{
    detect_videos_in_html, parse_m3u8, validate_video_variants, VideoVariantInput, UNKNOWN_QUALITY,
};
use fetchline_engine::{DownloadManager, ProgressEvent, RetryInfo, Store};
use tokio::sync::broadcast;

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

fn iso_in(seconds_from_now: i64) -> String {
    unix_to_iso(now_unix() + seconds_from_now)
}

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("extension")
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()))
}

fn set_excluded(store: &Store, sites: &[&str]) {
    let mut s = store.get_settings().unwrap();
    s.excluded_sites = sites.iter().map(|x| x.to_string()).collect();
    store.update_settings(&s).unwrap();
}

fn all_types() -> Vec<String> {
    ["image", "video", "audio", "document", "archive", "program"]
        .iter()
        .map(|s| s.to_string())
        .collect()
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

async fn wait_for_retry_event(
    rx: &mut broadcast::Receiver<ProgressEvent>,
    secs: u64,
) -> Option<RetryInfo> {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return None;
        }
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Ok(ev)) => {
                if let Some(retry) = ev.retry {
                    return Some(retry);
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            _ => return None,
        }
    }
}

/// Minimal server for the video spot-checks: one HTML page linking
/// one playlist, plus the playlist bytes themselves. HEAD is
/// answered with headers only (the probe path).
struct MiniVideoServer {
    base: String,
    stop: Arc<AtomicBool>,
}

impl MiniVideoServer {
    fn start(playlist_name: &str, playlist_body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let b = base.clone();
        let name = playlist_name.to_string();
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let b = b.clone();
                        let name = name.clone();
                        let body = playlist_body.clone();
                        thread::spawn(move || {
                            let mut stream = stream;
                            let mut buf = [0u8; 8192];
                            let Ok(n) = stream.read(&mut buf) else { return };
                            let req = String::from_utf8_lossy(&buf[..n]).to_string();
                            let first = req.lines().next().unwrap_or("");
                            let mut parts = first.split_whitespace();
                            let method = parts.next().unwrap_or("");
                            let path = parts.next().unwrap_or("");
                            let (ctype, out): (&str, Vec<u8>) = if path == "/page.html" {
                                (
                                    "text/html",
                                    format!(
                                        "<html><body><video src=\"{b}/{name}\"></video></body></html>"
                                    )
                                    .into_bytes(),
                                )
                            } else if path == format!("/{name}") {
                                ("application/vnd.apple.mpegurl", body.into_bytes())
                            } else {
                                let _ = stream.write_all(
                                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                );
                                return;
                            };
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"mini1\"\r\nConnection: close\r\n\r\n",
                                out.len()
                            );
                            let _ = stream.write_all(head.as_bytes());
                            if method != "HEAD" {
                                let _ = stream.write_all(&out);
                            }
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self { base, stop }
    }
}

impl Drop for MiniVideoServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

// -- (a) periodic sync ------------------------------------------------------

async fn make_periodic(mgr: &Arc<DownloadManager>, name: &str, interval: i64) -> String {
    let queue = mgr.create_queue(name).unwrap();
    mgr.update_queue(
        &queue.id,
        "periodic",
        None,
        None,
        false,
        "none",
        None,
        Some(interval),
    )
    .await
    .unwrap();
    queue.id
}

async fn add_completed(
    mgr: &Arc<DownloadManager>,
    server_url: &str,
    dir: &std::path::Path,
    name: &str,
    queue_id: &str,
) -> String {
    let (id, _) = mgr
        .add_download(
            server_url,
            Some(name.to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            Some(queue_id.to_string()),
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(mgr, &id, "complete", 30).await);
    id
}

#[tokio::test]
async fn final_sync_changed_redownloaded_unchanged_skipped() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    let before_bytes = std::fs::read(mgr.get_download(&id).unwrap().unwrap().save_path).unwrap();

    // Unchanged: skipped, zero GETs.
    let gets_before = server.get_requests();
    let r = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!((r.unchanged, r.changed, r.removed, r.failed), (1, 0, 0, 0));
    assert_eq!(
        server.get_requests(),
        gets_before,
        "unchanged file must not be re-downloaded"
    );

    // Changed (new size + ETag): re-downloaded byte-identical.
    server.bump_version();
    let r = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(
        (r.changed, r.unchanged),
        (1, 0),
        "changed file must be re-downloaded"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    let after = std::fs::read(&row.save_path).unwrap();
    assert_eq!(
        after,
        payload(512 * 1024 + 1024),
        "re-downloaded bytes must match the new version"
    );
    assert_ne!(after, before_bytes);
    assert_eq!(row.status, "complete");
}

#[tokio::test]
async fn final_sync_remote_deleted_reported_and_local_kept() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    let save_path = mgr.get_download(&id).unwrap().unwrap().save_path;

    server.set_gone(true);
    let r = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(
        (r.removed, r.changed, r.failed),
        (1, 0, 0),
        "remote-deleted must be reported"
    );
    assert_eq!(r.files[0].status, "removed");
    assert!(
        std::path::Path::new(&save_path).exists(),
        "the local copy must be kept"
    );
}

#[tokio::test]
async fn final_sync_interval_below_5_minutes_rejected() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = mgr.create_queue("Sync").unwrap();
    for bad in [0, 1, 4] {
        let err = mgr
            .update_queue(
                &queue.id,
                "periodic",
                None,
                None,
                false,
                "none",
                None,
                Some(bad),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("5"), "interval {bad}: {err}");
    }
    // No interval at all is also rejected for a periodic queue.
    let err = mgr
        .update_queue(&queue.id, "periodic", None, None, false, "none", None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("5"), "{err}");
    // The 5-minute floor itself is accepted.
    mgr.update_queue(
        &queue.id,
        "periodic",
        None,
        None,
        false,
        "none",
        None,
        Some(5),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn final_sync_one_time_and_periodic_are_exclusive() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = mgr.create_queue("Q").unwrap();
    // A one-time queue cannot carry a sync interval.
    let err = mgr
        .update_queue(
            &queue.id,
            "one_time",
            Some(&iso_in(60)),
            None,
            false,
            "none",
            None,
            Some(30),
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("only applies to periodic"),
        "{err}"
    );
    // A periodic queue cannot be drained as a one-time queue…
    let pq = make_periodic(&mgr, "Sync", 30).await;
    assert!(
        mgr.start_queue(&pq).await.is_err(),
        "start_queue must refuse a periodic queue"
    );
    // …and a one-time queue cannot be synced.
    assert!(
        mgr.run_sync_queue(&queue.id).await.is_err(),
        "run_sync_queue must refuse a one-time queue"
    );
    let _ = server;
}

// -- (b) browser catch ------------------------------------------------------

#[tokio::test]
async fn final_drain_starts_download_not_just_queued() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    // Round 67: a 'download_all' row — a browser capture waits
    // for the Add URL dialog and is never drained.
    let row = store
        .enqueue_pending_link(&server.url, Some("http://example.com/page"), "download_all")
        .unwrap();
    let summary = mgr.drain_pending_links().await.unwrap();
    assert_eq!(summary.accepted.len(), 1);
    assert_eq!(summary.accepted[0].0, row.id);
    assert!(summary.rejected.is_empty());
    assert!(
        summary.start_failed.is_empty(),
        "a drained link must start, not sit queued"
    );
    let (_, download_id) = &summary.accepted[0];
    assert!(
        wait_for_status(&mgr, download_id, "complete", 30).await,
        "drained link must finish"
    );
    let dl = mgr.get_download(download_id).unwrap().unwrap();
    assert_eq!(std::fs::read(&dl.save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn final_drain_refuses_excluded_site() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    store
        .enqueue_pending_link(&server.url, None, "browser")
        .unwrap();
    // Excluded AFTER the link was queued: the drain must still refuse it.
    set_excluded(store, &["127.0.0.1"]);
    let summary = mgr.drain_pending_links().await.unwrap();
    assert!(summary.accepted.is_empty());
    assert_eq!(summary.rejected.len(), 1);
    assert!(
        summary.rejected[0].1.contains("excluded"),
        "reason: {}",
        summary.rejected[0].1
    );
    assert!(
        mgr.list_downloads().unwrap().is_empty(),
        "nothing must be downloaded from an excluded site"
    );
}

#[tokio::test]
async fn final_drain_duplicate_links_get_different_paths() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    // Round 67: 'download_all' rows — a browser capture waits
    // for the Add URL dialog and is never drained.
    store
        .enqueue_pending_link(&server.url, None, "download_all")
        .unwrap();
    store
        .enqueue_pending_link(&server.url, None, "download_all")
        .unwrap();
    let summary = mgr.drain_pending_links().await.unwrap();
    assert_eq!(
        summary.accepted.len(),
        2,
        "both copies are accepted (the guard renames, not rejects)"
    );
    let ids: Vec<&String> = summary.accepted.iter().map(|(_, d)| d).collect();
    let p0 = mgr.get_download(ids[0]).unwrap().unwrap().save_path;
    let p1 = mgr.get_download(ids[1]).unwrap().unwrap().save_path;
    assert_ne!(
        p0, p1,
        "the duplicate guard must give the second copy a different file"
    );
    assert!(wait_for_status(&mgr, ids[0], "complete", 30).await);
    assert!(wait_for_status(&mgr, ids[1], "complete", 30).await);
    assert_eq!(std::fs::read(&p0).unwrap(), payload(512 * 1024));
    assert_eq!(std::fs::read(&p1).unwrap(), payload(512 * 1024));
}

// -- (c) video grabber ------------------------------------------------------

#[tokio::test]
async fn final_video_encrypted_playlist_not_offered_as_downloadable() {
    // Parser level: the shared fixture is flagged encrypted.
    let enc = parse_m3u8(&fixture("encrypted.m3u8")).expect("encrypted fixture parses");
    assert!(
        enc.encrypted,
        "an AES-128 playlist must be flagged encrypted"
    );
    // Scan level: a page linking that playlist lists it as not downloadable.
    let server = MiniVideoServer::start("enc.m3u8", fixture("encrypted.m3u8"));
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let found = mgr
        .detect_page_videos(&format!("{}/page.html", server.base))
        .await
        .unwrap();
    assert_eq!(found.len(), 1, "found: {found:?}");
    assert!(
        !found[0].downloadable,
        "an encrypted playlist alone will not play"
    );
    assert!(
        found[0].error.as_deref().unwrap().contains("encrypted"),
        "error: {:?}",
        found[0].error
    );
}

#[tokio::test]
async fn final_video_m3u8_downloaded_as_is_bytes_exact() {
    let playlist = fixture("vod.m3u8");
    let server = MiniVideoServer::start("vod.m3u8", playlist.clone());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let found = mgr
        .detect_page_videos(&format!("{}/page.html", server.base))
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].downloadable);
    assert!(
        found[0].note.as_deref().unwrap().contains("not merged"),
        "note: {:?}",
        found[0].note
    );
    let url = found[0].url.clone();
    let (id, _) = mgr
        .add_download(
            &url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(dl.filename, "vod.m3u8");
    // Bytes exact: the playlist file, nothing merged, no segment fetched.
    assert_eq!(std::fs::read(&dl.save_path).unwrap(), playlist.into_bytes());
}

#[test]
fn final_video_quality_never_invented() {
    let settings = Store::open(&tmpdir().join("s.db"))
        .unwrap()
        .get_settings()
        .unwrap();
    // An invented marketing claim is replaced by the honest derivation.
    let (kept, skipped) = validate_video_variants(
        "http://127.0.0.1/watch",
        &[VideoVariantInput {
            url: "http://127.0.0.1/clip.mp4".to_string(),
            quality: Some("Ultra HD 8K Pro".to_string()),
            format: None,
            live: false,
            drm: false,
            note: None,
            ..Default::default()
        }],
        &settings,
    )
    .unwrap();
    assert!(skipped.is_empty());
    assert_eq!(kept[0].quality.as_deref(), Some(UNKNOWN_QUALITY));
    // A real clean label is kept.
    let (kept, _) = validate_video_variants(
        "http://127.0.0.1/watch",
        &[VideoVariantInput {
            url: "http://127.0.0.1/clip.mp4".to_string(),
            quality: Some("720p".to_string()),
            format: None,
            live: false,
            drm: false,
            note: None,
            ..Default::default()
        }],
        &settings,
    )
    .unwrap();
    assert_eq!(kept[0].quality.as_deref(), Some("720p"));
    // A plain video with no dimensions and no label is unknown, not guessed.
    let found = detect_videos_in_html(
        "<html><body><video src=\"http://127.0.0.1/clip.mp4\"></video></body></html>",
        "http://127.0.0.1/page.html",
    );
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].quality, UNKNOWN_QUALITY);
}

// -- (d) site grabber -------------------------------------------------------

#[tokio::test]
async fn final_grabber_status_running_immediately_after_run() {
    // BUG-019 regression: a read right after run_grabber must already
    // see a running state, never the previous terminal state.
    let site = GrabberSite::start(RobotsMode::None);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let p = mgr
        .create_grabber_project("p", &site.page_url, &all_types(), 2, 200, "download", None)
        .unwrap();
    mgr.run_grabber(&p.id).await.unwrap();
    let immediate = mgr.get_grabber_project(&p.id).unwrap().unwrap();
    assert!(
        matches!(immediate.status.as_str(), "exploring" | "downloading"),
        "status immediately after run must be running, got {}",
        immediate.status
    );
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    assert!(done.files_downloaded >= 1);
}

#[tokio::test]
async fn final_grabber_queue_choice_respected() {
    let site = GrabberSite::start(RobotsMode::None);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = mgr.create_queue("Grabbed").unwrap();
    let p = mgr
        .create_grabber_project_full(
            "p",
            &site.page_url,
            &all_types(),
            2,
            200,
            "download",
            None,
            Some(&queue.id),
            "none",
            None,
        )
        .unwrap();
    assert_eq!(p.queue_id, queue.id, "the chosen queue must be stored");
    mgr.run_grabber(&p.id).await.unwrap();
    let done = wait_grabber(&mgr, &p.id, 30).await;
    assert_eq!(done.status, "done");
    assert!(done.files_downloaded >= 1);
    let in_queue = mgr.list_queue_files(&queue.id).unwrap();
    assert!(
        !in_queue.is_empty(),
        "grabbed files must be filed into the chosen queue"
    );
    assert!(in_queue.iter().all(|d| d.status == "complete"));
}

#[tokio::test]
async fn final_grabber_completion_action_not_armed_without_downloads() {
    let site = GrabberSite::start(RobotsMode::None);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);

    // (i) Explore-only run with a shutdown action: nothing downloaded, no action.
    let q1 = mgr.create_queue("ExploreQ").unwrap();
    let p1 = mgr
        .create_grabber_project_full(
            "explore",
            &site.page_url,
            &all_types(),
            2,
            200,
            "explore",
            None,
            Some(&q1.id),
            "shutdown",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p1.id).await.unwrap();
    let done = wait_grabber(&mgr, &p1.id, 30).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_downloaded, 0);
    assert!(
        mgr.pending_action(&q1.id).await.is_none(),
        "explore-only must not arm an action"
    );

    // (ii) Download run where every matched file is dead: 0 downloads, no action.
    let q2 = mgr.create_queue("DeadQ").unwrap();
    let p2 = mgr
        .create_grabber_project_full(
            "dead",
            &format!("{}/only-dead.html", site.base),
            &all_types(),
            1,
            200,
            "download",
            None,
            Some(&q2.id),
            "shutdown",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p2.id).await.unwrap();
    let done = wait_grabber(&mgr, &p2.id, 30).await;
    assert_eq!(done.status, "done");
    assert_eq!(done.files_downloaded, 0, "the only matched file is dead");
    assert!(
        mgr.pending_action(&q2.id).await.is_none(),
        "0 downloads must not arm an action"
    );

    // (iii) Positive control: a run that DID download arms the action (then dismiss it).
    let q3 = mgr.create_queue("RealQ").unwrap();
    let p3 = mgr
        .create_grabber_project_full(
            "real",
            &site.page_url,
            &all_types(),
            2,
            200,
            "download",
            None,
            Some(&q3.id),
            "shutdown",
            None,
        )
        .unwrap();
    mgr.run_grabber(&p3.id).await.unwrap();
    let done = wait_grabber(&mgr, &p3.id, 30).await;
    assert_eq!(done.status, "done");
    assert!(done.files_downloaded >= 1);
    assert!(
        mgr.pending_action(&q3.id).await.is_some(),
        ">=1 download must arm the action"
    );
    mgr.dismiss_queue_action(&q3.id).await.unwrap();
}

// -- (e) progress dialog ----------------------------------------------------

#[tokio::test]
async fn final_retry_countdown_on_dropped_connection_then_retry_now_recovers() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    // NOTE: set max_retries through the manager (not the store
    // directly): the manager caches the retry count and only
    // `update_settings` / `apply_settings` refresh it — the same
    // call the Tauri shell makes once at launch.
    let mut s = mgr.get_settings().unwrap();
    s.max_retries = 3;
    mgr.update_settings(&s).unwrap();
    server.set_drop_get(true);
    let (id, _) = mgr
        .add_download(
            &server.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let mut rx = mgr.subscribe();
    mgr.start_download(&id).await.unwrap();
    let retry = wait_for_retry_event(&mut rx, 15)
        .await
        .expect("a dropped connection must surface a retry countdown");
    assert!(retry.attempt >= 1, "attempt: {}", retry.attempt);
    assert_eq!(retry.max_attempts, 3);
    assert!(
        (1..=16).contains(&retry.retry_in_secs),
        "countdown: {}",
        retry.retry_in_secs
    );
    assert!(!retry.error.is_empty());
    assert!(mgr.get_retry_state(&id).await.is_some());
    // Server recovers; Retry now skips the backoff and the file finishes.
    server.set_drop_get(false);
    mgr.retry_now(&id).await.expect("Retry now while waiting");
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(mgr.get_retry_state(&id).await.is_none());
}

#[tokio::test]
async fn final_cancel_during_retry_wait_works() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    server.set_fail_get(true);
    let (id, _) = mgr
        .add_download(
            &server.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let mut rx = mgr.subscribe();
    mgr.start_download(&id).await.unwrap();
    wait_for_retry_event(&mut rx, 15)
        .await
        .expect("a retry wait to cancel during");
    mgr.cancel_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "deleted", 5).await,
        "cancel during the wait must delete at once"
    );
    assert!(
        mgr.get_retry_state(&id).await.is_none(),
        "no retry wait may survive the cancel"
    );
}

#[tokio::test]
async fn final_open_folder_rejects_deleted_folder() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let sub = dir.join("downloads");
    std::fs::create_dir_all(&sub).unwrap();
    let mgr = test_mgr(&dir);
    let (id, _) = mgr
        .add_download(
            &server.url,
            None,
            Some(sub.to_string_lossy().to_string()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    // Folder present: validation resolves it.
    assert_eq!(mgr.folder_for_download(&id).unwrap(), sub);
    // Folder deleted after the download finished: a clear error, not a silent no-op.
    std::fs::remove_dir_all(&sub).unwrap();
    let err = mgr.folder_for_download(&id).unwrap_err();
    assert!(err.to_string().contains("no longer exists"), "{err}");
    assert!(mgr.open_folder(&id).is_err());
    // Unknown download: not found.
    assert!(mgr.folder_for_download("no-such-download").is_err());
}
