//! Round 57 — fix round for the r56 live defects: downloads stall
//! at 7–8% with no watchdog line in the log, and both webviews go
//! blank white. What is pinned here is the engine behaviour the fix
//! depends on: the truly-independent thread watchdog (§9.2f), which
//! must fire when the tokio runtime itself is wedged — the case the
//! Round 56 tokio watchdog cannot cover, because it is a task on the
//! same runtime it watches. The shell pieces (native reload,
//! auto-reload, `ProcessFailed`, ping fix, A/B test, poll guard)
//! are verified by `npm run build` + source (the Tauri shell never
//! compiles on this VM) and by the live VPS retest. No public
//! internet is used; the fixture server is the shared mock.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;
use std::time::Duration;

use mock::*;

use fetchline_engine::{DownloadManager, Store};

async fn set_max_retries(mgr: &DownloadManager, n: i64) {
    let mut s = mgr.get_settings().unwrap();
    s.max_retries = n;
    mgr.update_settings(&s).unwrap();
}

/// Round 57 (§9.2f): the thread watchdog aborts a stalled run even
/// when the tokio runtime is wedged. This test's runtime has
/// exactly 2 workers — like the VPS — and two tasks doing a sync
/// sleep block both of them, freezing every timer and every task,
/// including the tokio run watchdog and this test's own poll. The
/// `std::thread` watchdog is the only thing still running: it must
/// fail the download with the watchdog's message, and the log must
/// show the THREAD watchdog fired — the tokio one must not have
/// (its timers never ran while the runtime was wedged).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_watchdog_fires_when_tokio_runtime_is_wedged() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    set_max_retries(&mgr, 1).await;
    // A diagnostics/test knob only: production never shortens
    // this (default 180 s). The 2 s value keeps the test fast
    // while the 30 s per-chunk watchdog is far slower, so only
    // a run watchdog can fire first.
    mgr.set_run_watchdog_timeout_secs(2);
    server.set_stall_get(true);
    let (id, _) = mgr
        .add_download(
            &server.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    // Let the run start and park on the stalled stream.
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Wedge the runtime: a sync sleep on a tokio task blocks its
    // worker — two of them freeze the whole 2-worker runtime.
    // The blockers must start well before the 2 s watchdog
    // deadline, so the tokio watchdog's next check can never run
    // before the thread watchdog fires.
    for _ in 0..2 {
        tokio::spawn(async move {
            std::thread::sleep(Duration::from_secs(8));
        });
    }
    // This poll cannot run until the blockers finish (~8 s); by
    // then the thread watchdog (timeout 2 s, check every 1 s)
    // must already have fired and marked the row error.
    assert!(
        wait_for_status(&mgr, &id, "error", 20).await,
        "the thread watchdog must fail a stalled run while the tokio runtime is wedged"
    );
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.contains("thread watchdog aborted"),
        "the log must show the THREAD watchdog fired, got:\n{log}"
    );
    assert!(
        !log.contains("run watchdog aborted"),
        "the tokio watchdog must not have fired while the runtime was wedged, got:\n{log}"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    let message = row.error_message.unwrap_or_default();
    assert!(
        message.contains("No data arrived for 2 seconds"),
        "the error must name the watchdog stall, got: {message}"
    );
}

/// Round 57 (§9.2f): the thread watchdog spares a slow download
/// that keeps making progress — the timeout is about *no*
/// progress, not about total duration. A 4 MiB file at ~50 ms
/// per 8 KiB chunk takes ~6 s with the watchdog at 2 s, and must
/// still complete with no watchdog line in the log at all.
#[tokio::test]
async fn thread_watchdog_spares_a_slow_but_moving_download() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(50),
        size_bytes: Some(4 * 1024 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    mgr.set_run_watchdog_timeout_secs(2);
    let (id, _) = mgr
        .add_download(
            &server.url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "a slow download that keeps receiving bytes must complete, not be aborted"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.downloaded_bytes, 4 * 1024 * 1024);
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        !log.contains("thread watchdog aborted") && !log.contains("run watchdog aborted"),
        "no watchdog must fire on a download that keeps making progress, got:\n{log}"
    );
}
