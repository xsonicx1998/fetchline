//! Video grabber (S08 / F05) tests: quality derivation, variant
//! validation at the host boundary, the `video_found` enqueue +
//! drain through the real `add_download` path, the desktop page
//! scan, and the pre-S08 `pending_links` migration. No public
//! internet is used; downloads go through the local mock servers
//! from `common/mock.rs`.

#![allow(dead_code)]

#[path = "common/mock.rs"]
mod mock;

use mock::*;
use std::sync::Arc;

use fetchline_engine::native_host::{handle_host_message, HostMessage};
use fetchline_engine::video::{
    detect_videos_in_html, parse_m3u8, quality_for, validate_video_variants, VideoVariantInput,
    UNKNOWN_QUALITY,
};
use fetchline_engine::{DownloadManager, Store};

fn test_mgr(dir: &std::path::Path) -> Arc<DownloadManager> {
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
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

fn set_file_types_auto(store: &Store, types: &[&str]) {
    let mut s = store.get_settings().unwrap();
    s.file_types_auto = types.iter().map(|x| x.to_string()).collect();
    store.update_settings(&s).unwrap();
}

fn variant(url: &str) -> VideoVariantInput {
    VideoVariantInput {
        url: url.to_string(),
        quality: None,
        format: None,
        live: false,
        drm: false,
        note: None,
    }
}

// -- (b) quality derivation never invents a label -------------------------

#[test]
fn quality_is_unknown_without_evidence() {
    assert_eq!(quality_for("http://127.0.0.1/clip.mp4", 0, 0), UNKNOWN_QUALITY);
    assert_eq!(quality_for("http://127.0.0.1/clip.mp4", 0, 0), "Unknown quality");
}

#[test]
fn quality_comes_from_dimensions_then_label() {
    assert_eq!(quality_for("http://x/a.mp4", 1920, 1080), "1920 × 1080");
    assert_eq!(quality_for("http://x/a.mp4", 0, 720), "720p");
    assert_eq!(quality_for("http://x/clip-1080p.mp4", 0, 0), "1080p");
    assert_eq!(quality_for("http://x/trailer-4k.webm", 0, 0), "4K");
}

// -- (a) detection over sample HTML fixtures ------------------------------
// The fixtures live in app/extension/tests/fixtures/ and are the
// same files the JavaScript detector is tested against, so the two
// detectors cannot drift apart silently: the Rust test reads the
// fixture from disk and asserts the same counts the JS test does.

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("extension")
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()))
}

#[test]
fn fixture_video_with_sources_and_multiple_qualities() {
    let found = detect_videos_in_html(
        &fixture("video-sources.html"),
        "http://127.0.0.1/page.html",
    );
    let urls: Vec<&str> = found.iter().map(|v| v.url.as_str()).collect();
    assert_eq!(
        urls,
        vec![
            "http://127.0.0.1/clip-720p.mp4",
            "http://127.0.0.1/clip-480p.webm",
            "http://127.0.0.1/clip-1080p.mp4",
        ]
    );
    assert_eq!(found[0].quality, "720p");
    assert_eq!(found[0].format, "MP4");
    assert_eq!(found[2].quality, "1080p");
}

#[test]
fn fixture_audio_only() {
    let found = detect_videos_in_html(&fixture("audio-only.html"), "http://127.0.0.1/page.html");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].url, "http://127.0.0.1/song.mp3");
    assert_eq!(found[0].format, "MP3");
    assert_eq!(found[0].quality, UNKNOWN_QUALITY);
}

#[test]
fn fixture_no_video() {
    let found = detect_videos_in_html(&fixture("no-video.html"), "http://127.0.0.1/page.html");
    assert!(found.is_empty(), "a page with no media must find none: {found:?}");
}

#[test]
fn fixture_duplicates_are_deduped() {
    let found = detect_videos_in_html(&fixture("duplicates.html"), "http://127.0.0.1/page.html");
    let urls: Vec<&str> = found.iter().map(|v| v.url.as_str()).collect();
    assert_eq!(urls, vec!["http://127.0.0.1/clip.mp4"]);
}

#[test]
fn fixture_live_stream_is_marked_live() {
    let found = detect_videos_in_html(&fixture("live-stream.html"), "http://127.0.0.1/page.html");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].format, "M3U8");
    assert!(found[0].live, "an .m3u8 playlist is a stream, not a file");
}

#[test]
fn fixture_drm_marked_video_is_listed_but_flagged() {
    // Static HTML cannot prove DRM — the fixture marks the element
    // explicitly (data-drm), and the JS detector reads the live
    // element's mediaKeys instead. The Rust scan lists the URL; the
    // host boundary is where a drm:true variant is refused (see
    // video_validation_refuses_drm below).
    let html = fixture("drm-video.html");
    assert!(html.contains("data-drm"));
    let found = detect_videos_in_html(&html, "http://127.0.0.1/page.html");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].url, "http://127.0.0.1/protected.mp4");
}

// -- (c) engine variant validation -----------------------------------------

#[test]
fn video_validation_accepts_good_variants_and_dedups() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let settings = store.get_settings().unwrap();
    let (accepted, skipped) = validate_video_variants(
        "https://example.com/watch",
        &[
            variant("https://example.com/clip-720p.mp4"),
            variant("https://example.com/clip-720p.mp4"),
            variant("https://example.com/song.mp3"),
        ],
        &settings,
    )
    .unwrap();
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].quality.as_deref(), Some("720p"));
    assert_eq!(accepted[1].format.as_deref(), Some("MP3"));
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].contains("duplicate"), "skipped: {skipped:?}");
}

#[test]
fn video_validation_skips_bad_scheme_and_unknown_extension() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let settings = store.get_settings().unwrap();
    let (accepted, skipped) = validate_video_variants(
        "https://example.com/watch",
        &[
            variant("ftp://example.com/clip.mp4"),
            variant("https://example.com/page.html"),
            variant("https://example.com/clip.mp4"),
        ],
        &settings,
    )
    .unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(skipped.len(), 2);
    assert!(skipped[0].contains("ftp://"), "skipped: {skipped:?}");
    assert!(skipped[1].contains(".html"), "skipped: {skipped:?}");
}

#[test]
fn video_validation_refuses_drm() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let settings = store.get_settings().unwrap();
    let mut v = variant("https://example.com/protected.mp4");
    v.drm = true;
    let (accepted, skipped) =
        validate_video_variants("https://example.com/watch", &[v], &settings).unwrap();
    assert!(accepted.is_empty());
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].contains("DRM"), "skipped: {skipped:?}");
}

#[test]
fn video_validation_rejects_empty_variants_excluded_page_and_integration_off() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let settings = store.get_settings().unwrap();
    let err = validate_video_variants("https://example.com/watch", &[], &settings).unwrap_err();
    assert!(err.to_string().contains("No video variants"), "error: {err}");

    set_excluded(&store, &["example.com"]);
    let settings = store.get_settings().unwrap();
    let err = validate_video_variants(
        "https://example.com/watch",
        &[variant("https://example.com/clip.mp4")],
        &settings,
    )
    .unwrap_err();
    assert!(err.to_string().contains("excluded"), "error: {err}");

    set_excluded(&store, &[]);
    set_browser_integration(&store, false);
    let settings = store.get_settings().unwrap();
    let err = validate_video_variants(
        "https://example.com/watch",
        &[variant("https://example.com/clip.mp4")],
        &settings,
    )
    .unwrap_err();
    assert!(err.to_string().contains("turned off"), "error: {err}");
}

#[test]
fn video_validation_skips_variant_from_excluded_site_and_outside_file_types_auto() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    set_excluded(&store, &["blocked.example"]);
    set_file_types_auto(&store, &["mp4"]);
    let settings = store.get_settings().unwrap();
    let (accepted, skipped) = validate_video_variants(
        "https://example.com/watch",
        &[
            variant("https://blocked.example/clip.mp4"),
            variant("https://example.com/clip.webm"),
            variant("https://example.com/clip.mp4"),
        ],
        &settings,
    )
    .unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(skipped.len(), 2);
    assert!(skipped[0].contains("excluded"), "skipped: {skipped:?}");
    assert!(skipped[1].contains("file types"), "skipped: {skipped:?}");
}

#[test]
fn video_validation_replaces_an_invented_quality_and_keeps_a_real_one() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let settings = store.get_settings().unwrap();
    let mut invented = variant("https://example.com/clip.mp4");
    invented.quality = Some("Ultra HD 8K Pro".to_string());
    let mut real = variant("https://example.com/clip-720p.mp4");
    real.quality = Some("720p".to_string());
    let (accepted, _) =
        validate_video_variants("https://example.com/watch", &[invented, real], &settings)
            .unwrap();
    assert_eq!(accepted[0].quality.as_deref(), Some(UNKNOWN_QUALITY));
    assert_eq!(accepted[1].quality.as_deref(), Some("720p"));
}

#[test]
fn video_validation_notes_hls_playlists_and_live_streams() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let settings = store.get_settings().unwrap();
    let mut live = variant("https://example.com/stream.m3u8");
    live.live = true;
    let (accepted, _) =
        validate_video_variants("https://example.com/watch", &[live], &settings).unwrap();
    assert_eq!(accepted.len(), 1);
    let note = accepted[0].note.as_deref().unwrap();
    assert!(note.contains("playlist"), "note: {note}");
    assert!(note.contains("not merged"), "note: {note}");
}

// -- host boundary: video_found enqueue ------------------------------------

#[test]
fn host_video_found_enqueues_into_pending_links_as_video() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let reply = handle_host_message(
        &store,
        &HostMessage::VideoFound {
            page_url: "https://example.com/watch".to_string(),
            variants: vec![
                variant("https://example.com/clip-720p.mp4"),
                variant("https://example.com/clip-720p.mp4"),
            ],
            cookies: vec![],
        },
    );
    assert!(reply.ok, "reply: {:?}", reply);
    assert_eq!(reply.enqueued, Some(1));
    assert!(reply.message.as_deref().unwrap().contains("skipped"), "message: {:?}", reply);
    let pending = store.list_pending_links(Some("pending")).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].source, "video");
    assert_eq!(pending[0].url, "https://example.com/clip-720p.mp4");
    assert_eq!(pending[0].referrer.as_deref(), Some("https://example.com/watch"));
}

#[test]
fn host_video_found_all_skipped_is_an_error() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut drm = variant("https://example.com/protected.mp4");
    drm.drm = true;
    let reply = handle_host_message(
        &store,
        &HostMessage::VideoFound {
            page_url: "https://example.com/watch".to_string(),
            variants: vec![drm],
            cookies: vec![],
        },
    );
    assert!(!reply.ok, "all-DRM video_found must be an error: {:?}", reply);
    assert!(reply.error.as_deref().unwrap().contains("DRM"));
    assert!(store.list_pending_links(None).unwrap().is_empty());
}

#[test]
fn host_video_found_refuses_excluded_site_and_integration_off() {
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    set_excluded(&store, &["example.com"]);
    let reply = handle_host_message(
        &store,
        &HostMessage::VideoFound {
            page_url: "https://example.com/watch".to_string(),
            variants: vec![variant("https://example.com/clip.mp4")],
            cookies: vec![],
        },
    );
    assert!(!reply.ok);
    assert!(reply.error.as_deref().unwrap().contains("excluded"));
    set_excluded(&store, &[]);
    set_browser_integration(&store, false);
    let reply = handle_host_message(
        &store,
        &HostMessage::VideoFound {
            page_url: "https://example.com/watch".to_string(),
            variants: vec![variant("https://example.com/clip.mp4")],
            cookies: vec![],
        },
    );
    assert!(!reply.ok);
    assert!(store.list_pending_links(None).unwrap().is_empty());
}

// -- pre-S08 migration: old pending_links CHECK widened --------------------

#[test]
fn pre_s08_database_migrates_pending_links_to_allow_video_source() {
    let dir = tmpdir();
    let db_path = dir.join("app.db");
    // A database as S13 made it: pending_links whose CHECK allows
    // only 'browser' and 'download_all', with one queued row.
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE pending_links (
               id TEXT PRIMARY KEY, url TEXT NOT NULL, referrer TEXT,
               source TEXT NOT NULL DEFAULT 'browser'
                 CHECK (source IN ('browser', 'download_all')),
               status TEXT NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'accepted', 'rejected')),
               download_id TEXT, error TEXT,
               created_at TEXT NOT NULL DEFAULT (datetime('now')));
             INSERT INTO pending_links (id, url, referrer, source, status)
               VALUES ('old-row', 'https://example.com/file.zip', NULL, 'browser', 'pending');",
        )
        .unwrap();
    }
    let store = Store::open(&db_path).unwrap();
    // The old row survived the rebuild, and a 'video' row can now
    // be enqueued — before the migration the CHECK refused it.
    let old = store.get_pending_link("old-row").unwrap().unwrap();
    assert_eq!(old.url, "https://example.com/file.zip");
    let row = store
        .enqueue_pending_link("https://example.com/clip.mp4", Some("https://example.com/watch"), "video")
        .unwrap();
    assert_eq!(row.source, "video");
}

// -- desktop page scan (S08 panel data) -------------------------------------

#[tokio::test]
async fn detect_page_videos_lists_media_with_probe_results() {
    let page = PageServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let found = mgr.detect_page_videos(&page.page_url).await.unwrap();
    // The page links a video, an audio track and a dead video; the
    // images/documents/archives are not media for this panel.
    assert_eq!(found.len(), 3);
    let video = found.iter().find(|v| v.url.ends_with("/video.mp4")).unwrap();
    assert_eq!(video.format, "MP4");
    assert_eq!(video.quality, UNKNOWN_QUALITY);
    assert_eq!(video.size, Some(100 * 1024));
    assert!(video.downloadable);
    let audio = found.iter().find(|v| v.url.ends_with("/song.mp3")).unwrap();
    assert_eq!(audio.format, "MP3");
    let dead = found.iter().find(|v| v.url.ends_with("/missing.mp4")).unwrap();
    assert!(!dead.downloadable, "a dead link must not offer a download button");
    assert!(dead.error.is_some());
}

// -- (d) end to end: video_found -> pending_links -> Add URL dialog
// (confirm_pending_link) -> add_download --
// Round 67: a video capture is interactive — the drain leaves it
// pending for the Add URL dialog (like IDM), and the dialog's
// confirm starts the download (browser_start_drained defaults
// on), so the confirm's own start must carry it to completion.

#[tokio::test]
async fn video_found_end_to_end_drains_into_a_real_download() {
    let page = PageServer::start();
    let video_url = format!("{}/video.mp4", page.base);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let reply = handle_host_message(
        store,
        &HostMessage::VideoFound {
            page_url: page.page_url.clone(),
            variants: vec![variant(&video_url)],
            cookies: vec![],
        },
    );
    assert!(reply.ok, "reply: {:?}", reply);
    assert_eq!(reply.enqueued, Some(1));
    let pending_id = store.list_pending_links(Some("pending")).unwrap()[0].id.clone();
    let summary = mgr.drain_pending_links().await.unwrap();
    assert!(
        summary.accepted.is_empty(),
        "the drain must not auto-add a video capture: {:?}",
        summary
    );
    assert!(summary.rejected.is_empty());
    let (download_id, _) = mgr
        .confirm_pending_link(&pending_id, None, None, None, None, None, true)
        .await
        .unwrap();
    let dl = mgr.get_download(&download_id).unwrap().unwrap();
    assert_eq!(dl.url, video_url);
    assert_eq!(dl.category_id, "cat-video");
    assert_eq!(dl.filename, "video.mp4");
    // The confirm started it: it completes byte-exact from the fixture.
    assert!(wait_for_status(&mgr, &download_id, "complete", 15).await);
    let dl = mgr.get_download(&download_id).unwrap().unwrap();
    assert_eq!(dl.downloaded_bytes, 100 * 1024);
    let bytes = std::fs::read(&dl.save_path).unwrap();
    assert_eq!(bytes, payload(100 * 1024));
}

// -- (e) HLS, decided in Milestone 3: playlists offered as-is ---------------
// The shared playlist fixtures live in app/extension/tests/fixtures/
// and are the same files the JavaScript parseM3u8 tests assert
// against, so the two parsers cannot drift apart silently.

#[test]
fn hls_parse_matches_shared_fixtures() {
    let master = parse_m3u8(&fixture("master.m3u8")).expect("master parses");
    assert!(master.is_master);
    assert!(!master.live);
    assert!(!master.encrypted);
    assert_eq!(master.variants.len(), 3);
    assert_eq!(master.variants[0].resolution, Some((1280, 720)));
    assert_eq!(master.variants[1].resolution, Some((1920, 1080)));
    assert_eq!(master.variants[2].resolution, None);
    assert_eq!(master.variants[0].bandwidth, Some(800_000));

    let vod = parse_m3u8(&fixture("vod.m3u8")).expect("VOD parses");
    assert!(!vod.is_master);
    assert!(!vod.live);
    assert_eq!(vod.segments, 3);
    assert!((vod.duration_secs - 25.0).abs() < 0.001);

    let live = parse_m3u8(&fixture("live.m3u8")).expect("live parses");
    assert!(live.live);
    assert_eq!(live.segments, 2);

    let enc = parse_m3u8(&fixture("encrypted.m3u8")).expect("encrypted parses");
    assert!(enc.encrypted);
    assert!(parse_m3u8("").is_none());
    assert!(parse_m3u8("<html>not a playlist</html>").is_none());
}

#[test]
fn fixture_hls_master_dash_and_low_quality() {
    let master = detect_videos_in_html(&fixture("hls-master.html"), "http://127.0.0.1/page.html");
    assert_eq!(master.len(), 1);
    assert_eq!(master[0].format, "M3U8");
    assert!(master[0].live);
    assert!(master[0].note.as_deref().unwrap().contains("not merged"));

    let dash = detect_videos_in_html(&fixture("dash-manifest.html"), "http://127.0.0.1/page.html");
    assert_eq!(dash.len(), 1);
    assert_eq!(dash[0].format, "MPD");
    assert_eq!(dash[0].quality, UNKNOWN_QUALITY);
    assert!(dash[0].note.as_deref().unwrap().contains("DASH manifest"));

    let low = detect_videos_in_html(&fixture("low-quality.html"), "http://127.0.0.1/page.html");
    assert_eq!(low.len(), 1);
    assert_eq!(low[0].quality, UNKNOWN_QUALITY);
}

#[test]
fn video_validation_accepts_mpd_with_dash_note() {
    let dir = tmpdir();
    let store = Store::open(&dir.join("app.db")).unwrap();
    let settings = store.get_settings().unwrap();
    let (accepted, skipped) = validate_video_variants(
        "https://example.com/watch",
        &[variant("https://example.com/stream.mpd")],
        &settings,
    )
    .unwrap();
    assert_eq!(accepted.len(), 1);
    assert!(skipped.is_empty());
    assert_eq!(accepted[0].format.as_deref(), Some("MPD"));
    assert!(accepted[0].note.as_deref().unwrap().contains("DASH manifest"));
}

// -- a local HLS fixture server -------------------------------------------
// Serves pages that link playlists, the playlists themselves (the
// shared fixture texts), variant playlists, segments and a real
// .mp4 — no real website is ever touched. HEAD + Range are
// supported, like the shared mock, so probes and downloads work.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};

struct HlsServer {
    base: String,
    stop: Arc<AtomicBool>,
}

fn hls_route(path: &str, base: &str) -> Option<(&'static str, Vec<u8>)> {
    match path {
        "/page-master.html" => Some((
            "text/html",
            format!("<html><body><video src=\"{base}/master.m3u8\"></video></body></html>")
                .into_bytes(),
        )),
        "/page-vod.html" => Some((
            "text/html",
            format!("<html><body><video src=\"{base}/vod.m3u8\"></video></body></html>")
                .into_bytes(),
        )),
        "/page-live.html" => Some((
            "text/html",
            format!("<html><body><video src=\"{base}/live.m3u8\"></video></body></html>")
                .into_bytes(),
        )),
        "/page-enc.html" => Some((
            "text/html",
            format!("<html><body><video src=\"{base}/enc.m3u8\"></video></body></html>")
                .into_bytes(),
        )),
        "/page-dash.html" => Some((
            "text/html",
            format!(
                "<html><body><video><source src=\"{base}/stream.mpd\"></video>\
                 <video src=\"{base}/clip.mp4\"></video></body></html>"
            )
            .into_bytes(),
        )),
        "/page-master10.html" => Some((
            "text/html",
            format!("<html><body><video src=\"{base}/master10.m3u8\"></video></body></html>")
                .into_bytes(),
        )),
        "/master.m3u8" => Some(("application/vnd.apple.mpegurl", fixture("master.m3u8").into_bytes())),
        "/vod.m3u8" => Some(("application/vnd.apple.mpegurl", fixture("vod.m3u8").into_bytes())),
        "/live.m3u8" => Some(("application/vnd.apple.mpegurl", fixture("live.m3u8").into_bytes())),
        "/enc.m3u8" => Some(("application/vnd.apple.mpegurl", fixture("encrypted.m3u8").into_bytes())),
        "/v720.m3u8" => Some((
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10.0,\nseg1.ts\n#EXTINF:10.0,\nseg2.ts\n#EXT-X-ENDLIST\n"
                .to_vec(),
        )),
        "/v1080.m3u8" => Some((
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10.0,\nseg1.ts\n#EXTINF:10.0,\nseg2.ts\n#EXTINF:10.0,\nseg3.ts\n#EXT-X-ENDLIST\n"
                .to_vec(),
        )),
        // The low variant is a live stream (no ENDLIST).
        "/v-low.m3u8" => Some((
            "application/vnd.apple.mpegurl",
            b"#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10.0,\nseg1.ts\n".to_vec(),
        )),
        "/master10.m3u8" => {
            let mut text = String::from("#EXTM3U\n");
            for i in 0..10 {
                text.push_str(&format!(
                    "#EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=640x360\nmissing{i}.m3u8\n"
                ));
            }
            Some(("application/vnd.apple.mpegurl", text.into_bytes()))
        }
        "/stream.mpd" => Some((
            "application/dash+xml",
            b"<?xml version=\"1.0\"?><MPD xmlns=\"urn:mpeg:dash:schema:mpd:2011\" type=\"static\"><Period><AdaptationSet><Representation width=\"1280\" height=\"720\"/></AdaptationSet></Period></MPD>"
                .to_vec(),
        )),
        "/clip.mp4" => Some(("video/mp4", payload(100 * 1024))),
        "/seg1.ts" | "/seg2.ts" | "/seg3.ts" => Some(("video/mp2t", payload(1024))),
        _ => None,
    }
}

fn hls_handle(mut stream: std::net::TcpStream, base: String) {
    let mut buf = [0u8; 8192];
    let Ok(n) = stream.read(&mut buf) else { return };
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let first = req.lines().next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("").to_string();
    let Some((ctype, body)) = hls_route(&path, &base) else {
        let head = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        return;
    };
    let size = body.len();
    let mut status = "200 OK";
    let mut headers = vec![
        format!("Content-Length: {size}"),
        "Accept-Ranges: bytes".to_string(),
        "ETag: \"hls1\"".to_string(),
        format!("Content-Type: {ctype}"),
        "Connection: close".to_string(),
    ];
    let mut out_body: Vec<u8> = Vec::new();
    if method != "HEAD" {
        let range = req.lines().find_map(|l| {
            let (name, value) = l.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("range") {
                Some(value.trim().to_string())
            } else {
                None
            }
        });
        if let Some(range) = range {
            if let Some(spec) = range.strip_prefix("bytes=") {
                let mut it = spec.split('-');
                let start: usize = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let end: usize = it
                    .next()
                    .and_then(|s| if s.is_empty() { None } else { s.parse().ok() })
                    .unwrap_or(size - 1)
                    .min(size - 1);
                if start >= size {
                    status = "416 Range Not Satisfiable";
                    headers = vec![
                        format!("Content-Range: bytes */{size}"),
                        "Connection: close".to_string(),
                    ];
                } else {
                    status = "206 Partial Content";
                    headers = vec![
                        format!("Content-Length: {}", end - start + 1),
                        format!("Content-Range: bytes {start}-{end}/{size}"),
                        "Accept-Ranges: bytes".to_string(),
                        "ETag: \"hls1\"".to_string(),
                        format!("Content-Type: {ctype}"),
                        "Connection: close".to_string(),
                    ];
                    out_body = body[start..=end].to_vec();
                }
            } else {
                out_body = body;
            }
        } else {
            out_body = body;
        }
    }
    let head = format!("HTTP/1.1 {status}\r\n{}\r\n\r\n", headers.join("\r\n"));
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    if method != "HEAD" {
        let _ = stream.write_all(&out_body);
    }
}

impl HlsServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let b = base.clone();
        std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream.set_nonblocking(false).unwrap(); // BUG-036: on Windows an accepted socket inherits the listener's non-blocking mode
                        let bb = b.clone();
                        std::thread::spawn(move || hls_handle(stream, bb));
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
            }
        });
        Self { base, stop }
    }
}

impl Drop for HlsServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn detect_page_videos_expands_master_playlist_into_real_variants() {
    let server = HlsServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let found = mgr
        .detect_page_videos(&format!("{}/page-master.html", server.base))
        .await
        .unwrap();
    // One master playlist becomes its three real variants — the
    // master itself is not offered, so the user picks a quality.
    assert_eq!(found.len(), 3, "found: {found:?}");
    let urls: Vec<&str> = found.iter().map(|v| v.url.as_str()).collect();
    assert!(urls[0].ends_with("/v720.m3u8"));
    assert!(urls[1].ends_with("/v1080.m3u8"));
    assert!(urls[2].ends_with("/v-low.m3u8"));
    assert_eq!(found[0].quality, "1280 × 720");
    assert_eq!(found[1].quality, "1920 × 1080");
    assert_eq!(found[2].quality, UNKNOWN_QUALITY, "a variant with no RESOLUTION stays unknown");
    assert!(!found[0].live, "v720 ends (ENDLIST), so it is not live");
    assert!(found[2].live, "v-low has no ENDLIST, so it is live");
    assert!(found.iter().all(|v| v.downloadable));
    assert!(found.iter().all(|v| v.size.is_some()));
    assert!(found
        .iter()
        .all(|v| v.note.as_deref().unwrap().contains("not merged")));
}

#[tokio::test]
async fn detect_page_videos_vod_playlist_reports_segments_and_not_live() {
    let server = HlsServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let found = mgr
        .detect_page_videos(&format!("{}/page-vod.html", server.base))
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert!(!found[0].live);
    assert!(found[0].downloadable);
    let note = found[0].note.as_deref().unwrap();
    assert!(note.contains("3 segments"), "note: {note}");
    assert!(note.contains("25 s"), "note: {note}");
    assert!(note.contains("not merged"), "note: {note}");
}

#[tokio::test]
async fn detect_page_videos_live_playlist_is_live() {
    let server = HlsServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let found = mgr
        .detect_page_videos(&format!("{}/page-live.html", server.base))
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert!(found[0].live);
    assert!(found[0].downloadable);
    assert!(found[0].note.as_deref().unwrap().contains("Live stream"));
}

#[tokio::test]
async fn detect_page_videos_encrypted_playlist_is_not_downloadable() {
    let server = HlsServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let found = mgr
        .detect_page_videos(&format!("{}/page-enc.html", server.base))
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert!(!found[0].downloadable, "an encrypted playlist alone will not play");
    assert!(found[0].error.as_deref().unwrap().contains("encrypted"), "error: {:?}", found[0].error);
}

#[tokio::test]
async fn detect_page_videos_dash_manifest_is_listed_as_is() {
    let server = HlsServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let found = mgr
        .detect_page_videos(&format!("{}/page-dash.html", server.base))
        .await
        .unwrap();
    assert_eq!(found.len(), 2, "found: {found:?}");
    let mpd = found.iter().find(|v| v.format == "MPD").unwrap();
    assert_eq!(mpd.quality, UNKNOWN_QUALITY);
    assert!(mpd.downloadable);
    assert!(mpd.note.as_deref().unwrap().contains("DASH manifest"));
    assert!(found.iter().any(|v| v.format == "MP4"));
}

#[tokio::test]
async fn detect_page_videos_master_variant_list_is_capped() {
    let server = HlsServer::start();
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let found = mgr
        .detect_page_videos(&format!("{}/page-master10.html", server.base))
        .await
        .unwrap();
    // Ten variants in the playlist, eight shown; the dead variant
    // playlists are listed (not hidden) as not downloadable.
    assert_eq!(found.len(), 8, "found: {found:?}");
    assert!(found.iter().all(|v| !v.downloadable));
    assert!(found.iter().all(|v| v.error.is_some()));
}

#[tokio::test]
async fn video_found_m3u8_drains_into_a_real_playlist_download() {
    let server = HlsServer::start();
    let playlist_url = format!("{}/vod.m3u8", server.base);
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let reply = handle_host_message(
        store,
        &HostMessage::VideoFound {
            page_url: format!("{}/page-vod.html", server.base),
            variants: vec![variant(&playlist_url)],
            cookies: vec![],
        },
    );
    assert!(reply.ok, "reply: {:?}", reply);
    assert_eq!(reply.enqueued, Some(1));
    let pending_id = store.list_pending_links(Some("pending")).unwrap()[0].id.clone();
    // Round 67: the drain leaves the video capture pending for
    // the Add URL dialog; the dialog's confirm adds it.
    let summary = mgr.drain_pending_links().await.unwrap();
    assert!(
        summary.accepted.is_empty(),
        "the drain must not auto-add a video capture: {:?}",
        summary
    );
    let (download_id, _) = mgr
        .confirm_pending_link(&pending_id, None, None, None, None, None, true)
        .await
        .unwrap();
    assert!(wait_for_status(&mgr, &download_id, "complete", 15).await);
    let dl = mgr.get_download(&download_id).unwrap().unwrap();
    // The playlist is saved as-is: the bytes on disk are exactly
    // the playlist text — no segments fetched, nothing merged.
    let bytes = std::fs::read(&dl.save_path).unwrap();
    assert_eq!(bytes, fixture("vod.m3u8").into_bytes());
    assert_eq!(dl.filename, "vod.m3u8");
}
