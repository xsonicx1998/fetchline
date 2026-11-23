//! Download quota + per-download speed cap (S12) tests.
//!
//! Every test serves its own fixture on 127.0.0.1 (`MockServer` in
//! `common/mock.rs`) — no real website is ever contacted. Quota
//! windows are driven with the clock seam (`*_at(now)` methods
//! taking Unix seconds), never by waiting hours: a 1-hour window
//! rolls over by calling the method with `now + 3601`.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use mock::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fetchline_engine::timeutil::now_unix;
use fetchline_engine::{DownloadManager, Store};

/// A 2 MiB file: big enough to cross a 1 MB quota mid-download,
/// small enough to finish fast when nothing throttles it.
const TWO_MIB: usize = 2 * 1024 * 1024;
/// 1.5 MiB: the pause/resume tests use this size, so the ~0.5 MB
/// left after the 1 MB pause fits in the fresh window that a
/// rollover or a reset opens (a 2 MiB file would pause again).
const ONE_AND_HALF_MIB: usize = 1536 * 1024;
/// The smallest quota the engine accepts: 1 MB.
const ONE_MB: i64 = 1;

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

async fn add_named(
    mgr: &Arc<DownloadManager>,
    server_url: &str,
    dir: &std::path::Path,
    name: &str,
) -> String {
    let (id, _) = mgr
        .add_download(
            server_url,
            Some(name.to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    id
}

/// Add a 2 MiB download and start it under a 1 MB / 1 h quota.
/// Returns once it is paused by the quota.
async fn download_paused_by_quota(
    mgr: &Arc<DownloadManager>,
    server_url: &str,
    dir: &std::path::Path,
) -> String {
    let id = add_named(mgr, server_url, dir, "big.bin").await;
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(mgr, &id, "paused", 30).await,
        "download was not paused by the quota"
    );
    id
}

// -- quota settings + validation ------------------------------------------

#[tokio::test]
async fn quota_settings_validate_and_round_trip() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    // 0 MB, 0 hours, and absurd values are all rejected, not clamped.
    assert!(mgr.set_download_limit(0, 1).is_err());
    assert!(mgr.set_download_limit(1, 0).is_err());
    assert!(mgr.set_download_limit(1_000_001, 1).is_err());
    assert!(mgr.set_download_limit(1, 721).is_err());
    // Enabling a quota with no size/window stored is rejected too.
    let mut s = mgr.get_settings().unwrap();
    s.quota_enabled = true;
    s.quota_mb = None;
    s.quota_hours = None;
    assert!(mgr.update_settings(&s).is_err());
    // A valid quota round-trips, enabled.
    let saved = mgr.set_download_limit(5, 24).unwrap();
    assert!(saved.quota_enabled);
    assert_eq!(saved.quota_mb, Some(5));
    assert_eq!(saved.quota_hours, Some(24));
    let back = mgr.get_settings().unwrap();
    assert_eq!(back.quota_mb, Some(5));
    assert_eq!(back.quota_hours, Some(24));
    // Turning it off keeps the values, so re-enabling restores them.
    let off = mgr.set_quota_enabled(false).unwrap();
    assert!(!off.quota_enabled);
    assert_eq!(off.quota_mb, Some(5));
    let status = mgr.get_quota_status().unwrap();
    assert!(!status.enabled);
    assert!(!status.reached);
}

#[tokio::test]
async fn quota_window_math_uses_the_clock_seam() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    // No window yet: nothing used, no start.
    let (start, used) = store.get_quota_state().unwrap();
    assert_eq!(start, None);
    assert_eq!(used, 0);
    // The first counted byte anchors the window at that moment.
    let used = store
        .record_quota_bytes(512 * 1024, 3600, 1_000_000)
        .unwrap();
    assert_eq!(used, 512 * 1024);
    let (start, used) = store.get_quota_state().unwrap();
    assert_eq!(start, Some(1_000_000));
    assert_eq!(used, 512 * 1024);
    // Inside the window, bytes accumulate; past its end, the next
    // byte anchors a fresh window instead of adding to the old one.
    let used = store
        .record_quota_bytes(512 * 1024, 3600, 1_000_000 + 3599)
        .unwrap();
    assert_eq!(used, 1024 * 1024);
    let used = store
        .record_quota_bytes(100, 3600, 1_000_000 + 3600)
        .unwrap();
    assert_eq!(
        used, 100,
        "a byte past the window end must start a new window"
    );
    let (start, _) = store.get_quota_state().unwrap();
    assert_eq!(start, Some(1_000_000 + 3600));
}

// -- quota reached: pause, queue stop, status ------------------------------

#[tokio::test]
async fn quota_reached_pauses_the_download_mid_file() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(ONE_AND_HALF_MIB),
        chunk: 8192,
        delay: Duration::from_millis(5),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.set_download_limit(ONE_MB, 1).unwrap();
    let id = download_paused_by_quota(&mgr, &server.url, &dir).await;
    let row = mgr.get_download(&id).unwrap().unwrap();
    // Between 1 MB (the quota) and 1 MB + one in-flight chunk per
    // segment — the pause lands on chunk boundaries, never past
    // the file, and never before the quota.
    assert!(
        row.downloaded_bytes >= 1024 * 1024,
        "paused too early: {}",
        row.downloaded_bytes
    );
    assert!(
        row.downloaded_bytes < ONE_AND_HALF_MIB as i64,
        "quota did not pause the download"
    );
    assert!(
        row.quota_paused,
        "a quota pause must be marked quota_paused"
    );
    // The status names the quota: used >= limit, window end set.
    let status = mgr.get_quota_status().unwrap();
    assert!(status.enabled);
    assert!(status.reached);
    assert!(status.used_bytes >= 1024 * 1024);
    assert_eq!(status.quota_paused_downloads, 1);
    assert!(status.window_start_unix.is_some());
    assert_eq!(
        status.window_end_unix,
        status.window_start_unix.map(|s| s + 3600)
    );
    // A manual start while the quota is reached is refused with
    // the resume time in the message — the download stays paused.
    let err = mgr.start_download(&id).await.unwrap_err();
    assert!(
        err.to_string().contains("quota"),
        "expected a quota error, got: {err}"
    );
    assert!(wait_for_status(&mgr, &id, "paused", 2).await);
}

#[tokio::test]
async fn quota_reached_stops_the_queue_and_leaves_the_rest_queued() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(ONE_AND_HALF_MIB),
        chunk: 8192,
        delay: Duration::from_millis(5),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.set_download_limit(ONE_MB, 1).unwrap();
    let queue = mgr.create_queue("Evening").unwrap();
    let first = add_named(&mgr, &server.url, &dir, "one.bin").await;
    let second = add_named(&mgr, &server.url, &dir, "two.bin").await;
    mgr.add_to_queue(&first, &queue.id).unwrap();
    mgr.add_to_queue(&second, &queue.id).unwrap();
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_status(&mgr, &first, "paused", 30).await);
    // Give the runner a moment to meet the second file and stop.
    tokio::time::sleep(Duration::from_millis(800)).await;
    let second_row = mgr.get_download(&second).unwrap().unwrap();
    assert_eq!(
        second_row.status, "queued",
        "the queue must stop at the quota, not start the next file"
    );
    assert!(!second_row.quota_paused);
    let queues = mgr.list_queues().await.unwrap();
    let q = queues.iter().find(|q| q.id == queue.id).unwrap();
    assert!(
        !q.running,
        "the queue run must have ended when the quota was reached"
    );
}

#[tokio::test]
async fn quota_disabled_never_pauses_and_counts_nothing() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(TWO_MIB),
        chunk: 8192,
        delay: Duration::from_millis(5),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    // Values stored but the switch off: the download completes
    // and the window stays empty.
    let mut s = mgr.get_settings().unwrap();
    s.quota_enabled = false;
    s.quota_mb = Some(1);
    s.quota_hours = Some(1);
    mgr.update_settings(&s).unwrap();
    let id = add_named(&mgr, &server.url, &dir, "big.bin").await;
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(TWO_MIB));
    let status = mgr.get_quota_status().unwrap();
    assert_eq!(status.used_bytes, 0, "a disabled quota must count nothing");
    assert!(!status.reached);
}

// -- rollover + reset + restart -------------------------------------------

#[tokio::test]
async fn quota_window_rollover_resumes_the_paused_download() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(ONE_AND_HALF_MIB),
        chunk: 8192,
        delay: Duration::from_millis(5),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.set_download_limit(ONE_MB, 1).unwrap();
    let id = download_paused_by_quota(&mgr, &server.url, &dir).await;
    // The clock seam: one second past the 1-hour window, without
    // waiting an hour. The tick-time rollover check resumes it.
    let rolled = mgr
        .check_quota_rollover_at(now_unix() + 3601)
        .await
        .unwrap();
    assert!(rolled, "the window should have rolled over");
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        std::fs::read(&row.save_path).unwrap(),
        payload(ONE_AND_HALF_MIB)
    );
    assert!(
        !row.quota_paused,
        "a resumed download is no longer quota-paused"
    );
    // The usage in the NEW window is only the bytes downloaded
    // after the rollover (the remainder of the file).
    let status = mgr.get_quota_status().unwrap();
    assert!(status.used_bytes > 0 && status.used_bytes < ONE_AND_HALF_MIB as i64);
}

#[tokio::test]
async fn quota_rollover_via_scheduler_tick_resumes_without_restarting_the_queue() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(ONE_AND_HALF_MIB),
        chunk: 8192,
        delay: Duration::from_millis(5),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.set_download_limit(ONE_MB, 1).unwrap();
    let queue = mgr.create_queue("Evening").unwrap();
    let first = add_named(&mgr, &server.url, &dir, "one.bin").await;
    let second = add_named(&mgr, &server.url, &dir, "two.bin").await;
    mgr.add_to_queue(&first, &queue.id).unwrap();
    mgr.add_to_queue(&second, &queue.id).unwrap();
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_status(&mgr, &first, "paused", 30).await);
    tokio::time::sleep(Duration::from_millis(800)).await;
    // The 30 s tick, driven with a fake clock past the window end,
    // rolls the quota over and resumes the paused file — but it
    // does NOT restart the queue run, so the second file stays
    // queued until the user starts the queue again.
    mgr.scheduler_tick_at(now_unix() + 3601).await.unwrap();
    assert!(wait_for_status(&mgr, &first, "complete", 30).await);
    let second_row = mgr.get_download(&second).unwrap().unwrap();
    assert_eq!(second_row.status, "queued");
}

#[tokio::test]
async fn quota_reset_resumes_now_and_starts_a_fresh_window() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(ONE_AND_HALF_MIB),
        chunk: 8192,
        delay: Duration::from_millis(5),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.set_download_limit(ONE_MB, 1).unwrap();
    let id = download_paused_by_quota(&mgr, &server.url, &dir).await;
    // Reset is the manual "resume now": a fresh window at once,
    // and the paused download resumes into it.
    let status = mgr.reset_quota().await.unwrap();
    assert_eq!(status.used_bytes, 0);
    assert!(!status.reached);
    assert!(status.window_start_unix.is_some());
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        std::fs::read(&row.save_path).unwrap(),
        payload(ONE_AND_HALF_MIB)
    );
}

#[tokio::test]
async fn quota_window_survives_a_restart() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(ONE_AND_HALF_MIB),
        chunk: 8192,
        delay: Duration::from_millis(5),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.set_download_limit(ONE_MB, 1).unwrap();
    let id = download_paused_by_quota(&mgr, &server.url, &dir).await;
    let used_before = mgr.get_quota_status().unwrap().used_bytes;
    let window_before = mgr.get_quota_status().unwrap().window_start_unix;
    // Drop the whole manager (as if the app closed) and reopen
    // the same database file: the window must still be there.
    drop(mgr);
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr2 = DownloadManager::new(store, 4);
    let status = mgr2.get_quota_status().unwrap();
    assert_eq!(
        status.used_bytes, used_before,
        "a restart must not reset the window usage"
    );
    assert_eq!(status.window_start_unix, window_before);
    assert!(status.reached);
    // A new download still cannot start after the restart…
    let fresh = add_named(&mgr2, &server.url, &dir, "fresh.bin").await;
    assert!(mgr2.start_download(&fresh).await.is_err());
    // …but a reset lets the paused download finish from where it
    // stopped, byte-exact.
    mgr2.reset_quota().await.unwrap();
    assert!(wait_for_status(&mgr2, &id, "complete", 30).await);
    let row = mgr2.get_download(&id).unwrap().unwrap();
    assert_eq!(
        std::fs::read(&row.save_path).unwrap(),
        payload(ONE_AND_HALF_MIB)
    );
}

#[tokio::test]
async fn user_paused_download_is_never_auto_resumed_by_a_rollover() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(TWO_MIB),
        chunk: 8192,
        delay: Duration::from_millis(50),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.set_download_limit(ONE_MB, 1).unwrap();
    let id = add_named(&mgr, &server.url, &dir, "big.bin").await;
    mgr.start_download(&id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Paused by hand, well before the quota is reached.
    mgr.pause_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "paused", 10).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        !row.quota_paused,
        "a user's pause must not be marked quota-paused"
    );
    assert!(row.downloaded_bytes < 1024 * 1024);
    // A rollover (fake clock) resets the expired window's usage —
    // but it must leave this download paused: only quota-paused
    // downloads are auto-resumed, never one the user paused.
    assert!(mgr
        .check_quota_rollover_at(now_unix() + 3601)
        .await
        .unwrap());
    assert_eq!(mgr.get_quota_status().unwrap().used_bytes, 0);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().status, "paused");
}

// -- per-download cap ------------------------------------------------------

#[tokio::test]
async fn per_download_cap_throttles_one_download() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(256 * 1024),
        chunk: 65536,
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let id = add_named(&mgr, &server.url, &dir, "capped.bin").await;
    mgr.set_download_speed_limit(&id, Some(32)).unwrap();
    assert_eq!(
        mgr.get_download(&id).unwrap().unwrap().speed_limit_kbps,
        Some(32)
    );
    let start = Instant::now();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 40).await);
    let elapsed = start.elapsed();
    // 256 KiB at 32 KiB/s: ~7 s after the 32 KiB burst.
    assert!(
        elapsed >= Duration::from_secs(5),
        "cap did not throttle: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(25), "elapsed: {elapsed:?}");
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
}

#[tokio::test]
async fn per_download_cap_zero_means_global_cap_only() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(256 * 1024),
        chunk: 65536,
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let id = add_named(&mgr, &server.url, &dir, "uncapped.bin").await;
    mgr.set_download_speed_limit(&id, Some(0)).unwrap();
    assert_eq!(
        mgr.get_download(&id).unwrap().unwrap().speed_limit_kbps,
        None
    );
    let start = Instant::now();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "cap 0 must not throttle"
    );
}

#[tokio::test]
async fn per_download_cap_and_global_cap_compose_min_wins() {
    // Case 1: the global cap binds (32 KiB/s global, 128 per-download).
    let server = MockServer::start(MockConfig {
        size_bytes: Some(256 * 1024),
        chunk: 65536,
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.set_speed_limit(Some(32)).unwrap();
    let id = add_named(&mgr, &server.url, &dir, "global-binds.bin").await;
    mgr.set_download_speed_limit(&id, Some(128)).unwrap();
    let start = Instant::now();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 40).await);
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_secs(5),
        "global cap did not bind: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(25), "elapsed: {elapsed:?}");

    // Case 2: the per-download cap binds (128 KiB/s global, 32 per-download).
    let server2 = MockServer::start(MockConfig {
        size_bytes: Some(256 * 1024),
        chunk: 65536,
        ..Default::default()
    });
    let dir2 = tmpdir();
    let mgr2 = test_mgr(&dir2);
    mgr2.set_speed_limit(Some(128)).unwrap();
    let id2 = add_named(&mgr2, &server2.url, &dir2, "per-download-binds.bin").await;
    mgr2.set_download_speed_limit(&id2, Some(32)).unwrap();
    let start = Instant::now();
    mgr2.start_download(&id2).await.unwrap();
    assert!(wait_for_status(&mgr2, &id2, "complete", 40).await);
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_secs(5),
        "per-download cap did not bind: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(25), "elapsed: {elapsed:?}");
    let row = mgr2.get_download(&id2).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
}

#[tokio::test]
async fn per_download_cap_throttles_one_download_while_another_runs_at_the_global_cap() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(256 * 1024),
        chunk: 65536,
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    mgr.set_speed_limit(Some(64)).unwrap();
    let capped = add_named(&mgr, &server.url, &dir, "capped.bin").await;
    let free = add_named(&mgr, &server.url, &dir, "free.bin").await;
    mgr.set_download_speed_limit(&capped, Some(16)).unwrap();
    mgr.start_download(&capped).await.unwrap();
    mgr.start_download(&free).await.unwrap();
    // The uncapped download has the rest of the global bucket
    // (~48 KiB/s) and finishes well before the 16 KiB/s one.
    assert!(
        wait_for_status(&mgr, &free, "complete", 15).await,
        "uncapped download did not finish first"
    );
    let capped_row = mgr.get_download(&capped).unwrap().unwrap();
    assert_eq!(
        capped_row.status, "downloading",
        "the capped download must still be running when the other finishes"
    );
    assert!(wait_for_status(&mgr, &capped, "complete", 30).await);
    for (id, name) in [(&capped, "capped.bin"), (&free, "free.bin")] {
        let row = mgr.get_download(id).unwrap().unwrap();
        assert_eq!(row.filename, name);
        assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
    }
}

#[tokio::test]
async fn per_download_cap_can_be_cleared_live_mid_download() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(256 * 1024),
        chunk: 65536,
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let id = add_named(&mgr, &server.url, &dir, "live.bin").await;
    mgr.set_download_speed_limit(&id, Some(16)).unwrap();
    let start = Instant::now();
    mgr.start_download(&id).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    // Clearing the cap mid-download speeds the rest up at once —
    // the whole file must finish far faster than 16 KiB/s allows
    // (~15 s for 256 KiB).
    mgr.set_download_speed_limit(&id, Some(0)).unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(12),
        "clearing the cap did not speed the download up: {elapsed:?}"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
}

#[tokio::test]
async fn per_download_cap_validation_rejects_absurd_and_negative_values() {
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let server = MockServer::start(MockConfig::default());
    let id = add_named(&mgr, &server.url, &dir, "file.bin").await;
    assert!(mgr.set_download_speed_limit(&id, Some(-5)).is_err());
    // Same bound as the global cap (BUG-008): rejected, not clamped.
    assert!(mgr.set_download_speed_limit(&id, Some(10_000_001)).is_err());
    assert!(mgr.set_download_speed_limit(&id, Some(10_000_000)).is_ok());
    assert!(mgr
        .set_download_speed_limit("no-such-id", Some(32))
        .is_err());
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.speed_limit_kbps, Some(10_000_000));
}

// -- sync counts toward the quota ------------------------------------------

/// A periodic queue holding one finished 512 KiB file.
async fn sync_setup(
    mgr: &Arc<DownloadManager>,
    server_url: &str,
    dir: &std::path::Path,
) -> (String, String) {
    let queue = mgr.create_queue("Sync").unwrap();
    mgr.update_queue(
        &queue.id,
        "periodic",
        None,
        None,
        false,
        "none",
        None,
        Some(30),
    )
    .await
    .unwrap();
    let (id, _) = mgr
        .add_download(
            server_url,
            Some("file.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            Some(queue.id.clone()),
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(mgr, &id, "complete", 30).await);
    (queue.id, id)
}

#[tokio::test]
async fn sync_redownload_counts_toward_the_quota() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let (queue, _id) = sync_setup(&mgr, &server.url, &dir).await;
    // The quota is enabled AFTER the first download, so only the
    // sync re-download can count toward it.
    mgr.set_download_limit(ONE_MB, 1).unwrap();
    server.bump_version(); // 512 KiB -> 513 KiB, new ETag.
    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.changed, 1);
    assert_eq!(result.failed, 0);
    // Exactly the re-downloaded bytes — the HEAD probe counted
    // nothing, and the file was re-downloaded whole (513 KiB).
    let status = mgr.get_quota_status().unwrap();
    assert_eq!(
        status.used_bytes,
        513 * 1024,
        "sync re-download bytes must count, probe bytes must not"
    );
}

#[tokio::test]
async fn sync_redownload_paused_by_quota_fails_fast_not_after_120s() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(TWO_MIB),
        chunk: 8192,
        delay: Duration::from_millis(5),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let (queue, id) = sync_setup(&mgr, &server.url, &dir).await;
    mgr.set_download_limit(ONE_MB, 1).unwrap();
    server.bump_version(); // 2 MiB -> 2 MiB + 1 KiB, new ETag.
    let start = Instant::now();
    let result = mgr.run_sync_queue(&queue).await.unwrap();
    // The re-download pauses at the 1 MB quota; the sync reports
    // it failed (and retries at the next sync) instead of waiting
    // out the 120 s re-download timeout.
    assert!(
        start.elapsed() < Duration::from_secs(60),
        "sync must not wait out the 120 s timeout: {:?}",
        start.elapsed()
    );
    assert_eq!(result.failed, 1);
    assert_eq!(result.files[0].status, "failed");
    assert!(
        result.files[0]
            .message
            .as_deref()
            .unwrap_or("")
            .contains("quota"),
        "failure must name the quota: {:?}",
        result.files[0].message
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "paused");
    assert!(row.quota_paused);
}

// -- migration --------------------------------------------------------------

#[test]
fn pre_quota_database_migrates_quota_columns_and_state() {
    let dir = tmpdir();
    let path = dir.join("old.db");
    {
        // A database from before S12: settings has no quota
        // columns, downloads has no cap / quota-paused columns,
        // and there is no quota_state table at all.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (id INTEGER PRIMARY KEY CHECK (id = 1),
             default_download_folder TEXT NOT NULL DEFAULT '',
             max_connections INTEGER NOT NULL DEFAULT 8,
             speed_limit_kbps INTEGER, language TEXT NOT NULL DEFAULT 'en',
             theme TEXT NOT NULL DEFAULT 'light',
             browser_integration INTEGER NOT NULL DEFAULT 1,
             antivirus_command TEXT, file_types_auto TEXT NOT NULL DEFAULT '[]',
             excluded_sites TEXT NOT NULL DEFAULT '[]',
             check_updates_weekly INTEGER NOT NULL DEFAULT 1,
             updated_at TEXT NOT NULL DEFAULT (datetime('now')));
             INSERT INTO settings (id) VALUES (1);
             CREATE TABLE downloads (id TEXT PRIMARY KEY, url TEXT NOT NULL,
             final_url TEXT, filename TEXT NOT NULL, save_path TEXT NOT NULL,
             category_id TEXT NOT NULL, queue_id TEXT, queue_position INTEGER NOT NULL DEFAULT 0,
             size_bytes INTEGER, downloaded_bytes INTEGER NOT NULL DEFAULT 0,
             status TEXT NOT NULL DEFAULT 'queued', speed_bps INTEGER NOT NULL DEFAULT 0,
             eta_seconds INTEGER, grabber_project_id TEXT,
             etag TEXT, last_modified TEXT, error_message TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')), completed_at TEXT);
             INSERT INTO downloads (id, url, filename, save_path, category_id, status)
             VALUES ('dl-1', 'http://127.0.0.1/file.bin', 'file.bin', '/tmp/file.bin', 'cat-video', 'paused');",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let settings = store.get_settings().unwrap();
    assert!(!settings.quota_enabled);
    assert_eq!(settings.quota_mb, None);
    assert_eq!(settings.quota_hours, None);
    let (start, used) = store.get_quota_state().unwrap();
    assert_eq!(start, None);
    assert_eq!(used, 0);
    let row = store.get_download("dl-1").unwrap().unwrap();
    assert_eq!(row.speed_limit_kbps, None);
    assert!(!row.quota_paused);
    // The migrated columns are usable at once.
    store.set_download_speed_limit("dl-1", Some(64)).unwrap();
    assert_eq!(
        store
            .get_download("dl-1")
            .unwrap()
            .unwrap()
            .speed_limit_kbps,
        Some(64)
    );
    store.set_quota_paused("dl-1", true).unwrap();
    assert!(store.get_download("dl-1").unwrap().unwrap().quota_paused);
}
