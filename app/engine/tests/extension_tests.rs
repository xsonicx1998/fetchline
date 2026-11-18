//! Browser-extension (S13 / F01) tests: native-messaging framing,
//! the pending-links queue, the host boundary (excluded sites +
//! browser-integration flag) and the drain through the real
//! `add_download` path. No public internet is used; downloads go
//! through the local mock HTTP server from `common/mock.rs`.

#![allow(dead_code)] // the shared mock also serves proxy tests this file does not use

#[path = "common/mock.rs"]
mod mock;

use std::io::{Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use mock::*;
use serde_json::json;

use fetchline_engine::native_host::{
    cookies_to_header, decode_message, encode_message, handle_host_message, host_of_url,
    CookiePair, HostMessage, MAX_MESSAGE_BYTES,
};
use fetchline_engine::{DownloadManager, Settings, Store};

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    // Drain-created downloads must land in the test dir, not the real
    // ~/Downloads: the category folders are empty in the seed data,
    // so add_download falls through to this setting.
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    DownloadManager::new(store, 4)
}

fn set_excluded(store: &Store, sites: &[&str]) {
    let mut s = store.get_settings().unwrap();
    s.excluded_sites = sites.iter().map(|x| x.to_string()).collect();
    store.update_settings(&s).unwrap();
}

fn set_browser_integration(store: &Store, on: bool) {
    let mut s = store.get_settings().unwrap();
    s.browser_integration = on;
    store.update_settings(&s).unwrap();
}

fn set_browser_start_drained(store: &Store, on: bool) {
    let mut s = store.get_settings().unwrap();
    s.browser_start_drained = on;
    store.update_settings(&s).unwrap();
}

// -- native-messaging framing (a) -----------------------------------------

#[test]
fn framing_round_trip_download_link() {
    let msg = json!({"type": "download_link", "url": "http://127.0.0.1/file.bin", "referrer": "http://127.0.0.1/page.html"});
    let bytes = encode_message(&msg);
    // 4-byte little-endian length prefix, then the JSON payload.
    assert_eq!(bytes.len(), 4 + msg.to_string().len());
    assert_eq!(
        u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize,
        msg.to_string().len()
    );
    let decoded = decode_message(&mut Cursor::new(bytes)).unwrap().unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn framing_empty_stream_is_clean_eof() {
    assert!(decode_message(&mut Cursor::new(Vec::new())).unwrap().is_none());
}

#[test]
fn framing_rejects_oversized_message() {
    let mut bytes = (MAX_MESSAGE_BYTES as u32 + 1).to_le_bytes().to_vec();
    bytes.extend_from_slice(b"{}");
    let err = decode_message(&mut Cursor::new(bytes)).unwrap_err();
    assert!(err.to_string().contains("too large"));
}

#[test]
fn framing_rejects_truncated_payload() {
    let mut bytes = 100u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(b"{bad");
    assert!(decode_message(&mut Cursor::new(bytes)).is_err());
}

#[test]
fn host_of_url_extracts_lowercase_host() {
    assert_eq!(
        host_of_url("https://Sub.Example.com/path?q=1").as_deref(),
        Some("sub.example.com")
    );
    assert_eq!(host_of_url("not a url"), None);
    assert_eq!(host_of_url("ftp://files.example.org/x.zip").as_deref(), Some("files.example.org"));
}

// -- host boundary: enqueue + excluded sites + integration flag (c) ------

#[test]
fn host_enqueues_download_link() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let reply = handle_host_message(
        &store,
        &HostMessage::DownloadLink {
            url: "https://example.com/file.zip".to_string(),
            referrer: Some("https://example.com/page".to_string()),
            cookies: vec![],
        },
    );
    assert!(reply.ok, "reply should be ok: {:?}", reply);
    let pending = store.list_pending_links(Some("pending")).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].url, "https://example.com/file.zip");
    assert_eq!(pending[0].referrer.as_deref(), Some("https://example.com/page"));
    assert_eq!(pending[0].source, "browser");
    assert_eq!(pending[0].status, "pending");
    assert_eq!(reply.pending_link_id.as_deref(), Some(pending[0].id.as_str()));
}

#[test]
fn host_rejects_invalid_url_without_enqueueing() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let reply = handle_host_message(
        &store,
        &HostMessage::DownloadLink {
            url: "not a url".to_string(),
            referrer: None,
            cookies: vec![],
        },
    );
    assert!(!reply.ok);
    assert!(store.list_pending_links(Some("pending")).unwrap().is_empty());
}

#[test]
fn host_rejects_excluded_site_without_enqueueing() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    set_excluded(&store, &["blocked.example"]);
    let reply = handle_host_message(
        &store,
        &HostMessage::DownloadLink {
            url: "https://sub.blocked.example/file.zip".to_string(),
            referrer: None,
            cookies: vec![],
        },
    );
    assert!(!reply.ok, "reply should refuse: {:?}", reply);
    assert!(reply.error.as_deref().unwrap().contains("excluded"));
    // The URL never reaches the queue.
    assert!(store.list_pending_links(None).unwrap().is_empty());
}

#[test]
fn host_rejects_when_browser_integration_is_off() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    set_browser_integration(&store, false);
    let reply = handle_host_message(
        &store,
        &HostMessage::DownloadLink {
            url: "https://example.com/file.zip".to_string(),
            referrer: None,
            cookies: vec![],
        },
    );
    assert!(!reply.ok);
    assert!(store.list_pending_links(None).unwrap().is_empty());
}

#[test]
fn host_ping_and_get_settings() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let pong = handle_host_message(&store, &HostMessage::Ping);
    assert!(pong.ok);
    let settings = handle_host_message(&store, &HostMessage::GetSettings);
    assert!(settings.ok);
    assert_eq!(settings.browser_integration, Some(true));
    assert_eq!(settings.excluded_sites.as_deref(), Some(&[] as &[String]));
}

#[test]
fn host_video_found_enqueues_a_video() {
    // S08: video_found is implemented — one accepted variant is
    // enqueued into pending_links with source "video". The full
    // validation matrix (DRM, bad scheme, excluded site, dedup,
    // empty variants) lives in video_tests.rs.
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let reply = handle_host_message(
        &store,
        &HostMessage::VideoFound {
            page_url: "https://example.com/watch".to_string(),
            variants: vec![fetchline_engine::video::VideoVariantInput {
                url: "https://example.com/clip.mp4".to_string(),
                quality: Some("720p".to_string()),
                format: Some("MP4".to_string()),
                live: false,
                drm: false,
                note: None,
            }],
            cookies: vec![],
        },
    );
    assert!(reply.ok, "video_found must succeed: {:?}", reply);
    assert_eq!(reply.enqueued, Some(1));
    let pending = store.list_pending_links(Some("pending")).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].source, "video");
}

#[test]
fn host_download_all_with_links_enqueues_each() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let reply = handle_host_message(
        &store,
        &HostMessage::DownloadAll {
            page_url: "https://example.com/page.html".to_string(),
            links: vec![
                "https://example.com/a.zip".to_string(),
                "https://example.com/b.zip".to_string(),
            ],
        },
    );
    assert!(reply.ok, "reply: {:?}", reply);
    assert_eq!(reply.enqueued, Some(2));
    let pending = store.list_pending_links(Some("pending")).unwrap();
    assert_eq!(pending.len(), 2);
    assert!(pending.iter().all(|p| p.source == "download_all"));
    assert!(pending.iter().all(|p| p.referrer.as_deref() == Some("https://example.com/page.html")));
}

// -- drain through the real add_download path (b, d) ----------------------

#[tokio::test]
async fn drain_leaves_browser_capture_for_the_add_url_dialog() {
    // Round 67: a browser capture is INTERACTIVE — the drain
    // leaves it pending for the Add URL dialog (like IDM) instead
    // of auto-adding it, and the dialog confirms it through the
    // real add path (cookies, probe, category auto-assign, start).
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let reply = handle_host_message(
        store,
        &HostMessage::DownloadLink {
            url: server.url.clone(),
            referrer: Some("http://127.0.0.1/page.html".to_string()),
            cookies: vec![],
        },
    );
    assert!(reply.ok, "reply: {:?}", reply);
    let pending_id = store.list_pending_links(Some("pending")).unwrap()[0].id.clone();
    let summary = mgr.drain_pending_links().await.unwrap();
    assert!(
        summary.accepted.is_empty(),
        "the drain must not auto-add a browser capture: {:?}",
        summary
    );
    assert!(summary.rejected.is_empty());
    assert!(mgr.list_downloads().unwrap().is_empty());
    let row = store.get_pending_link(&pending_id).unwrap().unwrap();
    assert_eq!(row.status, "pending");
    // The dialog's confirm adds it through the real path and
    // starts it (the default is start-immediately). It must
    // complete byte-exact from the fixture, not just sit queued.
    let (download_id, _probe) = mgr
        .confirm_pending_link(&pending_id, None, None, None, None, None, true)
        .await
        .unwrap();
    let row = store.get_pending_link(&pending_id).unwrap().unwrap();
    assert_eq!(row.status, "accepted");
    assert_eq!(row.download_id.as_deref(), Some(download_id.as_str()));
    let dl = mgr.get_download(&download_id).unwrap().unwrap();
    assert_eq!(dl.url, server.url);
    assert!(
        wait_for_status(&mgr, &download_id, "complete", 15).await,
        "confirmed download should complete, last status: {}",
        mgr.get_download(&download_id).unwrap().unwrap().status
    );
    let dl = mgr.get_download(&download_id).unwrap().unwrap();
    assert_eq!(dl.downloaded_bytes, 512 * 1024);
    assert_eq!(std::fs::read(&dl.save_path).unwrap(), payload(512 * 1024));
    assert!(store.list_pending_links(Some("pending")).unwrap().is_empty());
}

#[tokio::test]
async fn drain_skips_browser_and_video_rows_but_drains_download_all() {
    // Round 67: only 'download_all' rows are drained — a browser
    // capture and a video capture both wait for the Add URL dialog.
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let browser = handle_host_message(
        store,
        &HostMessage::DownloadLink {
            url: server.url.clone(),
            referrer: None,
            cookies: vec![],
        },
    );
    assert!(browser.ok);
    store
        .enqueue_pending_link(&server.url, None, "video")
        .unwrap();
    let all = store
        .enqueue_pending_link(&server.url, None, "download_all")
        .unwrap();
    let summary = mgr.drain_pending_links().await.unwrap();
    assert_eq!(summary.accepted.len(), 1, "summary: {:?}", summary);
    assert_eq!(summary.accepted[0].0, all.id);
    assert!(summary.rejected.is_empty());
    let pending = store.list_pending_links(Some("pending")).unwrap();
    assert_eq!(pending.len(), 2, "the browser and video rows must stay pending");
    assert!(pending.iter().any(|r| r.source == "browser"));
    assert!(pending.iter().any(|r| r.source == "video"));
}

#[tokio::test]
async fn confirm_pending_link_twice_is_an_error() {
    // A double confirm can never add the same link twice.
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let row = store
        .enqueue_pending_link(&server.url, None, "browser")
        .unwrap();
    let (download_id, _) = mgr
        .confirm_pending_link(&row.id, None, None, None, None, None, false)
        .await
        .unwrap();
    assert_eq!(mgr.get_download(&download_id).unwrap().unwrap().status, "queued");
    let err = mgr
        .confirm_pending_link(&row.id, None, None, None, None, None, false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("already decided"), "error: {err}");
    assert_eq!(mgr.list_downloads().unwrap().len(), 1);
}

#[tokio::test]
async fn confirm_pending_link_bad_url_is_rejected() {
    // A dead link is rejected with the reason, and the dialog can
    // show it and stay open for the user to fix the URL.
    let server = MockServer::start(MockConfig {
        fail_status: Some(404),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let row = store
        .enqueue_pending_link(&server.url, None, "browser")
        .unwrap();
    let err = mgr
        .confirm_pending_link(&row.id, None, None, None, None, None, true)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404"), "error: {err}");
    let row = store.get_pending_link(&row.id).unwrap().unwrap();
    assert_eq!(row.status, "rejected");
    assert!(row.error.as_deref().unwrap().contains("404"));
    assert!(mgr.list_downloads().unwrap().is_empty());
}

#[tokio::test]
async fn description_round_trip() {
    // Round 67 — the Add URL dialog's Description note is stored
    // on the download and cleared again; a fresh row has none.
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let (id, _) = mgr
        .add_download(&server.url, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().description, None);
    mgr.set_download_description(&id, Some("  my note  ")).unwrap();
    assert_eq!(
        mgr.get_download(&id).unwrap().unwrap().description.as_deref(),
        Some("my note"),
        "the note is trimmed on write"
    );
    mgr.set_download_description(&id, Some("")).unwrap();
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().description, None);
    mgr.set_download_description(&id, None).unwrap();
    assert_eq!(mgr.get_download(&id).unwrap().unwrap().description, None);
}

#[tokio::test]
async fn probe_url_for_capture_uses_the_capture_cookies() {
    // Round 67 — the Add URL dialog's size probe sends a
    // capture's cookies, so a cookie-gated link shows its size
    // instead of failing with 401.
    let server = CookieGatedServer::start(true);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let err = mgr.probe_url_for_capture(&server.url, None).await.unwrap_err();
    assert!(err.to_string().contains("401"), "error: {err}");
    let cookies = json!([{ "name": DUMMY_COOKIE_NAME, "value": DUMMY_COOKIE_VALUE }]).to_string();
    let info = mgr
        .probe_url_for_capture(&server.url, Some(&cookies))
        .await
        .unwrap();
    assert_eq!(info.size, Some(64 * 1024));
    assert!(server.cookie_seen() >= 1, "the probe must have sent the cookies");
}

#[tokio::test]
async fn drain_leaves_download_queued_when_start_drained_is_off() {
    // The old queued-only behaviour stays available as a setting:
    // with browser_start_drained off, the drain creates the
    // download and leaves it queued for the user to start.
    // (Round 67: the row is 'download_all' — a browser capture
    // waits for the Add URL dialog and is never drained.)
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    set_browser_start_drained(store, false);
    store
        .enqueue_pending_link(&server.url, None, "download_all")
        .unwrap();
    let summary = mgr.drain_pending_links().await.unwrap();
    assert_eq!(summary.accepted.len(), 1);
    assert!(summary.rejected.is_empty());
    let (_, download_id) = &summary.accepted[0];
    let dl = mgr.get_download(download_id).unwrap().unwrap();
    assert_eq!(dl.status, "queued");
    // Nothing downloaded on its own: no bytes, no file yet.
    assert_eq!(dl.downloaded_bytes, 0);
    assert!(!std::path::Path::new(&dl.save_path).exists());
    // And the user can still start it by hand afterwards.
    mgr.start_download(download_id).await.unwrap();
    assert!(wait_for_status(&mgr, download_id, "complete", 15).await);
}

#[test]
fn settings_browser_start_drained_defaults_on_and_round_trips() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    // Default: start drained links immediately.
    assert!(store.get_settings().unwrap().browser_start_drained);
    let mut s = store.get_settings().unwrap();
    s.browser_start_drained = false;
    store.update_settings(&s).unwrap();
    assert!(!store.get_settings().unwrap().browser_start_drained);
    let mut s = store.get_settings().unwrap();
    s.browser_start_drained = true;
    store.update_settings(&s).unwrap();
    assert!(store.get_settings().unwrap().browser_start_drained);
}

#[test]
fn pre_m2_database_migrates_settings_with_browser_start_drained() {
    // A database made before Milestone 2 has a settings table
    // without browser_start_drained. Opening it must add the
    // column (Store::migrate) defaulting to ON, without losing
    // the existing settings row.
    let dir = tmpdir();
    let path = dir.join("old.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (
               id INTEGER PRIMARY KEY CHECK (id = 1),
               default_download_folder TEXT NOT NULL DEFAULT '',
               max_connections INTEGER NOT NULL DEFAULT 8,
               speed_limit_kbps INTEGER,
               language TEXT NOT NULL DEFAULT 'en',
               theme TEXT NOT NULL DEFAULT 'light',
               browser_integration INTEGER NOT NULL DEFAULT 1,
               antivirus_command TEXT,
               file_types_auto TEXT NOT NULL DEFAULT '[]',
               excluded_sites TEXT NOT NULL DEFAULT '[]',
               check_updates_weekly INTEGER NOT NULL DEFAULT 1,
               start_on_startup INTEGER NOT NULL DEFAULT 0,
               default_action TEXT NOT NULL DEFAULT 'start_now',
               duplicate_action TEXT NOT NULL DEFAULT 'rename',
               confirm_delete INTEGER NOT NULL DEFAULT 1,
               connection_timeout_secs INTEGER NOT NULL DEFAULT 30,
               max_retries INTEGER NOT NULL DEFAULT 5,
               updated_at TEXT NOT NULL DEFAULT (datetime('now')));
             INSERT INTO settings (id, default_download_folder, language)
               VALUES (1, '/tmp/old-downloads', 'en');",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let settings = store.get_settings().unwrap();
    assert_eq!(settings.default_download_folder, "/tmp/old-downloads");
    assert!(settings.browser_start_drained, "migration must default start-drained to ON");
}

#[test]
fn host_get_settings_reports_pending_count() {
    // The popup shows how many links are waiting for the desktop
    // app, so get_settings must report the pending count.
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let reply = handle_host_message(&store, &HostMessage::GetSettings);
    assert!(reply.ok);
    assert_eq!(reply.pending_count, Some(0));
    for (i, url) in ["https://example.com/a.zip", "https://example.com/b.zip"]
        .iter()
        .enumerate()
    {
        let r = handle_host_message(
            &store,
            &HostMessage::DownloadLink {
                url: url.to_string(),
                referrer: None,
                cookies: vec![],
            },
        );
        assert!(r.ok, "reply: {:?}", r);
        assert_eq!(
            r.pending_count,
            Some(i + 1),
            "a queued reply reports the pending count after enqueue: {:?}",
            r
        );
    }
    let reply = handle_host_message(&store, &HostMessage::GetSettings);
    assert_eq!(reply.pending_count, Some(2));
}

#[tokio::test]
async fn drain_rejects_excluded_site_at_drain_time() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    // Enqueued while the site was allowed …
    let reply = handle_host_message(
        store,
        &HostMessage::DownloadLink {
            url: server.url.clone(),
            referrer: None,
            cookies: vec![],
        },
    );
    assert!(reply.ok);
    // … then the site is excluded before the drain runs.
    set_excluded(store, &["127.0.0.1"]);
    let summary = mgr.drain_pending_links().await.unwrap();
    assert!(summary.accepted.is_empty());
    assert_eq!(summary.rejected.len(), 1);
    assert!(summary.rejected[0].1.contains("excluded"));
    // Nothing was downloaded, and the row records the reason.
    assert!(mgr.list_downloads().unwrap().is_empty());
    let row = store.get_pending_link(&summary.rejected[0].0).unwrap().unwrap();
    assert_eq!(row.status, "rejected");
    assert!(row.error.as_deref().unwrap().contains("excluded"));
}

#[tokio::test]
async fn drain_rejects_when_browser_integration_turned_off() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let reply = handle_host_message(
        store,
        &HostMessage::DownloadLink {
            url: server.url.clone(),
            referrer: None,
            cookies: vec![],
        },
    );
    assert!(reply.ok);
    set_browser_integration(store, false);
    let summary = mgr.drain_pending_links().await.unwrap();
    assert!(summary.accepted.is_empty());
    assert_eq!(summary.rejected.len(), 1);
    assert!(mgr.list_downloads().unwrap().is_empty());
}

#[tokio::test]
async fn drain_duplicate_url_gets_different_save_path() {
    let server = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    // Round 67: 'download_all' rows — a browser capture waits
    // for the Add URL dialog and is never drained.
    for _ in 0..2 {
        store
            .enqueue_pending_link(&server.url, None, "download_all")
            .unwrap();
    }
    let summary = mgr.drain_pending_links().await.unwrap();
    // Both rows are accepted — the duplicate guard does not reject
    // the second add, it gives it a different file (BUG-002).
    assert_eq!(summary.accepted.len(), 2);
    assert!(summary.rejected.is_empty());
    let paths: Vec<String> = summary
        .accepted
        .iter()
        .map(|(_, id)| mgr.get_download(id).unwrap().unwrap().save_path)
        .collect();
    assert_ne!(paths[0], paths[1], "duplicate guard must suffix the second path");
    assert!(paths[1].contains("(2)"), "second path should be suffixed: {}", paths[1]);
}

#[tokio::test]
async fn drain_marks_dead_link_rejected_and_keeps_going() {
    let server = MockServer::start(MockConfig {
        fail_status: Some(404),
        ..Default::default()
    });
    let good = MockServer::start(MockConfig::default());
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    // Round 67: 'download_all' rows — a browser capture waits
    // for the Add URL dialog and is never drained.
    for url in [&server.url, &good.url] {
        store
            .enqueue_pending_link(url, None, "download_all")
            .unwrap();
    }
    let summary = mgr.drain_pending_links().await.unwrap();
    assert_eq!(summary.accepted.len(), 1);
    assert_eq!(summary.rejected.len(), 1);
    assert!(store.list_pending_links(Some("pending")).unwrap().is_empty());
}

#[tokio::test]
async fn enqueue_page_links_expands_a_page_into_pending_rows() {
    let page = PageServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    // The page links to 7 files plus its stylesheet (the extractor
    // also reads <link href>); the dead link is skipped, the rest
    // are enqueued. The duplicate video link is de-duplicated by
    // the extractor, so it is enqueued once.
    let n = mgr.enqueue_page_links(&page.page_url).await.unwrap();
    assert_eq!(n, 8);
    assert_eq!(store.list_pending_links(Some("pending")).unwrap().len(), 8);
    let summary = mgr.drain_pending_links().await.unwrap();
    assert_eq!(summary.accepted.len(), 8);
    assert!(summary.rejected.is_empty());
}

// -- regression: BUG-012 (S13 boundary bypass) ---------------------------
// Failing-test-first: both tests below failed before the fix.

#[tokio::test]
async fn enqueue_page_links_refuses_when_browser_integration_is_off() {
    // BUG-012a: the host's download_all-with-no-links path expanded the
    // page via enqueue_page_links without checking the desktop
    // browser_integration flag, so links were queued while catching
    // was off — and the drain accepts them once the flag is turned
    // back on. The drain's own re-check only rejects rows while the
    // flag is still off.
    let page = PageServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    set_browser_integration(store, false);
    let err = mgr.enqueue_page_links(&page.page_url).await.unwrap_err();
    assert!(err.to_string().contains("turned off"), "error: {err}");
    assert!(store.list_pending_links(None).unwrap().is_empty());
}

#[test]
fn host_download_all_all_skipped_is_an_error() {
    // BUG-012b: a download_all whose every link was refused (here:
    // integration off) replied ok:true with enqueued 0 and a message
    // buried in the reply — the extension treats ok:true as success,
    // so the user was never told nothing was queued.
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    set_browser_integration(&store, false);
    let reply = handle_host_message(
        &store,
        &HostMessage::DownloadAll {
            page_url: "https://example.com/page.html".to_string(),
            links: vec!["https://example.com/a.zip".to_string()],
        },
    );
    assert!(!reply.ok, "all-skipped download_all must be an error: {:?}", reply);
    assert!(reply.error.as_deref().unwrap().contains("turned off"));
    assert!(store.list_pending_links(None).unwrap().is_empty());
}

// -- extension manifest validates (e) -------------------------------------

#[test]
fn extension_manifest_is_mv3_and_minimal() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("extension")
        .join("manifest.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let m: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(m["manifest_version"], 3);
    assert_eq!(m["name"], "Sonic Manager");
    assert!(m["description"].as_str().unwrap().len() > 10);
    assert_eq!(m["background"]["service_worker"], "background.js");
    assert_eq!(m["action"]["default_popup"], "popup.html");
    let permissions: Vec<&str> = m["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    // Minimal set only: no <all_urls>, no tabs, no
    // webRequest. `scripting` is allowed since S08, and only for
    // injecting the video detector into the active tab on a user
    // gesture (see validate_manifest.py). `cookies` is allowed
    // since the S13 finish, and only for reading the cookies of
    // the exact URL being handed over (background.js `cookiesFor`).
    // Firefox needs a stable add-on id so its host manifest can
    // name it in allowed_extensions (BUG-023).
    for p in ["activeTab", "contextMenus", "cookies", "downloads", "storage", "nativeMessaging", "scripting"] {
        assert!(permissions.contains(&p), "missing permission {p}");
    }
    for forbidden in ["tabs", "webRequest", "history"] {
        assert!(!permissions.contains(&forbidden), "must not request {forbidden}");
    }
    let gecko_id = m["browser_specific_settings"]["gecko"]["id"].as_str().unwrap_or("");
    assert!(!gecko_id.is_empty(), "browser_specific_settings.gecko.id must be set");
    assert!(m.get("host_permissions").is_none(), "no host_permissions expected");
    for size in ["16", "32", "48", "128"] {
        assert!(
            m["icons"][size].as_str().is_some(),
            "manifest must list a {size}px icon"
        );
    }
    // The extension must not run a content script on every page:
    // the video detector is injected only on a user gesture.
    assert!(m.get("content_scripts").is_none());
    let _ = Settings::default();
    let _ = Duration::from_secs(0);
}

// -- cookies (S13 finish) -------------------------------------------------
//
// Test-only dummy cookie values, never real ones.
const DUMMY_COOKIE_NAME: &str = "session";
const DUMMY_COOKIE_VALUE: &str = "test-dummy-value";

fn dummy_cookies() -> Vec<CookiePair> {
    vec![CookiePair {
        name: DUMMY_COOKIE_NAME.to_string(),
        value: DUMMY_COOKIE_VALUE.to_string(),
    }]
}

/// A cookie-gated fixture server (S13 finish), local to this file —
/// the shared mock is the sync tests' fixture, not this one. Every
/// request must carry `Cookie: session=<dummy>` or it answers 401.
/// Serves the deterministic `payload()` bytes (64 KiB) with Range
/// support, so a drained download completes byte-exact. Counts how
/// many requests carried any `Cookie` header at all — the
/// cross-download test proves a plain download never sends one.
struct CookieGatedServer {
    url: String,
    cookie_seen: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl CookieGatedServer {
    /// `require_cookie`: when true, a request without the dummy
    /// session cookie answers 401; when false, everything is
    /// served but Cookie headers are still counted (the plain
    /// server in the cross-download test).
    fn start(require_cookie: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cookie_seen = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&cookie_seen);
        let stop_flag = Arc::clone(&stop);
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !stop_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let seen = Arc::clone(&seen);
                        thread::spawn(move || {
                            handle_cookie_request(stream, require_cookie, &seen);
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}/file.bin"),
            cookie_seen,
            stop,
        }
    }

    fn cookie_seen(&self) -> usize {
        self.cookie_seen.load(Ordering::SeqCst)
    }
}

impl Drop for CookieGatedServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn handle_cookie_request(mut stream: TcpStream, require_cookie: bool, cookie_seen: &AtomicUsize) {
    // Read until the end of the headers (or the buffer fills).
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16384 {
                    break;
                }
            }
        }
    }
    let req = String::from_utf8_lossy(&buf).to_string();
    let mut lines = req.lines();
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    // Header names are case-insensitive; reqwest sends them
    // lowercase, so both spellings are checked.
    let mut cookie: Option<String> = None;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("cookie:") {
            cookie = Some(v.trim().to_string());
        }
    }
    // A present header counts even when the cookie is wrong — the
    // cross-download test counts ANY cookie.
    if cookie.is_some() {
        cookie_seen.fetch_add(1, Ordering::SeqCst);
    }
    let authorised =
        !require_cookie || cookie.as_deref() == Some("session=test-dummy-value");
    if !authorised {
        let _ = stream.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    }
    let data = payload(64 * 1024);
    let size = data.len();
    if method == "HEAD" {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nAccept-Ranges: bytes\r\nETag: \"cookie-1\"\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(head.as_bytes());
        return;
    }
    // GET: honour Range, else serve the whole file.
    let mut start = 0usize;
    let mut end = size - 1;
    let mut ranged = false;
    for line in req.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(range) = lower.strip_prefix("range: bytes=") {
            if let Some((a, b)) = range.trim().split_once('-') {
                start = a.parse().unwrap_or(0);
                end = b.parse().unwrap_or(size - 1);
                ranged = true;
            }
        }
    }
    let end = end.min(size - 1);
    let body = &data[start..=end];
    if ranged {
        let head = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{size}\r\nAccept-Ranges: bytes\r\nETag: \"cookie-1\"\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
    } else {
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nAccept-Ranges: bytes\r\nETag: \"cookie-1\"\r\nConnection: close\r\n\r\n"
        );
        let _ = stream.write_all(head.as_bytes());
    }
    let _ = stream.write_all(body);
}

#[test]
fn cookies_to_header_validates_pairs() {
    assert_eq!(
        cookies_to_header(&dummy_cookies()).unwrap(),
        "session=test-dummy-value"
    );
    assert_eq!(
        cookies_to_header(&[
            CookiePair { name: "a".into(), value: "1".into() },
            CookiePair { name: "b".into(), value: "2".into() },
        ])
        .unwrap(),
        "a=1; b=2"
    );
    // A ';' in a value would smuggle a second cookie in.
    assert!(cookies_to_header(&[CookiePair {
        name: "session".into(),
        value: "a;b".into()
    }])
    .is_err());
    // A space in a name would split the header.
    assert!(cookies_to_header(&[CookiePair {
        name: "bad name".into(),
        value: "x".into()
    }])
    .is_err());
    // 33 pairs is too many.
    let many: Vec<CookiePair> = (0..33)
        .map(|i| CookiePair { name: format!("c{i}"), value: "x".into() })
        .collect();
    assert!(cookies_to_header(&many).is_err());
}

#[test]
fn host_download_link_with_cookies_stores_them_on_the_row() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let reply = handle_host_message(
        &store,
        &HostMessage::DownloadLink {
            url: "https://example.com/file.zip".to_string(),
            referrer: None,
            cookies: dummy_cookies(),
        },
    );
    assert!(reply.ok, "reply: {:?}", reply);
    let rows = store.list_pending_links(Some("pending")).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        serde_json::from_str::<Vec<CookiePair>>(rows[0].cookies.as_deref().unwrap()).unwrap(),
        dummy_cookies()
    );
    // The reply never carries the cookie value back.
    assert!(!serde_json::to_string(&reply).unwrap().contains(DUMMY_COOKIE_VALUE));
}

#[test]
fn host_download_link_rejects_an_invalid_cookie() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    for cookies in [
        vec![CookiePair { name: "session".into(), value: "a;b".into() }],
        vec![CookiePair { name: "bad name".into(), value: "x".into() }],
    ] {
        let reply = handle_host_message(
            &store,
            &HostMessage::DownloadLink {
                url: "https://example.com/file.zip".to_string(),
                referrer: None,
                cookies,
            },
        );
        assert!(!reply.ok, "reply: {:?}", reply);
    }
    assert!(store.list_pending_links(Some("pending")).unwrap().is_empty());
}

#[tokio::test]
async fn drain_cookie_gated_link_downloads_when_cookie_present() {
    // Round 67: a browser capture waits for the Add URL dialog —
    // the drain leaves it pending, and the dialog's confirm
    // carries its cookies through the real add path.
    let server = CookieGatedServer::start(true);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let reply = handle_host_message(
        store,
        &HostMessage::DownloadLink {
            url: server.url.clone(),
            referrer: None,
            cookies: dummy_cookies(),
        },
    );
    assert!(reply.ok, "reply: {:?}", reply);
    let pending_id = store.list_pending_links(Some("pending")).unwrap()[0].id.clone();
    let summary = mgr.drain_pending_links().await.unwrap();
    assert!(
        summary.accepted.is_empty(),
        "the drain must not auto-add a browser capture: {:?}",
        summary
    );
    assert!(summary.rejected.is_empty());
    let (download_id, _) = mgr
        .confirm_pending_link(&pending_id, None, None, None, None, None, true)
        .await
        .unwrap();
    assert!(
        wait_for_status(&mgr, &download_id, "complete", 15).await,
        "gated download should complete, last status: {}",
        mgr.get_download(&download_id).unwrap().unwrap().status
    );
    let dl = mgr.get_download(&download_id).unwrap().unwrap();
    assert_eq!(dl.downloaded_bytes, 64 * 1024);
    assert_eq!(std::fs::read(&dl.save_path).unwrap(), payload(64 * 1024));
    // Deciding the row wipes its cookies: the history row keeps no
    // session behind.
    let row = store.get_pending_link(&pending_id).unwrap().unwrap();
    assert_eq!(row.status, "accepted");
    assert_eq!(row.cookies, None, "cookies must be wiped when the row is decided");
    // The gated server saw cookies (probe + segments).
    assert!(server.cookie_seen() >= 1, "server saw no Cookie header at all");
}

#[tokio::test]
async fn drain_cookie_gated_link_rejected_without_cookie() {
    // Round 67: the dialog's confirm (not the drain) decides a
    // browser capture — without its cookies the gated link fails
    // with 401 and the row is rejected with the reason.
    let server = CookieGatedServer::start(true);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let reply = handle_host_message(
        store,
        &HostMessage::DownloadLink {
            url: server.url.clone(),
            referrer: None,
            cookies: vec![],
        },
    );
    assert!(reply.ok, "reply: {:?}", reply);
    let pending_id = store.list_pending_links(Some("pending")).unwrap()[0].id.clone();
    let summary = mgr.drain_pending_links().await.unwrap();
    assert!(
        summary.accepted.is_empty(),
        "the drain must not auto-add a browser capture: {:?}",
        summary
    );
    assert!(summary.rejected.is_empty());
    let err = mgr
        .confirm_pending_link(&pending_id, None, None, None, None, None, true)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("401"), "error: {err}");
    let row = store.get_pending_link(&pending_id).unwrap().unwrap();
    assert_eq!(row.status, "rejected");
    assert_eq!(row.cookies, None);
    assert!(store.list_pending_links(Some("pending")).unwrap().is_empty());
    assert_eq!(server.cookie_seen(), 0, "no Cookie header should have been sent");
}

#[tokio::test]
async fn drain_sends_cookies_only_to_the_download_that_carries_them() {
    // Round 67: the drain still handles 'download_all' rows, and
    // a row's cookies ride only its own download.
    let gated = CookieGatedServer::start(true);
    let plain = CookieGatedServer::start(false);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let cookies = json!([{ "name": DUMMY_COOKIE_NAME, "value": DUMMY_COOKIE_VALUE }]).to_string();
    store
        .enqueue_pending_link_with_cookies(&gated.url, None, "download_all", Some(&cookies))
        .unwrap();
    store
        .enqueue_pending_link(&plain.url, None, "download_all")
        .unwrap();
    let summary = mgr.drain_pending_links().await.unwrap();
    assert_eq!(summary.accepted.len(), 2, "summary: {:?}", summary);
    for (_, download_id) in &summary.accepted {
        assert!(
            wait_for_status(&mgr, download_id, "complete", 15).await,
            "download should complete, last status: {}",
            mgr.get_download(download_id).unwrap().unwrap().status
        );
        let dl = mgr.get_download(download_id).unwrap().unwrap();
        assert_eq!(std::fs::read(&dl.save_path).unwrap(), payload(64 * 1024));
    }
    // The gated server saw cookies; the plain one never did, even
    // though both downloads ran in the same manager at the same time.
    assert!(gated.cookie_seen() >= 1);
    assert_eq!(plain.cookie_seen(), 0, "cookies leaked to a download that carries none");
}

#[tokio::test]
async fn cookies_are_dropped_when_the_app_restarts() {
    let server = CookieGatedServer::start(true);
    let dir = tmpdir();
    let download_id = {
        let mgr = test_mgr(&dir);
        let store = mgr.store();
        let reply = handle_host_message(
            store,
            &HostMessage::DownloadLink {
                url: server.url.clone(),
                referrer: None,
                cookies: dummy_cookies(),
            },
        );
        assert!(reply.ok, "reply: {:?}", reply);
        let pending_id = store.list_pending_links(Some("pending")).unwrap()[0].id.clone();
        // Round 67: the drain leaves the browser capture pending;
        // the dialog's confirm (Download Later) adds it queued
        // without starting — its cookies are wiped when the row
        // is decided, so they live only in this manager's memory.
        let summary = mgr.drain_pending_links().await.unwrap();
        assert!(
        summary.accepted.is_empty(),
        "the drain must not auto-add a browser capture: {:?}",
        summary
    );
        let (download_id, _) = mgr
            .confirm_pending_link(&pending_id, None, None, None, None, None, false)
            .await
            .unwrap();
        let row = store.get_pending_link(&pending_id).unwrap().unwrap();
        assert_eq!(row.status, "accepted");
        assert_eq!(row.cookies, None, "cookies must be wiped when the row is decided");
        // The download is queued, not started — its cookies are
        // still only in this manager's memory.
        assert_eq!(
            mgr.get_download(&download_id).unwrap().unwrap().status,
            "queued"
        );
        download_id
        // mgr (and its store) drop here: the restart.
    };
    // A new manager on the same database is a restart: no cookies
    // in memory any more, so the gated download fails with a clear
    // 401 instead of silently reusing a stale session. Retries are
    // turned off for this manager so the failure lands at once
    // instead of after the exponential backoff (2/4/8/16 s).
    let mgr2 = test_mgr(&dir);
    let mut s = mgr2.store().get_settings().unwrap();
    s.max_retries = 0;
    mgr2.update_settings(&s).unwrap();
    mgr2.start_download(&download_id).await.unwrap();
    assert!(
        wait_for_status(&mgr2, &download_id, "error", 15).await,
        "resumed download should fail without its cookies, last status: {}",
        mgr2.get_download(&download_id).unwrap().unwrap().status
    );
    let dl = mgr2.get_download(&download_id).unwrap().unwrap();
    let err = dl.error_message.unwrap_or_default();
    assert!(err.contains("401"), "error should name the 401: {err}");
}
