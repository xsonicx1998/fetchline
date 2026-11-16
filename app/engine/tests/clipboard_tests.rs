//! Clipboard monitoring (Round 52): the engine half. Pure logic —
//! no test ever touches the real clipboard; the text is handed to
//! `DownloadManager::check_clipboard_offer` directly.

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
        .check_clipboard_offer("https://example.com/videos/movie.mp4")
        .unwrap()
        .expect("a .mp4 link is offered");
    assert_eq!(offer.url, "https://example.com/videos/movie.mp4");
    assert_eq!(offer.filename, "movie.mp4");
}

#[tokio::test]
async fn clipboard_zip_url_offers_with_percent_decoded_filename() {
    let (_dir, _store, mgr) = fresh_mgr();
    let offer = mgr
        .check_clipboard_offer("https://example.com/files/my%20archive.zip")
        .unwrap()
        .expect("a .zip link is offered");
    assert_eq!(offer.filename, "my archive.zip");
}

#[tokio::test]
async fn clipboard_plain_text_offers_nothing() {
    let (_dir, _store, mgr) = fresh_mgr();
    assert!(mgr.check_clipboard_offer("hello world").unwrap().is_none());
    assert!(mgr.check_clipboard_offer("").unwrap().is_none());
    assert!(mgr.check_clipboard_offer("   ").unwrap().is_none());
}

#[tokio::test]
async fn clipboard_non_file_url_offers_nothing() {
    let (_dir, _store, mgr) = fresh_mgr();
    // A web page, a bare host, and a URL with no file extension.
    assert!(mgr
        .check_clipboard_offer("https://example.com/page")
        .unwrap()
        .is_none());
    assert!(mgr
        .check_clipboard_offer("https://example.com/")
        .unwrap()
        .is_none());
    assert!(mgr
        .check_clipboard_offer("https://example.com/image.png")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn clipboard_multi_line_paste_offers_nothing() {
    let (_dir, _store, mgr) = fresh_mgr();
    assert!(mgr
        .check_clipboard_offer("https://example.com/a.mp4\nhttps://example.com/b.mp4")
        .unwrap()
        .is_none());
    // A link with a stray space is not one link either.
    assert!(mgr
        .check_clipboard_offer("https://example.com/my file.mp4")
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn clipboard_ftp_url_with_file_extension_is_offered() {
    let (_dir, _store, mgr) = fresh_mgr();
    let offer = mgr
        .check_clipboard_offer("ftp://ftp.example.com/pub/setup.exe")
        .unwrap()
        .expect("an ftp .exe link is offered");
    assert_eq!(offer.filename, "setup.exe");
}

#[tokio::test]
async fn clipboard_uppercase_extension_is_offered() {
    let (_dir, _store, mgr) = fresh_mgr();
    let offer = mgr
        .check_clipboard_offer("https://example.com/files/Report.PDF")
        .unwrap()
        .expect("an uppercase .PDF link is offered");
    assert_eq!(offer.filename, "Report.PDF");
}

#[tokio::test]
async fn clipboard_too_long_text_offers_nothing() {
    let (_dir, _store, mgr) = fresh_mgr();
    let long = format!("https://example.com/{}.zip", "a".repeat(2048));
    assert!(mgr.check_clipboard_offer(&long).unwrap().is_none());
}

// -- dedupe ---------------------------------------------------------------

#[tokio::test]
async fn clipboard_same_text_twice_offers_once() {
    let (_dir, _store, mgr) = fresh_mgr();
    let text = "https://example.com/videos/movie.mp4";
    assert!(mgr.check_clipboard_offer(text).unwrap().is_some());
    assert!(mgr.check_clipboard_offer(text).unwrap().is_none());
}

#[tokio::test]
async fn clipboard_same_url_recopied_after_other_text_offers_nothing() {
    let (_dir, _store, mgr) = fresh_mgr();
    let url = "https://example.com/videos/movie.mp4";
    assert!(mgr.check_clipboard_offer(url).unwrap().is_some());
    // A different clipboard text in between does not reset the offer.
    assert!(mgr
        .check_clipboard_offer("https://example.com/other.zip")
        .unwrap()
        .is_some());
    assert!(mgr.check_clipboard_offer(url).unwrap().is_none());
}

#[tokio::test]
async fn clipboard_dismissed_url_is_never_offered_again() {
    let (_dir, _store, mgr) = fresh_mgr();
    let url = "https://example.com/videos/movie.mp4";
    assert!(mgr.check_clipboard_offer(url).unwrap().is_some());
    mgr.dismiss_clipboard_offer(url);
    // Re-copying the same link after a dismiss offers nothing —
    // dismiss is idempotent, so calling it twice is fine too.
    mgr.dismiss_clipboard_offer(url);
    assert!(mgr
        .check_clipboard_offer("https://example.com/other.zip")
        .unwrap()
        .is_some());
    assert!(mgr.check_clipboard_offer(url).unwrap().is_none());
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
        .check_clipboard_offer("https://example.com/videos/movie.mp4")
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
        .check_clipboard_offer("https://example.com/videos/movie.mp4")
        .unwrap()
        .is_none());
    // A subdomain of an excluded site is excluded too (BUG-011).
    assert!(mgr
        .check_clipboard_offer("https://cdn.example.com/videos/movie.mp4")
        .unwrap()
        .is_none());
    // Another site is still offered.
    assert!(mgr
        .check_clipboard_offer("https://other.example/videos/movie.mp4")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn clipboard_already_downloaded_url_offers_nothing() {
    let server = MockServer::start(MockConfig::default());
    let (dir, _store, mgr) = fresh_mgr();
    // A .mp4 on the mock server: the mock serves any path.
    let url = server.url.replace("file.bin", "movie.mp4");
    let (_id, _) = mgr
        .add_download(&url, None, Some(dir.to_string_lossy().to_string()), None, None)
        .await
        .unwrap();
    assert!(mgr.check_clipboard_offer(&url).unwrap().is_none());
}
