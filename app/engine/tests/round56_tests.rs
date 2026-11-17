//! Round 56 — fix round for the r55 live defects: the invoke path
//! wedged after a download completed (UI frozen at 0% while the
//! DB row was complete) and both webviews went blank white. What
//! is pinned here is the engine behaviour the fix depends on: the
//! independent run watchdog (§7.2), the probe-duration log
//! (§7.1d) and the watchdog timeout knob. The frontend guards
//! (§7.3), the heartbeat mechanism (§7.4) and the shell logging
//! (§7.1a/b/c/e/f) are verified by `npm run build` + source
//! (the Tauri shell never compiles on this VM) and by the live
//! VPS retest. No public internet is used; the fixture server is
//! the shared mock.

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

/// Round 56 (§7.2): the independent run watchdog aborts a run
/// whose supervisor loop wedges — the in-loop watchdog never
/// fires when the loop itself is stuck, so a separate task watches
/// the shared last-progress timestamp. A server that sends headers
/// and then no body, with the watchdog timeout shortened to 2 s,
/// must fail the download with the watchdog's message in ~2 s —
/// not after the 30 s per-chunk watchdog, and never hang.
#[tokio::test]
async fn independent_watchdog_aborts_a_stalled_run() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    set_max_retries(&mgr, 1).await;
    // A diagnostics/test knob only: production never shortens
    // this (default 180 s). The 2 s value keeps the test fast
    // while the 30 s per-chunk watchdog is far slower, so only
    // the independent watchdog can fire first.
    mgr.set_run_watchdog_timeout_secs(2);
    server.set_stall_get(true);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    let start = std::time::Instant::now();
    assert!(
        wait_for_status(&mgr, &id, "error", 15).await,
        "the independent watchdog must fail a stalled run"
    );
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "the watchdog must fire in ~2 s, not after the 30 s chunk watchdog (took {:?})",
        start.elapsed()
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    let message = row.error_message.unwrap_or_default();
    assert!(
        message.contains("No data arrived for 2 seconds"),
        "the error must name the watchdog stall, got: {message}"
    );
}

/// Round 56 (§7.2): a slow download that keeps making progress
/// is NOT aborted by the watchdog — the timeout is about *no*
/// progress, not about total duration. A 4 MiB file at ~50 ms
/// per 8 KiB chunk takes ~6 s with the watchdog at 2 s, and
/// must still complete.
#[tokio::test]
async fn independent_watchdog_spares_a_slow_but_moving_download() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(50),
        size_bytes: Some(4 * 1024 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    mgr.set_run_watchdog_timeout_secs(2);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "a slow download that keeps receiving bytes must complete, not be aborted"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.downloaded_bytes, 4 * 1024 * 1024);
}

/// Round 56 (§7.2): the watchdog timeout defaults to the same
/// 180 s the in-loop watchdog uses, and the setter clamps to
/// >= 1 s — a zero must never make the watchdog fire instantly
/// on a healthy run.
#[test]
fn run_watchdog_timeout_defaults_to_180s_and_clamps() {
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    assert_eq!(mgr.run_watchdog_timeout_secs(), 180);
    mgr.set_run_watchdog_timeout_secs(0);
    assert_eq!(mgr.run_watchdog_timeout_secs(), 1);
    mgr.set_run_watchdog_timeout_secs(30);
    assert_eq!(mgr.run_watchdog_timeout_secs(), 30);
}

/// Round 56 (§7.1d): probe duration is logged to the store's
/// diagnostics sink (which the shell points at startup.log), so
/// a slow or hanging probe is visible in the log instead of a
/// silent Add-dialog spin. The line names the URL, the outcome
/// and the elapsed time.
#[tokio::test]
async fn probe_duration_is_logged() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (_id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("probe") && log.contains(&server.url),
        "the probe must be logged with its URL, got: {log}"
    );
    assert!(
        log.contains("ok in") && log.contains("ms"),
        "the probe line must carry its duration, got: {log}"
    );
}
