//! Round 55 — fix round for the run #29 live defects: total
//! timeout, stall + run watchdogs, checkpoint on first emit,
//! try_split guard, startup reconciliation, stale-status resume.
//! No public internet is used; the fixture server is the shared
//! mock. The progress-window and Add-dialog UI parts are React
//! (verified by `npm run build` + source); what is pinned here is
//! the engine behaviour the UI depends on.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use std::io::Read;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mock::*;

use fetchline_engine::proxy::{
    build_client_with_total_timeout, CLIENT_TOTAL_TIMEOUT,
};
use fetchline_engine::{DownloadManager, Store};

async fn set_max_retries(mgr: &DownloadManager, n: i64) {
    let mut s = mgr.get_settings().unwrap();
    s.max_retries = n;
    mgr.update_settings(&s).unwrap();
}

/// Round 55: the production client carries a total timeout — a
/// stalled server must fail the probe instead of blocking the Add
/// dialog for minutes (defect 3b). Round 56 (§7.5): 60 s → 30 s.
#[test]
fn total_timeout_is_30s_by_default() {
    assert_eq!(CLIENT_TOTAL_TIMEOUT, Duration::from_secs(30));
}

/// Round 55: a client built with a total timeout fails a server
/// that accepts the connection and then never answers — the
/// request errors as a timeout within seconds, instead of hanging
/// forever. (The production 30 s value is pinned by the const
/// test above; the 1 s value here keeps this test fast.)
#[tokio::test]
async fn total_timeout_fails_a_stalling_server() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // Accept, then never answer.
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            std::thread::sleep(Duration::from_secs(30));
        }
    });
    let client =
        build_client_with_total_timeout(None, 30, Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    let err = client
        .get(format!("http://127.0.0.1:{port}/file"))
        .send()
        .await
        .unwrap_err();
    assert!(err.is_timeout(), "expected a timeout error, got {err}");
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "timed out after {:?}",
        start.elapsed()
    );
}

/// Round 55 (defect 1): a server that sends headers and then no
/// body must fail the download with a clear "stalled" error —
/// instead of hanging forever at 0% with status `downloading`.
/// One attempt only, so the failure lands after a single stall
/// timeout (~30 s) instead of five retries.
#[tokio::test]
async fn stall_watchdog_fails_a_wedged_download() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    set_max_retries(&mgr, 1).await;
    server.set_stall_get(true);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "error", 60).await,
        "a stalled download must fail instead of hanging"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    let message = row.error_message.unwrap_or_default();
    assert!(
        message.contains("stalled"),
        "the error must name the stall, got: {message}"
    );
}

/// Round 55 (defect 1): the store learns progress on the first
/// tick, not after a full checkpoint interval — a download that
/// is still running at ~1 s already shows bytes in the store, so
/// the main window's 1 s poll can show live progress. (With the
/// old 2 s interval the row still read 0 at this point.)
#[tokio::test]
async fn checkpoint_reports_progress_within_the_first_second() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(50),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    // ~1.4 s in, the store must already show bytes: the first tick
    // checkpoints at once and the interval is 1 s. (With the old
    // 2 s interval the row still read 0 at this point.)
    tokio::time::sleep(Duration::from_millis(1400)).await;
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "downloading", "the download must still be running");
    assert!(
        row.downloaded_bytes > 0,
        "the store must already show bytes ~1.4 s in, got {}",
        row.downloaded_bytes
    );
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
}

/// Round 55: an open-ended segment (unknown size — the server sent
/// no Content-Length) is never split. Splitting it used to compute
/// a midpoint near 2^63 and only the schema CHECKs refused it;
/// on a fast server the bogus split could land as a second segment
/// row. The download still completes byte-exact, with exactly one
/// segment.
#[tokio::test]
async fn try_split_never_splits_an_open_ended_segment() {
    let server = MockServer::start(MockConfig {
        hide_length: true,
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    let segments = store.get_segments(&id).unwrap();
    assert_eq!(
        segments.len(),
        1,
        "an open-ended segment must never split, got {} segment rows",
        segments.len()
    );
}

/// Round 55 (defects 1 + 4): a row left in `downloading` by a
/// previous run is a zombie — startup reconciliation resets it to
/// `paused` (bytes kept) so it can be resumed. Other statuses are
/// untouched, and a second call finds nothing to do.
#[test]
fn startup_reconciliation_resets_zombie_rows() {
    let store = Store::open_in_memory().unwrap();
    let zombie = store
        .create_download(
            "http://127.0.0.1/file.bin",
            None,
            "file.bin",
            "/tmp/file.bin",
            "cat-video",
            Some(1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    let paused = store
        .create_download(
            "http://127.0.0.1/other.bin",
            None,
            "other.bin",
            "/tmp/other.bin",
            "cat-video",
            Some(1024),
            None,
            None,
            Some(true),
        )
        .unwrap();
    let done = store
        .create_download(
            "http://127.0.0.1/done.bin",
            None,
            "done.bin",
            "/tmp/done.bin",
            "cat-video",
            // No size: the schema CHECK forbids status `complete`
            // with size set but zero bytes downloaded.
            None,
            None,
            None,
            Some(true),
        )
        .unwrap();
    store.set_status(&zombie, "downloading", None).unwrap();
    store.set_status(&paused, "paused", None).unwrap();
    store.set_status(&done, "complete", None).unwrap();
    assert_eq!(store.reconcile_stuck_downloads().unwrap(), 1);
    let row = store.get_download(&zombie).unwrap().unwrap();
    assert_eq!(row.status, "paused");
    assert!(
        row.error_message.unwrap_or_default().contains("Interrupted"),
        "the reset row must say why it is paused"
    );
    assert_eq!(store.get_download(&paused).unwrap().unwrap().status, "paused");
    assert_eq!(store.get_download(&done).unwrap().unwrap().status, "complete");
    assert_eq!(store.reconcile_stuck_downloads().unwrap(), 0);
}

/// Round 55 (defect 4): Resume on a row that says `downloading`
/// while no run exists for it (a wedged run, or a crash before
/// reconciliation) restarts it instead of refusing with "already
/// running" — and the download then completes.
#[tokio::test]
async fn resume_recovers_a_stale_downloading_row() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    // Fake the zombie: the row says downloading, but no run exists.
    store.set_status(&id, "downloading", None).unwrap();
    mgr.resume_download(&id)
        .await
        .expect("resume must recover a stale 'downloading' row instead of refusing it");
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

/// Round 55 (defect 4, preserved behaviour): Resume on a download
/// that is genuinely running still refuses with "already running".
#[tokio::test]
async fn resume_still_refuses_a_running_download() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(50),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "downloading", "the download must still be running");
    let err = mgr.resume_download(&id).await.unwrap_err();
    assert!(
        err.to_string().contains("already running"),
        "expected 'already running', got: {err}"
    );
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
}
