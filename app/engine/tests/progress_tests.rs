//! Progress dialog engine tests (S03, Milestone 5): the retry
//! countdown the dialog shows, "Retry now", cancel-during-retry,
//! and the "Open folder" validation. The dialog itself is React
//! (verified by `npm run build` + source); what is pinned here is
//! the engine state the dialog is driven by — the retry wait on
//! `ProgressEvent`, `retry_now`, and `folder_for_download` —
//! plus the fixture server that drops a connection mid-download.
//! No public internet is used; the Tauri `open_folder` /
//! `retry_now` commands are uncompiled on this Linux VM (gdk-3.0),
//! as with every milestone.

#![allow(dead_code)] // the shared mock also serves other test files

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mock::*;

use fetchline_engine::{DownloadManager, ProgressEvent, RetryInfo, Store};
use tokio::sync::broadcast;

/// Drain progress events until one carries a retry wait, or the
/// timeout runs out. Broadcast lag (the channel holds 64 events)
/// is skipped, not treated as a failure.
async fn wait_for_retry_event(
    rx: &mut broadcast::Receiver<ProgressEvent>,
    secs: u64,
) -> Option<RetryInfo> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
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

async fn set_max_retries(mgr: &DownloadManager, n: i64) {
    let mut s = mgr.get_settings().unwrap();
    s.max_retries = n;
    mgr.update_settings(&s).unwrap();
}

/// S03-R1: a server that answers GETs with 500 puts the download
/// into a retry wait — the event carries attempt 1 of 3 with a
/// ~2 s countdown (the existing `1 << attempt` backoff) — and
/// when the server recovers, the same download finishes byte-exact
/// instead of failing. The wait is visible; it is not a silent stall.
#[tokio::test]
async fn retry_countdown_appears_on_500_and_recovers() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    set_max_retries(&mgr, 3).await;
    server.set_fail_get(true);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let mut rx = mgr.subscribe();
    mgr.start_download(&id).await.unwrap();
    let retry = wait_for_retry_event(&mut rx, 10)
        .await
        .expect("a progress event with a retry wait");
    assert_eq!(retry.attempt, 1, "first failure is attempt 1");
    assert_eq!(retry.max_attempts, 3);
    assert!(
        (1..=2).contains(&retry.retry_in_secs),
        "first backoff is 2 s, got {} s",
        retry.retry_in_secs
    );
    assert!(retry.error.contains("500"), "error: {}", retry.error);
    // The getter agrees with the event — the dialog could poll it.
    let polled = mgr.get_retry_state(&id).await.expect("waiting");
    assert_eq!(polled.attempt, 1);
    // The server recovers: the waiting segment retries on its own
    // and the download completes byte-exact.
    server.set_fail_get(false);
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(mgr.get_retry_state(&id).await.is_none());
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

/// S03-R2: a connection dropped mid-download (the fixture closes
/// the socket part-way through the body) also lands in a retry
/// wait — after bytes were already downloaded — and recovers the
/// same way once the connection holds.
#[tokio::test]
async fn retry_countdown_appears_when_connection_drops_mid_download() {
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
    let mut rx = mgr.subscribe();
    mgr.start_download(&id).await.unwrap();
    // Let it get part-way, then drop the connection mid-body.
    tokio::time::sleep(Duration::from_millis(500)).await;
    server.set_drop_get(true);
    let mut downloaded_at_retry = 0u64;
    let retry = {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut found = None;
        while Instant::now() < deadline {
            match tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                rx.recv(),
            )
            .await
            {
                Ok(Ok(ev)) => {
                    if let Some(r) = ev.retry {
                        downloaded_at_retry = ev.downloaded_bytes;
                        found = Some(r);
                        break;
                    }
                }
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                _ => break,
            }
        }
        found.expect("a progress event with a retry wait after the drop")
    };
    assert!(downloaded_at_retry > 0, "the drop landed mid-download");
    assert!(retry.attempt >= 1);
    assert!(retry.max_attempts >= 1);
    assert!(retry.retry_in_secs >= 1 && retry.retry_in_secs <= 16);
    assert!(!retry.error.is_empty(), "the wait names the failure");
    server.set_drop_get(false);
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

/// S03-R3: "Retry now" cuts the backoff short. With 3 attempts the
/// second failure would normally arrive 2 s after the first (the
/// first backoff); hurrying it lands the second wait well inside
/// 1.5 s. The download still ends in `error` once the attempts
/// are exhausted — Retry now hurries, it does not add attempts.
#[tokio::test]
async fn retry_now_skips_the_backoff() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    set_max_retries(&mgr, 3).await;
    server.set_fail_get(true);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let mut rx = mgr.subscribe();
    mgr.start_download(&id).await.unwrap();
    let first = wait_for_retry_event(&mut rx, 10)
        .await
        .expect("first retry wait");
    assert_eq!(first.attempt, 1);
    let hurried_at = Instant::now();
    mgr.retry_now(&id).await.expect("retry now while waiting");
    let second = wait_for_retry_event(&mut rx, 10)
        .await
        .expect("second retry wait");
    assert_eq!(second.attempt, 2);
    assert!(
        hurried_at.elapsed() < Duration::from_millis(1500),
        "second wait arrived {:?} after Retry now — the backoff was not skipped",
        hurried_at.elapsed()
    );
    assert!(wait_for_status(&mgr, &id, "error", 15).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert!(row.error_message.as_deref().unwrap_or("").contains("500"));
}

/// S03-R4: "Retry now" has nothing to hurry when no segment is
/// waiting — a healthy running download, a queued download, and an
/// unknown id all refuse with a clear error instead of silently
/// doing nothing (or hurrying a download that is not waiting).
#[tokio::test]
async fn retry_now_errors_when_nothing_is_waiting() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(100),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    // Queued, never started: not running.
    let err = mgr.retry_now(&id).await.unwrap_err();
    assert!(err.to_string().contains("not running"), "error: {err}");
    // Running, but healthy: not waiting.
    mgr.start_download(&id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let err = mgr.retry_now(&id).await.unwrap_err();
    assert!(err.to_string().contains("not waiting"), "error: {err}");
    // Unknown id: not running either.
    assert!(mgr.retry_now("no-such-download").await.is_err());
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
}

/// S03-R5: cancelling while a download waits for a retry deletes
/// it at once — the user is not stuck waiting out a backoff to
/// cancel, and no retry wait survives the cancel.
#[tokio::test]
async fn cancel_during_retry_deletes_the_download() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    server.set_fail_get(true);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let mut rx = mgr.subscribe();
    mgr.start_download(&id).await.unwrap();
    wait_for_retry_event(&mut rx, 10)
        .await
        .expect("a retry wait to cancel during");
    mgr.cancel_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "deleted", 5).await);
    assert!(mgr.get_retry_state(&id).await.is_none());
}

/// S03-R6: a healthy download never carries a retry wait — the
/// slot is only ever set by a failed segment, so the dialog never
/// shows a countdown for a download that is simply downloading.
#[tokio::test]
async fn no_retry_state_while_downloading_normally() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(100),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let mut rx = mgr.subscribe();
    mgr.start_download(&id).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_event = false;
    while Instant::now() < deadline {
        match tokio::time::timeout(
            deadline.saturating_duration_since(Instant::now()),
            rx.recv(),
        )
        .await
        {
            Ok(Ok(ev)) => {
                saw_event = true;
                assert!(ev.retry.is_none(), "a healthy event carries no retry");
            }
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            _ => break,
        }
    }
    assert!(saw_event, "progress events arrived during the download");
    assert!(mgr.get_retry_state(&id).await.is_none());
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
}

/// S03-O1: "Open folder" validation — the folder of a finished
/// download resolves, a folder deleted after the download finished
/// is a clear "no longer exists" error (not a silent no-op), and an
/// unknown download is not found. The actual shell-open spawn is
/// NOT executed here: it needs the desktop app, and on this Linux
/// VM there is no file manager to open.
#[tokio::test]
async fn open_folder_validation() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    // BUG-038: the download folder must be a subdirectory of the
    // tmpdir, not the tmpdir itself — the SQLite database lives in
    // the tmpdir and stays open for the whole test, so deleting the
    // tmpdir fails on Windows (error 32, file in use) while Linux
    // lets you unlink open files. The downloaded file's handle is
    // closed once the download completes, so deleting the subfolder
    // works on both platforms.
    let downloads = dir.join("downloads");
    let (id, _) = mgr
        .add_download(&server.url, None, Some(downloads.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let folder = mgr.folder_for_download(&id).unwrap();
    assert_eq!(folder, downloads);
    // The folder is deleted after the download finished: the dialog
    // must say so instead of opening nothing.
    std::fs::remove_dir_all(&downloads).unwrap();
    let err = mgr.folder_for_download(&id).unwrap_err();
    assert!(
        err.to_string().contains("no longer exists"),
        "error: {err}"
    );
    let err = mgr.open_folder(&id).unwrap_err();
    assert!(
        err.to_string().contains("no longer exists"),
        "open_folder must fail before spawning anything: {err}"
    );
    // Unknown download: not found, for both entry points.
    assert!(mgr.folder_for_download("no-such-download").is_err());
    assert!(mgr.open_folder("no-such-download").is_err());
}
