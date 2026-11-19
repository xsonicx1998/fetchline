//! Round 74 (A) — whole-download stall retry: a download that
//! goes quiet at 0 B/s MID-FILE (bytes already received, every
//! connection stalled at once — the OVH 100Mb.dat shape) must NOT
//! fail after the 180 s no-data timeout. It retries the stalled
//! segments instead, and only gives up at the patient-tail
//! zero-progress cap — with a clear "while retrying" error, never
//! silently. A download that never received a byte still fails
//! fast (Round 55 pins that), and a body stall on the tail alone
//! still fails (Round 72 pins that). No public internet is used;
//! the fixture server is the shared mock, with the new
//! mid-body stall mode (`set_stall_after_bytes`).

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mock::*;

use fetchline_engine::{DownloadManager, Store};

/// Wait until the stored row shows at least `n` downloaded bytes
/// (proves the download got mid-file before it stalled), or the
/// deadline passes.
async fn wait_for_bytes(mgr: &DownloadManager, id: &str, n: i64, secs: u64) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if let Ok(Some(row)) = mgr.get_download(id) {
            if row.downloaded_bytes >= n {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// Wait until the engine log contains `needle`, or the deadline
/// passes. Async polling only — a blocking sleep here would stall
/// the runtime worker the supervisor's ticks run on (the thread
/// watchdog would then win the race and abort the run before the
/// supervisor can enter stall retry).
async fn wait_for_log(log_path: &std::path::Path, needle: &str, secs: u64) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if let Ok(log) = std::fs::read_to_string(log_path) {
            if log.contains(needle) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

/// Round 74 (A): a server that serves the first half of a file
/// and then goes quiet at 0 B/s must NOT fail the download after
/// the stall timeout — the download keeps retrying, and when the
/// server comes back it completes byte-exact from the bytes
/// already downloaded. The watchdog timeout is shortened to 5 s
/// (test knob, production default 180 s) and the give-up cap to
/// 120 s, so the test proves the retry instead of waiting three
/// minutes.
#[tokio::test]
async fn whole_download_stall_mid_file_recovers_and_completes() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("engine.log");
    store.set_log_path(log_path.clone());
    // One connection: a single segment stalls mid-file, the
    // whole-download shape with nothing else to hide behind.
    let mgr = DownloadManager::new(Arc::clone(&store), 1);
    mgr.set_run_watchdog_timeout_secs(5);
    mgr.set_tail_zero_progress_cap_secs(120);
    server.set_stall_after_bytes(256 * 1024);
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
        wait_for_bytes(&mgr, &id, 256 * 1024, 30).await,
        "the download must receive its first half before stalling"
    );
    assert!(
        wait_for_log(&log_path, "whole-download stall retry", 30).await,
        "the supervisor must enter whole-download stall retry instead of failing at the stall timeout"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        row.status, "downloading",
        "the download must still be running while it retries the stall"
    );
    // Server recovers: the next attempt resumes from the stalled
    // byte and completes.
    server.set_stall_after_bytes(0);
    assert!(
        wait_for_status(&mgr, &id, "complete", 120).await,
        "the download must complete after the server recovers"
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

/// Round 74 (A): a server that stalls mid-file and NEVER comes
/// back must keep retrying past the stall timeout — it must NOT
/// fail at the 5 s stall point — and only give up at the
/// zero-progress cap, with a clear "while retrying" error (never
/// silently, never with the plain watchdog message).
#[tokio::test]
async fn persistently_stalled_download_keeps_retrying_then_gives_up_at_cap() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("engine.log");
    store.set_log_path(log_path.clone());
    let mgr = DownloadManager::new(Arc::clone(&store), 1);
    mgr.set_run_watchdog_timeout_secs(5);
    // 20 s of zero progress from the last byte, then give up.
    mgr.set_tail_zero_progress_cap_secs(20);
    server.set_stall_after_bytes(256 * 1024);
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
    let start = Instant::now();
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_bytes(&mgr, &id, 256 * 1024, 30).await,
        "the download must receive its first half before stalling"
    );
    assert!(
        wait_for_log(&log_path, "whole-download stall retry", 30).await,
        "the supervisor must enter whole-download stall retry instead of failing at the stall timeout"
    );
    // Past the 5 s stall point the download must STILL be running
    // — it kept retrying instead of failing there.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        row.status, "downloading",
        "the download must still be retrying past the stall timeout, not failed"
    );
    assert!(
        wait_for_status(&mgr, &id, "error", 60).await,
        "a server that never comes back must eventually give up"
    );
    // The give-up lands at the cap (~20 s after the last byte),
    // not at the 5 s stall point.
    assert!(
        start.elapsed() >= Duration::from_secs(15),
        "the download gave up after {:?} — too early for a 20 s cap",
        start.elapsed()
    );
    let row = mgr.get_download(&id).unwrap().unwrap();
    let message = row.error_message.unwrap_or_default();
    assert!(
        message.contains("while retrying"),
        "the give-up error must say it was retrying, got: {message}"
    );
    assert!(
        message.contains("server appears to be down"),
        "the give-up error must name the dead server, got: {message}"
    );
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log.contains("whole-download stall retry gave up"),
        "the log must record the give-up; log tail: {}",
        log.lines().rev().take(10).collect::<Vec<_>>().join("\n")
    );
}

/// Round 74 (6): a freshly created queue can be renamed and
/// deleted — the reported bug was that neither worked on a new
/// queue, because no `delete_queue` / `rename_queue` command (or
/// engine method) existed at all. Guards: empty and duplicate
/// names are refused, and the main queue can be neither renamed
/// nor deleted.
#[tokio::test]
async fn fresh_queue_can_be_renamed_and_deleted() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 1);
    let queue = mgr.create_queue("Evening").unwrap();
    let renamed = mgr.rename_queue(&queue.id, "Night").unwrap();
    assert_eq!(renamed.name, "Night");
    assert_eq!(
        mgr.get_queue(&queue.id).await.unwrap().unwrap().name,
        "Night",
        "the renamed queue must read back with its new name"
    );
    assert!(
        mgr.rename_queue(&queue.id, "   ").is_err(),
        "an empty name must be refused"
    );
    let _other = mgr.create_queue("Other").unwrap();
    assert!(
        mgr.rename_queue(&queue.id, "Other").is_err(),
        "a duplicate name must be refused"
    );
    assert!(
        mgr.rename_queue("queue-main", "Main 2").is_err(),
        "the main queue must keep its name"
    );
    assert!(
        mgr.delete_queue("queue-main").await.is_err(),
        "the main queue cannot be deleted"
    );
    assert!(
        mgr.delete_queue("no-such-queue").await.is_err(),
        "deleting a missing queue must fail, not silently succeed"
    );
    mgr.delete_queue(&queue.id).await.unwrap();
    assert!(
        mgr.get_queue(&queue.id).await.unwrap().is_none(),
        "the deleted queue must be gone"
    );
}

/// Round 74 (6): deleting a queue that still holds downloads must
/// NOT delete the downloads — they move back to the main queue,
/// keeping their order.
#[tokio::test]
async fn deleting_a_queue_moves_its_downloads_to_the_main_queue() {
    let server = MockServer::start(MockConfig {
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 1);
    let queue = mgr.create_queue("Evening").unwrap();
    let (first, _) = mgr
        .add_download(
            &server.url,
            Some("one.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            Some(queue.id.clone()),
        )
        .await
        .unwrap();
    let (second, _) = mgr
        .add_download(
            &server.url,
            Some("two.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            Some(queue.id.clone()),
        )
        .await
        .unwrap();
    assert_eq!(mgr.list_queue_files(&queue.id).unwrap().len(), 2);
    mgr.delete_queue(&queue.id).await.unwrap();
    assert!(
        mgr.get_queue(&queue.id).await.unwrap().is_none(),
        "the deleted queue must be gone"
    );
    for id in [first, second] {
        let row = mgr.get_download(&id).unwrap().unwrap();
        assert_eq!(
            row.queue_id.as_deref(),
            Some("queue-main"),
            "a download in a deleted queue must move back to the main queue"
        );
    }
}
