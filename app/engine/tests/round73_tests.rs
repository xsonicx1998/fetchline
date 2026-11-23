//! Round 73 — the patient tail's coalesce gate and zero-progress
//! cap, the probe's fast retries with stored-info fallbacks for
//! Resume/Redownload, and the per-host connection cap after a
//! 429/503.
//!
//! The Round 72 tail test
//! (`tail_keeps_retrying_until_the_server_recovers`,
//! round72_tests.rs) still pins the single-segment tail — a lone
//! segment that never splits waits patiently and completes on
//! recovery — and must keep passing alongside these.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mock::*;

use fetchline_engine::{DownloadManager, Store};

/// Poll the store's log file for a line, up to `secs` seconds.
/// (Same helper as round72_tests.rs — each test binary is
/// standalone, so it is duplicated here rather than shared.)
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

/// Round 73 (Group 2) — the OVH 92% shape: a segment fails
/// while its successor is still pending, and the tail gate used
/// to fail the run because `unfinished_count` was 2, not 1. The
/// gate now coalesces the unstarted pending successor into the
/// failing segment and respawns the merged segment as a patient
/// tail, so the download completes byte-exact once the server
/// recovers.
///
/// The shape is built with the server refusing every 2nd
/// concurrent GET: the download grows to 2 segments, the 2nd
/// GET 429s and defers (done == 0, not running), and the
/// effective concurrency ratchets to 1 — so the 1st segment
/// downloads alone. Cutting its GET then fails it while its
/// successor is pending, exactly the OVH shape.
#[tokio::test]
async fn tail_coalesces_unstarted_pending_segments_and_completes() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(4 * 1024 * 1024),
        chunk: 8192,
        delay: Duration::from_millis(50),
        max_concurrent_gets: Some(1),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("engine.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 8);
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
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    // The 429 deferral proves the ratchet fired (and the run is
    // now serial: the 1st segment first, the 2nd deferred at
    // zero bytes). Cut the 1st segment's GET at once — it fails
    // mid-transfer while its successor is pending.
    assert!(
        wait_for_log_line(&log_path, "segment deferred", 60).await,
        "no segment ever deferred; log: {:?}",
        std::fs::read_to_string(&log_path).unwrap_or_default()
    );
    server.set_drop_get(true);
    // The gate merges the pending successor into the failing
    // segment and respawns it as a patient tail.
    assert!(
        wait_for_log_line(&log_path, "tail gate:", 60).await,
        "the gate never logged its decision; log: {:?}",
        std::fs::read_to_string(&log_path).unwrap_or_default()
    );
    assert!(
        wait_for_log_line(&log_path, "coalesced 1 unstarted pending segment", 60).await,
        "the gate never coalesced; log: {:?}",
        std::fs::read_to_string(&log_path).unwrap_or_default()
    );
    assert!(
        wait_for_log_line_after(
            &log_path,
            "coalesced 1 unstarted pending segment",
            "entering patient retry (tail)",
            60
        )
        .await,
        "the merged segment never entered patient retry; log: {:?}",
        std::fs::read_to_string(&log_path).unwrap_or_default()
    );
    // Server recovers mid-wait: the tail's next attempt
    // downloads the merged range to the end, byte-exact.
    server.set_drop_get(false);
    assert!(
        wait_for_status(&mgr, &id, "complete", 90).await,
        "download did not complete after recovery; log: {:?}",
        std::fs::read_to_string(&log_path).unwrap_or_default()
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        std::fs::read(&row.save_path).unwrap(),
        payload(4 * 1024 * 1024)
    );
    // The merge survived to the database: the pending successor
    // is gone and the one surviving segment covers the whole
    // file — no orphan rows overlap it.
    let segs = store.get_segments(&id).unwrap();
    assert_eq!(segs.len(), 1, "segments after coalesce: {segs:?}");
    assert_eq!(segs[0].start_byte, 0);
    assert_eq!(segs[0].end_byte, Some(4 * 1024 * 1024 - 1));
}

/// Poll the store's log file until the text AFTER the first
/// occurrence of `after_needle` contains `needle`, up to `secs`
/// seconds. Proves an event happened after another one, not
/// merely somewhere in the log (a stale earlier line must not
/// satisfy the wait).
async fn wait_for_log_line_after(
    path: &std::path::Path,
    after_needle: &str,
    needle: &str,
    secs: u64,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Some(pos) = text.find(after_needle) {
                if text[pos..].contains(needle) {
                    return true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// Round 73 (Group 2) — the zero-progress cap: a patient tail
/// on a server that never delivers another byte must fail
/// visibly after the cap instead of retrying forever. The cap
/// is shortened to 10 s for the test (the setting defaults to
/// 10 minutes).
#[tokio::test]
async fn dead_server_fails_after_the_zero_progress_cap() {
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
    // 10 s, not 10 minutes — the test would otherwise take all
    // morning to prove the cap fires.
    mgr.set_tail_zero_progress_cap_secs(10);
    assert_eq!(mgr.tail_zero_progress_cap_secs(), 10);
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
    mgr.start_download(&id).await.unwrap();
    // The tail enters patient retry first (it must not fail on
    // the first dropped connection) — then the cap, not a
    // watchdog, ends it.
    assert!(
        wait_for_log_line(&log_path, "entering patient retry (tail)", 60).await,
        "tail never entered patient retry; log: {:?}",
        std::fs::read_to_string(&log_path).unwrap_or_default()
    );
    assert!(
        wait_for_status(&mgr, &id, "error", 60).await,
        "a dead server must fail after the cap, not retry forever"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        row.error_message
            .as_deref()
            .unwrap_or("")
            .contains("server appears to be down"),
        "error: {:?}",
        row.error_message
    );
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.contains("gave up"),
        "no 'gave up' line in the log:\n{log}"
    );
}

/// Round 73 (Group 3) — Redownload retries a transient probe:
/// the server drops the first five probe requests (HEAD, then
/// the ranged-GET fallback, per probe attempt) and answers
/// after that, so the redownload of a failed row starts and
/// completes instead of erroring on the first dropped probe.
#[tokio::test]
async fn redownload_retries_transient_probe_and_restarts_failed_row() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("engine.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
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
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    // Fail the row: every GET 500s, so the redownload's run
    // errors and the row is Failed.
    server.set_fail_get(true);
    mgr.redownload_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "error", 30).await);
    // The server is back but its probes drop: five probe-like
    // requests go unanswered (attempt 1: HEAD + ranged GET,
    // attempt 2: HEAD + ranged GET, attempt 3: HEAD), then the
    // ranged GET answers — the probe's fast retries absorb it.
    server.set_fail_get(false);
    server.set_drop_probe_attempts(5);
    mgr.redownload_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "redownload did not complete after the transient probe failure"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.contains("retrying after 500ms"),
        "no probe retry in the log:\n{log}"
    );
}

/// Round 73 (Group 3) — Redownload refuses a dead link: a 404
/// probe fails immediately (no retries — 404 is not retryable),
/// the redownload errors, and the Failed row survives intact
/// (same segments, same stored size, 404 in the message).
#[tokio::test]
async fn redownload_refuses_on_404_probe() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
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
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    // Fail the row first, so the refusal is on a Failed row —
    // the case the Round 72 retest hit.
    server.set_fail_get(true);
    mgr.redownload_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "error", 30).await);
    server.set_fail_get(false);
    server.set_gone(true);
    assert!(mgr.redownload_download(&id).await.is_err());
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "error");
    assert_eq!(row.size_bytes, Some(512 * 1024));
    assert!(
        row.error_message.as_deref().unwrap_or("").contains("404"),
        "error: {:?}",
        row.error_message
    );
    assert_eq!(
        store.get_segments(&id).unwrap().len(),
        1,
        "the row's segments must survive a refused redownload"
    );
}

/// Round 73 (Group 3) — Resume continues from the stored info
/// when the probe fails transiently after its retries: the row
/// already knows its size and segment map, so a dropped probe
/// must not set the row to error or block the resume — the
/// engine verifies the bytes at the end.
#[tokio::test]
async fn resume_continues_from_stored_info_on_transient_probe_failure() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("engine.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
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
            None,
        )
        .await
        .unwrap();
    // Fail the row: every GET 500s.
    server.set_fail_get(true);
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "error", 30).await);
    // The server is back for GETs, but every probe request is
    // dropped — far more than the probe's four attempts can
    // absorb — so the probe fails transiently and the resume
    // must fall back to the stored size and segment map.
    server.set_fail_get(false);
    server.set_drop_probe_attempts(100);
    mgr.resume_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "resume did not complete from the stored info"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.contains("continuing from the stored info"),
        "no stored-info fallback in the log:\n{log}"
    );
}

/// Round 73 (Group 2) — a host that answers 429 is capped at 2
/// connections for the rest of the session: the first 429
/// deferral records the cap, and the download still completes
/// under it.
#[tokio::test]
async fn host_is_capped_at_two_connections_after_a_429() {
    // Same shape as the Round 71 concurrency test, but the
    // limit is 1: the 2nd concurrent GET is refused with 429
    // (the mock's 429 carries `Retry-After: 1`), so the
    // deferral path records the host cap.
    let server = MockServer::start(MockConfig {
        size_bytes: Some(4 * 1024 * 1024),
        chunk: 8192,
        delay: Duration::from_millis(50),
        max_concurrent_gets: Some(1),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("engine.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 8);
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
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_log_line(&log_path, "segment deferred", 60).await,
        "no segment ever deferred; log: {:?}",
        std::fs::read_to_string(&log_path).unwrap_or_default()
    );
    // The cap is recorded on the deferral, not only at the end.
    assert_eq!(mgr.host_connection_cap("127.0.0.1"), Some(2));
    assert!(
        server.rate_limited() > 0,
        "the mock never refused a request"
    );
    assert!(
        wait_for_status(&mgr, &id, "complete", 90).await,
        "download did not complete under the host cap"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        std::fs::read(&row.save_path).unwrap(),
        payload(4 * 1024 * 1024)
    );
}
