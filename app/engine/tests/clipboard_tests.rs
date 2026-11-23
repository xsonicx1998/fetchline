//! Clipboard monitoring (Round 52, reworked Round 75): the engine
//! half. Pure logic — no test ever touches the real clipboard; the
//! text is handed to `DownloadManager::check_clipboard_offer`
//! directly, together with a FAKE sequence number (the test is the
//! sequence source: `Some(n)` stands in for the OS clipboard
//! sequence the shell reads on Windows; `None` is the
//! no-sequence fallback, which evaluates the text every time).

#![allow(dead_code)] // the shared mock also serves tests this file does not use

#[path = "common/mock.rs"]
mod mock;

use std::sync::Arc;

use mock::*;

use fetchline_engine::{DownloadManager, Store};

fn fresh_mgr() -> (std::path::PathBuf, Arc<Store>, Arc<DownloadManager>) {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mgr = DownloadManager::new(Arc::clone(&store), 4);
    (dir, store, mgr)
}

// -- detection ------------------------------------------------------------

#[tokio::test]
async fn clipboard_file_url_offers_with_filename() {
    let (_dir, _store, mgr) = fresh_mgr();
    let offer = mgr
        .check_clipboard_offer("https://example.com/videos/movie.mp4", None)
        .unwrap()
        .expect("a .mp4 link is offered");
    assert_eq!(offer.url, "https://example.com/videos/movie.mp4");
    assert_eq!(offer.filename, "movie.mp4");
}

#[tokio::test]
async fn clipboard_zip_url_offers_with_percent_decoded_filename() {
    let (_dir, _store, mgr) = fresh_mgr();
    let offer = mgr
        .check_clipboard_offer("https://example.com/files/my%20archive.zip", None)
        .unwrap()
        .expect("a .zip link is offered");
    assert_eq!(offer.filename, "my archive.zip");
}

#[tokio::test]
async fn clipboard_plain_text_offers_nothing() {
    let (_dir, _store, mgr) = fresh_mgr();
    assert!(mgr
        .check_clipboard_offer("hello world", None)
        .unwrap()
        .is_none());
    assert!(mgr.check_clipboard_offer("", None).unwrap().is_none());
    assert!(mgr.check_clipboard_offer("   ", None).unwrap().is_none());
}

#[tokio::test]
async fn clipboard_non_file_url_offers_nothing() {
    let (_dir, _store, mgr) = fresh_mgr();
    // A web page, a bare host, and a URL with no file extension.
    assert!(mgr
        .check_clipboard_offer("https://example.com/page", None)
        .unwrap()
        .is_none());
    assert!(mgr
        .check_clipboard_offer("https://example.com/", None)
        .unwrap()
        .is_none());
    assert!(mgr
        .check_clipboard_offer("https://example.com/image.png", None)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn clipboard_multi_line_paste_offers_nothing() {
    let (_dir, _store, mgr) = fresh_mgr();
    assert!(mgr
        .check_clipboard_offer("https://example.com/a.mp4\nhttps://example.com/b.mp4", None)
        .unwrap()
        .is_none());
    // A link with a stray space is not one link either.
    assert!(mgr
        .check_clipboard_offer("https://example.com/my file.mp4", None)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn clipboard_ftp_url_with_file_extension_is_offered() {
    let (_dir, _store, mgr) = fresh_mgr();
    let offer = mgr
        .check_clipboard_offer("ftp://ftp.example.com/pub/setup.exe", None)
        .unwrap()
        .expect("an ftp .exe link is offered");
    assert_eq!(offer.filename, "setup.exe");
}

#[tokio::test]
async fn clipboard_uppercase_extension_is_offered() {
    let (_dir, _store, mgr) = fresh_mgr();
    let offer = mgr
        .check_clipboard_offer("https://example.com/files/Report.PDF", None)
        .unwrap()
        .expect("an uppercase .PDF link is offered");
    assert_eq!(offer.filename, "Report.PDF");
}

#[tokio::test]
async fn clipboard_too_long_text_offers_nothing() {
    let (_dir, _store, mgr) = fresh_mgr();
    let long = format!("https://example.com/{}.zip", "a".repeat(2048));
    assert!(mgr.check_clipboard_offer(&long, None).unwrap().is_none());
}

// -- dedupe: the sequence number (Round 75) ---------------------
//
// The old hash + `seen` + dismiss blocks are gone: a fresh copy of
// the same link is a NEW sequence and is offered again; the same
// clipboard sitting unchanged across polls is offered once.

#[tokio::test]
async fn clipboard_same_text_twice_offers_once() {
    let (_dir, _store, mgr) = fresh_mgr();
    let text = "https://example.com/videos/movie.mp4";
    // Same text, same sequence (the clipboard did not change):
    // offered once, then nothing — 5 polls, 1 offer.
    assert!(mgr.check_clipboard_offer(text, Some(1)).unwrap().is_some());
    for _ in 0..4 {
        assert!(mgr.check_clipboard_offer(text, Some(1)).unwrap().is_none());
    }
}

#[tokio::test]
async fn clipboard_same_text_recopied_with_new_sequence_offers_again() {
    let (_dir, _store, mgr) = fresh_mgr();
    let text = "https://example.com/videos/movie.mp4";
    // The user's PC bug: copy once, dismiss, copy again — the
    // second copy must offer again, because the sequence changed.
    assert!(mgr.check_clipboard_offer(text, Some(1)).unwrap().is_some());
    assert!(mgr.check_clipboard_offer(text, Some(1)).unwrap().is_none());
    assert!(mgr.check_clipboard_offer(text, Some(2)).unwrap().is_some());
    assert!(mgr.check_clipboard_offer(text, Some(3)).unwrap().is_some());
}

#[tokio::test]
async fn clipboard_same_url_recopied_after_other_text_offers_again() {
    let (_dir, _store, mgr) = fresh_mgr();
    let url = "https://example.com/videos/movie.mp4";
    assert!(mgr.check_clipboard_offer(url, Some(1)).unwrap().is_some());
    // A different clipboard text in between, then the first URL
    // copied again — a new sequence, so it offers again.
    assert!(mgr
        .check_clipboard_offer("https://example.com/other.zip", Some(2))
        .unwrap()
        .is_some());
    assert!(mgr.check_clipboard_offer(url, Some(3)).unwrap().is_some());
}

#[tokio::test]
async fn clipboard_sequence_seen_matches_the_last_recorded_sequence() {
    let (_dir, _store, mgr) = fresh_mgr();
    assert!(!mgr.clipboard_sequence_seen(1));
    mgr.check_clipboard_offer("https://example.com/videos/movie.mp4", Some(1))
        .unwrap();
    assert!(mgr.clipboard_sequence_seen(1));
    assert!(!mgr.clipboard_sequence_seen(2));
}

#[tokio::test]
async fn clipboard_own_write_sequence_is_not_offered() {
    let (_dir, _store, mgr) = fresh_mgr();
    let url = "https://example.com/videos/movie.mp4";
    // The app itself wrote this to the clipboard (the update
    // dialog copies the installer URL) and recorded the
    // sequence right after: the next poll must not offer our
    // own link back.
    mgr.note_clipboard_sequence(7);
    assert!(mgr.check_clipboard_offer(url, Some(7)).unwrap().is_none());
    // A later fresh copy of the same link still offers.
    assert!(mgr.check_clipboard_offer(url, Some(8)).unwrap().is_some());
}

// -- settings, exclusion, already-downloaded ------------------------------

#[tokio::test]
async fn clipboard_monitoring_defaults_on_and_round_trips() {
    let (_dir, store, mgr) = fresh_mgr();
    // Fresh DB (schema.sql default) and Settings::default() agree.
    assert!(store.get_settings().unwrap().clipboard_monitoring);
    let mut settings = store.get_settings().unwrap();
    settings.clipboard_monitoring = false;
    let saved = mgr.update_settings(&settings).unwrap();
    assert!(!saved.clipboard_monitoring);
    assert!(!store.get_settings().unwrap().clipboard_monitoring);
    settings.clipboard_monitoring = true;
    mgr.update_settings(&settings).unwrap();
    assert!(store.get_settings().unwrap().clipboard_monitoring);
}

#[tokio::test]
async fn clipboard_toggle_off_offers_nothing() {
    let (_dir, store, mgr) = fresh_mgr();
    let mut settings = store.get_settings().unwrap();
    settings.clipboard_monitoring = false;
    mgr.update_settings(&settings).unwrap();
    assert!(mgr
        .check_clipboard_offer("https://example.com/videos/movie.mp4", None)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn clipboard_excluded_site_offers_nothing() {
    let (_dir, store, mgr) = fresh_mgr();
    let mut settings = store.get_settings().unwrap();
    settings.excluded_sites = vec!["example.com".to_string()];
    mgr.update_settings(&settings).unwrap();
    assert!(mgr
        .check_clipboard_offer("https://example.com/videos/movie.mp4", None)
        .unwrap()
        .is_none());
    // A subdomain of an excluded site is excluded too (BUG-011).
    assert!(mgr
        .check_clipboard_offer("https://cdn.example.com/videos/movie.mp4", None)
        .unwrap()
        .is_none());
    // Another site is still offered.
    assert!(mgr
        .check_clipboard_offer("https://other.example/videos/movie.mp4", None)
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn clipboard_already_downloaded_url_offers_with_already_in_list_flag() {
    let server = MockServer::start(MockConfig::default());
    let (dir, _store, mgr) = fresh_mgr();
    // A .mp4 on the mock server: the mock serves any path.
    let url = server.url.replace("file.bin", "movie.mp4");
    let (_id, _) = mgr
        .add_download(
            &url,
            None,
            Some(dir.to_string_lossy().to_string()),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    // Round 75: an already-downloaded URL is still offered — with
    // the flag, so the Add URL window can show "This link is
    // already in your list" and let the user continue or cancel.
    let offer = mgr
        .check_clipboard_offer(&url, Some(1))
        .unwrap()
        .expect("an already-downloaded URL is still offered");
    assert!(offer.already_in_list);
    // A fresh URL is offered with the flag clear.
    let fresh = mgr
        .check_clipboard_offer("https://example.com/videos/other.mp4", Some(2))
        .unwrap()
        .expect("a fresh URL is offered");
    assert!(!fresh.already_in_list);
}
