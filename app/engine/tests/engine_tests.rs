//! Engine tests against a local mock HTTP server (std TcpListener).
//! No public internet is used. The server serves a deterministic payload
//! and honours HEAD, GET and Range requests, optionally throttled so a
//! pause can land mid-download.

#![allow(dead_code)] // the shared mock also serves grabber/proxy/sync tests this file does not use

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;
use std::time::Duration;

use mock::*;

use fetchline_engine::{probe, DownloadManager, Store};

#[tokio::test]
async fn probe_returns_size_ranges_and_etag() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let client = reqwest::Client::new();
    let info = probe(&client, &server.url).await.unwrap();
    assert_eq!(info.size, Some(512 * 1024));
    assert!(info.accept_ranges);
    assert_eq!(info.etag.as_deref(), Some("\"v1\""));
}

#[tokio::test]
async fn probe_falls_back_when_head_hides_length() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: true, ..Default::default() });
    let client = reqwest::Client::new();
    // HEAD without Content-Length: size is None, download still works as one segment.
    let info = probe(&client, &server.url).await.unwrap();
    assert_eq!(info.size, None);
}

#[tokio::test]
async fn full_download_multi_segment_matches_bytes() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
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
    let bytes = std::fs::read(&row.save_path).unwrap();
    assert_eq!(bytes, payload(512 * 1024));
    assert!(!std::path::Path::new(&format!("{}.part", row.save_path)).exists());
    // Segments were created and all finished.
    assert!(!store.get_segments(&id).unwrap().is_empty());
    assert!(store
        .get_segments(&id)
        .unwrap()
        .iter()
        .all(|s| s.status == "done"));
}

#[tokio::test]
async fn pause_then_resume_completes_with_correct_bytes() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(100),
        hide_length: false,
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
    // Let it get part-way, then pause.
    tokio::time::sleep(Duration::from_millis(600)).await;
    mgr.pause_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "paused", 10).await);
    let mid = mgr.get_download(&id).unwrap().unwrap();
    assert!(mid.downloaded_bytes > 0, "pause must land mid-download");
    assert!(mid.downloaded_bytes < mid.size_bytes.unwrap());
    // Resume from the persisted progress and finish.
    mgr.resume_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn resume_restarts_when_remote_file_changes() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(100),
        hide_length: false,
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
    tokio::time::sleep(Duration::from_millis(400)).await;
    mgr.pause_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "paused", 10).await);
    // The server's file changes (new size + new ETag) while paused.
    server.bump_version();
    mgr.resume_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.size_bytes, Some(513 * 1024));
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(513 * 1024));
}

#[tokio::test]
async fn schema_loads_and_seeds_categories() {
    let store = Store::open_in_memory().unwrap();
    let cats = store.list_categories().unwrap();
    assert_eq!(cats.len(), 5);
    assert!(cats.iter().any(|c| c.id == "cat-video"));
    assert_eq!(store.category_for_extension("mp4").unwrap(), "cat-video");
    assert_eq!(store.category_for_extension("zip").unwrap(), "cat-compressed");
}

// -- milestone 2: queues, scheduler, categories ---------------------------

use fetchline_engine::timeutil::{now_unix, unix_to_iso};

fn iso_in(seconds_from_now: i64) -> String {
    unix_to_iso(now_unix() + seconds_from_now)
}

async fn wait_for_queue_drained(mgr: &DownloadManager, queue_id: &str, secs: u64) -> bool {
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

async fn wait_for_queue_running(mgr: &DownloadManager, queue_id: &str, want: bool, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        if let Ok(queues) = mgr.list_queues().await {
            if let Some(q) = queues.iter().find(|q| q.id == queue_id) {
                if q.running == want {
                    return true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Add one download (same mock file, distinct name) to a queue.
async fn add_queued(mgr: &Arc<DownloadManager>, server_url: &str, dir: &std::path::Path, name: &str, queue_id: &str) -> String {
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
    id
}

#[tokio::test]
async fn queue_drains_in_order_and_clears_queue_id() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Evening").unwrap();
    let first = add_queued(&mgr, &server.url, &dir, "one.bin", &queue.id).await;
    let second = add_queued(&mgr, &server.url, &dir, "two.bin", &queue.id).await;
    let third = add_queued(&mgr, &server.url, &dir, "three.bin", &queue.id).await;
    assert_eq!(mgr.list_queue_files(&queue.id).unwrap().len(), 3);

    let mut rx = mgr.subscribe();
    mgr.start_queue(&queue.id).await.unwrap();
    // Collect the three "complete" events in the order they arrive.
    let mut completed: Vec<String> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while completed.len() < 3 && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(event)) if event.status == "complete" => completed.push(event.id),
            _ => {}
        }
    }
    assert_eq!(completed, vec![first.clone(), second.clone(), third.clone()], "queue must drain in insertion order");
    assert!(wait_for_queue_drained(&mgr, &queue.id, 10).await);
    // Finished files leave the queue and land on disk, byte-exact.
    for (id, _) in [(first, "one.bin"), (second, "two.bin"), (third, "three.bin")] {
        let row = mgr.get_download(&id).unwrap().unwrap();
        assert_eq!(row.status, "complete");
        assert_eq!(row.queue_id, None);
        assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    }
}

#[tokio::test]
async fn queue_stop_leaves_remaining_files_queued() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(100),
        hide_length: false,
    ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Slow").unwrap();
    let first = add_queued(&mgr, &server.url, &dir, "one.bin", &queue.id).await;
    let second = add_queued(&mgr, &server.url, &dir, "two.bin", &queue.id).await;
    let third = add_queued(&mgr, &server.url, &dir, "three.bin", &queue.id).await;
    mgr.start_queue(&queue.id).await.unwrap();
    // Stop while the first file is still downloading.
    tokio::time::sleep(Duration::from_millis(500)).await;
    mgr.stop_queue(&queue.id).await.unwrap();
    assert!(wait_for_queue_running(&mgr, &queue.id, false, 10).await);
    // The current file paused (resumable); the rest never started.
    let one = mgr.get_download(&first).unwrap().unwrap();
    assert_eq!(one.status, "paused");
    assert!(one.downloaded_bytes > 0);
    for id in [second, third] {
        let row = mgr.get_download(&id).unwrap().unwrap();
        assert_eq!(row.status, "queued");
        assert_eq!(row.queue_id.as_deref(), Some(queue.id.as_str()));
    }
    // Starting again resumes the paused file and drains the rest.
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_queue_drained(&mgr, &queue.id, 60).await);
    assert_eq!(std::fs::read(&mgr.get_download(&first).unwrap().unwrap().save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn scheduler_starts_queue_at_start_time() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    // Start time in the past: the tick starts it.
    let due = mgr.create_queue("Due").unwrap();
    add_queued(&mgr, &server.url, &dir, "due.bin", &due.id).await;
    mgr.update_queue(&due.id, "one_time", Some(&iso_in(-60)), None, false, "none", None, None)
        .await
        .unwrap();
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    assert!(wait_for_queue_running(&mgr, &due.id, true, 5).await);
    assert!(wait_for_queue_drained(&mgr, &due.id, 30).await);
    // Start time in the future: the tick leaves it alone.
    let later = mgr.create_queue("Later").unwrap();
    add_queued(&mgr, &server.url, &dir, "later.bin", &later.id).await;
    mgr.update_queue(&later.id, "one_time", Some(&iso_in(3600)), None, false, "none", None, None)
        .await
        .unwrap();
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let row = mgr.get_queue(&later.id).await.unwrap().unwrap();
    assert!(!row.running);
    assert_eq!(mgr.get_download(&mgr.list_queue_files(&later.id).unwrap()[0].id).unwrap().unwrap().status, "queued");
}

#[tokio::test]
async fn scheduler_respects_stop_time() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(100),
        hide_length: false,
    ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    // Stop time already passed: never starts.
    let stale = mgr.create_queue("Stale").unwrap();
    add_queued(&mgr, &server.url, &dir, "stale.bin", &stale.id).await;
    mgr.update_queue(&stale.id, "one_time", Some(&iso_in(-3600)), Some(&iso_in(-60)), false, "none", None, None)
        .await
        .unwrap();
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!mgr.get_queue(&stale.id).await.unwrap().unwrap().running);
    // Stop time arrives mid-run: the current file pauses (resumable),
    // the rest stays queued and untouched.
    let timed = mgr.create_queue("Timed").unwrap();
    let first = add_queued(&mgr, &server.url, &dir, "one.bin", &timed.id).await;
    let second = add_queued(&mgr, &server.url, &dir, "two.bin", &timed.id).await;
    mgr.update_queue(&timed.id, "one_time", Some(&iso_in(-60)), Some(&iso_in(1)), false, "none", None, None)
        .await
        .unwrap();
    mgr.start_queue(&timed.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    mgr.scheduler_tick_at(now_unix() + 2).await.unwrap();
    assert!(wait_for_queue_running(&mgr, &timed.id, false, 10).await);
    let first_row = mgr.get_download(&first).unwrap().unwrap();
    assert_eq!(first_row.status, "paused");
    assert!(first_row.downloaded_bytes > 0, "the file must have made progress before stopping");
    let row = mgr.get_download(&second).unwrap().unwrap();
    assert_eq!(row.status, "queued");
    assert_eq!(row.queue_id.as_deref(), Some(timed.id.as_str()));
}

#[tokio::test]
async fn scheduler_one_time_queue_does_not_restart() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Once").unwrap();
    add_queued(&mgr, &server.url, &dir, "one.bin", &queue.id).await;
    mgr.update_queue(&queue.id, "one_time", Some(&iso_in(-60)), None, false, "none", None, None)
        .await
        .unwrap();
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    assert!(wait_for_queue_drained(&mgr, &queue.id, 30).await);
    // A file added afterwards must not be auto-started by later ticks.
    let extra = add_queued(&mgr, &server.url, &dir, "two.bin", &queue.id).await;
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!mgr.get_queue(&queue.id).await.unwrap().unwrap().running);
    assert_eq!(mgr.get_download(&extra).unwrap().unwrap().status, "queued");
}

#[tokio::test]
async fn queue_reorder_changes_order() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Order").unwrap();
    let a = add_queued(&mgr, &server.url, &dir, "a.bin", &queue.id).await;
    let b = add_queued(&mgr, &server.url, &dir, "b.bin", &queue.id).await;
    let c = add_queued(&mgr, &server.url, &dir, "c.bin", &queue.id).await;
    mgr.reorder_queue(&queue.id, &c, 0).unwrap();
    let ids: Vec<String> = mgr
        .list_queue_files(&queue.id)
        .unwrap()
        .iter()
        .map(|d| d.id.clone())
        .collect();
    assert_eq!(ids, vec![c.clone(), a.clone(), b.clone()]);
    mgr.reorder_queue(&queue.id, &a, 2).unwrap();
    let ids: Vec<String> = mgr
        .list_queue_files(&queue.id)
        .unwrap()
        .iter()
        .map(|d| d.id.clone())
        .collect();
    assert_eq!(ids, vec![c.clone(), b.clone(), a.clone()]);
}

#[tokio::test]
async fn category_auto_assign_uses_custom_category() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let clips = mgr
        .create_category("Clips", dir.to_string_lossy().as_ref(), &["xyz".to_string()])
        .unwrap();
    let (id, _) = mgr
        .add_download(&server.url, Some("clip.xyz".to_string()), Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.category_id, clips.id);
}

#[tokio::test]
async fn category_delete_blocked_with_downloads_then_allowed() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let clips = mgr
        .create_category("Clips", dir.to_string_lossy().as_ref(), &["xyz".to_string()])
        .unwrap();
    let (id, _) = mgr
        .add_download(&server.url, Some("clip.xyz".to_string()), Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let err = mgr.delete_category(&clips.id).unwrap_err();
    assert!(err.to_string().contains("move them"), "blocked delete must say why: {err}");
    mgr.move_to_category(&id, "cat-documents").unwrap();
    mgr.delete_category(&clips.id).unwrap();
    assert!(!mgr.store().list_categories().unwrap().iter().any(|c| c.id == clips.id));
    // Built-in categories cannot be deleted at all.
    let err = mgr.delete_category("cat-video").unwrap_err();
    assert!(err.to_string().contains("built-in"), "builtin delete must say why: {err}");
}

/// S05/F04: periodic synchronisation is built now — a periodic
/// schedule saves with a valid interval, and the old "not built yet"
/// rejection is gone (this test replaces
/// `update_queue_rejects_periodic_sync`).
#[tokio::test]
async fn update_queue_accepts_periodic_sync_with_valid_interval() {
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    let queue = mgr.create_queue("Sync").unwrap();
    let saved = mgr
        .update_queue(&queue.id, "periodic", None, None, false, "none", None, Some(30))
        .await
        .unwrap();
    assert_eq!(saved.schedule_type, "periodic");
    assert_eq!(saved.sync_interval_minutes, Some(30));
}

/// S05/F04: a periodic queue without an interval, or with an
/// interval under 5 minutes, is rejected at save time with a clear
/// error — nothing is stored.
#[tokio::test]
async fn update_queue_rejects_periodic_sync_without_interval_or_too_often() {
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    let queue = mgr.create_queue("Sync").unwrap();
    let err = mgr
        .update_queue(&queue.id, "periodic", None, None, false, "none", None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("how often"), "{err}");
    for bad in [0, 1, 4, -30] {
        let err = mgr
            .update_queue(&queue.id, "periodic", None, None, false, "none", None, Some(bad))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("at least every 5 minutes"),
            "interval {bad}: {err}"
        );
    }
    // Nothing was stored: the queue is still unscheduled.
    let row = mgr.get_queue(&queue.id).await.unwrap().unwrap();
    assert_eq!(row.schedule_type, "one_time");
    assert_eq!(row.sync_interval_minutes, None);
    // The 5-minute minimum itself is accepted.
    mgr.update_queue(&queue.id, "periodic", None, None, false, "none", None, Some(5))
        .await
        .unwrap();
}

/// S05/F04: exclusivity — a one-time queue must not carry a check
/// interval, and switching a queue back to one-time clears the
/// interval it had as a periodic queue.
#[tokio::test]
async fn update_queue_rejects_interval_on_one_time_queue() {
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    let queue = mgr.create_queue("Sync").unwrap();
    let err = mgr
        .update_queue(&queue.id, "one_time", None, None, false, "none", None, Some(30))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("only applies to periodic"), "{err}");
    mgr.update_queue(&queue.id, "periodic", None, None, false, "none", None, Some(30))
        .await
        .unwrap();
    let back = mgr
        .update_queue(&queue.id, "one_time", Some(&iso_in(-60)), None, false, "none", None, None)
        .await
        .unwrap();
    assert_eq!(back.schedule_type, "one_time");
    assert_eq!(back.sync_interval_minutes, None);
}

#[tokio::test]
async fn queue_action_run_program_executes_and_dismiss_clears() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    // A script that proves it ran by writing a marker file.
    // Windows runs .bat files directly (CreateProcess handles
    // them); Unix runs a .sh with +x.
    let marker = dir.join("ran.txt");
    #[cfg(windows)]
    let script = dir.join("on_done.bat");
    #[cfg(not(windows))]
    let script = dir.join("on_done.sh");
    #[cfg(windows)]
    std::fs::write(&script, format!("@echo done > \"{}\"\n", marker.display())).unwrap();
    #[cfg(not(windows))]
    std::fs::write(&script, format!("#!/bin/sh\necho done > {}\n", marker.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let queue = mgr.create_queue("Scripted").unwrap();
    mgr.update_queue(
        &queue.id,
        "one_time",
        None,
        None,
        false,
        "run_program",
        Some(script.to_string_lossy().as_ref()), None)
    .await
    .unwrap();
    add_queued(&mgr, &server.url, &dir, "one.bin", &queue.id).await;
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_queue_drained(&mgr, &queue.id, 30).await);
    let pending = mgr.pending_action(&queue.id).await.expect("action must be pending");
    assert_eq!(pending.action, "run_program");
    mgr.execute_queue_action(&queue.id).await.unwrap();
    for _ in 0..50 {
        if marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(marker.exists(), "the program must have run");
    assert!(mgr.pending_action(&queue.id).await.is_none());
    // A second queue, dismissed instead of run: nothing happens.
    let quiet = mgr.create_queue("Quiet").unwrap();
    mgr.update_queue(&quiet.id, "one_time", None, None, false, "shutdown", None, None)
        .await
        .unwrap();
    add_queued(&mgr, &server.url, &dir, "two.bin", &quiet.id).await;
    mgr.start_queue(&quiet.id).await.unwrap();
    assert!(wait_for_queue_drained(&mgr, &quiet.id, 30).await);
    assert!(mgr.pending_action(&quiet.id).await.is_some());
    mgr.dismiss_queue_action(&quiet.id).await.unwrap();
    assert!(mgr.pending_action(&quiet.id).await.is_none());
}

/// The shutdown action refuses off Windows — and it must never
/// run on Windows in a test: there the action really executes
/// `shutdown /s /t 0` and powers the machine off (BUG-033: an
/// ungated run of this test stopped the AWS VPS mid-suite).
/// Linux-only by nature; Windows has no equivalent assertion.
#[tokio::test]
#[cfg(not(target_os = "windows"))]
async fn queue_action_shutdown_errors_off_windows() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Power").unwrap();
    mgr.update_queue(&queue.id, "one_time", None, None, false, "shutdown", None, None)
        .await
        .unwrap();
    add_queued(&mgr, &server.url, &dir, "one.bin", &queue.id).await;
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_queue_drained(&mgr, &queue.id, 30).await);
    assert!(mgr.pending_action(&queue.id).await.is_some());
    // On this Linux VM the shutdown action must refuse, never run.
    let err = mgr.execute_queue_action(&queue.id).await.unwrap_err();
    assert!(err.to_string().contains("only available on Windows"), "{err}");
}




// -- replica-test: edge cases (failing-test-first) ------------------------
// Areas: F02 add URL, F03 pause/resume, F10 categories, F04 queues,
// F04 scheduler + on-completion action. Local mock server only.

fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn complete_one(mgr: &Arc<DownloadManager>, server_url: &str, dir: &std::path::Path, name: &str) -> String {
    let (id, _) = mgr
        .add_download(server_url, Some(name.to_string()), Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(mgr, &id, "complete", 30).await);
    id
}

// -- F02: add URL -----------------------------------------------------------

#[tokio::test]
async fn add_download_rejects_empty_url() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let err = mgr
        .add_download("", None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Invalid address"), "{err}");
}

#[tokio::test]
async fn add_download_rejects_unsupported_scheme() {
    // FTP is a supported scheme since the S09 finish (FTP
    // transport), so this now probes a scheme with no transport
    // at all instead of ftp://.
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let err = mgr
        .add_download("gopher://example.com/file.zip", None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Only http"), "{err}");
}

#[tokio::test]
async fn add_download_rejects_unparsable_url() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let err = mgr
        .add_download("not a url", None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Invalid address"), "{err}");
}

#[tokio::test]
async fn add_download_rejects_404() {
    let server = MockServer::start(MockConfig { fail_status: Some(404), ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let err = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404"), "{err}");
}

#[tokio::test]
async fn add_download_unreachable_host_errors() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let url = format!("http://127.0.0.1:{}/file.bin", closed_port());
    let err = mgr
        .add_download(&url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Network error"), "{err}");
}

#[tokio::test]
async fn duplicate_add_gets_different_save_path() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (first, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let (second, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let a = mgr.get_download(&first).unwrap().unwrap();
    let b = mgr.get_download(&second).unwrap().unwrap();
    assert_ne!(a.save_path, b.save_path, "two adds of the same link must not share one file");
    assert!(b.save_path.ends_with(" (2).bin"), "second add should be suffixed: {}", b.save_path);
}

#[tokio::test]
async fn second_add_after_first_completes_gets_suffixed_path() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    complete_one(&mgr, &server.url, &dir, "file.bin").await;
    let (second, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let b = mgr.get_download(&second).unwrap().unwrap();
    assert!(b.save_path.ends_with(" (2).bin"), "second add should be suffixed: {}", b.save_path);
}

#[tokio::test]
async fn start_and_resume_on_complete_error() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let id = complete_one(&mgr, &server.url, &dir, "file.bin").await;
    assert!(mgr.start_download(&id).await.unwrap_err().to_string().contains("already complete"));
    assert!(mgr.resume_download(&id).await.unwrap_err().to_string().contains("already complete"));
}

#[tokio::test]
async fn pause_download_when_not_running_errors() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert!(mgr.pause_download(&id).await.unwrap_err().to_string().contains("not running"));
}

// -- F03: pause / resume ----------------------------------------------------

#[tokio::test]
async fn resume_restarts_when_only_etag_changes() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(100),
        hide_length: false,
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
    tokio::time::sleep(Duration::from_millis(400)).await;
    mgr.pause_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "paused", 10).await);
    // Same size, same bytes — only the validator changes.
    server.bump_etag();
    mgr.resume_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.size_bytes, Some(512 * 1024));
    assert_eq!(row.etag.as_deref(), Some("\"v2\""), "resume must have re-probed and stored the new validator");
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn start_download_on_paused_download_resumes() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(100),
        hide_length: false,
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
    tokio::time::sleep(Duration::from_millis(600)).await;
    mgr.pause_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "paused", 10).await);
    let mid = mgr.get_download(&id).unwrap().unwrap();
    assert!(mid.downloaded_bytes > 0 && mid.downloaded_bytes < mid.size_bytes.unwrap());
    // Starting a paused download directly (not via resume) continues it.
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

// -- F10: categories --------------------------------------------------------

#[tokio::test]
async fn category_auto_assign_unknown_extension_falls_back_to_documents() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(&server.url, Some("notes.qqq".to_string()), Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().category_id, "cat-documents");
}

#[tokio::test]
async fn category_auto_assign_is_case_insensitive() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(&server.url, Some("TRAILER.MP4".to_string()), Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().category_id, "cat-video");
}

#[tokio::test]
async fn create_category_validation_errors() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.create_category("", "/tmp", &["xyz".to_string()]).is_err());
    assert!(store.create_category("   ", "/tmp", &["xyz".to_string()]).is_err());
    assert!(store.create_category("Clips", "", &["xyz".to_string()]).is_err());
    assert!(store.create_category("Clips", "/tmp", &[]).is_err());
    assert!(store.create_category("Clips", "/tmp", &["  ".to_string(), ".".to_string()]).is_err());
    store.create_category("Clips", "/tmp", &["xyz".to_string()]).unwrap();
    let err = store.create_category("Clips", "/tmp", &["abc".to_string()]).unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
}

#[tokio::test]
async fn move_to_category_unknown_ids_error() {
    let store = Store::open_in_memory().unwrap();
    let id = store
        .create_download("http://127.0.0.1/x.bin", None, "x.bin", "/tmp/x.bin", "cat-video", None, None, None)
        .unwrap();
    assert!(store.move_to_category("no-such-download", "cat-video").is_err());
    let err = store.move_to_category(&id, "no-such-category").unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}

#[tokio::test]
async fn delete_category_unknown_id_errors() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.delete_category("no-such-category").is_err());
}

#[tokio::test]
async fn delete_category_allowed_when_downloads_deleted() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let clips = mgr
        .create_category("Clips", dir.to_string_lossy().as_ref(), &["xyz".to_string()])
        .unwrap();
    let (id, _) = mgr
        .add_download(&server.url, Some("clip.xyz".to_string()), Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert!(mgr.delete_category(&clips.id).is_err());
    mgr.store().set_status(&id, "deleted", None).unwrap();
    mgr.delete_category(&clips.id).unwrap();
}

// -- F04: queues ------------------------------------------------------------

#[tokio::test]
async fn queue_state_errors() {
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    let queue = mgr.create_queue("Q").unwrap();
    assert!(mgr.start_queue(&queue.id).await.unwrap_err().to_string().contains("no files"));
    assert!(mgr.stop_queue(&queue.id).await.unwrap_err().to_string().contains("not running"));
    assert!(mgr.add_to_queue("no-such-download", &queue.id).is_err());
    assert!(mgr.add_to_queue("no-such-download", "no-such-queue").is_err());
    assert!(mgr.reorder_queue(&queue.id, "no-such-download", 0).is_err());
    assert!(mgr.remove_from_queue("no-such-download").is_err());
    // Empty and whitespace-only queue names are refused; duplicates too.
    assert!(mgr.create_queue("").is_err());
    assert!(mgr.create_queue("   ").is_err());
    assert!(mgr.create_queue("Q").is_err());
}

#[tokio::test]
async fn start_queue_twice_errors() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(100),
        hide_length: false,
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Slow").unwrap();
    add_queued(&mgr, &server.url, &dir, "one.bin", &queue.id).await;
    add_queued(&mgr, &server.url, &dir, "two.bin", &queue.id).await;
    mgr.start_queue(&queue.id).await.unwrap();
    let err = mgr.start_queue(&queue.id).await.unwrap_err();
    assert!(err.to_string().contains("already running"), "{err}");
    mgr.stop_queue(&queue.id).await.unwrap();
    assert!(wait_for_queue_running(&mgr, &queue.id, false, 10).await);
}

#[tokio::test]
async fn reorder_single_item_and_out_of_range_clamps() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Order").unwrap();
    let a = add_queued(&mgr, &server.url, &dir, "a.bin", &queue.id).await;
    mgr.reorder_queue(&queue.id, &a, 99).unwrap();
    let ids: Vec<String> = mgr.list_queue_files(&queue.id).unwrap().iter().map(|d| d.id.clone()).collect();
    assert_eq!(ids, vec![a.clone()]);
    let b = add_queued(&mgr, &server.url, &dir, "b.bin", &queue.id).await;
    let c = add_queued(&mgr, &server.url, &dir, "c.bin", &queue.id).await;
    mgr.reorder_queue(&queue.id, &a, 99).unwrap();
    let ids: Vec<String> = mgr.list_queue_files(&queue.id).unwrap().iter().map(|d| d.id.clone()).collect();
    assert_eq!(ids, vec![b, c, a]);
}

#[tokio::test]
async fn add_to_queue_rejects_completed_download() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let id = complete_one(&mgr, &server.url, &dir, "file.bin").await;
    let queue = mgr.create_queue("Done").unwrap();
    let err = mgr.add_to_queue(&id, &queue.id).unwrap_err();
    assert!(err.to_string().contains("already finished"), "{err}");
}

#[tokio::test]
async fn queue_with_only_completed_files_registers_no_action_and_drops_them() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Done").unwrap();
    mgr.update_queue(&queue.id, "one_time", None, None, false, "shutdown", None, None)
        .await
        .unwrap();
    // The file is queued first, then finished by a direct start: the
    // finished file stays filed in the queue (only the queue runner
    // unqueues files) — the state a finished row dragged onto a queue
    // used to produce.
    let id = add_queued(&mgr, &server.url, &dir, "one.bin", &queue.id).await;
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert_eq!(mgr.list_queue_files(&queue.id).unwrap().len(), 1);
    mgr.start_queue(&queue.id).await.unwrap();
    // Wait until the run has settled: the finished file must be gone.
    let mut drained = false;
    for _ in 0..50 {
        if mgr.list_queue_files(&queue.id).unwrap().is_empty() {
            drained = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(drained, "a finished file must leave the queue when the queue runs");
    assert!(
        mgr.pending_action(&queue.id).await.is_none(),
        "a queue that downloaded nothing must not arm its completion action"
    );
    assert!(!mgr.get_queue(&queue.id).await.unwrap().unwrap().running);
}

// -- F04: scheduler + on-completion action ----------------------------------

#[tokio::test]
async fn update_queue_accepts_none_and_clears_schedule() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Evening").unwrap();
    add_queued(&mgr, &server.url, &dir, "one.bin", &queue.id).await;
    mgr.update_queue(&queue.id, "one_time", Some(&iso_in(-60)), Some(&iso_in(3600)), false, "none", None, None)
        .await
        .unwrap();
    // "No schedule" must save: it clears the times, it is not an error.
    let row = mgr
        .update_queue(&queue.id, "none", None, None, false, "none", None, None)
        .await
        .unwrap();
    assert_eq!(row.start_at, None);
    assert_eq!(row.stop_at, None);
    // The cleared queue must not start on a tick, even though its old
    // start time has passed.
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!mgr.get_queue(&queue.id).await.unwrap().unwrap().running);
    assert_eq!(mgr.get_download(&mgr.list_queue_files(&queue.id).unwrap()[0].id).unwrap().unwrap().status, "queued");
}

#[tokio::test]
async fn scheduler_tick_with_no_queues_is_noop() {
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    // Only the seeded main queue exists; the tick must not error or start it.
    assert_eq!(mgr.list_queues().await.unwrap().len(), 1);
    assert!(!mgr.list_queues().await.unwrap()[0].running);
}

#[tokio::test]
async fn start_startup_queues_skips_empty_queues() {
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    let queue = mgr.create_queue("Empty").unwrap();
    mgr.update_queue(&queue.id, "one_time", None, None, true, "none", None, None)
        .await
        .unwrap();
    mgr.start_startup_queues().await.unwrap();
    assert!(!mgr.get_queue(&queue.id).await.unwrap().unwrap().running);
}

#[tokio::test]
async fn execute_and_dismiss_without_pending() {
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    let queue = mgr.create_queue("Q").unwrap();
    assert!(mgr.execute_queue_action(&queue.id).await.is_err());
    assert!(mgr.dismiss_queue_action(&queue.id).await.is_ok());
    assert!(mgr.pending_action(&queue.id).await.is_none());
}

#[tokio::test]
async fn update_queue_rejects_malformed_start_time() {
    // BUG-005: a malformed start time must be rejected at save time,
    // not stored and silently never run. (Converts the round-1 DOC
    // test `update_queue_malformed_start_time_is_stored_but_never_runs`.)
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    let queue = mgr.create_queue("Broken").unwrap();
    let err = mgr
        .update_queue(&queue.id, "one_time", Some("tomorrow at noon"), None, false, "none", None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("start time"), "unexpected error: {err}");
    // Nothing was stored: the queue is still unscheduled.
    let row = mgr.get_queue(&queue.id).await.unwrap().unwrap();
    assert_eq!(row.start_at, None);
}

#[tokio::test]
async fn update_queue_rejects_malformed_stop_time() {
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    let queue = mgr.create_queue("Broken").unwrap();
    let err = mgr
        .update_queue(&queue.id, "one_time", Some(&iso_in(3600)), Some("soon"), false, "none", None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("stop time"), "unexpected error: {err}");
    let row = mgr.get_queue(&queue.id).await.unwrap().unwrap();
    assert_eq!(row.stop_at, None);
}

#[tokio::test]
async fn update_queue_rejects_stop_before_start() {
    // BUG-006: an inverted window must be rejected at save time.
    // (Converts the round-1 DOC test
    // `update_queue_stop_before_start_never_runs`.)
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    let queue = mgr.create_queue("Inverted").unwrap();
    let err = mgr
        .update_queue(&queue.id, "one_time", Some(&iso_in(3600)), Some(&iso_in(60)), false, "none", None, None)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "The stop time must be after the start time.");
    // Nothing was stored.
    let row = mgr.get_queue(&queue.id).await.unwrap().unwrap();
    assert_eq!(row.start_at, None);
    assert_eq!(row.stop_at, None);
}

#[tokio::test]
async fn update_queue_rejects_stop_equal_to_start_and_stop_without_start() {
    let store = Store::open_in_memory().unwrap();
    let mgr = DownloadManager::new(Arc::new(store), 4);
    let queue = mgr.create_queue("Edge").unwrap();
    let same = iso_in(3600);
    let err = mgr
        .update_queue(&queue.id, "one_time", Some(&same), Some(&same), false, "none", None, None)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "The stop time must be after the start time.");
    let err = mgr
        .update_queue(&queue.id, "one_time", None, Some(&iso_in(3600)), false, "none", None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("start time"), "unexpected error: {err}");
}

#[tokio::test]
async fn scheduler_tick_reports_legacy_malformed_schedule_but_runs_valid_queues() {
    // BUG-005 (legacy rows): a queue whose malformed times were stored
    // before validation existed (a pre-fix database) must be reported
    // by the tick, not silently skipped — and it must not block other
    // queues from running.
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let db_path = dir.join("app.db");
    let store = Arc::new(Store::open(&db_path).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let broken = mgr.create_queue("Broken").unwrap();
    add_queued(&mgr, &server.url, &dir, "one.bin", &broken.id).await;
    let good = mgr.create_queue("Good").unwrap();
    add_queued(&mgr, &server.url, &dir, "two.bin", &good.id).await;
    // Plant a malformed schedule straight into the DB, bypassing
    // update_queue validation — as a database written before the fix.
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE queues SET start_at = 'tomorrow at noon' WHERE id = ?1",
            rusqlite::params![broken.id],
        )
        .unwrap();
    }
    mgr.update_queue(&good.id, "one_time", Some(&iso_in(-60)), None, false, "none", None, None)
        .await
        .unwrap();
    let err = mgr.scheduler_tick_at(now_unix()).await.unwrap_err();
    assert!(err.to_string().contains("Broken"), "unexpected error: {err}");
    // The valid queue still ran and drained despite the bad one.
    assert!(wait_for_queue_drained(&mgr, &good.id, 30).await);
    assert_eq!(mgr.list_queue_files(&good.id).unwrap().len(), 0);
}

// -- replica-test: second batch (probe fallback, queue error path) --------

#[tokio::test]
async fn probe_falls_back_when_head_refused() {
    let server = MockServer::start(MockConfig { head_405: true, ..Default::default() });
    let client = reqwest::Client::new();
    // HEAD answers 405: the probe must fall back to a ranged GET and
    // still learn the size and range support.
    let info = probe(&client, &server.url).await.unwrap();
    assert_eq!(info.size, Some(512 * 1024));
    assert!(info.accept_ranges);
    assert_eq!(info.etag.as_deref(), Some("\"v1\""));
}

#[tokio::test]
async fn add_download_keeps_unicode_filename() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let (id, _) = mgr
        .add_download(
            &server.url,
            Some("🎬 فيلم.mp4".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
        )
        .await
        .unwrap();
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.filename, "🎬 فيلم.mp4");
    assert_eq!(row.category_id, "cat-video");
}

#[tokio::test]
async fn add_download_with_queue_files_it_in_main_queue() {
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    // This is what S02 "Save for later" does: file in the main queue.
    let (id, _) = mgr
        .add_download(
            &server.url,
            Some("later.bin".to_string()),
            Some(dir.to_string_lossy().to_string()),
            None,
            Some("queue-main".to_string()),
        )
        .await
        .unwrap();
    let files = mgr.list_queue_files("queue-main").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, id);
    assert_eq!(files[0].status, "queued");
}

#[tokio::test]
async fn queue_stops_when_a_file_errors() {
    let good = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let bad = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Mixed").unwrap();
    let first = add_queued(&mgr, &good.url, &dir, "one.bin", &queue.id).await;
    let second = add_queued(&mgr, &bad.url, &dir, "two.bin", &queue.id).await;
    // The second server starts failing GETs (HEAD still works, so the
    // add succeeded): its download retries, then goes to error, and
    // the queue run must stop with the failed file still queued.
    bad.set_fail_get(true);
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_queue_running(&mgr, &queue.id, false, 120).await);
    let one = mgr.get_download(&first).unwrap().unwrap();
    assert_eq!(one.status, "complete");
    assert_eq!(one.queue_id, None);
    let two = mgr.get_download(&second).unwrap().unwrap();
    assert_eq!(two.status, "error");
    assert!(two.error_message.as_deref().unwrap_or("").contains("500"), "error must say why: {:?}", two.error_message);
    assert_eq!(two.queue_id.as_deref(), Some(queue.id.as_str()));
}

#[tokio::test]
async fn resume_fails_when_server_ignores_range() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(100),
        hide_length: false,
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
    tokio::time::sleep(Duration::from_millis(400)).await;
    mgr.pause_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "paused", 10).await);
    // The server drops resume support: ranged GETs now answer 200
    // with the whole file. Resume must fail loudly, never write the
    // wrong bytes at a mid-file offset.
    server.set_ignore_range(true);
    mgr.resume_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "error", 120).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert!(row.error_message.as_deref().unwrap_or("").contains("ignored the range"), "error must say why: {:?}", row.error_message);
    assert!(row.downloaded_bytes < row.size_bytes.unwrap(), "no wrong bytes may be stitched in");
}

#[tokio::test]
async fn open_file_action_executes() {
    // xdg-open exists on this Linux VM; spawn must succeed (the child
    // may fail to find an opener for a .bin file, but the spawn itself
    // is the action). On Windows this runs `cmd /C start`.
    let server = MockServer::start(MockConfig { chunk: 65536, delay: Duration::ZERO, hide_length: false, ..Default::default() });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Open").unwrap();
    mgr.update_queue(&queue.id, "one_time", None, None, false, "open_file", None, None)
        .await
        .unwrap();
    add_queued(&mgr, &server.url, &dir, "one.bin", &queue.id).await;
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_queue_drained(&mgr, &queue.id, 30).await);
    let pending = mgr.pending_action(&queue.id).await.expect("open_file action must be pending");
    assert_eq!(pending.action, "open_file");
    assert!(pending.file_path.as_deref().unwrap_or("").ends_with("one.bin"));
    mgr.execute_queue_action(&queue.id).await.unwrap();
    assert!(mgr.pending_action(&queue.id).await.is_none());
}

// -- S07 Download All / S10 batch -----------------------------------------

#[tokio::test]
async fn extract_links_finds_all_tag_types_and_resolves_relative() {
    use fetchline_engine::{extract_links, LinkKind, classify_kind};
    let html = "<html><head><link href=\"style.css\"></head><body>\
        <a href=\"https://example.com/abs.mp4\">abs</a>\
        <a href=\"/root.png\">root</a>\
        <a href=\"rel/song.mp3\">rel</a>\
        <a href=\"rel/song.mp3\">dup</a>\
        <a href=\"javascript:void(0)\">js</a>\
        <a href=\"mailto:a@b.c\">mail</a>\
        <a href=\"#frag\">frag</a>\
        <img src=\"pic.jpg\">\
        <video src=\"clip.webm\"></video>\
        <audio><source src=\"pod.ogg\"></audio>\
        <a href=\"data:text/plain,hi\">data</a>\
    </body></html>";
    let links = extract_links(html, "https://example.com/dir/page.html");
    assert_eq!(
        links,
        vec![
            "https://example.com/dir/style.css",
            "https://example.com/abs.mp4",
            "https://example.com/root.png",
            "https://example.com/dir/rel/song.mp3",
            "https://example.com/dir/pic.jpg",
            "https://example.com/dir/clip.webm",
            "https://example.com/dir/pod.ogg",
        ]
    );
    assert_eq!(classify_kind("movie.mp4"), LinkKind::Video);
    assert_eq!(classify_kind("song.MP3"), LinkKind::Audio);
    assert_eq!(classify_kind("photo.jpeg"), LinkKind::Image);
    assert_eq!(classify_kind("report.pdf"), LinkKind::Document);
    assert_eq!(classify_kind("bundle.tar.gz"), LinkKind::Archive);
    assert_eq!(classify_kind("setup.exe"), LinkKind::Program);
    assert_eq!(classify_kind("noext"), LinkKind::Other);
}

#[tokio::test]
async fn fetch_page_links_probes_kinds_sizes_and_dead_links() {
    use fetchline_engine::LinkKind;
    let page = PageServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let links = mgr.fetch_page_links(&page.page_url).await.unwrap();
    // 9 unique links: style.css first (head), then body order; the
    // video appears 3 times (a x2, source x1) and the pic twice.
    assert_eq!(links.len(), 9, "links: {:?}", links.iter().map(|l| &l.url).collect::<Vec<_>>());
    let by_url = |u: &str| links.iter().find(|l| l.url == format!("{}{u}", page.base)).unwrap();
    let video = by_url("/video.mp4");
    assert_eq!(video.kind, LinkKind::Video);
    assert_eq!(video.size, Some(100 * 1024));
    assert!(video.error.is_none());
    assert!(!video.already_exists);
    assert_eq!(by_url("/song.mp3").kind, LinkKind::Audio);
    assert_eq!(by_url("/song.mp3").size, Some(50 * 1024));
    assert_eq!(by_url("/pic.png").kind, LinkKind::Image);
    assert_eq!(by_url("/doc.pdf").kind, LinkKind::Document);
    assert_eq!(by_url("/file.zip").kind, LinkKind::Archive);
    assert_eq!(by_url("/setup.exe").kind, LinkKind::Program);
    assert_eq!(by_url("/notes.txt").kind, LinkKind::Document);
    assert_eq!(by_url("/style.css").kind, LinkKind::Other);
    let dead = by_url("/missing.mp4");
    assert_eq!(dead.kind, LinkKind::Video);
    assert_eq!(dead.size, None);
    assert!(dead.error.as_deref().unwrap_or("").contains("404"), "dead link must carry its error: {:?}", dead.error);
}

#[tokio::test]
async fn fetch_page_links_flags_already_downloaded() {
    let page = PageServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let video = format!("{}/video.mp4", page.base);
    mgr.add_download(&video, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let links = mgr.fetch_page_links(&page.page_url).await.unwrap();
    let flagged = links.iter().find(|l| l.url == video).unwrap();
    assert!(flagged.already_exists);
    assert!(links.iter().filter(|l| l.url != video).all(|l| !l.already_exists));
}

#[tokio::test]
async fn fetch_page_links_rejects_bad_page() {
    let page = PageServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let err = mgr
        .fetch_page_links(&format!("{}/no-such-page.html", page.base))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404"), "error: {err}");
}

#[tokio::test]
async fn batch_add_validates_lines_with_numbers() {
    let page = PageServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let lines = vec![
        format!("{}/video.mp4", page.base),
        String::new(),
        "   ".to_string(),
        "not a url".to_string(),
        "ftp://example.com/file.zip".to_string(),
        format!("{}/song.mp3", page.base),
        format!("{}/missing.mp4", page.base),
    ];
    let result = mgr
        .add_batch_downloads(&lines, Some(dir.to_string_lossy().to_string()), None, None, false)
        .await
        .unwrap();
    assert_eq!(result.created.len(), 2, "only the two good links are created");
    // Blank lines are ignored, not errors; the bad lines are 4, 5, 7.
    assert_eq!(result.invalid.len(), 3);
    assert_eq!(result.invalid[0].line, 4);
    assert_eq!(result.invalid[1].line, 5);
    assert_eq!(result.invalid[2].line, 7);
    assert!(result.invalid[2].error.contains("404"), "error: {}", result.invalid[2].error);
    assert_eq!(mgr.list_downloads().unwrap().len(), 2);
}

#[tokio::test]
async fn batch_add_dedups_within_batch() {
    let page = PageServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let url = format!("{}/video.mp4", page.base);
    let result = mgr
        .add_batch_downloads(
            &[url.clone(), url.clone(), format!("{}/song.mp3", page.base)],
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(result.created.len(), 2);
    assert_eq!(result.invalid.len(), 1);
    assert_eq!(result.invalid[0].line, 2);
    assert!(result.invalid[0].error.contains("Duplicate"), "error: {}", result.invalid[0].error);
}

#[tokio::test]
async fn batch_add_reuses_duplicate_path_guard() {
    // Same URL added in two separate batches must not share a file
    // (BUG-002 fix): the second add gets a suffixed save path.
    let page = PageServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let url = format!("{}/video.mp4", page.base);
    let folder = dir.to_string_lossy().to_string();
    let first = mgr
        .add_batch_downloads(&[url.clone()], Some(folder.clone()), None, None, false)
        .await
        .unwrap();
    let second = mgr
        .add_batch_downloads(&[url.clone()], Some(folder.clone()), None, None, false)
        .await
        .unwrap();
    assert_eq!(first.created.len(), 1);
    assert_eq!(second.created.len(), 1);
    let a = mgr.get_download(&first.created[0]).unwrap().unwrap();
    let b = mgr.get_download(&second.created[0]).unwrap().unwrap();
    assert_ne!(a.save_path, b.save_path);
    assert!(b.save_path.contains(" (2)"), "second path: {}", b.save_path);
}

#[tokio::test]
async fn batch_add_to_queue_and_auto_assigns_categories() {
    let page = PageServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let queue = mgr.create_queue("Batch queue").unwrap();
    let result = mgr
        .add_batch_downloads(
            &[
                format!("{}/video.mp4", page.base),
                format!("{}/file.zip", page.base),
                format!("{}/notes.txt", page.base),
            ],
            Some(dir.to_string_lossy().to_string()),
            None,
            Some(queue.id.clone()),
            false,
        )
        .await
        .unwrap();
    assert_eq!(result.created.len(), 3);
    assert!(result.invalid.is_empty());
    let files = mgr.list_queue_files(&queue.id).unwrap();
    assert_eq!(files.len(), 3);
    // Queue order follows the batch order.
    assert_eq!(files[0].id, result.created[0]);
    assert_eq!(files[1].id, result.created[1]);
    assert_eq!(files[2].id, result.created[2]);
    // Category auto-assign reuses the single-add rules.
    assert_eq!(files[0].category_id, "cat-video");
    assert_eq!(files[1].category_id, "cat-compressed");
    assert_eq!(files[2].category_id, "cat-documents");
}

#[tokio::test]
async fn download_all_adds_only_selected_links() {
    let page = PageServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let result = mgr
        .download_all_links(
            &page.page_url,
            vec![
                format!("{}/song.mp3", page.base),
                format!("{}/pic.png", page.base),
            ],
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(result.created.len(), 2);
    assert!(result.invalid.is_empty());
    let rows = mgr.list_downloads().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| r.filename == "song.mp3" && r.category_id == "cat-music"));
    assert!(rows.iter().any(|r| r.filename == "pic.png"));
}

#[tokio::test]
async fn batch_add_rejects_unknown_queue_and_category_up_front() {
    let page = PageServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let url = format!("{}/video.mp4", page.base);
    let err = mgr
        .add_batch_downloads(
            &[url.clone()],
            Some(dir.to_string_lossy().to_string()),
            None,
            Some("no-such-queue".to_string()),
            false,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no-such-queue"), "error: {err}");
    assert!(mgr.list_downloads().unwrap().is_empty(), "nothing may be added before the queue check");
    let err = mgr
        .add_batch_downloads(
            &[url],
            Some(dir.to_string_lossy().to_string()),
            Some("no-such-category".to_string()),
            None,
            false,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no-such-category"), "error: {err}");
    assert!(mgr.list_downloads().unwrap().is_empty());
}

// -- milestone 4: settings (S09), proxy, speed limiter (S12) -------------

use fetchline_engine::ProxyConfigInput;
use std::time::Instant;

fn proxy_input(proxy_type: &str, host: &str, port: i64) -> ProxyConfigInput {
    ProxyConfigInput {
        proxy_type: proxy_type.to_string(),
        host: host.to_string(),
        port,
        username: None,
        password: None,
        use_for: vec![],
        bypass: vec![],
        enabled: true,
        auth: None,
    }
}

#[tokio::test]
async fn settings_defaults_and_round_trip() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let defaults = store.get_settings().unwrap();
    assert_eq!(defaults.max_connections, 8);
    assert_eq!(defaults.speed_limit_kbps, None);
    assert_eq!(defaults.theme, "light");
    assert_eq!(defaults.connection_timeout_secs, 30);
    assert_eq!(defaults.max_retries, 5);
    assert_eq!(defaults.default_action, "start_now");
    assert_eq!(defaults.duplicate_action, "rename");
    assert!(defaults.confirm_delete);

    let mut changed = defaults.clone();
    changed.default_download_folder = dir.to_string_lossy().to_string();
    changed.max_connections = 3;
    changed.speed_limit_kbps = Some(256);
    changed.theme = "dark".to_string();
    changed.start_on_startup = true;
    changed.default_action = "save_for_later".to_string();
    changed.duplicate_action = "overwrite".to_string();
    changed.confirm_delete = false;
    changed.connection_timeout_secs = 60;
    changed.max_retries = 2;
    let saved = store.update_settings(&changed).unwrap();
    assert_eq!(saved.max_connections, 3);
    assert_eq!(saved.speed_limit_kbps, Some(256));
    assert_eq!(saved.theme, "dark");
    assert!(saved.start_on_startup);
    assert_eq!(saved.default_action, "save_for_later");
    assert_eq!(saved.duplicate_action, "overwrite");
    assert!(!saved.confirm_delete);
    assert_eq!(saved.connection_timeout_secs, 60);
    assert_eq!(saved.max_retries, 2);
    // A speed limit of 0 means unlimited and is stored as NULL.
    let mut unlimited = saved.clone();
    unlimited.speed_limit_kbps = Some(0);
    assert_eq!(store.update_settings(&unlimited).unwrap().speed_limit_kbps, None);
}

#[tokio::test]
async fn settings_reject_invalid_values() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let base = store.get_settings().unwrap();
    let mut bad = base.clone();
    bad.max_connections = 0;
    assert!(store.update_settings(&bad).is_err());
    bad = base.clone();
    bad.max_connections = 17;
    assert!(store.update_settings(&bad).is_err());
    bad = base.clone();
    bad.speed_limit_kbps = Some(-5);
    assert!(store.update_settings(&bad).is_err());
    bad = base.clone();
    bad.theme = "blue".to_string();
    assert!(store.update_settings(&bad).is_err());
    bad = base.clone();
    bad.default_action = "later".to_string();
    assert!(store.update_settings(&bad).is_err());
    bad = base.clone();
    bad.duplicate_action = "ask".to_string();
    assert!(store.update_settings(&bad).is_err());
    bad = base.clone();
    bad.connection_timeout_secs = 0;
    assert!(store.update_settings(&bad).is_err());
    bad = base.clone();
    bad.connection_timeout_secs = 301;
    assert!(store.update_settings(&bad).is_err());
    bad = base.clone();
    bad.max_retries = -1;
    assert!(store.update_settings(&bad).is_err());
    bad = base.clone();
    bad.max_retries = 11;
    assert!(store.update_settings(&bad).is_err());
    bad = base.clone();
    bad.language = "   ".to_string();
    assert!(store.update_settings(&bad).is_err());
    // Nothing was stored: the row is unchanged after all the rejections.
    assert_eq!(store.get_settings().unwrap().max_connections, base.max_connections);
}

#[test]
fn settings_migration_adds_new_columns_to_old_db() {
    let dir = tmpdir();
    let path = dir.join("old.db");
    {
        // A database from before this milestone: the settings and
        // proxy tables without the new columns.
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
             CREATE TABLE proxy_configs (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE,
             type TEXT NOT NULL DEFAULT 'http', host TEXT NOT NULL,
             port INTEGER NOT NULL, username TEXT, password_enc TEXT,
             use_for TEXT NOT NULL DEFAULT '[]',
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now')));",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let s = store.get_settings().unwrap();
    assert_eq!(s.max_connections, 8);
    assert_eq!(s.connection_timeout_secs, 30);
    assert_eq!(s.max_retries, 5);
    assert_eq!(s.default_action, "start_now");
    assert_eq!(s.duplicate_action, "rename");
    let mut s2 = s.clone();
    s2.max_retries = 2;
    store.update_settings(&s2).unwrap();
    assert_eq!(store.get_settings().unwrap().max_retries, 2);
    // The proxy table got its columns too.
    let mut input = proxy_input("http", "127.0.0.1", 8080);
    input.enabled = false;
    let row = store.save_proxy_config(&input).unwrap();
    assert!(!row.enabled);
    assert!(row.bypass.is_empty());
}

#[tokio::test]
async fn max_connections_from_settings_changes_segment_count() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(2 * 1024 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    // Default (4): a 2 MiB range-capable file is split into 4.
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(store.get_segments(&id).unwrap().len(), 4);
    // Settings say 1 connection: the next download is a single segment.
    let mut s = mgr.get_settings().unwrap();
    s.max_connections = 1;
    mgr.update_settings(&s).unwrap();
    assert_eq!(mgr.max_connections(), 1);
    let (id2, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(store.get_segments(&id2).unwrap().len(), 1);
    // And 2 connections: two segments.
    s.max_connections = 2;
    mgr.update_settings(&s).unwrap();
    let (id3, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(store.get_segments(&id3).unwrap().len(), 2);
}

#[tokio::test]
async fn save_to_folder_and_file_types_edit_change_downloads() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let cat_dir = dir.join("books");
    let cat = mgr
        .create_category("Books", &cat_dir.to_string_lossy(), &["xyz".to_string()])
        .unwrap();
    // Edit the category (S09 File Types + Save To): new folder and
    // one more extension.
    let new_dir = dir.join("reading");
    let updated = mgr
        .update_category(
            &cat.id,
            Some(&new_dir.to_string_lossy()),
            Some(&["xyz".to_string(), "abc".to_string()]),
        )
        .unwrap();
    assert_eq!(updated.default_folder, new_dir.to_string_lossy());
    assert_eq!(updated.file_extensions, vec!["xyz".to_string(), "abc".to_string()]);
    // A .abc file is auto-assigned to Books and lands in its folder.
    let (id, _) = mgr
        .add_download(&server.url, Some("book.abc".to_string()), None, None, None)
        .await
        .unwrap();
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.category_id, cat.id);
    assert!(std::path::Path::new(&row.save_path).starts_with(&new_dir), "save_path: {}", row.save_path);
    // Editing a built-in category's folder works the same way.
    let video_dir = dir.join("videos");
    mgr.update_category("cat-video", Some(&video_dir.to_string_lossy()), None)
        .unwrap();
    let (id2, _) = mgr
        .add_download(&server.url, Some("clip.mp4".to_string()), None, None, None)
        .await
        .unwrap();
    let row2 = mgr.get_download(&id2).unwrap().unwrap();
    assert_eq!(row2.category_id, "cat-video");
    assert!(std::path::Path::new(&row2.save_path).starts_with(&video_dir), "save_path: {}", row2.save_path);
    // Validation: an empty folder or no extensions is refused.
    assert!(mgr.update_category(&cat.id, Some("  "), None).is_err());
    assert!(mgr.update_category(&cat.id, None, Some(&[])).is_err());
    assert!(mgr.update_category("no-such-category", None, None).is_err());
}

#[tokio::test]
async fn default_folder_from_settings_is_used() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let default_dir = dir.join("default-dl");
    let mut s = mgr.get_settings().unwrap();
    s.default_download_folder = default_dir.to_string_lossy().to_string();
    mgr.update_settings(&s).unwrap();
    // No folder given, category Documents has no folder of its own:
    // the settings default wins.
    let (id, _) = mgr
        .add_download(&server.url, Some("notes.txt".to_string()), None, None, None)
        .await
        .unwrap();
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert!(std::path::Path::new(&row.save_path).starts_with(&default_dir), "save_path: {}", row.save_path);
}

#[tokio::test]
async fn duplicate_overwrite_reuses_path() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut s = mgr.get_settings().unwrap();
    s.duplicate_action = "overwrite".to_string();
    mgr.update_settings(&s).unwrap();
    let folder = dir.to_string_lossy().to_string();
    let (id1, _) = mgr.add_download(&server.url, None, Some(folder.clone()), None, None).await.unwrap();
    let (id2, _) = mgr.add_download(&server.url, None, Some(folder.clone()), None, None).await.unwrap();
    let p1 = mgr.get_download(&id1).unwrap().unwrap().save_path;
    let p2 = mgr.get_download(&id2).unwrap().unwrap().save_path;
    assert_eq!(p1, p2, "overwrite reuses the same path");
    // Back to rename: the third add gets a suffixed path.
    s.duplicate_action = "rename".to_string();
    mgr.update_settings(&s).unwrap();
    let (id3, _) = mgr.add_download(&server.url, None, Some(folder), None, None).await.unwrap();
    let p3 = mgr.get_download(&id3).unwrap().unwrap().save_path;
    assert_ne!(p3, p1);
    assert!(p3.contains(" (2)."), "save_path: {p3}");
}

#[tokio::test]
async fn proxy_validation_rejects_bad_config() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let mut bad = proxy_input("http", "", 8080);
    assert!(store.save_proxy_config(&bad).is_err());
    bad = proxy_input("http", "http://proxy.example", 8080);
    assert!(store.save_proxy_config(&bad).is_err());
    bad = proxy_input("http", "proxy example", 8080);
    assert!(store.save_proxy_config(&bad).is_err());
    bad = proxy_input("http", "127.0.0.1", 0);
    assert!(store.save_proxy_config(&bad).is_err());
    bad = proxy_input("http", "127.0.0.1", 70000);
    assert!(store.save_proxy_config(&bad).is_err());
    bad = proxy_input("ntlm", "127.0.0.1", 8080);
    assert!(store.save_proxy_config(&bad).is_err());
    bad = proxy_input("http", "127.0.0.1", 8080);
    bad.use_for = vec!["ftp".to_string()];
    assert!(store.save_proxy_config(&bad).is_err());
    bad = proxy_input("http", "127.0.0.1", 8080);
    bad.bypass = vec!["has space".to_string()];
    assert!(store.save_proxy_config(&bad).is_err());
    assert!(store.get_proxy_config().unwrap().is_none(), "nothing was stored");
}

#[tokio::test]
async fn proxy_password_is_stored_and_hidden_from_ui() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let mut input = proxy_input("http", "127.0.0.1", 8080);
    input.username = Some("alice".to_string());
    input.password = Some("hunter2".to_string());
    let row = store.save_proxy_config(&input).unwrap();
    assert!(row.has_password);
    assert_eq!(row.username.as_deref(), Some("alice"));
    // The row the UI gets has no password field at all (compile-time:
    // ProxyConfigRow carries only has_password) — and the engine can
    // still read the secret back.
    let active = store.get_active_proxy().unwrap().unwrap();
    assert_eq!(active.password.as_deref(), Some("hunter2"));
    assert_eq!(active.username.as_deref(), Some("alice"));
    // Saving again without a password keeps the stored one.
    let mut again = proxy_input("http", "127.0.0.1", 9090);
    again.password = None;
    let row2 = store.save_proxy_config(&again).unwrap();
    assert!(row2.has_password);
    assert_eq!(row2.port, 9090);
    assert_eq!(store.get_active_proxy().unwrap().unwrap().password.as_deref(), Some("hunter2"));
    // An empty password clears it.
    let mut clear = proxy_input("http", "127.0.0.1", 9090);
    clear.password = Some(String::new());
    let row3 = store.save_proxy_config(&clear).unwrap();
    assert!(!row3.has_password);
    assert!(store.get_active_proxy().unwrap().unwrap().password.is_none());
}

#[tokio::test]
async fn download_goes_through_http_proxy() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start();
    assert!(proxy.url.starts_with("http://127.0.0.1:"));
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    mgr.save_proxy_config(&proxy_input("http", "127.0.0.1", proxy.port as i64)).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    assert!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst) > 0, "the proxy was never used");
}

#[tokio::test]
async fn download_goes_through_http_proxy_with_auth() {
    let server = MockServer::start(MockConfig::default());
    // The proxy demands Basic auth: the engine answers the 407
    // challenge with the saved credentials (the default
    // `negotiate-auto` setting waits for the challenge instead of
    // sending Basic preemptively — see `proxy.rs`).
    let proxy = HttpProxyServer::start_with_auth(Some(("alice", "hunter2")));
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.username = Some("alice".to_string());
    input.password = Some("hunter2".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(proxy.saw_auth.load(std::sync::atomic::Ordering::SeqCst), "no Proxy-Authorization was sent");
}

#[tokio::test]
async fn basic_auth_setting_sends_credentials_preemptively() {
    // With `auth: basic` the credentials go on the first request
    // (no 407 round trip): a proxy that does not challenge at all
    // still sees them. With the default `negotiate-auto` the same
    // proxy would see no credentials — the engine waits for a
    // challenge and picks the strongest offered scheme.
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.username = Some("alice".to_string());
    input.password = Some("hunter2".to_string());
    input.auth = Some("basic".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(proxy.saw_auth.load(std::sync::atomic::Ordering::SeqCst), "no Proxy-Authorization was sent preemptively");
}

#[tokio::test]
async fn proxy_disabled_download_goes_direct() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.enabled = false;
    mgr.save_proxy_config(&input).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert_eq!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst), 0, "disabled proxy was used");
}

#[tokio::test]
async fn proxy_bypass_list_skips_proxy() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.bypass = vec!["127.0.0.1".to_string()];
    mgr.save_proxy_config(&input).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert_eq!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst), 0, "bypassed host went through the proxy");
}

#[tokio::test]
async fn download_goes_through_socks5_proxy() {
    let server = MockServer::start(MockConfig::default());
    let proxy = Socks5Server::start();
    assert!(proxy.url.starts_with("socks5://127.0.0.1:"));
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    mgr.save_proxy_config(&proxy_input("socks", "127.0.0.1", proxy.port as i64)).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
    assert!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst) > 0, "the SOCKS5 proxy was never used");
}

#[tokio::test]
async fn socks5_proxy_with_auth() {
    let server = MockServer::start(MockConfig::default());
    let proxy = Socks5Server::start_with_auth(Some(("alice", "hunter2")));
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut input = proxy_input("socks", "127.0.0.1", proxy.port as i64);
    input.username = Some("alice".to_string());
    input.password = Some("hunter2".to_string());
    mgr.save_proxy_config(&input).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(proxy.saw_auth.load(std::sync::atomic::Ordering::SeqCst), "no SOCKS5 auth was sent");
    // Wrong password: the probe cannot reach the server at all.
    let dir2 = tmpdir();
    let store2 = Arc::new(Store::open(&dir2.join("app.db")).unwrap());
    let mgr2 = DownloadManager::new(Arc::clone(&store2), 4);
    let mut wrong = proxy_input("socks", "127.0.0.1", proxy.port as i64);
    wrong.username = Some("alice".to_string());
    wrong.password = Some("wrong".to_string());
    mgr2.save_proxy_config(&wrong).unwrap();
    let err = mgr2
        .add_download(&server.url, None, Some(dir2.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty(), "expected a proxy auth error");
}

#[tokio::test]
async fn test_proxy_reports_status_and_failure() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    let status = mgr.test_proxy(&input, &server.url).await.unwrap();
    assert_eq!(status, 200);
    assert!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst) > 0);
    // Saving is not needed for the test, and nothing was stored.
    assert!(store.get_proxy_config().unwrap().is_none());
    // A proxy that is down: the test fails instead of hanging.
    let down = proxy_input("http", "127.0.0.1", 1);
    assert!(mgr.test_proxy(&down, &server.url).await.is_err());
}

#[tokio::test]
async fn ftp_proxy_is_stored_but_not_applied() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    // An FTP proxy pointing at a plain HTTP proxy server: if the
    // engine routed HTTP through it, the request would fail. It must
    // download directly instead — an FTP proxy only routes FTP
    // traffic (the FTP transport applies it to ftp:// URLs; HTTP
    // downloads never go through it).
    let mut input = proxy_input("ftp", "127.0.0.1", proxy.port as i64);
    input.enabled = true;
    mgr.save_proxy_config(&input).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert_eq!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn speed_limit_caps_download() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(256 * 1024),
        chunk: 65536,
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    mgr.set_speed_limit(Some(32)).unwrap();
    assert_eq!(mgr.speed_limit_bps(), 32 * 1024);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let start = Instant::now();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 60).await);
    let elapsed = start.elapsed();
    // 256 KiB at 32 KiB/s takes ~8 s (first 32 KiB are the free
    // burst); anything under 5 s means the cap did not hold.
    assert!(elapsed >= Duration::from_secs(5), "finished too fast: {elapsed:?}");
    assert!(elapsed < Duration::from_secs(25), "finished too slow: {elapsed:?}");
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
}

#[tokio::test]
async fn speed_limit_unlimited_is_fast() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(256 * 1024),
        chunk: 65536,
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    assert_eq!(mgr.get_speed_limit().unwrap(), None);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let start = Instant::now();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(start.elapsed() < Duration::from_secs(5), "elapsed: {:?}", start.elapsed());
}

#[tokio::test]
async fn speed_limit_change_mid_download_takes_effect() {
    let server = MockServer::start(MockConfig {
        chunk: 8192,
        delay: Duration::from_millis(10),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    mgr.set_speed_limit(Some(16)).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let start = Instant::now();
    mgr.start_download(&id).await.unwrap();
    // Poll until the download has genuinely started: the database
    // only persists progress on the 2 s checkpoint, so a single
    // read right after starting can still show 0 bytes.
    let mut started = false;
    for _ in 0..25 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mid = mgr.get_download(&id).unwrap().unwrap();
        if mid.downloaded_bytes > 0 {
            started = true;
            assert_eq!(mid.status, "downloading");
            assert!(mid.downloaded_bytes < mid.size_bytes.unwrap(), "already done under the cap?");
            break;
        }
    }
    assert!(started, "nothing downloaded after 5 s under the cap");
    // Lift the cap: the rest must finish in seconds, not ~30 s.
    mgr.set_speed_limit(None).unwrap();
    assert_eq!(mgr.speed_limit_bps(), 0);
    assert!(wait_for_status(&mgr, &id, "complete", 15).await);
    assert!(start.elapsed() < Duration::from_secs(15), "elapsed: {:?}", start.elapsed());
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}


// -- replica-test round 2: settings (S09), proxy, speed limiter (S12),
//    and their integration with queues/categories/scheduler ----------

use fetchline_engine::ProxyConfigRow;

/// S09-H1: `set_speed_limit` rejects a negative cap, and a large but
/// sane cap is stored and applied (10_000_000 KB/s -> ~10 GB/s).
#[tokio::test]
async fn speed_limit_rejects_negative_and_accepts_huge_sane_value() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    assert!(mgr.set_speed_limit(Some(-5)).is_err());
    assert_eq!(mgr.get_speed_limit().unwrap(), None, "rejected save stored nothing");
    mgr.set_speed_limit(Some(10_000_000)).unwrap();
    assert_eq!(mgr.get_speed_limit().unwrap(), Some(10_000_000));
    assert_eq!(mgr.speed_limit_bps(), 10_000_000 * 1024);
    // 0 means unlimited, stored as NULL.
    mgr.set_speed_limit(Some(0)).unwrap();
    assert_eq!(mgr.get_speed_limit().unwrap(), None);
    assert_eq!(mgr.speed_limit_bps(), 0);
}

/// BUG-008 (was S12-E1, DOC): an absurd speed limit must be rejected
/// with a clear error — never stored, never applied. Before the fix
/// `set_speed_limit(Some(i64::MAX))` panicked in debug ("attempt to
/// multiply with overflow", manager.rs) and wrapped silently in
/// release, so some huge values became a small cap.
#[tokio::test]
async fn speed_limit_absurd_value_is_rejected() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let err = mgr.set_speed_limit(Some(i64::MAX)).unwrap_err();
    assert!(
        err.to_string().contains("too large"),
        "expected a too-large error, got: {err}"
    );
    // Nothing was stored: the cap is still unlimited.
    assert_eq!(mgr.get_speed_limit().unwrap(), None);
    assert_eq!(mgr.speed_limit_bps(), 0);
    // The same bound applies through the full settings save.
    let mut settings = store.get_settings().unwrap();
    settings.speed_limit_kbps = Some(i64::MAX);
    assert!(store.update_settings(&settings).is_err());
    assert_eq!(store.get_settings().unwrap().speed_limit_kbps, None);
    // The documented maximum is still accepted (see
    // `speed_limit_rejects_negative_and_accepts_huge_sane_value`).
    mgr.set_speed_limit(Some(10_000_000)).unwrap();
    assert_eq!(mgr.get_speed_limit().unwrap(), Some(10_000_000));
}

/// S09-H2 (DOC): changing max_connections does NOT change a download
/// that is already running — it keeps the segment count it started
/// with. Only downloads added afterwards use the new value.
#[tokio::test]
async fn max_connections_change_does_not_alter_running_download() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(2 * 1024 * 1024),
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
    assert_eq!(store.get_segments(&id).unwrap().len(), 4);
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "downloading", 10).await);
    let mut s = mgr.get_settings().unwrap();
    s.max_connections = 1;
    mgr.update_settings(&s).unwrap();
    assert_eq!(mgr.max_connections(), 1);
    // Still downloading, still 4 segments — the new value only
    // applies to the next download.
    assert_eq!(store.get_segments(&id).unwrap().len(), 4);
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let (id2, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert_eq!(store.get_segments(&id2).unwrap().len(), 1);
}

/// S09-H3: max_retries = 0 means a single attempt: a download whose
/// GETs all fail errors quickly instead of retrying with backoff.
#[tokio::test]
async fn max_retries_zero_means_single_attempt_and_fast_error() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut s = mgr.get_settings().unwrap();
    s.max_retries = 0;
    mgr.update_settings(&s).unwrap();
    server.set_fail_get(true);
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let start = Instant::now();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "error", 10).await);
    assert!(start.elapsed() < Duration::from_secs(10), "elapsed: {:?}", start.elapsed());
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert!(row.error_message.as_deref().unwrap_or("").contains("500"), "error: {:?}", row.error_message);
}

/// S09-N1: `test_proxy` validates its input before dialling anything.
#[tokio::test]
async fn test_proxy_rejects_invalid_input() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let bad_port = proxy_input("http", "127.0.0.1", 0);
    assert!(mgr.test_proxy(&bad_port, "http://127.0.0.1/").await.is_err());
    let bad_type = proxy_input("ntlm", "127.0.0.1", 8080);
    assert!(mgr.test_proxy(&bad_type, "http://127.0.0.1/").await.is_err());
}

/// S09-N2: the proxy row the UI receives can never contain the
/// password — serialised, it holds only `has_password`.
#[tokio::test]
async fn proxy_row_serialised_contains_no_password() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let mut input = proxy_input("http", "127.0.0.1", 8080);
    input.username = Some("alice".to_string());
    input.password = Some("hunter2".to_string());
    let row = store.save_proxy_config(&input).unwrap();
    assert!(row.has_password);
    let json = serde_json::to_string(&row).unwrap();
    assert!(!json.contains("hunter2"), "password leaked into the UI row: {json}");
    assert!(!json.contains("\"password\":"), "row carries a password field: {json}");
    // And the type itself has no password field to leak (compile-time):
    let _: ProxyConfigRow = row;
}

/// S12-H1: the cap is global, not per download — two 256 KiB downloads
/// at 32 KiB/s together take ~16 s (512 KiB / 32 KiB/s), not ~8 s each.
#[tokio::test]
async fn speed_limit_is_global_across_concurrent_downloads() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(256 * 1024),
        chunk: 65536,
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    mgr.set_speed_limit(Some(32)).unwrap();
    let folder = dir.to_string_lossy().to_string();
    let (id1, _) = mgr.add_download(&server.url, Some("one.bin".to_string()), Some(folder.clone()), None, None).await.unwrap();
    let (id2, _) = mgr.add_download(&server.url, Some("two.bin".to_string()), Some(folder), None, None).await.unwrap();
    let start = Instant::now();
    mgr.start_download(&id1).await.unwrap();
    mgr.start_download(&id2).await.unwrap();
    assert!(wait_for_status(&mgr, &id1, "complete", 40).await);
    assert!(wait_for_status(&mgr, &id2, "complete", 40).await);
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_secs(12), "cap did not hold across both downloads: {elapsed:?}");
    assert!(elapsed < Duration::from_secs(30), "elapsed: {elapsed:?}");
    for id in [&id1, &id2] {
        let row = mgr.get_download(id).unwrap().unwrap();
        assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(256 * 1024));
    }
}

/// S12-H2: a chunk larger than one second of rate still completes —
/// the round-1 limiter fix, pinned at integration level (64 KiB
/// chunks at a 16 KiB/s cap; a naive bucket would wait forever).
#[tokio::test]
async fn speed_limit_chunk_larger_than_rate_completes() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(128 * 1024),
        chunk: 65536,
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    mgr.set_speed_limit(Some(16)).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    let start = Instant::now();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_secs(5), "finished too fast for a 16 KiB/s cap: {elapsed:?}");
    assert!(elapsed < Duration::from_secs(20), "elapsed: {elapsed:?}");
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(128 * 1024));
}

/// S09-P1: an HTTP proxy that demands auth rejects wrong credentials —
/// the probe fails with 407 and no download is created.
#[tokio::test]
async fn http_proxy_wrong_credentials_are_rejected() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start_with_auth(Some(("alice", "hunter2")));
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    // Right credentials: the download goes through.
    let mut good = proxy_input("http", "127.0.0.1", proxy.port as i64);
    good.username = Some("alice".to_string());
    good.password = Some("hunter2".to_string());
    mgr.save_proxy_config(&good).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst) > 0);
    // Wrong password: the probe is refused with 407.
    let dir2 = tmpdir();
    let store2 = Arc::new(Store::open(&dir2.join("app.db")).unwrap());
    let mgr2 = DownloadManager::new(Arc::clone(&store2), 4);
    let mut wrong = proxy_input("http", "127.0.0.1", proxy.port as i64);
    wrong.username = Some("alice".to_string());
    wrong.password = Some("wrong".to_string());
    mgr2.save_proxy_config(&wrong).unwrap();
    let err = mgr2
        .add_download(&server.url, None, Some(dir2.to_string_lossy().to_string()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("407"), "expected a 407 error, got: {err}");
}

/// S09-P2 (DOC): `use_for = ["https"]` means an http:// download goes
/// direct — the proxy is never contacted. Documented, not a failure:
/// the scope is doing what it says.
#[tokio::test]
async fn proxy_https_only_scope_skips_http_downloads() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.use_for = vec!["https".to_string()];
    mgr.save_proxy_config(&input).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert_eq!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst), 0, "https-only proxy was used for http");
}

/// S09-P3: `use_for = ["http"]` routes http:// downloads through the proxy.
#[tokio::test]
async fn proxy_http_only_scope_uses_proxy_for_http() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.use_for = vec!["http".to_string()];
    mgr.save_proxy_config(&input).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    assert!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst) > 0, "http-only proxy was not used");
}

/// S09-P4: Download All's page fetch goes through the configured proxy
/// too — it uses the same engine client as probe and download.
#[tokio::test]
async fn fetch_page_links_goes_through_proxy() {
    let page = PageServer::start();
    let proxy = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    mgr.save_proxy_config(&proxy_input("http", "127.0.0.1", proxy.port as i64)).unwrap();
    let links = mgr.fetch_page_links(&page.page_url).await.unwrap();
    assert!(!links.is_empty(), "expected links from the test page");
    assert!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst) > 0, "page fetch bypassed the proxy");
}

/// S09-P5: saving a new proxy applies to the next download — the old
/// proxy stops being used, the new one starts, with no restart.
#[tokio::test]
async fn proxy_change_applies_to_next_download() {
    let server = MockServer::start(MockConfig::default());
    let proxy_a = HttpProxyServer::start();
    let proxy_b = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let folder = dir.to_string_lossy().to_string();
    mgr.save_proxy_config(&proxy_input("http", "127.0.0.1", proxy_a.port as i64)).unwrap();
    let (id1, _) = mgr.add_download(&server.url, Some("a.bin".to_string()), Some(folder.clone()), None, None).await.unwrap();
    mgr.start_download(&id1).await.unwrap();
    assert!(wait_for_status(&mgr, &id1, "complete", 30).await);
    assert!(proxy_a.hits.load(std::sync::atomic::Ordering::SeqCst) > 0);
    assert_eq!(proxy_b.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    let a_hits = proxy_a.hits.load(std::sync::atomic::Ordering::SeqCst);
    mgr.save_proxy_config(&proxy_input("http", "127.0.0.1", proxy_b.port as i64)).unwrap();
    let (id2, _) = mgr.add_download(&server.url, Some("b.bin".to_string()), Some(folder), None, None).await.unwrap();
    mgr.start_download(&id2).await.unwrap();
    assert!(wait_for_status(&mgr, &id2, "complete", 30).await);
    assert!(proxy_b.hits.load(std::sync::atomic::Ordering::SeqCst) > 0, "new proxy was not used");
    assert_eq!(proxy_a.hits.load(std::sync::atomic::Ordering::SeqCst), a_hits, "old proxy was still used");
}

/// BUG-010 (was S09-P6, DOC): `test_proxy` with an https-only proxy
/// and an http:// test URL must fail with a clear scope error instead
/// of reporting success for a proxy it never used (0 hits).
#[tokio::test]
async fn test_proxy_https_only_scope_rejects_http_test_url() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.use_for = vec!["https".to_string()];
    let err = mgr.test_proxy(&input, &server.url).await.unwrap_err();
    assert!(
        err.to_string().contains("only used for"),
        "expected a scope error, got: {err}"
    );
    assert_eq!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst), 0, "test went through the proxy after all");
}

/// BUG-010 positive case: a test URL whose scheme the proxy covers
/// really does go through the proxy and returns its status.
#[tokio::test]
async fn test_proxy_http_scope_uses_proxy_for_http_url() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut input = proxy_input("http", "127.0.0.1", proxy.port as i64);
    input.use_for = vec!["http".to_string()];
    let status = mgr.test_proxy(&input, &server.url).await.unwrap();
    assert_eq!(status, 200);
    assert!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst) > 0, "test never reached the proxy");
}

/// BUG-010: an FTP proxy cannot be tested with an HTTP(S) URL — the
/// engine has no FTP transport, so the test must say so instead of
/// going direct and reporting success.
#[tokio::test]
async fn test_proxy_ftp_type_is_rejected() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let input = proxy_input("ftp", "127.0.0.1", 2121);
    let err = mgr.test_proxy(&input, &server.url).await.unwrap_err();
    assert!(err.to_string().contains("FTP"), "expected an FTP error, got: {err}");
}

/// BUG-009 (was S09-P7, DOC): `test_proxy` with wrong proxy
/// credentials must fail — a 407 is an authentication failure, not a
/// success the UI can label "OK".
#[tokio::test]
async fn test_proxy_wrong_credentials_is_an_error() {
    let server = MockServer::start(MockConfig::default());
    let proxy = HttpProxyServer::start_with_auth(Some(("alice", "hunter2")));
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut wrong = proxy_input("http", "127.0.0.1", proxy.port as i64);
    wrong.username = Some("alice".to_string());
    wrong.password = Some("wrong".to_string());
    let err = mgr.test_proxy(&wrong, &server.url).await.unwrap_err();
    assert!(
        err.to_string().contains("407") && err.to_string().contains("authentication"),
        "expected a 407 authentication error, got: {err}"
    );
    assert_eq!(proxy.hits.load(std::sync::atomic::Ordering::SeqCst), 0, "refused request counted as a hit");
}

/// Serve one fixed HTML page once, on a local port, then stop.
/// Used for fetch_page_links tests that need links the fixed
/// PageServer cannot serve (e.g. links to an excluded host).
fn serve_one_page(html: &str) -> String {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = html.to_string();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    format!("http://127.0.0.1:{port}/page.html")
}

/// BUG-011: `file_types_auto` drives auto-assign. An empty list means
/// every extension is auto-assigned (the old behaviour); a non-empty
/// list restricts auto-assign to the listed extensions, and anything
/// else falls back to Documents.
#[tokio::test]
async fn file_types_auto_limits_auto_assign() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let folder = dir.to_string_lossy().to_string();
    // Empty list: every extension auto-assigns to its category.
    let (id, _) = mgr
        .add_download(&server.url, Some("pack.zip".to_string()), Some(folder.clone()), None, None)
        .await
        .unwrap();
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().category_id, "cat-compressed");
    // Restricted list: only mp4 is auto-assigned; zip falls back.
    let mut settings = store.get_settings().unwrap();
    settings.file_types_auto = vec!["mp4".to_string()];
    mgr.update_settings(&settings).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, Some("clip.mp4".to_string()), Some(folder.clone()), None, None)
        .await
        .unwrap();
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().category_id, "cat-video");
    let (id, _) = mgr
        .add_download(&server.url, Some("pack2.zip".to_string()), Some(folder), None, None)
        .await
        .unwrap();
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().category_id, "cat-documents");
}

/// BUG-011: `excluded_sites` blocks downloads from the listed hosts
/// (exact host and subdomains), before any network request is made.
#[tokio::test]
async fn excluded_sites_blocks_add_download() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let folder = dir.to_string_lossy().to_string();
    let mut settings = store.get_settings().unwrap();
    settings.excluded_sites = vec!["127.0.0.1".to_string()];
    mgr.update_settings(&settings).unwrap();
    let err = mgr
        .add_download(&server.url, None, Some(folder.clone()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("excluded"), "expected an excluded-site error, got: {err}");
    // Subdomain match, with no network attempt (the host does not
    // resolve here, so a probe would fail differently).
    settings.excluded_sites = vec!["example.com".to_string()];
    mgr.update_settings(&settings).unwrap();
    let err = mgr
        .add_download("http://sub.example.com/file.bin", None, Some(folder.clone()), None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("excluded"), "expected an excluded-site error, got: {err}");
    // A host that is not excluded still works.
    let (id, _) = mgr
        .add_download(&server.url, None, Some(folder), None, None)
        .await
        .unwrap();
    assert!(mgr.get_download(&id).unwrap().is_some());
}

/// BUG-011: fetch_page_links refuses an excluded page outright and
/// flags links to excluded hosts with an error instead of probing them.
#[tokio::test]
async fn fetch_page_links_flags_excluded_sites() {
    let page = PageServer::start();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut settings = store.get_settings().unwrap();
    settings.excluded_sites = vec!["127.0.0.1".to_string()];
    mgr.update_settings(&settings).unwrap();
    let err = mgr.fetch_page_links(&page.page_url).await.unwrap_err();
    assert!(err.to_string().contains("excluded"), "expected an excluded-site error, got: {err}");
    settings.excluded_sites = vec!["excluded.example".to_string()];
    mgr.update_settings(&settings).unwrap();
    let html = format!(
        "<html><body><a href=\"http://excluded.example/file.zip\">x</a>\
         <a href=\"{}/video.mp4\">v</a></body></html>",
        page.base
    );
    let url = serve_one_page(&html);
    let links = mgr.fetch_page_links(&url).await.unwrap();
    let bad = links.iter().find(|l| l.url.contains("excluded.example")).unwrap();
    assert!(
        bad.error.as_deref().unwrap_or("").contains("excluded"),
        "excluded link should carry an error, got: {bad:?}"
    );
    let good = links.iter().find(|l| l.url.ends_with("/video.mp4")).unwrap();
    assert!(good.error.is_none(), "clean link should have no error, got: {good:?}");
}

/// BUG-011: a batch line pointing at an excluded site is reported
/// with its line number and skipped; the rest of the batch is added.
#[tokio::test]
async fn batch_add_flags_excluded_site_lines() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut settings = store.get_settings().unwrap();
    settings.excluded_sites = vec!["example.com".to_string()];
    mgr.update_settings(&settings).unwrap();
    let folder = dir.to_string_lossy().to_string();
    let result = mgr
        .add_batch_downloads(
            &[server.url.clone(), "http://example.com/file.bin".to_string()],
            Some(folder),
            None,
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(result.created.len(), 1);
    assert_eq!(result.invalid.len(), 1);
    assert_eq!(result.invalid[0].line, 2);
    assert!(result.invalid[0].error.contains("excluded"), "error: {}", result.invalid[0].error);
}

/// BUG-011: settings normalise the two lists on save — extensions
/// lowercased without dots and de-duplicated, hosts lowercased
/// without a scheme or path, blanks dropped.
#[tokio::test]
async fn update_settings_normalizes_file_types_and_excluded_sites() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let mut s = store.get_settings().unwrap();
    s.file_types_auto = vec![
        "MP4".to_string(),
        ".mkv".to_string(),
        "  ".to_string(),
        "mp4".to_string(),
    ];
    s.excluded_sites = vec![
        "Example.COM ".to_string(),
        "https://foo.org/some/path".to_string(),
        "".to_string(),
    ];
    let saved = store.update_settings(&s).unwrap();
    assert_eq!(saved.file_types_auto, vec!["mp4".to_string(), "mkv".to_string()]);
    assert_eq!(saved.excluded_sites, vec!["example.com".to_string(), "foo.org".to_string()]);
}

/// S09-I1: a queue drain honours the current settings — with
/// max_connections = 1 a 2 MiB file is a single segment, and with a
/// speed cap the drain takes as long as the cap says.
#[tokio::test]
async fn queue_drain_honours_max_connections_and_speed_limit() {
    // Part 1: segment count from settings.
    let server = MockServer::start(MockConfig {
        size_bytes: Some(2 * 1024 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut s = mgr.get_settings().unwrap();
    s.max_connections = 1;
    mgr.update_settings(&s).unwrap();
    let queue = mgr.create_queue("drain-test").unwrap();
    let id1 = add_queued(&mgr, &server.url, &dir, "big1.bin", &queue.id).await;
    let id2 = add_queued(&mgr, &server.url, &dir, "big2.bin", &queue.id).await;
    assert_eq!(store.get_segments(&id1).unwrap().len(), 1);
    assert_eq!(store.get_segments(&id2).unwrap().len(), 1);
    mgr.start_queue(&queue.id).await.unwrap();
    assert!(wait_for_queue_drained(&mgr, &queue.id, 30).await);
    for id in [&id1, &id2] {
        let row = mgr.get_download(id).unwrap().unwrap();
        assert_eq!(row.status, "complete");
        assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(2 * 1024 * 1024));
    }
    // Part 2: speed cap during a drain (2 x 128 KiB at 64 KiB/s ~= 4 s).
    let small = MockServer::start(MockConfig {
        size_bytes: Some(128 * 1024),
        ..Default::default()
    });
    let dir2 = tmpdir();
    let store2 = Arc::new(Store::open(&dir2.join("app.db")).unwrap());
    let mgr2 = DownloadManager::new(Arc::clone(&store2), 4);
    mgr2.set_speed_limit(Some(64)).unwrap();
    let queue2 = mgr2.create_queue("capped").unwrap();
    add_queued(&mgr2, &small.url, &dir2, "s1.bin", &queue2.id).await;
    add_queued(&mgr2, &small.url, &dir2, "s2.bin", &queue2.id).await;
    let start = Instant::now();
    mgr2.start_queue(&queue2.id).await.unwrap();
    assert!(wait_for_queue_drained(&mgr2, &queue2.id, 30).await);
    assert!(start.elapsed() >= Duration::from_secs(3), "queue drained too fast under a 64 KiB/s cap: {:?}", start.elapsed());
}

/// S09-I2: editing a category's folder does NOT move files that are
/// already downloaded — only new downloads use the new folder.
#[tokio::test]
async fn category_folder_edit_does_not_move_completed_files() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let old_dir = dir.join("books-old");
    let cat = mgr.create_category("Books", &old_dir.to_string_lossy(), &["xyz".to_string()]).unwrap();
    let (id, _) = mgr
        .add_download(&server.url, Some("book.xyz".to_string()), None, None, None)
        .await
        .unwrap();
    mgr.start_download(&id).await.unwrap();
    assert!(wait_for_status(&mgr, &id, "complete", 30).await);
    let old_path = mgr.get_download(&id).unwrap().unwrap().save_path;
    assert!(std::path::Path::new(&old_path).starts_with(&old_dir), "save_path: {old_path}");
    // Move the category's folder; the finished file stays put.
    let new_dir = dir.join("books-new");
    mgr.update_category(&cat.id, Some(&new_dir.to_string_lossy()), None).unwrap();
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.save_path, old_path, "completed file was moved by a category edit");
    assert!(std::path::Path::new(&old_path).exists(), "completed file vanished from its folder");
    // A new download in the same category uses the new folder.
    let (id2, _) = mgr
        .add_download(&server.url, Some("book2.xyz".to_string()), None, None, None)
        .await
        .unwrap();
    let row2 = mgr.get_download(&id2).unwrap().unwrap();
    assert!(std::path::Path::new(&row2.save_path).starts_with(&new_dir), "save_path: {}", row2.save_path);
}

/// S09-I3: the scheduler still drains a queue with the new settings
/// applied (2 connections, a speed cap in place).
#[tokio::test]
async fn scheduler_tick_drains_queue_with_settings_applied() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(128 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    let mut s = mgr.get_settings().unwrap();
    s.max_connections = 2;
    mgr.update_settings(&s).unwrap();
    mgr.set_speed_limit(Some(256)).unwrap();
    let queue = mgr.create_queue("scheduled").unwrap();
    add_queued(&mgr, &server.url, &dir, "sched1.bin", &queue.id).await;
    add_queued(&mgr, &server.url, &dir, "sched2.bin", &queue.id).await;
    mgr.update_queue(&queue.id, "one_time", Some(&iso_in(-10)), None, false, "none", None, None).await.unwrap();
    mgr.scheduler_tick_at(now_unix()).await.unwrap();
    assert!(wait_for_queue_drained(&mgr, &queue.id, 30).await);
    let files = mgr.list_queue_files(&queue.id).unwrap();
    assert!(files.is_empty(), "queue did not drain under the new settings");
}
