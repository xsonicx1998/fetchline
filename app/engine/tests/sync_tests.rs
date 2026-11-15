//! Periodic synchronisation (S05 / F04) tests: a sync queue
//! re-probes each file on its schedule and re-downloads ONLY the
//! files whose remote size / ETag / Last-Modified changed.
//! Every test serves its own fixture on 127.0.0.1 (`MockServer` in
//! `common/mock.rs`) — no real website is ever contacted.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use mock::*;
use std::sync::Arc;
use std::time::Duration;

use fetchline_engine::timeutil::{now_unix, unix_to_iso};
use fetchline_engine::{DownloadManager, Store};

fn iso_in(seconds_from_now: i64) -> String {
    unix_to_iso(now_unix() + seconds_from_now)
}

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

/// Create a periodic queue with the given check interval (minutes).
async fn make_periodic(mgr: &Arc<DownloadManager>, name: &str, interval: i64) -> String {
    let queue = mgr.create_queue(name).unwrap();
    mgr.update_queue(&queue.id, "periodic", None, None, false, "none", None, Some(interval))
        .await
        .unwrap();
    queue.id
}

/// Add a file to a queue, download it to completion, and leave it in
/// the queue — that is the sync set: finished files that stay queued
/// to be re-probed.
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
            Some(queue_id.to_string()),
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(mgr, &id, "complete", 30).await);
    id
}

/// Wait until a queue's `last_sync_at` is set (a tick-spawned sync
/// finished), or the deadline passes.
async fn wait_for_sync(mgr: &DownloadManager, queue_id: &str, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        if let Ok(Some(q)) = mgr.get_queue(queue_id).await {
            if q.last_sync_at.is_some() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Wait until a queue has no files left (a run finished and the
/// runner removed them), or the deadline passes.
async fn wait_for_queue_empty(mgr: &DownloadManager, queue_id: &str, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        if let Ok(files) = mgr.list_queue_files(queue_id) {
            if files.is_empty() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Wait until a server has served more HEAD probes than `before`.
async fn wait_for_head_growth(server: &MockServer, before: usize, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        if server.head_requests() > before {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn sync_skips_unchanged_file_with_zero_get_requests() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    let row = mgr.get_download(&id).unwrap().unwrap();
    let before_bytes = std::fs::read(&row.save_path).unwrap();
    let before_mtime = std::fs::metadata(&row.save_path).unwrap().modified().unwrap();
    let gets_before = server.get_requests();
    let heads_before = server.head_requests();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.unchanged, 1);
    assert_eq!(result.changed, 0);
    assert_eq!(result.removed, 0);
    assert_eq!(result.failed, 0);
    assert_eq!(result.skipped, 0);
    assert_eq!(result.files[0].status, "unchanged");
    // Unchanged means skipped at the probe: HEAD only, zero GETs —
    // not a single byte was re-downloaded.
    assert_eq!(server.get_requests(), gets_before, "an unchanged file must not be re-downloaded");
    assert_eq!(server.head_requests(), heads_before + 1, "the sync must probe once per file");
    // The file on disk is untouched, and it stays in the queue.
    assert_eq!(std::fs::read(&row.save_path).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&row.save_path).unwrap().modified().unwrap(),
        before_mtime,
        "an unchanged file must not be rewritten"
    );
    assert_eq!(mgr.list_queue_files(&queue).unwrap().len(), 1);
    // The queue records the sync: last sync time + counts.
    let q = mgr.get_queue(&queue).await.unwrap().unwrap();
    assert!(q.last_sync_at.is_some());
    assert_eq!(q.last_sync_unchanged, 1);
    assert_eq!(q.last_sync_changed, 0);
}

#[tokio::test]
async fn sync_redownloads_changed_file_byte_identical() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    // The server's file changes (new size + new ETag) between syncs.
    server.bump_version();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.changed, 1);
    assert_eq!(result.unchanged, 0);
    assert_eq!(result.files[0].status, "changed");
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert_eq!(row.size_bytes, Some(513 * 1024));
    assert_eq!(row.etag.as_deref(), Some("\"v2\""));
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(513 * 1024));
    assert_eq!(mgr.list_queue_files(&queue).unwrap().len(), 1, "a re-downloaded file stays in its sync queue");
    let q = mgr.get_queue(&queue).await.unwrap().unwrap();
    assert_eq!(q.last_sync_changed, 1);
}

#[tokio::test]
async fn sync_redownloads_on_etag_only_change() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    // Same size, same bytes — only the validator changed. The file
    // still counts as changed: the stored copy can no longer be
    // proven to match the server.
    server.bump_etag();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.changed, 1, "an ETag-only change must re-download");
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert_eq!(row.size_bytes, Some(512 * 1024));
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn sync_reports_remote_deleted_file_and_keeps_the_local_copy() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    server.set_gone(true);

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.removed, 1);
    assert_eq!(result.changed, 0);
    assert_eq!(result.files[0].status, "removed");
    assert!(
        result.files[0].message.as_deref().unwrap_or("").contains("404"),
        "the report must say the file is gone: {:?}",
        result.files[0].message
    );
    // Reported, not silently dropped: the row and the local file stay.
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert!(std::path::Path::new(&row.save_path).exists());
    assert_eq!(mgr.list_queue_files(&queue).unwrap().len(), 1);
    let q = mgr.get_queue(&queue).await.unwrap().unwrap();
    assert_eq!(q.last_sync_removed, 1);
}

#[tokio::test]
async fn sync_does_not_redownload_everything() {
    let server_a = MockServer::start(MockConfig::default());
    let server_b = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id_a = add_completed(&mgr, &server_a.url, &dir, "a.bin", &queue).await;
    let id_b = add_completed(&mgr, &server_b.url, &dir, "b.bin", &queue).await;
    // Only B's file changes.
    server_b.bump_version();
    let gets_a_before = server_a.get_requests();
    let gets_b_before = server_b.get_requests();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.changed, 1);
    assert_eq!(result.unchanged, 1);
    assert_eq!(result.files.len(), 2);
    assert_eq!(server_a.get_requests(), gets_a_before, "the unchanged server must see no GET");
    assert!(server_b.get_requests() > gets_b_before, "the changed file must be re-downloaded");
    assert_eq!(
        std::fs::read(&mgr.get_download(&id_a).unwrap().unwrap().save_path).unwrap(),
        payload(512 * 1024)
    );
    assert_eq!(
        std::fs::read(&mgr.get_download(&id_b).unwrap().unwrap().save_path).unwrap(),
        payload(513 * 1024)
    );
}

#[tokio::test]
async fn sync_reports_failed_probe_and_retries_at_the_next_sync() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    server.bump_version();
    // The re-download's GETs now fail with 500 (the probe still
    // works, so sync sees the change and tries to fetch it).
    server.set_fail_get(true);

    let first = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(first.failed, 1);
    assert_eq!(first.files[0].status, "failed");
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().status, "error");
    // The old finished file is still on disk: a failed re-download
    // never destroys the copy the user already has.
    assert!(std::path::Path::new(&mgr.get_download(&id).unwrap().unwrap().save_path).exists());
    // The next sync retries the failed file — once per sync, not in
    // a tight loop — and this time it succeeds.
    server.set_fail_get(false);
    let second = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(second.changed, 1);
    assert_eq!(second.failed, 0);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(513 * 1024));
}

#[tokio::test]
async fn sync_reports_failed_probe_when_the_server_is_unreachable() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    // The server goes away entirely after the download.
    drop(server);
    // Give the listener a moment to close (the mock stops on drop).
    tokio::time::sleep(Duration::from_millis(200)).await;

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.failed, 1);
    assert_eq!(result.files[0].status, "failed");
    assert!(result.files[0].message.is_some());
    // The local copy and the row are untouched by a failed probe.
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn sync_skips_excluded_site_without_any_request() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let _id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    // The site is excluded after the download was made.
    let mut settings = mgr.get_settings().unwrap();
    settings.excluded_sites = vec!["127.0.0.1".to_string()];
    mgr.update_settings(&settings).unwrap();
    let heads_before = server.head_requests();
    let gets_before = server.get_requests();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.skipped, 1);
    assert_eq!(result.files[0].status, "skipped");
    assert!(
        result.files[0].message.as_deref().unwrap_or("").contains("excluded"),
        "the skip must say why: {:?}",
        result.files[0].message
    );
    assert_eq!(server.head_requests(), heads_before, "an excluded site must not even be probed");
    assert_eq!(server.get_requests(), gets_before);
}

#[tokio::test]
async fn sync_skips_a_file_that_has_not_finished_downloading() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    // Added to the sync queue but never started.
    let (id, _) = mgr
        .add_download(
            &server.url,
            Some("file.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            Some(queue.clone()),
        )
        .await
        .unwrap();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.skipped, 1);
    assert_eq!(result.files[0].status, "skipped");
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().status, "queued");
}

#[tokio::test]
async fn sync_redownloads_a_file_whose_local_copy_is_gone() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    let path = mgr.get_download(&id).unwrap().unwrap().save_path;
    std::fs::remove_file(&path).unwrap();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.changed, 1, "a missing local file must be fetched again");
    assert_eq!(std::fs::read(&path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn sync_refuses_a_one_time_queue_and_start_queue_refuses_a_periodic_queue() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    // A one-time queue cannot be synced …
    let one_time = mgr.create_queue("Once").unwrap();
    let err = mgr.run_sync_queue(&one_time.id).await.unwrap_err();
    assert!(err.to_string().contains("not a periodic"), "{err}");
    // … and a periodic queue cannot be drained as a one-time queue.
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let _id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    let err = mgr.start_queue(&queue).await.unwrap_err();
    assert!(err.to_string().contains("Sync now"), "{err}");
    // A finished download belongs in a periodic queue (that is what
    // it syncs), but the one-time guard from BUG-003 still holds:
    // a finished download cannot be added to a one-time queue.
    let (done, _) = mgr
        .add_download(
            &server.url,
            Some("other.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    mgr.start_download(&done).await.unwrap();
    assert!(wait_for_status(&mgr, &done, "complete", 30).await);
    mgr.add_to_queue(&done, &queue).unwrap();
    let err = mgr.add_to_queue(&done, &one_time.id).unwrap_err();
    assert!(err.to_string().contains("already finished"), "{err}");
}

#[tokio::test]
async fn sync_tick_runs_when_due_and_not_before_the_interval() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let _id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    // Never synced: the first tick runs a sync.
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    assert!(wait_for_sync(&mgr, &queue, 30).await);
    let heads_after_first = server.head_requests();
    assert!(heads_after_first > 0);
    // One minute later the interval (30 min) has not elapsed: the
    // tick must not sync again.
    mgr.scheduler_tick_at(now_unix() + 60).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        server.head_requests(),
        heads_after_first,
        "the tick must not re-sync before the interval elapses"
    );
    // 31 minutes later it is due again.
    mgr.scheduler_tick_at(now_unix() + 31 * 60).await.unwrap();
    assert!(wait_for_head_growth(&server, heads_after_first, 30).await);
}

#[tokio::test]
async fn sync_tick_respects_the_start_stop_window() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    // Start time in the future: no sync yet.
    let future = mgr.create_queue("Future").unwrap();
    mgr.update_queue(&future.id, "periodic", Some(&iso_in(3600)), None, false, "none", None, Some(5))
        .await
        .unwrap();
    let _id = add_completed(&mgr, &server.url, &dir, "f.bin", &future.id).await;
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(mgr.get_queue(&future.id).await.unwrap().unwrap().last_sync_at.is_none());
    // Window already passed: never syncs.
    let stale = mgr.create_queue("Stale").unwrap();
    mgr.update_queue(&stale.id, "periodic", Some(&iso_in(-3600)), Some(&iso_in(-60)), false, "none", None, Some(5))
        .await
        .unwrap();
    let _id2 = add_completed(&mgr, &server.url, &dir, "s.bin", &stale.id).await;
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(mgr.get_queue(&stale.id).await.unwrap().unwrap().last_sync_at.is_none());
    // Inside the window: syncs.
    let live = mgr.create_queue("Live").unwrap();
    mgr.update_queue(&live.id, "periodic", Some(&iso_in(-60)), Some(&iso_in(3600)), false, "none", None, Some(5))
        .await
        .unwrap();
    let _id3 = add_completed(&mgr, &server.url, &dir, "l.bin", &live.id).await;
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    assert!(wait_for_sync(&mgr, &live.id, 30).await);
}

#[tokio::test]
async fn sync_tick_does_not_fight_a_one_time_drain() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(50),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    // A periodic queue whose files are all complete, plus a one-time
    // queue draining slowly: the tick must run the sync without
    // touching the drain, and the drain must finish normally.
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let _id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    let drain = mgr.create_queue("Drain").unwrap();
    let (pending, _) = mgr
        .add_download(
            &server.url,
            Some("drain.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            Some(drain.id.clone()),
        )
        .await
        .unwrap();
    mgr.start_queue(&drain.id).await.unwrap();
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    assert!(wait_for_sync(&mgr, &queue, 30).await);
    assert!(wait_for_status(&mgr, &pending, "complete", 30).await);
    // BUG-039: "complete" and "removed from the queue" are two
    // separate steps — the download task flips the status, and only
    // the queue runner's 200 ms poll loop removes the file
    // afterwards. Asserting emptiness the instant the status flips
    // races the runner's next tick, so wait for the removal to land
    // (same pattern as the finished-file drain wait in
    // engine_tests.rs) before asserting.
    assert!(wait_for_queue_empty(&mgr, &drain.id, 30).await);
    assert!(mgr.list_queue_files(&drain.id).unwrap().is_empty(), "the one-time drain still empties its queue");
    assert_eq!(mgr.list_queue_files(&queue).unwrap().len(), 1, "the sync queue keeps its files");
}

#[tokio::test]
async fn sync_redownloads_on_last_modified_only_change() {
    // The missing fixture from the Round-6 note: a server that sends
    // Last-Modified and NO ETag. Same size, same bytes — only the
    // timestamp changed — so Last-Modified is the only signal that
    // can fire, and it must fire.
    let server = MockServer::start(MockConfig {
        no_etag: true,
        send_last_modified: true,
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.etag, None, "this fixture must send no ETag");
    assert_eq!(
        row.last_modified.as_deref(),
        Some("Wed, 01 Jan 2025 00:00:01 GMT"),
        "the probe must store the Last-Modified the server sent"
    );
    server.bump_last_modified();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.changed, 1, "a Last-Modified-only change must re-download");
    assert_eq!(result.unchanged, 0);
    assert_eq!(result.files[0].status, "changed");
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert_eq!(row.size_bytes, Some(512 * 1024));
    assert_eq!(
        row.last_modified.as_deref(),
        Some("Wed, 01 Jan 2025 00:00:02 GMT"),
        "the new Last-Modified must be stored"
    );
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn sync_skips_when_last_modified_unchanged() {
    // Same fixture, no change: same size, no ETag, same
    // Last-Modified — the sync must skip with zero GETs and leave
    // the file untouched.
    let server = MockServer::start(MockConfig {
        no_etag: true,
        send_last_modified: true,
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    let row = mgr.get_download(&id).unwrap().unwrap();
    let before_bytes = std::fs::read(&row.save_path).unwrap();
    let before_mtime = std::fs::metadata(&row.save_path).unwrap().modified().unwrap();
    let gets_before = server.get_requests();
    let heads_before = server.head_requests();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.unchanged, 1);
    assert_eq!(result.changed, 0);
    assert_eq!(result.files[0].status, "unchanged");
    assert_eq!(server.get_requests(), gets_before, "an unchanged file must not be re-downloaded");
    assert_eq!(server.head_requests(), heads_before + 1, "the sync must probe once per file");
    assert_eq!(std::fs::read(&row.save_path).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&row.save_path).unwrap().modified().unwrap(),
        before_mtime,
        "an unchanged file must not be rewritten"
    );
}

#[tokio::test]
async fn sync_redownloads_on_size_only_change() {
    // Size is the only signal: no ETag, no Last-Modified, and the
    // file grows by 1 KiB. (The existing growth test bumps the ETag
    // too, so it never isolated the size comparison.)
    let server = MockServer::start(MockConfig { no_etag: true, ..Default::default() });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().etag, None);
    server.bump_size();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.changed, 1, "a size-only change must re-download");
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert_eq!(row.size_bytes, Some(513 * 1024));
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(513 * 1024));
    assert_eq!(mgr.list_queue_files(&queue).unwrap().len(), 1);
}

#[tokio::test]
async fn sync_skips_when_size_only_unchanged() {
    // Same fixture, no change: the size comparison alone must be
    // enough to call the file unchanged — no ETag or Last-Modified
    // is needed to skip it.
    let server = MockServer::start(MockConfig { no_etag: true, ..Default::default() });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let _id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    let gets_before = server.get_requests();
    let heads_before = server.head_requests();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.unchanged, 1);
    assert_eq!(result.changed, 0);
    assert_eq!(server.get_requests(), gets_before, "an unchanged file must not be re-downloaded");
    assert_eq!(server.head_requests(), heads_before + 1);
}

#[tokio::test]
async fn sync_interval_boundaries_accept_5_min_and_1_week_reject_below_and_above() {
    // The interval is stored in whole minutes, so "4 minutes 59
    // seconds" has no representation in the API — 4 minutes is the
    // boundary probe for "below 5 minutes", and 10,081 minutes is
    // the probe for "over one week" (10,080 = 7 × 24 × 60).
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = mgr.create_queue("Bounds").unwrap();
    let save = |mgr: &Arc<DownloadManager>, id: &str, minutes: i64| {
        let mgr = Arc::clone(mgr);
        let id = id.to_string();
        async move {
            mgr.update_queue(&id, "periodic", None, None, false, "none", None, Some(minutes))
                .await
        }
    };
    let err = save(&mgr, &queue.id, 4).await.unwrap_err();
    assert!(err.to_string().contains("at least every 5 minutes"), "{err}");
    let row = save(&mgr, &queue.id, 5).await.unwrap();
    assert_eq!(row.sync_interval_minutes, Some(5));
    // A rejected save stores nothing: the interval is still 5.
    let err = save(&mgr, &queue.id, 10_081).await.unwrap_err();
    assert!(err.to_string().contains("maximum is 10080 minutes"), "{err}");
    assert_eq!(
        mgr.get_queue(&queue.id).await.unwrap().unwrap().sync_interval_minutes,
        Some(5),
        "a rejected interval must not overwrite the saved one"
    );
    let row = save(&mgr, &queue.id, 10_080).await.unwrap();
    assert_eq!(row.sync_interval_minutes, Some(10_080), "exactly one week is accepted");
}

#[tokio::test]
async fn sync_of_an_empty_queue_finishes_cleanly() {
    // A periodic queue with no files at all: the sync is a clean
    // no-op — zero counts, no files, no crash — and the last-sync
    // time is still recorded so the dialog can show it ran.
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.changed, 0);
    assert_eq!(result.unchanged, 0);
    assert_eq!(result.removed, 0);
    assert_eq!(result.failed, 0);
    assert_eq!(result.skipped, 0);
    assert!(result.files.is_empty());
    let q = mgr.get_queue(&queue).await.unwrap().unwrap();
    assert!(q.last_sync_at.is_some(), "even an empty sync records when it ran");
    assert_eq!(q.last_sync_changed, 0);
    assert_eq!(q.last_sync_unchanged, 0);
}

#[tokio::test]
async fn sync_skips_a_file_that_is_mid_download_and_leaves_it_byte_identical() {
    // Mid-download sync choice: SKIP, do not touch it. A sync must
    // never reset, pause or rewrite a file that is downloading —
    // the download owns its segments and its `.part` file until it
    // finishes, and the sync re-checks the file at the next run.
    // (A second guard lives in `redownload_changed_file`, which
    // re-reads the row and refuses a file that started downloading
    // while the sync was already running.)
    let server = MockServer::start(MockConfig {
        size_bytes: Some(2 * 1024 * 1024),
        chunk: 8192,
        delay: Duration::from_millis(100),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    // A 2 MiB file at 8 KiB / 100 ms per chunk takes ~6 s per
    // segment, so the download is still running when the sync runs.
    let (id, _) = mgr
        .add_download(
            &server.url,
            Some("slow.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            Some(queue.clone()),
        )
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "downloading", 10).await);
    let heads_before = server.head_requests();

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.skipped, 1, "a mid-download file must be skipped, not re-downloaded");
    assert_eq!(result.changed, 0);
    assert_eq!(result.failed, 0);
    assert_eq!(result.files[0].status, "skipped");
    assert!(
        result.files[0].message.as_deref().unwrap_or("").contains("not finished"),
        "the skip must say why: {:?}",
        result.files[0].message
    );
    // The sync did not even probe it, and the download is untouched.
    assert_eq!(server.head_requests(), heads_before, "a mid-download file must not be probed");
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().status, "downloading");
    // The download then finishes normally, byte-identical — the
    // sync did not corrupt it.
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(2 * 1024 * 1024));
    assert!(
        !std::path::Path::new(&format!("{}.part", row.save_path)).exists(),
        "no .part file may be left behind"
    );
    assert_eq!(mgr.list_queue_files(&queue).unwrap().len(), 1, "the file stays in its sync queue");
}

#[tokio::test]
async fn sync_redownloads_a_file_that_shrank_byte_identical() {
    // The remote file got smaller (512 KiB -> 256 KiB): the sync
    // must re-download the smaller file over the same path, not
    // keep the longer local copy and not truncate it to garbage.
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let queue = make_periodic(&mgr, "Sync", 30).await;
    let id = add_completed(&mgr, &server.url, &dir, "file.bin", &queue).await;
    server.shrink_to(256 * 1024);

    let result = mgr.run_sync_queue(&queue).await.unwrap();
    assert_eq!(result.changed, 1, "a shrunk remote file must re-download");
    assert_eq!(result.unchanged, 0);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
    assert_eq!(row.size_bytes, Some(256 * 1024));
    let bytes = std::fs::read(&row.save_path).unwrap();
    assert_eq!(bytes.len(), 256 * 1024, "the local file must be exactly the smaller file");
    assert_eq!(bytes, payload(256 * 1024));
    assert_eq!(mgr.list_queue_files(&queue).unwrap().len(), 1);
}

#[tokio::test]
async fn pre_sync_database_migrates_queues_with_sync_columns() {
    // A database made before S05/F04 has a queues table without the
    // sync columns. Opening it must add them (Store::migrate) without
    // losing the queue, and the queue must then accept a periodic
    // schedule.
    let dir = tmpdir();
    let path = dir.join("old.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE queues (
               id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE,
               type TEXT NOT NULL DEFAULT 'download',
               is_main INTEGER NOT NULL DEFAULT 0,
               schedule_type TEXT NOT NULL DEFAULT 'one_time',
               start_at TEXT, stop_at TEXT,
               start_on_startup INTEGER NOT NULL DEFAULT 0,
               on_complete_action TEXT NOT NULL DEFAULT 'none',
               on_complete_program TEXT,
               download_limit_mb_per_hours TEXT,
               created_at TEXT NOT NULL DEFAULT (datetime('now')),
               updated_at TEXT NOT NULL DEFAULT (datetime('now')));
             INSERT INTO queues (id, name, type, is_main, schedule_type)
               VALUES ('q-old', 'Old queue', 'download', 0, 'one_time');",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let queues = store.list_queues().unwrap();
    // Two rows: the seeded main queue plus the pre-sync one.
    let old = queues.iter().find(|q| q.id == "q-old").expect("the old queue must survive the migration");
    assert_eq!(old.name, "Old queue");
    assert_eq!(old.sync_interval_minutes, None);
    assert_eq!(old.last_sync_at, None);
    let mgr = DownloadManager::new(Arc::new(store), 4);
    mgr.update_queue("q-old", "periodic", None, None, false, "none", None, Some(60))
        .await
        .unwrap();
    let row = mgr.get_queue("q-old").await.unwrap().unwrap();
    assert_eq!(row.schedule_type, "periodic");
    assert_eq!(row.sync_interval_minutes, Some(60));
}
