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
        ..Default::default()
    }
}

// -- (b) quality derivation never invents a label -------------------------

#[test]
fn quality_is_unknown_without_evidence() {
    assert_eq!(
        quality_for("http://127.0.0.1/clip.mp4", 0, 0),
        UNKNOWN_QUALITY
    );
    assert_eq!(
        quality_for("http://127.0.0.1/clip.mp4", 0, 0),
        "Unknown quality"
    );
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
    let found = detect_videos_in_html(&fixture("video-sources.html"), "http://127.0.0.1/page.html");
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
    assert!(
        found.is_empty(),
        "a page with no media must find none: {found:?}"
    );
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
    assert!(
        err.to_string().contains("No video variants"),
        "error: {err}"
    );

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
        validate_video_variants("https://example.com/watch", &[invented, real], &settings).unwrap();
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
    assert!(
        reply.message.as_deref().unwrap().contains("skipped"),
        "message: {:?}",
        reply
    );
    let pending = store.list_pending_links(Some("pending")).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].source, "video");
    assert_eq!(pending[0].url, "https://example.com/clip-720p.mp4");
    assert_eq!(
        pending[0].referrer.as_deref(),
        Some("https://example.com/watch")
    );
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
    assert!(
        !reply.ok,
        "all-DRM video_found must be an error: {:?}",
        reply
    );
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
        .enqueue_pending_link(
            "https://example.com/clip.mp4",
            Some("https://example.com/watch"),
            "video",
        )
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
    let video = found
        .iter()
        .find(|v| v.url.ends_with("/video.mp4"))
        .unwrap();
    assert_eq!(video.format, "MP4");
    assert_eq!(video.quality, UNKNOWN_QUALITY);
    assert_eq!(video.size, Some(100 * 1024));
    assert!(video.downloadable);
    let audio = found.iter().find(|v| v.url.ends_with("/song.mp3")).unwrap();
    assert_eq!(audio.format, "MP3");
    let dead = found
        .iter()
        .find(|v| v.url.ends_with("/missing.mp4"))
        .unwrap();
    assert!(
        !dead.downloadable,
        "a dead link must not offer a download button"
    );
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
    let pending_id = store.list_pending_links(Some("pending")).unwrap()[0]
        .id
        .clone();
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
    assert!(accepted[0]
        .note
        .as_deref()
        .unwrap()
        .contains("DASH manifest"));
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
    assert_eq!(
        found[2].quality, UNKNOWN_QUALITY,
        "a variant with no RESOLUTION stays unknown"
    );
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
    assert!(
        !found[0].downloadable,
        "an encrypted playlist alone will not play"
    );
    assert!(
        found[0].error.as_deref().unwrap().contains("encrypted"),
        "error: {:?}",
        found[0].error
    );
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
    let pending_id = store.list_pending_links(Some("pending")).unwrap()[0]
        .id
        .clone();
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

// -- Round 79 — manual Add URL with a video URL ---------------------------
// The Round 78 retest: the dialog resolved the metadata (title +
// "1080p + 128kbps — merged") but the download used the raw watch
// URL — the list item was "watch", 0 B, Error, "Server ignored
// the range request". `add_resolved_video_download` must create
// the download from the RESOLVED stream URL(s) with the resolved
// filename — never from the page URL.

fn resolved_row(url: &str) -> fetchline_engine::ytdlp::QualityRow {
    fetchline_engine::ytdlp::QualityRow {
        height: 360,
        label: "360p".to_string(),
        ext: "mp4".to_string(),
        has_audio: true,
        needs_merge: false,
        filesize: Some(512 * 1024),
        url: url.to_string(),
        audio_url: None,
        audio_format_id: None,
        format_id: "18".to_string(),
        headers: std::collections::HashMap::new(),
        fragmented: false,
    }
}

#[tokio::test]
async fn resolved_video_download_uses_stream_url_not_page_url() {
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let page_url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
    let (id, probe) = mgr
        .add_resolved_video_download(
            page_url,
            &resolved_row(&server.url),
            Some(
                "Rick Astley - Never Gonna Give You Up (Official Video) (4K Remaster).mp4"
                    .to_string(),
            ),
            None,
            None,
            None,
            None,
            true,
            None,
        )
        .await
        .unwrap();
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(
        dl.url, server.url,
        "the download must use the resolved stream URL, not the page URL"
    );
    assert!(
        !dl.url.contains("youtube.com"),
        "no watch-page URL may be downloaded: {}",
        dl.url
    );
    assert_eq!(
        dl.filename,
        "Rick Astley - Never Gonna Give You Up (Official Video) (4K Remaster).mp4"
    );
    assert_eq!(dl.source_url.as_deref(), Some(page_url));
    assert_eq!(dl.format_id.as_deref(), Some("18"));
    assert_eq!(probe.size, Some(512 * 1024));
    assert!(wait_for_status(&mgr, &id, "complete", 15).await);
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(dl.downloaded_bytes, 512 * 1024);
    assert_eq!(std::fs::read(&dl.save_path).unwrap(), payload(512 * 1024));
}

#[tokio::test]
async fn resolved_dash_pair_creates_audio_child_even_when_saved_for_later() {
    let video = MockServer::start(MockConfig {
        size_bytes: Some(64 * 1024),
        ..Default::default()
    });
    let audio = MockServer::start(MockConfig {
        size_bytes: Some(32 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let mut row = resolved_row(&video.url);
    row.height = 1080;
    row.label = "1080p".to_string();
    row.has_audio = false;
    row.needs_merge = true;
    row.filesize = Some(64 * 1024);
    row.audio_url = Some(audio.url.clone());
    row.audio_format_id = Some("140".to_string());
    row.format_id = "137".to_string();
    let (id, _) = mgr
        .add_resolved_video_download(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            &row,
            Some("video.mp4".to_string()),
            None,
            None,
            None,
            None,
            false, // saved for later — the children must still exist
            None,
        )
        .await
        .unwrap();
    let children = store.list_child_downloads(&id).unwrap();
    assert_eq!(
        children.len(),
        2,
        "video + audio children must exist before any start: {:?}",
        children
            .iter()
            .map(|c| c.child_kind.clone())
            .collect::<Vec<_>>()
    );
    let audio_child = children
        .iter()
        .find(|c| c.child_kind.as_deref() == Some("audio"))
        .expect("an audio child");
    assert_eq!(audio_child.url, audio.url);
    assert_eq!(audio_child.parent_id.as_deref(), Some(id.as_str()));
    // The link marks the PARENT's merge pending (children carry
    // no merge status of their own).
    let parent = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(parent.merge_status.as_deref(), Some("pending"));
    // The parent itself was saved for later, not started.
    assert_ne!(parent.status, "downloading");
}

#[tokio::test]
async fn resolved_video_download_failure_creates_no_download() {
    // A stream URL that cannot be probed (or a resolve failure in
    // the dialog) must never leave a size-0 download behind —
    // the Round 78 retest's "watch", 0 B, Error row.
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let row = resolved_row("http://127.0.0.1:1/unreachable.mp4");
    let err = mgr
        .add_resolved_video_download(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            &row,
            Some("video.mp4".to_string()),
            None,
            None,
            None,
            None,
            true,
            None,
        )
        .await
        .unwrap_err();
    assert!(
        store.list_downloads().unwrap().is_empty(),
        "a failed add must leave no download row behind: {err}"
    );
}

// -- Round 80 — a TikTok page goes direct to yt-dlp -------------
// TikTok's CDN answers 403 to our raw probe even with the
// format's headers (Round 79 retest: resolve OK, Start 403, and
// the retry repeated the identical 403). Round 85 (Claude delta)
// skips the probe for TikTok pages entirely — the add goes
// direct to yt-dlp's own downloader (the (B) path) from the
// start, so the mock's 403 is never even seen. The 401/403
// fallback arm stays as a safety net for any caller that still
// probes a TikTok URL first. A 404 keeps today's behaviour (no
// row, the real error) — see the YouTube 404 test below.

#[tokio::test]
async fn resolved_video_download_tiktok_page_goes_direct_from_the_start() {
    let server = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let page_url = "https://www.tiktok.com/@user/video/123";
    let (id, probe) = mgr
        .add_resolved_video_download(
            page_url,
            &resolved_row(&server.url),
            Some("video.mp4".to_string()),
            None,
            None,
            None,
            None,
            false, // not started — no yt-dlp binary on this VM
            None,
        )
        .await
        .unwrap();
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        dl.direct_ytdlp,
        "a TikTok page must go direct to yt-dlp from the start"
    );
    assert_eq!(dl.url, server.url);
    assert_eq!(dl.source_url.as_deref(), Some(page_url));
    assert_eq!(dl.format_id.as_deref(), Some("18"));
    assert_eq!(
        probe.size, None,
        "the fallback creates no probe size — yt-dlp downloads it"
    );
}

#[tokio::test]
async fn resolved_video_download_tiktok_dash_pair_goes_direct_merged() {
    // A DASH pair on a TikTok page goes direct from the start
    // (Round 85 Claude delta — no probe) with the combined
    // "video+audio" selector, so yt-dlp merges the pair itself
    // — and must NOT create DASH children (the (B) runner owns
    // the merge; orphan children would sit pending forever).
    let video = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let audio = MockServer::start(MockConfig {
        size_bytes: Some(32 * 1024),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let mut row = resolved_row(&video.url);
    row.height = 1080;
    row.label = "1080p".to_string();
    row.has_audio = false;
    row.needs_merge = true;
    row.filesize = Some(64 * 1024);
    row.audio_url = Some(audio.url.clone());
    row.audio_format_id = Some("140".to_string());
    row.format_id = "137".to_string();
    let (id, _) = mgr
        .add_resolved_video_download(
            // Round 82 — the probe-403 fallback is TikTok-only:
            // a YouTube DASH pair with a refused probe must NOT
            // fall back (it used to, and the yt-dlp download
            // failed with exit code 1 on the VPS). This test
            // keeps proving the merged direct path, on a TikTok page.
            "https://www.tiktok.com/@user/video/123",
            &row,
            Some("video.mp4".to_string()),
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .await
        .unwrap();
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(dl.direct_ytdlp);
    assert_eq!(
        dl.format_id.as_deref(),
        Some("137+140"),
        "the fallback must carry the combined selector so yt-dlp merges"
    );
    assert!(
        store.list_child_downloads(&id).unwrap().is_empty(),
        "a fallback (B) row must not gain DASH children"
    );
}

#[tokio::test]
async fn resolved_video_download_probe_404_still_fails_with_no_row() {
    // The fallback is scoped to 401/403: a genuinely dead stream
    // URL (404) must still fail with no row behind it.
    let server = MockServer::start(MockConfig {
        fail_status: Some(404),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let err = mgr
        .add_resolved_video_download(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            &resolved_row(&server.url),
            Some("video.mp4".to_string()),
            None,
            None,
            None,
            None,
            true,
            None,
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("404"),
        "the real 404 must surface: {err}"
    );
    assert!(
        store.list_downloads().unwrap().is_empty(),
        "a 404 add must leave no download row behind: {err}"
    );
}

// -- Round 82 — the probe-403 fallback is TikTok-only ---------
// A YouTube stream whose probe is refused with 403 must surface
// the real probe error with no row — exactly like Round 79.
// The unscoped Round 80 fallback switched it to a direct yt-dlp
// download instead, and yt-dlp on the VPS fails that with exit
// code 1, so a manual YouTube download that worked in Round 79
// broke in Round 80+81 ("yt-dlp exited with exit code: 1", 0 B).

#[tokio::test]
async fn resolved_video_download_probe_403_youtube_does_not_fall_back() {
    let server = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let dir = tmpdir();
    let mgr = test_mgr(&dir);
    let store = mgr.store();
    let err = mgr
        .add_resolved_video_download(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            &resolved_row(&server.url),
            Some("video.mp4".to_string()),
            None,
            None,
            None,
            None,
            true,
            None,
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("403"),
        "the real 403 must surface, not a yt-dlp fallback: {err}"
    );
    assert!(
        store.list_downloads().unwrap().is_empty(),
        "a refused YouTube probe must leave no download row behind: {err}"
    );
}

// -- Round 83 — the direct yt-dlp runner ----------------------
// A fake yt-dlp next to the test binary (sidecar_exe_dir
// resolves to the test binary's dir) records its argv, prints
// a [download] line to STDOUT and an ERROR to stderr, then
// exits 1. Unix only: a fake Windows yt-dlp would need a batch
// file with different mechanics, and the VPS retest covers the
// Windows path. The fake is installed for one test and removed
// on drop — no other test in this binary starts a direct
// download, so it cannot leak into one. The two fake tests
// share the one install path, so they take turns on the lock
// below — a parallel run would let one test's Drop delete the
// fake under the other's version check.
#[cfg(unix)]
static FAKE_YTDLP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(unix)]
struct FakeYtDlp {
    dir: std::path::PathBuf,
}

#[cfg(unix)]
impl FakeYtDlp {
    fn install(argv_path: &std::path::Path) -> Self {
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let script = dir.join("yt-dlp");
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"2026.10.26\"; exit 0; fi\necho \"$@\" >> \"{}\"\necho \"[download]  42.1% of 10.00MiB at  1.00MiB/s ETA 00:10\"\nsleep 1\necho \"ERROR: [TikTok] 123: Requested format is not available\" >&2\nexit 1\n",
            argv_path.display()
        );
        std::fs::write(&script, body).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir }
    }
}

#[cfg(unix)]
impl Drop for FakeYtDlp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join("yt-dlp"));
    }
}

#[tokio::test]
#[cfg(unix)]
async fn direct_ytdlp_failure_surfaces_stderr_tail_and_logs_argv() {
    // Round 83 — the R82 retest's TikTok failure ("yt-dlp
    // exited with exit code: 1", 0 B, no reason anywhere) must
    // never happen again: the run reports the real stderr
    // reason, logs the redacted argv + yt-dlp version + tail,
    // passes the format's headers and the fallback selector,
    // and parses the stdout progress into a checkpoint.
    let _fake_guard = FAKE_YTDLP_LOCK.lock().unwrap();
    let server = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(store, 4);
    let mut row = resolved_row(&server.url);
    row.headers = [
        (
            "User-Agent".to_string(),
            "Mozilla/5.0 TestAgent".to_string(),
        ),
        ("Referer".to_string(), "https://www.tiktok.com/".to_string()),
    ]
    .into_iter()
    .collect();
    let (id, _) = mgr
        .add_resolved_video_download(
            "https://www.tiktok.com/@user/video/123",
            &row,
            Some("video.mp4".to_string()),
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .await
        .unwrap();
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        dl.direct_ytdlp,
        "the TikTok page must go direct from the start"
    );
    let argv_path = dir.join("fake-argv.txt");
    let _fake = FakeYtDlp::install(&argv_path);
    mgr.start_direct_ytdlp(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "error", 30).await,
        "the direct run must fail, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let dl = mgr.get_download(&id).unwrap().unwrap();
    let err = dl.error_message.clone().unwrap_or_default();
    assert!(
        err.contains("yt-dlp exited with"),
        "the exit status must stay in the message: {err}"
    );
    assert!(
        err.contains("Requested format is not available"),
        "the stderr tail must surface in the message, not just the exit code: {err}"
    );
    assert!(
        dl.downloaded_bytes > 0,
        "the stdout [download] line must checkpoint bytes, got {}",
        dl.downloaded_bytes
    );
    let argv = std::fs::read_to_string(&argv_path).unwrap();
    assert!(
        argv.contains("18/b[ext=mp4]/b"),
        "the selector must fall back when the id changes: {argv}"
    );
    assert!(
        argv.contains("User-Agent: Mozilla/5.0 TestAgent"),
        "the format's User-Agent must reach yt-dlp: {argv}"
    );
    assert!(
        argv.contains("Referer: https://www.tiktok.com/"),
        "the format's Referer must reach yt-dlp: {argv}"
    );
    assert!(argv.contains("-P"), "the output must go through -P: {argv}");
    assert!(
        argv.contains("video.%(ext)s"),
        "the output template must reach yt-dlp (stem + %(ext)s, Round 85): {argv}"
    );
    assert!(
        !argv.contains("--cookies"),
        "the manual Add URL flow has no cookies to forward: {argv}"
    );
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log.contains("direct yt-dlp download") && log.contains("Requested format is not available"),
        "the failure must be logged with its tail, got: {log}"
    );
    assert!(
        log.contains("2026.10.26"),
        "the log must name the yt-dlp version, got: {log}"
    );
    assert!(
        log.contains("18/b[ext=mp4]/b"),
        "the log must carry the redacted argv, got: {log}"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn direct_ytdlp_download_forwards_capture_cookies_and_headers() {
    // Round 83 — a direct download created from a browser
    // capture must run yt-dlp WITH the capture's cookies and
    // the format's headers: the old code created the row with
    // neither, so a cookie-gated format failed with exit code 1.
    // The temp cookie file is deleted when the run returns.
    let _fake_guard = FAKE_YTDLP_LOCK.lock().unwrap();
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(store, 4);
    let store = mgr.store();
    let cookies_json = r#"[{"name":"session","value":"abc123"},{"name":"consent","value":"yes"}]"#;
    let pending = store
        .enqueue_pending_link_resolved(
            "https://cdn.example.com/video.mp4",
            Some("https://www.tiktok.com/@user/video/123"),
            Some(cookies_json),
            &fetchline_engine::store::ResolvedLink {
                filename: Some("video.mp4".to_string()),
                headers: Some(
                    r#"{"User-Agent":"Mozilla/5.0 TestAgent","Referer":"https://www.tiktok.com/"}"#
                        .to_string(),
                ),
                source_url: Some("https://www.tiktok.com/@user/video/123".to_string()),
                format_id: Some("18".to_string()),
                audio_url: None,
                fragmented: true,
                filesize: None,
            },
        )
        .unwrap();
    let (id, _) = mgr
        .confirm_pending_link(&pending.id, None, None, None, None, None, false)
        .await
        .unwrap();
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(dl.direct_ytdlp, "a fragmented capture is a direct download");
    let argv_path = dir.join("fake-argv2.txt");
    let _fake = FakeYtDlp::install(&argv_path);
    mgr.start_direct_ytdlp(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "error", 30).await,
        "the direct run must fail (the fake exits 1), last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let argv = std::fs::read_to_string(&argv_path).unwrap();
    assert!(
        argv.contains("--cookies"),
        "the direct call must forward the capture's cookies: {argv}"
    );
    assert!(
        argv.contains("fetchline-ytdlp-cookies-"),
        "the cookies must ride a temp Netscape file: {argv}"
    );
    assert!(
        argv.contains("User-Agent: Mozilla/5.0 TestAgent"),
        "the format's User-Agent must reach yt-dlp: {argv}"
    );
    // The temp cookie file is deleted when the run returns
    // (RAII): find its path in the recorded argv and check it
    // is gone.
    let cookie_path = argv
        .split_whitespace()
        .skip_while(|w| *w != "--cookies")
        .nth(1)
        .expect("a --cookies value in the argv");
    assert!(
        !std::path::Path::new(cookie_path).exists(),
        "the temp cookie file must be deleted after the run: {cookie_path}"
    );
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        dl.error_message
            .clone()
            .unwrap_or_default()
            .contains("Requested format is not available"),
        "the stderr reason must surface: {:?}",
        dl.error_message
    );
}

// -- Round 85 — cookie forwarding + the -o template -----------

#[tokio::test]
async fn round85_tiktok_segmented_download_sends_row_cookies_and_referer() {
    // Round 85 — the cURL removal test PROVED TikTok's CDN
    // requires Cookie AND Referer: no-Cookie -> Akamai 403,
    // no-Referer -> Akamai 403. A capture whose resolve was
    // cookieless used to carry `cookies: []` through the
    // extension untouched, so the pending row got no cookies
    // and the engine GET sent no Cookie at all. The mock's
    // header gate stands in for the CDN: it refuses any
    // request missing the exact Cookie / Referer, so a
    // completing download proves both headers reach the
    // segmented GET — for a stream host (the mock) that is
    // NOT the page host (tiktok.com), i.e. no same-host
    // scoping drops them.
    let server = MockServer::start(MockConfig {
        size_bytes: Some(512 * 1024),
        require_headers: vec![
            ("Cookie".to_string(), "ttwid=abc; msToken=def".to_string()),
            ("Referer".to_string(), "https://www.tiktok.com/".to_string()),
        ],
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(store, 4);
    let store = mgr.store();
    let cookies_json = r#"[{"name":"ttwid","value":"abc"},{"name":"msToken","value":"def"}]"#;
    let pending = store
        .enqueue_pending_link_resolved(
            &server.url,
            Some("https://www.tiktok.com/@user/video/123"),
            Some(cookies_json),
            &fetchline_engine::store::ResolvedLink {
                filename: Some("video.mp4".to_string()),
                headers: Some(
                    r#"{"User-Agent":"Mozilla/5.0 TestAgent","Referer":"https://www.tiktok.com/"}"#
                        .to_string(),
                ),
                source_url: Some("https://www.tiktok.com/@user/video/123".to_string()),
                format_id: Some("18".to_string()),
                audio_url: None,
                fragmented: false,
                filesize: Some(512 * 1024),
            },
        )
        .unwrap();
    let (id, _) = mgr
        .confirm_pending_link(&pending.id, None, None, None, None, None, false)
        .await
        .unwrap();
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(!dl.direct_ytdlp, "a non-fragmented capture is segmented");
    mgr.start_download(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "the segmented download must complete once Cookie + Referer reach the CDN, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert_eq!(dl.downloaded_bytes, 512 * 1024);
    assert_eq!(std::fs::read(&dl.save_path).unwrap(), payload(512 * 1024));
}

// A fake yt-dlp that SUCCEEDS — and emulates yt-dlp 2026.10.26's
// own `download()` guard: more than one positional after `--`
// with no '%' in the `-o` value is the "Fixed output name but
// more than one file to download" error (exit 1). On success it
// writes the merged file under the `-o` template (expanding
// "%(ext)s" to "mp4") in the `-P` dir, so the completion check
// has a file to find. Same install path / lock as the failing
// fake above: one test at a time.
#[cfg(unix)]
struct FakeYtDlpSuccess {
    dir: std::path::PathBuf,
}

#[cfg(unix)]
impl FakeYtDlpSuccess {
    fn install(argv_path: &std::path::Path) -> Self {
        let dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let script = dir.join("yt-dlp");
        let body = format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"2026.10.26\"; exit 0; fi\necho \"$@\" >> \"{}\"\nprev=\"\"; after=0; count=0; outtmpl=\"\"; outdir=\"\"\nfor a in \"$@\"; do\n  if [ \"$after\" = 1 ]; then count=$((count + 1)); continue; fi\n  case \"$prev\" in -o) outtmpl=\"$a\" ;; -P) outdir=\"$a\" ;; esac\n  case \"$a\" in --) after=1 ;; esac\n  prev=\"$a\"\ndone\ncase \"$outtmpl\" in *%*) ;; *) if [ \"$count\" -gt 1 ]; then echo \"ERROR: Fixed output name but more than one file to download: $outtmpl\" >&2; exit 1; fi ;; esac\nname=$(printf '%s' \"$outtmpl\" | sed 's/%(ext)s/mp4/')\nif [ -n \"$outdir\" ]; then target=\"$outdir/$name\"; else target=\"$name\"; fi\nprintf 'fake-merged-video-bytes' > \"$target\"\necho \"[download] 100% of 22.00B at 1.00B/s ETA 00:00\"\nexit 0\n",
            argv_path.display()
        );
        std::fs::write(&script, body).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir }
    }
}

#[cfg(unix)]
impl Drop for FakeYtDlpSuccess {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.dir.join("yt-dlp"));
    }
}

#[tokio::test]
#[cfg(unix)]
async fn round85_dash_merge_direct_fallback_succeeds_with_ext_template() {
    // Round 85 — the R84 TikTok manual-path failure: yt-dlp
    // refused with "Fixed output name but more than one file
    // to download" because (a) the `-o` template was a fixed
    // name with no %(ext)s, and (b) --ffmpeg-location /
    // --js-runtimes were appended AFTER "--" + URL, so yt-dlp
    // counted them as extra URLs. A DASH video+audio selection
    // (137+140) direct from the start (Round 85 Claude delta —
    // the TikTok page skips the probe) must now complete, with
    // exactly one positional after "--" and "%(ext)s" in the
    // template.
    let _fake_guard = FAKE_YTDLP_LOCK.lock().unwrap();
    let server = MockServer::start(MockConfig {
        fail_status: Some(403),
        ..Default::default()
    });
    let dir = tmpdir();
    let store = Arc::new(Store::open(&dir.join("app.db")).unwrap());
    let log_path = dir.join("startup.log");
    store.set_log_path(log_path.clone());
    let mut settings = store.get_settings().unwrap();
    settings.default_download_folder = dir.to_string_lossy().to_string();
    store.update_settings(&settings).unwrap();
    let mgr = DownloadManager::new(store, 4);
    let mut row = resolved_row(&server.url);
    row.height = 1080;
    row.label = "1080p".to_string();
    row.has_audio = false;
    row.needs_merge = true;
    row.filesize = Some(64 * 1024);
    row.audio_url = Some(server.url.clone());
    row.audio_format_id = Some("140".to_string());
    row.format_id = "137".to_string();
    let (id, _) = mgr
        .add_resolved_video_download(
            "https://www.tiktok.com/@user/video/123",
            &row,
            Some("video.mp4".to_string()),
            None,
            None,
            None,
            None,
            false,
            None,
        )
        .await
        .unwrap();
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        dl.direct_ytdlp,
        "the TikTok page must go direct from the start"
    );
    let argv_path = dir.join("fake-argv3.txt");
    let _fake = FakeYtDlpSuccess::install(&argv_path);
    mgr.start_direct_ytdlp(&id).await.unwrap();
    assert!(
        wait_for_status(&mgr, &id, "complete", 30).await,
        "the direct run must complete, last status: {}",
        mgr.get_download(&id).unwrap().unwrap().status
    );
    let dl = mgr.get_download(&id).unwrap().unwrap();
    assert!(
        std::fs::metadata(&dl.save_path)
            .map(|m| m.len() > 0)
            .unwrap_or(false),
        "the merged file must land on the save path: {}",
        dl.save_path
    );
    let argv = std::fs::read_to_string(&argv_path).unwrap();
    let after_dashdash = argv.split(" -- ").nth(1).unwrap_or("");
    assert_eq!(
        after_dashdash.split_whitespace().count(),
        1,
        "exactly one positional (the URL) may follow --: {argv}"
    );
    assert!(
        argv.contains("137+140/b[ext=mp4]/b"),
        "the DASH selector must combine video+audio: {argv}"
    );
    assert!(
        argv.contains("video.%(ext)s"),
        "the -o template must carry %(ext)s: {argv}"
    );
    assert!(
        !argv.contains("Fixed output name"),
        "no SameFileError may be raised: {argv}"
    );
}
