//! Round 72 — dynamic segmentation (start with one segment,
//! grow by splitting), the patient tail (the last unfinished
//! segment retries until the server recovers instead of failing
//! the download after `max_attempts`), and in-place redownload
//! (the row survives a failed probe).
//!
//! The Round 71 OVH mock test
//! (`download_completes_when_server_limits_concurrency_to_three`,
//! round71_tests.rs) still pins the non-tail behaviour — a 2 MiB
//! download completes with concurrency capped at 3 — and must
//! keep passing alongside these.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mock::*;

use fetchline_engine::{DownloadManager, Store};

/// Poll the store's log file for a line, up to `secs` seconds.
/// The engine writes the interesting decisions there (patient
/// tail entry, deferrals), and polling for the line is how a
/// test knows the code took the path it asserts — instead of
/// sleeping a fixed time and hoping.
async fn wait_for_log_line(path: &std::path::Path, needle: &str, secs: u64) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if let Ok(text) = std::fs::read_to_string(path) {
            if text.contains(needle) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// Round 72 (D) — a download starts with ONE segment and grows
/// by splitting the largest remaining segment whenever a worker
/// frees (IDM model). An 8 MiB file at 8 connections grows to 8
/// segments of 1 MiB (the 1 MiB split floor applies to the bytes
/// REMAINING, so a segment stops splitting once it has less than
/// 1 MiB left — a 4 MiB file would only reach 4); the file is
/// byte-exact, so no merge step is needed — every segment writes
/// in place at its own offset.
#[tokio::test]
async fn download_starts_with_one_segment_and_grows_by_splitting() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(8 * 1024 * 1024),
        chunk: 8192,
        delay: Duration::from_millis(50),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 8);
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
    // One segment at creation — the setting is a concurrency cap,
    // not a pre-split count.
    assert_eq!(store.get_segments(&id).unwrap().len(), 1);
    mgr.start_download(&id).await.unwrap();
    // It grows while it runs: the supervisor splits on every tick
    // while a slot is free, so the count passes 1 long before the
    // download finishes.
    let mut grew = false;
    for _ in 0..100 {
        if store.get_segments(&id).unwrap().len() > 1 {
            grew = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(grew, "segments never grew past 1");
    assert!(
        wait_for_status(&mgr, &id, "complete", 60).await,
        "download did not complete"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        std::fs::read(&row.save_path).unwrap(),
        payload(8 * 1024 * 1024)
    );
    // 8 MiB / 1 MiB split floor = 8 segments of 1 MiB.
    assert_eq!(store.get_segments(&id).unwrap().len(), 8);
    // The checkpoint carried the growth to the row the main window
    // polls: the part count, at least, made it there.
    assert!(
        row.segment_count >= 2,
        "row segment_count did not record the growth: {}",
        row.segment_count
    );
}

/// Round 72 (C) — the tail: a single-segment download whose
/// server drops every connection (OVH's burst behaviour) must
/// NOT fail after `max_attempts`. It enters patient retry (5 s,
/// then 15 s, 30 s, 60 s) and completes once the server comes
/// back — the row stays `downloading` the whole time, never
/// `error`.
#[tokio::test]
async fn tail_keeps_retrying_until_the_server_recovers() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("engine.log");
    store.set_log_path(log_path.clone());
    // One connection: the single segment is the tail from the
    // start, and nothing can split it (512 KiB is below the
    // 1 MiB split floor).
    let mgr = DownloadManager::new(Arc::clone(&store), 1);
    server.set_drop_get(true);
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
    // A dropped connection is NOT deferrable (its slow-path
    // backoffs are 2 s / 4 s / 8 s / 16 s — see `is_tail_patient`),
    // so the tail reaches patient retry after ~30 s of those,
    // not after the ~5 s fast-retry schedule. The log line proves
    // it took that path, not the fail path. (Without the patient
    // tail this download would be `error` at ~30 s.)
    assert!(
        wait_for_log_line(&log_path, "entering patient retry (tail)", 60).await,
        "tail never entered patient retry; log: {:?}",
        std::fs::read_to_string(&log_path).unwrap_or_default()
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        row.status, "downloading",
        "the tail must wait, not fail, while the server is down"
    );
    // Server recovers mid-wait: the next attempt (after the 5 s
    // backoff) succeeds and the file completes byte-exact.
    server.set_drop_get(false);
    assert!(
        wait_for_status(&mgr, &id, "complete", 60).await,
        "tail did not complete after the server recovered"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

/// Round 72 (C) — a body stall is NEVER patient: a segment that
/// started and then wedged must still fail (Round 55 pins it),
/// even when it is the tail. The stall watchdog test in
/// round55_tests.rs covers the failure itself; this test pins
/// that the tail does NOT enter patient retry on a stall — the
/// log must not contain the patient line.
#[tokio::test]
async fn tail_never_waits_patiently_on_a_body_stall() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("engine.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 1);
    let mut s = mgr.get_settings().unwrap();
    s.max_retries = 1;
    mgr.update_settings(&s).unwrap();
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
    assert!(
        wait_for_status(&mgr, &id, "error", 60).await,
        "a wedged tail must fail, not wait forever"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        row.error_message
            .as_deref()
            .unwrap_or("")
            .contains("stalled"),
        "error: {:?}",
        row.error_message
    );
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        !log.contains("entering patient retry (tail)"),
        "a body stall must never enter patient retry; log: {log}"
    );
}

/// Round 72 (C) — Redownload in place: the row survives a failed
/// probe (the old delete-first flow lost the row entirely when
/// the re-add probe failed on a flaky server), and a redownload
/// of a failed row restarts it from scratch.
#[tokio::test]
async fn redownload_survives_a_failed_probe_and_restarts_from_scratch() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    // One retry: part 3's failing run must error fast (a 500 is
    // not patient-eligible, so it fails after its attempts — with
    // the default 5 retries the backoffs alone would take ~30 s).
    let mut s = mgr.get_settings().unwrap();
    s.max_retries = 1;
    mgr.update_settings(&s).unwrap();
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
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);

    // Part 1 — the server is gone: the probe fails, the redownload
    // errors, and the row SURVIVES untouched (still complete, file
    // still on disk, byte-exact).
    server.set_gone(true);
    assert!(mgr.redownload_download(&id).await.is_err());
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));

    // Part 2 — the server is back: the redownload resets the row
    // and downloads it again from scratch (512 KiB never splits,
    // so exactly one segment).
    server.set_gone(false);
    mgr.redownload_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    assert_eq!(store.get_segments(&id).unwrap().len(), 1);

    // Part 3 — a failed row: every GET 500s, so the run errors.
    // A redownload resets it and starts it again; once the server
    // recovers it completes.
    server.set_fail_get(true);
    mgr.redownload_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "error", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "error");
    assert_eq!(store.get_segments(&id).unwrap().len(), 1);
    server.set_fail_get(false);
    mgr.redownload_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}
