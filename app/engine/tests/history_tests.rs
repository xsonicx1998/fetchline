//! Download history (Round 53): the engine half. Finished
//! downloads are recorded when they complete (through the real
//! manager path, never by hand), survive deleting the download,
//! are searchable by filename or URL, can be cleared, and can be
//! re-downloaded from the stored URL.

#![allow(dead_code)] // the shared mock also serves tests this file does not use

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;
use std::time::Duration;

use mock::*;

use fetchline_engine::{DownloadManager, Store};

fn fresh_mgr() -> (std::path::PathBuf, Arc<Store>, Arc<DownloadManager>) {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    (dir, store, mgr)
}

/// A real download driven to completion through the manager
/// against the local mock server. Returns the download id, the
/// server (kept alive by the caller), and the manager pieces.
async fn complete_download(
    dir: &std::path::Path,
    store: &Arc<Store>,
    mgr: &Arc<DownloadManager>,
) -> (String, MockServer) {
    let server = MockServer::start(MockConfig {
        chunk: 65536,
        delay: Duration::ZERO,
        hide_length: false,
        ..Default::default()
    });
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
    assert!(wait_for_status(mgr, &id, "complete", 30).await);
    let _ = store;
    (id, server)
}

// -- recording ------------------------------------------------------------

#[tokio::test]
async fn history_records_a_real_completed_download() {
    let (dir, store, mgr) = fresh_mgr();
    let (id, server) = complete_download(&dir, &store, &mgr).await;
    let entries = store.list_history("").unwrap();
    assert_eq!(entries.len(), 1, "one history row after one completion");
    let entry = &entries[0];
    assert_eq!(entry.download_id, id);
    assert_eq!(entry.filename, "file.bin");
    assert_eq!(entry.url, server.url);
    assert_eq!(entry.final_url.as_deref(), Some(server.url.as_str()));
    assert_eq!(entry.size_bytes, Some(512 * 1024));
    assert_eq!(entry.folder, dir.to_string_lossy());
    assert!(!entry.completed_at.is_empty());
    // The finished file is really there, in the recorded folder.
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert!(std::path::Path::new(&row.save_path).exists());
    assert_eq!(std::fs::read(&row.save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn history_recording_twice_keeps_one_row() {
    let (dir, store, mgr) = fresh_mgr();
    let (id, _server) = complete_download(&dir, &store, &mgr).await;
    assert_eq!(store.list_history("").unwrap().len(), 1);
    // Record the same finished download twice more by hand.
    let row = mgr.get_download(&id).unwrap().unwrap();
    store.record_history(&row).unwrap();
    store.record_history(&row).unwrap();
    let entries = store.list_history("").unwrap();
    assert_eq!(
        entries.len(),
        1,
        "UNIQUE(download_id) keeps exactly one row"
    );
    assert_eq!(entries[0].download_id, id);
}

#[tokio::test]
async fn history_survives_deleting_the_download() {
    let (dir, store, mgr) = fresh_mgr();
    let (id, _server) = complete_download(&dir, &store, &mgr).await;
    // Deleting a download only sets status 'deleted' (the same
    // call the Tauri delete_download command makes).
    store.set_status(&id, "deleted", None).unwrap();
    assert!(mgr.list_downloads().unwrap().iter().all(|r| r.id != id));
    let entries = store.list_history("").unwrap();
    assert_eq!(
        entries.len(),
        1,
        "deleting the download must not touch history"
    );
    assert_eq!(entries[0].download_id, id);
    assert_eq!(entries[0].filename, "file.bin");
}

// -- search ---------------------------------------------------------------

#[tokio::test]
async fn history_search_matches_filename_and_url() {
    let (dir, store, mgr) = fresh_mgr();
    let (_id, server) = complete_download(&dir, &store, &mgr).await;
    assert_eq!(store.list_history("file.bin").unwrap().len(), 1);
    assert_eq!(store.list_history("file").unwrap().len(), 1);
    assert_eq!(store.list_history("127.0.0.1").unwrap().len(), 1);
    assert_eq!(store.list_history(&server.url).unwrap().len(), 1);
    assert!(store
        .list_history("zzz-no-such-download")
        .unwrap()
        .is_empty());
    assert!(store.list_history("other.bin").unwrap().is_empty());
    // '%' is a LIKE wildcard: it must match literally, so a
    // query of '%' alone finds nothing (no filename has one).
    assert!(store.list_history("%").unwrap().is_empty());
    assert!(store.list_history("_").unwrap().is_empty());
}

// -- clear ----------------------------------------------------------------

#[tokio::test]
async fn history_clear_empties_the_table_only() {
    let (dir, store, mgr) = fresh_mgr();
    let (id, _server) = complete_download(&dir, &store, &mgr).await;
    assert_eq!(store.list_history("").unwrap().len(), 1);
    store.clear_history().unwrap();
    assert!(store.list_history("").unwrap().is_empty());
    assert!(store.list_history("file").unwrap().is_empty());
    // The download itself is untouched by clearing history.
    let row = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(row.status, "complete");
}

// -- re-download ----------------------------------------------------------

#[tokio::test]
async fn history_redownload_creates_a_new_download() {
    let (dir, store, mgr) = fresh_mgr();
    let (id, server) = complete_download(&dir, &store, &mgr).await;
    let entry = store.list_history("").unwrap().into_iter().next().unwrap();
    let (new_id, _info) = mgr.redownload_from_history(&entry.id).await.unwrap();
    assert_ne!(new_id, id, "re-download is a new download, not the old row");
    let new_row = mgr.get_download(&new_id).unwrap().unwrap();
    assert_eq!(new_row.url, server.url);
    assert_eq!(new_row.filename, "file.bin");
    assert_eq!(new_row.category_id, entry.category_id.unwrap());
    // And it really downloads, end to end.
    mgr.start_download(&new_id).await.unwrap();
    assert!(wait_for_status(&mgr, &new_id, "complete", 30).await);
    assert_eq!(
        std::fs::read(&new_row.save_path).unwrap(),
        payload(512 * 1024)
    );
    // The re-downloaded file is itself recorded in history.
    assert_eq!(store.list_history("").unwrap().len(), 2);
}

#[tokio::test]
async fn history_redownload_unknown_entry_errors() {
    let (_dir, _store, mgr) = fresh_mgr();
    assert!(mgr
        .redownload_from_history("no-such-history-id")
        .await
        .is_err());
    assert!(mgr.open_history_folder("no-such-history-id").is_err());
}

// -- open folder ----------------------------------------------------------

#[tokio::test]
async fn history_open_folder_errors_when_folder_is_gone() {
    let (dir, store, mgr) = fresh_mgr();
    let (id, _server) = complete_download(&dir, &store, &mgr).await;
    // Point the history row at a folder that does not exist: the
    // opener must refuse with a clear error, never launch anything.
    let mut row = mgr.get_download(&id).unwrap().unwrap();
    row.save_path = "/tmp/fetchline-test-no-such-folder-53/history-file.bin".to_string();
    store.record_history(&row).unwrap();
    let entry = store.list_history("").unwrap().into_iter().next().unwrap();
    assert_eq!(entry.folder, "/tmp/fetchline-test-no-such-folder-53");
    let err = mgr.open_history_folder(&entry.id).unwrap_err();
    assert!(err.to_string().contains("no longer exists"), "{err}");
}
